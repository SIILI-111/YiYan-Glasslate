//! 一眼 Glasslate — M1 主链路
//!
//! 现在的样子：Alt+Q 唤起全屏遮罩 → 框选 → 抓屏 → Windows OCR → 结果显示在主窗口。
//! 翻译（T8）还没接，所以主窗口先只显示 OCR 出来的原文。
//!
//! 分层：
//! - `win32`  Win32 FFI：抓屏、窗口矩形、DPI、点击穿透、排除抓取
//! - `ocr`    Windows.Media.Ocr（WinRT）
//! - `lib.rs` 窗口/托盘/快捷键/命令

mod ocr;
mod win32;

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// 主窗口（结果窗 / 玻璃窗）
const MAIN: &str = "main";
/// 全屏选择遮罩
const OVERLAY: &str = "overlay";
/// 设置窗
const SETTINGS: &str = "settings";

fn hwnd_of(window: &WebviewWindow) -> Result<win32::Hwnd, String> {
    window
        .hwnd()
        .map(|h| h.0 as win32::Hwnd)
        .map_err(|e| format!("拿不到 HWND: {e}"))
}

/// 让窗口不进入任何截屏画面。我们自己的窗口（遮罩、结果窗）都必须这样，
/// 否则「框选 → 抓屏」会把遮罩自己也抓进去。实测零闪烁、零往返开销。
///
/// 调试时设环境变量 `GLASSLATE_CAPTURABLE=1` 可以关掉这个行为，
/// 否则用截图工具验证界面时**根本看不到自己的窗口**（这也是它生效的旁证）。
fn exclude_from_capture(window: &WebviewWindow) -> bool {
    if std::env::var("GLASSLATE_CAPTURABLE").is_ok() {
        return false;
    }
    match hwnd_of(window) {
        Ok(h) => win32::set_exclude_from_capture(h, true),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------- 遮罩框选（T6）

/// 遮罩窗口的物理坐标与缩放，前端据此把 CSS 坐标换算成屏幕物理坐标
#[derive(Serialize, Clone)]
struct OverlayGeo {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    dpr: f64,
}

fn position_overlay(window: &WebviewWindow) -> Result<OverlayGeo, String> {
    let vs = win32::virtual_screen();
    window
        .set_position(PhysicalPosition::new(vs.left, vs.top))
        .map_err(|e| e.to_string())?;
    window
        .set_size(PhysicalSize::new(vs.width() as u32, vs.height() as u32))
        .map_err(|e| e.to_string())?;
    let dpr = window.scale_factor().map_err(|e| e.to_string())?;
    Ok(OverlayGeo {
        x: vs.left,
        y: vs.top,
        w: vs.width(),
        h: vs.height(),
        dpr,
    })
}

fn show_overlay(app: &AppHandle) -> Result<(), String> {
    let window = match app.get_webview_window(OVERLAY) {
        Some(w) => w,
        None => {
            let vs = win32::virtual_screen();
            let w = WebviewWindowBuilder::new(app, OVERLAY, WebviewUrl::App("overlay.html".into()))
                .title("Glasslate Select")
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .shadow(false)
                .resizable(false)
                .visible(false)
                .focused(false)
                .build()
                .map_err(|e| format!("创建遮罩窗口失败: {e}"))?;
            w.set_position(PhysicalPosition::new(vs.left, vs.top))
                .map_err(|e| e.to_string())?;
            w.set_size(PhysicalSize::new(vs.width() as u32, vs.height() as u32))
                .map_err(|e| e.to_string())?;
            w
        }
    };
    exclude_from_capture(&window);
    let geo = position_overlay(&window)?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().map_err(|e| e.to_string())?;
    let _ = app.emit_to(OVERLAY, "overlay-ready", geo);
    Ok(())
}

/// 调试用：直接抓一块屏幕物理矩形做 OCR，返回纯文本（不经过遮罩、不需要鼠标）。
/// 用来在无人值守环境里验证「物理坐标 → 抓屏 → OCR」这一段。
#[tauri::command]
fn ocr_rect(x: i32, y: i32, w: i32, h: i32) -> Result<String, String> {
    let bgra = win32::capture_rect_bgra(x, y, w, h)?;
    let r = ocr::recognize_bgra(&bgra, w, h, None)?;
    let text = r
        .lines
        .iter()
        .map(|l| l.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!("{}ms|{}", r.ms, text))
}

/// 无人值守自检用：从 GLASSLATE_SELFTEST="x,y,w,h" 读矩形
#[tauri::command]
fn selftest_rect() -> Option<Vec<i32>> {
    let raw = std::env::var("GLASSLATE_SELFTEST").ok()?;
    let parts: Vec<i32> = raw
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    if parts.len() == 4 {
        Some(parts)
    } else {
        None
    }
}

#[tauri::command]
fn debug_log(msg: String) {
    eprintln!("[webview] {msg}");
}

#[tauri::command]
fn overlay_geo(window: WebviewWindow) -> Result<OverlayGeo, String> {
    let rect = win32::window_rect(hwnd_of(&window)?);
    let dpr = window.scale_factor().map_err(|e| e.to_string())?;
    Ok(OverlayGeo {
        x: rect.left,
        y: rect.top,
        w: rect.width(),
        h: rect.height(),
        dpr,
    })
}

#[tauri::command]
fn cancel_selection(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(OVERLAY) {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 框选结束：抓这块屏幕 → OCR → 把结果推给主窗
#[tauri::command]
fn finish_selection(app: AppHandle, x: i32, y: i32, w: i32, h: i32) -> Result<(), String> {
    if w < 8 || h < 8 {
        return cancel_selection(app);
    }
    let started = std::time::Instant::now();

    // 遮罩已经设了 WDA_EXCLUDEFROMCAPTURE，抓屏时它天然不在画面里，不用先隐藏
    let bgra = win32::capture_rect_bgra(x, y, w, h)?;
    let cap_ms = started.elapsed().as_millis();

    if let Some(ov) = app.get_webview_window(OVERLAY) {
        let _ = ov.hide();
    }

    match ocr::recognize_bgra(&bgra, w, h, None) {
        Ok(result) => {
            let main = ensure_main(&app)?;
            let _ = main.show();
            let _ = main.set_focus();
            let payload = serde_json::json!({
                "rect": { "x": x, "y": y, "w": w, "h": h },
                "capture_ms": cap_ms,
                "total_ms": started.elapsed().as_millis(),
                "lines": result.lines,
                "words": result.words,
                "image": { "w": result.width, "h": result.height },
                "engine_lang": result.engine_lang,
                "ocr_ms": result.ms,
            });
            let _ = app.emit_to(MAIN, "ocr-result", payload);
            Ok(())
        }
        Err(e) => {
            let _ = app.emit_to(MAIN, "ocr-failed", serde_json::json!({ "message": e }));
            Err(e)
        }
    }
}

// ---------------------------------------------------------------- 主窗（结果窗）

fn ensure_main(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window(MAIN)
        .ok_or_else(|| "主窗口不存在".to_string())
}

#[tauri::command]
fn start_selection(app: AppHandle) -> Result<(), String> {
    show_overlay(&app)
}

#[tauri::command]
fn show_main(app: AppHandle) -> Result<(), String> {
    let w = ensure_main(&app)?;
    w.show().map_err(|e| e.to_string())?;
    w.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn hide_main(app: AppHandle) -> Result<(), String> {
    ensure_main(&app)?.hide().map_err(|e| e.to_string())
}

/// 设置窗：只创建一次，之后复用
#[tauri::command]
fn show_settings(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(SETTINGS) {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }
    let w = WebviewWindowBuilder::new(&app, SETTINGS, WebviewUrl::App("settings.html".into()))
        .title("一眼 Glasslate · 设置")
        .inner_size(520.0, 420.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(true)
        .center()
        .build()
        .map_err(|e| format!("创建设置窗口失败: {e}"))?;
    let _ = w.set_focus();
    Ok(())
}

/// Alt+E：整窗点击穿透开关（隐形态）。工具条的悬停恢复由前端轮询控制。
#[tauri::command]
fn set_glass_click_through(
    window: WebviewWindow,
    enable: bool,
    app: AppHandle,
) -> Result<bool, String> {
    window
        .set_ignore_cursor_events(enable)
        .map_err(|e| e.to_string())?;
    // 客观证据：穿透在 Windows 上最终体现为 WS_EX_TRANSPARENT(0x20)，往往还带 LAYERED
    if let Ok(h) = hwnd_of(&window) {
        let ex = win32::ex_style(h);
        let transparent = (ex & 0x20) != 0;
        let layered = (ex & 0x8_0000) != 0;
        let _ = app.emit_to(
            MAIN,
            "click-through-state",
            serde_json::json!({
                "enable": enable,
                "ex_style": format!("0x{ex:08X}"),
                "transparent": transparent,
                "layered": layered,
            }),
        );
        eprintln!("[click-through] enable={enable} ex=0x{ex:08X} transparent={transparent} layered={layered}");
    }
    Ok(enable)
}

static HOVER_RUNNING: AtomicBool = AtomicBool::new(false);

/// 顶部条带悬停检测：鼠标进条带 → 关掉穿透让工具条可点；离开 → 恢复穿透。
/// 40ms 轮询，够用且不占 CPU。
#[tauri::command]
fn start_strip_hover(window: WebviewWindow, strip_height: i32) -> Result<(), String> {
    if HOVER_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    let win = window.clone();
    let app = window.app_handle().clone();
    std::thread::spawn(move || {
        let mut last_inside = false;
        while HOVER_RUNNING.load(Ordering::SeqCst) {
            let Ok(h) = win.hwnd() else { break };
            let rect = win32::window_rect(h.0 as win32::Hwnd);
            let c = win32::cursor_pos();
            let inside = c.x >= rect.left
                && c.x < rect.right
                && c.y >= rect.top
                && c.y < rect.top + strip_height;
            if inside != last_inside {
                last_inside = inside;
                let _ = win.set_ignore_cursor_events(!inside);
                let _ = app.emit_to(
                    MAIN,
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
fn stop_strip_hover(window: WebviewWindow) -> Result<(), String> {
    HOVER_RUNNING.store(false, Ordering::SeqCst);
    window.set_ignore_cursor_events(false).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- 诊断

#[tauri::command]
fn ocr_languages() -> Result<Vec<String>, String> {
    ocr::available_languages()
}

#[tauri::command]
fn env_probe(window: WebviewWindow) -> Result<serde_json::Value, String> {
    let hwnd = hwnd_of(&window)?;
    let rect = win32::window_rect(hwnd);
    let vs = win32::virtual_screen();
    Ok(serde_json::json!({
        "hwnd": hwnd,
        "window_rect": rect,
        "dpi": win32::dpi_for_window(hwnd),
        "monitor": win32::window_monitor(hwnd),
        "virtual_screen": vs,
        "scale_factor": window.scale_factor().unwrap_or(1.0),
    }))
}

#[tauri::command]
fn save_debug(name: String, text: String) -> Result<String, String> {
    let dir = std::env::temp_dir().join("glasslate");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if text.len() > 4 * 1024 * 1024 {
        return Err("内容过大".into());
    }
    let path = dir.join(name);
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

// ---------------------------------------------------------------- 启动

fn register_shortcuts(app: &AppHandle) {
    let bindings: [(Shortcut, &str); 4] = [
        (Shortcut::new(Some(Modifiers::ALT), Code::KeyQ), "new"),
        (Shortcut::new(Some(Modifiers::ALT), Code::KeyW), "retranslate"),
        (Shortcut::new(Some(Modifiers::ALT), Code::KeyE), "stealth"),
        (Shortcut::new(Some(Modifiers::ALT), Code::KeyR), "close"),
    ];
    for (sc, action) in bindings {
        let action = action.to_string();
        let label = action.clone();
        let res = app.global_shortcut().on_shortcut(sc, move |app, _sc, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            match action.as_str() {
                "new" => {
                    if let Err(e) = show_overlay(app) {
                        eprintln!("[hotkey] 唤起遮罩失败: {e}");
                    }
                }
                "retranslate" => {
                    let _ = app.emit_to(MAIN, "hotkey-retranslate", ());
                }
                "stealth" => {
                    let _ = app.emit_to(MAIN, "hotkey-stealth", ());
                }
                "close" => {
                    if let Some(w) = app.get_webview_window(MAIN) {
                        let _ = w.hide();
                    }
                }
                _ => {}
            }
        });
        if let Err(e) = res {
            eprintln!("[hotkey] 注册 {label} 失败: {e}");
        }
    }
}

fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let new_item = MenuItem::with_id(app, "new", "新建截图\tAlt+Q", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, "show", "显示结果窗", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &new_item,
            &show_item,
            &settings_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("glasslate-tray")
        .tooltip("一眼 Glasslate")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "new" => {
                let _ = show_overlay(app);
            }
            "show" => {
                if let Some(w) = app.get_webview_window(MAIN) {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "settings" => {
                let _ = show_settings(app.clone());
            }
            "quit" => app.exit(0),
            _ => {}
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window(MAIN) {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            overlay_geo,
            debug_log,
            ocr_rect,
            selftest_rect,
            start_selection,
            cancel_selection,
            finish_selection,
            show_main,
            hide_main,
            show_settings,
            set_glass_click_through,
            start_strip_hover,
            stop_strip_hover,
            ocr_languages,
            env_probe,
            save_debug
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            if let Some(main) = app.get_webview_window(MAIN) {
                // 保险：tauri.conf.json 里写了 decorations:false，但实测启动后
                // 窗口仍带着系统标题栏（WS_CAPTION 还在）。运行期再压一次，
                // 顺手确认 shadow 也是关的（无边框窗口带阴影会出现一圈灰边）。
                let _ = main.set_decorations(false);
                let _ = main.set_shadow(false);
                exclude_from_capture(&main);
            }
            setup_tray(&handle)?;
            register_shortcuts(&handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
