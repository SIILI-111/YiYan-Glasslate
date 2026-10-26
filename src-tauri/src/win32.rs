//! Win32 FFI：抓屏、窗口矩形、DPI、点击穿透、排除抓取。
//!
//! 手写 FFI 而不是引 `windows` crate 的 Win32 部分：这里用到的函数就那么十几个，
//! 手写零依赖、编译快，也不会和 Tauri 内部的 windows 版本打架。
//! （WinRT 那部分——OCR——才需要 `windows` crate。）

use serde::Serialize;

pub type Hwnd = isize;

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[repr(C)]
#[derive(Default, Clone, Copy, Debug, Serialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
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
    #[cfg(target_pointer_width = "64")]
    fn GetWindowLongPtrW(h: Hwnd, index: i32) -> isize;
    #[cfg(target_pointer_width = "32")]
    fn GetWindowLongW(h: Hwnd, index: i32) -> i32;
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

/// WDA_EXCLUDEFROMCAPTURE：窗口本身不进任何截屏/录屏画面（Win10 2004+）
pub const WDA_EXCLUDEFROMCAPTURE: u32 = 0x0000_0011;
pub const WDA_NONE: u32 = 0x0000_0000;

pub fn cursor_pos() -> Point {
    let mut p = Point::default();
    unsafe { GetCursorPos(&mut p) };
    p
}

pub fn window_rect(hwnd: Hwnd) -> Rect {
    let mut r = Rect::default();
    unsafe { GetWindowRect(hwnd, &mut r) };
    r
}

pub fn dpi_for_window(hwnd: Hwnd) -> u32 {
    unsafe { GetDpiForWindow(hwnd) }
}

/// 读 GWL_EXSTYLE。用来客观验证"点击穿透"到底有没有落到窗口样式上
/// （WS_EX_TRANSPARENT=0x20 / WS_EX_LAYERED=0x80000）。
pub fn ex_style(hwnd: Hwnd) -> u32 {
    #[cfg(target_pointer_width = "64")]
    unsafe {
        GetWindowLongPtrW(hwnd, -20) as u32
    }
    #[cfg(target_pointer_width = "32")]
    unsafe {
        GetWindowLongW(hwnd, -20) as u32
    }
}

pub fn window_monitor(hwnd: Hwnd) -> Rect {
    unsafe {
        let mon = MonitorFromWindow(hwnd, 2 /* MONITOR_DEFAULTTONEAREST */);
        let mut info = MonitorInfo {
            cb_size: std::mem::size_of::<MonitorInfo>() as u32,
            rc_monitor: Rect::default(),
            rc_work: Rect::default(),
            flags: 0,
        };
        if GetMonitorInfoW(mon, &mut info) != 0 {
            info.rc_monitor
        } else {
            Rect::default()
        }
    }
}

/// 整个虚拟桌面（所有显示器合起来的包围盒），单位物理像素
pub fn virtual_screen() -> Rect {
    unsafe {
        Rect {
            left: GetSystemMetrics(76),                          // SM_XVIRTUALSCREEN
            top: GetSystemMetrics(77),                           // SM_YVIRTUALSCREEN
            right: GetSystemMetrics(76) + GetSystemMetrics(78),  // + SM_CXVIRTUALSCREEN
            bottom: GetSystemMetrics(77) + GetSystemMetrics(79), // + SM_CYVIRTUALSCREEN
        }
    }
}

/// 让窗口在截屏里消失（或恢复）。返回是否成功。
pub fn set_exclude_from_capture(hwnd: Hwnd, enable: bool) -> bool {
    let affinity = if enable { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) != 0 }
}

/// 抓一块屏幕区域，返回 BGRA 像素（自上而下，每像素 4 字节）。
pub fn capture_rect_bgra(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>, String> {
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
            GetDIBits(mem, bmp, 0, h as u32, buf.as_mut_ptr(), &mut info, 0)
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

/// BGRA → RGBA（alpha 统一按不透明处理：BitBlt 出来的 alpha 不可信）
#[allow(dead_code)]
pub fn bgra_to_rgba(buf: &[u8]) -> Vec<u8> {
    let mut rgba = buf.to_vec();
    for chunk in rgba.chunks_exact_mut(4) {
        chunk.swap(0, 2);
        chunk[3] = 255;
    }
    rgba
}
