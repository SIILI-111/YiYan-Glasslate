//! 一眼 Glasslate — M0 验证实验室
//!
//! 四个验证任务的探针都放在这里：
//! - T1 透明 / 无边框 / 置顶      → tauri.conf.json 的窗口配置（本文件提供自检信息）
//! - T2 点击穿透 + 悬停工具条     → set_click_through / start_hover_poll
//! - T3 多显示器 + DPI 坐标对齐   → env_info（Tauri 视角 vs Win32 视角同屏对比）
//! - T4 屏幕矩形实时抓取          → grab_rect / grab_hidden / set_exclude_from_capture
//!
//! 所有 Win32 调用都是手写 FFI，刻意不引入 `windows` crate，避免与 Tauri 内部版本打架。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use base64::Engine as _;
use serde::Serialize;
use tauri::{Emitter, Manager, WebviewWindow};

// ---------------------------------------------------------------- Win32 FFI

type Hwnd = isize;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug, Serialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[allow(dead_code)]
impl Rect {
    fn width(&self) -> i32 {
        self.right - self.left
    }
    fn height(&self) -> i32 {
        self.bottom - self.top
    }
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MonitorInfo {
    cb_size: u32,
    rc_monitor: Rect,
    rc_work: Rect,
    flags: u32,
}

#[repr(C)]
struct BitmapInfoHeader {
    size: u32,
    width: i32,
    height: i32,
    planes: u16,
    bit_count: u16,
    compression: u32,
    size_image: u32,
    x_pels_per_meter: i32,
    y_pels_per_meter: i32,
    clr_used: u32,
    clr_important: u32,
}

#[repr(C)]
struct RgbQuad {
    b: u8,
    g: u8,
    r: u8,
    a: u8,
}

#[repr(C)]
struct BitmapInfo {
    header: BitmapInfoHeader,
    colors: [RgbQuad; 1],
}

#[link(name = "user32")]
extern "system" {
    fn GetCursorPos(p: *mut Point) -> i32;
    fn GetWindowRect(h: Hwnd, r: *mut Rect) -> i32;
    fn GetDC(h: Hwnd) -> isize;
    fn ReleaseDC(h: Hwnd, dc: isize) -> i32;
    fn GetSystemMetrics(index: i32) -> i32;
    fn SetWindowDisplayAffinity(h: Hwnd, affinity: u32) -> i32;
    fn GetDpiForWindow(h: Hwnd) -> u32;
    fn MonitorFromWindow(h: Hwnd, flags: u32) -> isize;
    fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfo) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(dc: isize) -> isize;
    fn CreateCompatibleBitmap(dc: isize, w: i32, h: i32) -> isize;
    fn SelectObject(dc: isize, obj: isize) -> isize;
    fn BitBlt(dst: isize, x: i32, y: i32, w: i32, h: i32, src: isize, sx: i32, sy: i32, rop: u32) -> i32;
    fn GetDIBits(dc: isize, bmp: isize, start: u32, lines: u32, bits: *mut u8, info: *mut BitmapInfo, usage: u32) -> i32;
    fn DeleteObject(obj: isize) -> i32;
    fn DeleteDC(dc: isize) -> i32;
}

const SRCCOPY: u32 = 0x00CC_0020;
const CAPTUREBLT: u32 = 0x4000_0000;
const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
const WDA_NONE: u32 = 0x0000_0000;

fn hwnd_of(window: &WebviewWindow) -> Result<Hwnd, String> {
    window
        .hwnd()
        .map(|h| h.0 as Hwnd)
        .map_err(|e| format!("拿不到 HWND: {e}"))
}

fn cursor_pos() -> Point {
    let mut p = Point::default();
    unsafe { GetCursorPos(&mut p) };
    p
}

// ---------------------------------------------------------------- 环境信息

#[derive(Serialize)]
struct MonitorInfoDto {
    name: Option<String>,
    /// Tauri 报告的逻辑坐标与尺寸
    logical_pos: (i32, i32),
    logical_size: (u32, u32),
    /// 逻辑尺寸 × scale = 物理像素
    scale: f64,
    physical_size_est: (f64, f64),
}

#[derive(Serialize)]
struct EnvInfo {
    hwnd: isize,
    dpi_awareness_note: String,
    /// Tauri 视角
    tauri_outer_position: (i32, i32),
    tauri_outer_size: (u32, u32),
    tauri_scale_factor: f64,
    tauri_inner_size: (u32, u32),
    /// Win32 视角（物理像素，PA 进程下 GetWindowRect 返回物理像素）
    win32_window_rect: Rect,
    win32_dpi_for_window: u32,
    win32_window_monitor: Rect,
    win32_window_monitor_work: Rect,
    /// 光标（物理像素）
    cursor: (i32, i32),
    /// 虚拟桌面（物理像素）
    virtual_screen: Rect,
    monitors: Vec<MonitorInfoDto>,
}

#[tauri::command]
fn env_info(window: WebviewWindow) -> Result<EnvInfo, String> {
    let hwnd = hwnd_of(&window)?;

    let mut rect = Rect::default();
    let mut monitor_rect = Rect::default();
    let mut work_rect = Rect::default();
    let dpi: u32;
    unsafe {
        GetWindowRect(hwnd, &mut rect);
        dpi = GetDpiForWindow(hwnd);
        let mon = MonitorFromWindow(hwnd, 2 /* MONITOR_DEFAULTTONEAREST */);
        let mut info = MonitorInfo {
            cb_size: std::mem::size_of::<MonitorInfo>() as u32,
            rc_monitor: Rect::default(),
            rc_work: Rect::default(),
            flags: 0,
        };
        if GetMonitorInfoW(mon, &mut info) != 0 {
            monitor_rect = info.rc_monitor;
            work_rect = info.rc_work;
        }
    }

    let virtual_screen = unsafe {
        Rect {
            left: GetSystemMetrics(76),                                // SM_XVIRTUALSCREEN
            top: GetSystemMetrics(77),                                 // SM_YVIRTUALSCREEN
            right: GetSystemMetrics(76) + GetSystemMetrics(78),        // + SM_CXVIRTUALSCREEN
            bottom: GetSystemMetrics(77) + GetSystemMetrics(79),       // + SM_CYVIRTUALSCREEN
        }
    };

    let outer_position = window.outer_position().map_err(|e| e.to_string())?;
    let outer_size = window.outer_size().map_err(|e| e.to_string())?;
    let inner_size = window.inner_size().map_err(|e| e.to_string())?;
    let scale = window.scale_factor().map_err(|e| e.to_string())?;

    let monitors = window
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let p = m.position();
            let s = m.size();
            MonitorInfoDto {
                name: m.name().cloned(),
                logical_pos: (p.x, p.y),
                logical_size: (s.width, s.height),
                scale: m.scale_factor(),
                physical_size_est: (
                    s.width as f64 * m.scale_factor(),
                    s.height as f64 * m.scale_factor(),
                ),
            }
        })
        .collect();

    let c = cursor_pos();
    Ok(EnvInfo {
        hwnd,
        dpi_awareness_note: "Rust 进程 DPI 感知级别见 GetProcessDpiAwareness；这里 GetWindowRect 返回物理像素".into(),
        tauri_outer_position: (outer_position.x, outer_position.y),
        tauri_outer_size: (outer_size.width, outer_size.height),
        tauri_scale_factor: scale,
        tauri_inner_size: (inner_size.width, inner_size.height),
        win32_window_rect: rect,
        win32_dpi_for_window: dpi,
        win32_window_monitor: monitor_rect,
        win32_window_monitor_work: work_rect,
        cursor: (c.x, c.y),
        virtual_screen,
        monitors,
    })
}

// ---------------------------------------------------------------- T2 点击穿透

#[tauri::command]
fn set_click_through(window: WebviewWindow, enable: bool) -> Result<bool, String> {
    window
        .set_ignore_cursor_events(enable)
        .map_err(|e| e.to_string())?;
    Ok(enable)
}

static HOVER_RUNNING: AtomicBool = AtomicBool::new(false);

/// 悬停工具条 + 点击穿透的联合方案：
/// 鼠标进窗口顶部 strip 高度内 → 关掉穿透（工具条可点）；否则打开穿透（点穿到下面）。
/// 线程每 40ms 轮询一次光标，状态变化时通过事件通知前端。
#[tauri::command]
fn start_hover_poll(window: WebviewWindow, strip_height: i32) -> Result<(), String> {
    if HOVER_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let win = window.clone();
    let app = window.app_handle().clone();
    std::thread::spawn(move || {
        let mut last_inside = false;
        while HOVER_RUNNING.load(Ordering::SeqCst) {
            let hwnd = match win.hwnd() {
                Ok(h) => h.0 as Hwnd,
                Err(_) => break,
            };
            let mut rect = Rect::default();
            unsafe { GetWindowRect(hwnd, &mut rect) };
            let c = cursor_pos();
            let strip = Rect {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.top + strip_height,
            };
            let inside = strip.contains(c.x, c.y);
            if inside != last_inside {
                last_inside = inside;
                let _ = win.set_ignore_cursor_events(!inside);
                let _ = app.emit(
                    "strip-hover",
                    serde_json::json!({ "inside": inside, "cursor": [c.x, c.y] }),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
        HOVER_RUNNING.store(false, Ordering::SeqCst);
    });
    Ok(())
}

#[tauri::command]
fn stop_hover_poll(window: WebviewWindow) -> Result<(), String> {
    HOVER_RUNNING.store(false, Ordering::SeqCst);
    window.set_ignore_cursor_events(false).map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------- T4 抓屏

#[derive(Serialize)]
struct GrabResult {
    ms: u128,
    width: i32,
    height: i32,
    png_base64: String,
    saved_to: String,
    note: String,
}

fn capture_rect_bgra(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>, String> {
    if w <= 0 || h <= 0 {
        return Err("宽高必须为正".into());
    }
    unsafe {
        let screen = GetDC(0);
        if screen == 0 {
            return Err("GetDC(NULL) 失败".into());
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        if mem == 0 || bmp == 0 {
            ReleaseDC(0, screen);
            return Err("创建内存 DC/位图失败".into());
        }
        let old = SelectObject(mem, bmp);
        // CAPTUREBLT 让 layered 窗口也进画面（正是我们想验证的"会不会截到自己"）
        let ok = BitBlt(mem, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let mut info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: w,
                height: -h, // 负值 = 自上而下
                planes: 1,
                bit_count: 32,
                compression: 0,
                size_image: (w * h * 4) as u32,
                x_pels_per_meter: 0,
                y_pels_per_meter: 0,
                clr_used: 0,
                clr_important: 0,
            },
            colors: [RgbQuad { b: 0, g: 0, r: 0, a: 0 }],
        };
        let lines = if ok != 0 {
            GetDIBits(
                mem,
                bmp,
                0,
                h as u32,
                buf.as_mut_ptr(),
                &mut info,
                0,
            )
        } else {
            0
        };
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(0, screen);
        if ok == 0 {
            return Err("BitBlt 失败".into());
        }
        if lines == 0 {
            return Err("GetDIBits 失败".into());
        }
        Ok(buf)
    }
}

fn bgra_to_png_base64(buf: &[u8], w: i32, h: i32) -> Result<(String, String), String> {
    let mut rgba = buf.to_vec();
    for chunk in rgba.chunks_exact_mut(4) {
        chunk.swap(0, 2); // BGRA -> RGBA
        chunk[3] = 255;   // BitBlt 出来的 alpha 不可信，统一按不透明处理
    }
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| "构造图像失败".to_string())?;

    let dir = std::env::temp_dir().join("glasslate_m0");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("shot_{stamp}.png"));
    img.save(&path).map_err(|e| e.to_string())?;

    let mut png = std::io::Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(png.into_inner());
    Ok((b64, path.to_string_lossy().to_string()))
}

#[tauri::command]
fn grab_rect(x: i32, y: i32, w: i32, h: i32) -> Result<GrabResult, String> {
    let start = Instant::now();
    let buf = capture_rect_bgra(x, y, w, h)?;
    let cap_ms = start.elapsed().as_millis();
    let (b64, path) = bgra_to_png_base64(&buf, w, h)?;
    Ok(GrabResult {
        ms: cap_ms,
        width: w,
        height: h,
        png_base64: b64,
        saved_to: path,
        note: "BitBlt + CAPTUREBLT：叠在上面的窗口（含自己）也会被截进来".into(),
    })
}

/// 隐藏 → 等一帧 → 抓 → 恢复：验证"不截到自己"最朴素的做法
#[tauri::command]
fn grab_hidden(window: WebviewWindow, x: i32, y: i32, w: i32, h: i32) -> Result<GrabResult, String> {
    let start = Instant::now();
    let _ = window.hide();
    std::thread::sleep(std::time::Duration::from_millis(30));
    let buf = capture_rect_bgra(x, y, w, h);
    let _ = window.show();
    let buf = buf?;
    let total_ms = start.elapsed().as_millis();
    let (b64, path) = bgra_to_png_base64(&buf, w, h)?;
    Ok(GrabResult {
        ms: total_ms,
        width: w,
        height: h,
        png_base64: b64,
        saved_to: path,
        note: "隐藏 30ms 后抓取再显示（含 hide/show 往返耗时）".into(),
    })
}

#[tauri::command]
fn set_exclude_from_capture(window: WebviewWindow, enable: bool) -> Result<String, String> {
    let hwnd = hwnd_of(&window)?;
    let affinity = if enable { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    let ok = unsafe { SetWindowDisplayAffinity(hwnd, affinity) };
    if ok == 0 {
        Err("SetWindowDisplayAffinity 调用失败（需要 Win10 2004+ 且 DWM 合成开启）".into())
    } else {
        Ok(format!(
            "SetWindowDisplayAffinity({:#x}) 成功：此时 BitBlt 抓屏不会包含本窗口",
            affinity
        ))
    }
}

#[tauri::command]
fn set_window_pos(window: WebviewWindow, x: i32, y: i32) -> Result<(), String> {
    window
        .set_position(tauri::PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())
}

/// 把探针数据落盘，方便命令行侧直接读（M0 的证据收集口）
#[tauri::command]
fn save_debug(name: String, text: String) -> Result<String, String> {
    let dir = std::env::temp_dir().join("glasslate_m0");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let len = text.len();
    if len > 4 * 1024 * 1024 {
        return Err("内容过大".into());
    }
    let path = dir.join(name);
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            env_info,
            set_click_through,
            start_hover_poll,
            stop_hover_poll,
            grab_rect,
            grab_hidden,
            set_exclude_from_capture,
            set_window_pos,
            save_debug
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
