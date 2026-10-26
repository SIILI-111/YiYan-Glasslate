//! Windows.Media.Ocr 封装（T7）。
//!
//! 全部在本机完成，零下载零体积。两个实测结论写在这里当注释，免得以后忘：
//!
//! 1. `TryCreateFromUserProfileLanguages()` 才是可用的入口。
//!    中文系统上 `AvailableRecognizerLanguages` 往往只有 `zh-Hans-CN`，
//!    而 **zh-Hans 引擎能读英文**（会吞单词间空格、小字号会把 l 认成 I），
//!    所以「没装英文语言包就用不了」是错的，只是精度差一点。
//! 2. `BitmapDecoder.GetSoftwareBitmapAsync()` 拿到的是 **Bgra8 + Straight alpha**，
//!    不是 premultiplied；我们自己用 GDI 抓的 BGRA 数据，alpha 位不可信，
//!    必须用 `CreateCopyWithAlphaFromBuffer(..., BitmapAlphaMode::Ignore)` 把 alpha 忽略掉。

use serde::Serialize;
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;

/// 一行 OCR 结果。坐标是**图像内**的像素坐标（不是屏幕坐标）。
#[derive(Serialize, Clone, Debug)]
pub struct OcrLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// 每一行里每个词的框，用于后续「按行距聚合成段」和「取边缘底色」
#[derive(Serialize, Clone, Debug)]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Serialize, Clone, Debug)]
pub struct OcrResultDto {
    pub lines: Vec<OcrLine>,
    pub words: Vec<OcrWord>,
    pub width: i32,
    pub height: i32,
    pub engine_lang: String,
    pub ms: u128,
}

// WinRT 激活需要线程有 COM apartment。Tauri 的命令线程不在 STA 里，这里显式初始化成 MTA。
// 注意两个坑：
//   1. 别手写 `#[link(name = "combase")]` 调 RoInitialize——本机 SDK 的 um\x64 里没 combase.lib，
//      链接会报 LNK1181。CoInitializeEx（ole32.dll）同样满足 WinRT 的 apartment 要求，而且
//      ole32.lib 本来就在 tauri 的链接行里。
//   2. 已经初始化过会返回 S_FALSE(1) 或 RPC_E_CHANGED_MODE(0x80010106)，两种都算"能用"。
#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *const core::ffi::c_void, coinit: u32) -> i32;
    fn CoUninitialize();
}

const COINIT_MULTITHREADED: u32 = 0x0;

struct ComGuard {
    should_uninit: bool,
}

impl ComGuard {
    fn new() -> Self {
        let hr = unsafe { CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED) };
        // 0 = S_OK 由我们负责反初始化；1 = S_FALSE 已经初始化过，别动它
        ComGuard {
            should_uninit: hr == 0,
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.should_uninit {
            unsafe { CoUninitialize() };
        }
    }
}

fn err<E: std::fmt::Display>(stage: &str, e: E) -> String {
    format!("{stage}失败：{e}")
}

/// 当前系统可用的 OCR 语言
pub fn available_languages() -> Result<Vec<String>, String> {
    let _com = ComGuard::new();
    let langs = OcrEngine::AvailableRecognizerLanguages().map_err(|e| err("枚举 OCR 语言", e))?;
    let n = langs.Size().map_err(|e| err("读语言数量", e))?;
    let mut out = Vec::new();
    for i in 0..n {
        if let Ok(l) = langs.GetAt(i) {
            if let Ok(tag) = l.LanguageTag() {
                let display = l.DisplayName().map(|d| d.to_string()).unwrap_or_default();
                out.push(format!("{} ({})", tag.to_string(), display));
            }
        }
    }
    Ok(out)
}

fn make_engine(prefer_lang: Option<&str>) -> Result<(OcrEngine, String), String> {
    if let Some(tag) = prefer_lang {
        if let Ok(lang) = Language::CreateLanguage(&windows::core::HSTRING::from(tag)) {
            if let Ok(engine) = OcrEngine::TryCreateFromLanguage(&lang) {
                let name = engine
                    .RecognizerLanguage()
                    .and_then(|l| l.LanguageTag())
                    .map(|t| t.to_string())
                    .unwrap_or_else(|_| tag.to_string());
                return Ok((engine, name));
            }
        }
    }
    let engine =
        OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
            "系统没有可用的 OCR 语言包。请在 设置 → 时间和语言 → 语言 里装一个语言包".to_string()
        })?;
    let name = engine
        .RecognizerLanguage()
        .and_then(|l| l.LanguageTag())
        .map(|t| t.to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    Ok((engine, name))
}

/// 对一张 BGRA 图做 OCR
pub fn recognize_bgra(buf: &[u8], w: i32, h: i32, prefer_lang: Option<&str>) -> Result<OcrResultDto, String> {
    if w <= 0 || h <= 0 {
        return Err("图像尺寸不合法".into());
    }
    let expect = (w as usize) * (h as usize) * 4;
    if buf.len() < expect {
        return Err("像素缓冲区比声明的尺寸小".into());
    }

    let _com = ComGuard::new();
    let started = std::time::Instant::now();

    let max = OcrEngine::MaxImageDimension().unwrap_or(10000);
    if (w as u32) > max || (h as u32) > max {
        return Err(format!("选区太大（{}×{}），OCR 上限是 {max} 像素", w, h));
    }

    let (engine, lang) = make_engine(prefer_lang)?;

    let writer = DataWriter::new().map_err(|e| err("创建 DataWriter", e))?;
    writer.WriteBytes(&buf[..expect]).map_err(|e| err("写入像素", e))?;
    let buffer = writer.DetachBuffer().map_err(|e| err("取 IBuffer", e))?;

    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        w,
        h,
        BitmapAlphaMode::Ignore,
    )
    .map_err(|e| err("构造 SoftwareBitmap", e))?;

    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| err("启动识别", e))?
        .join()
        .map_err(|e| err("等待识别结果", e))?;

    let lines = result.Lines().map_err(|e| err("读行", e))?;
    let line_count = lines.Size().unwrap_or(0);

    let mut out_lines = Vec::new();
    let mut out_words = Vec::new();
    for i in 0..line_count {
        let Ok(line) = lines.GetAt(i) else { continue };
        let text = line.Text().map(|t| t.to_string()).unwrap_or_default();
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        if let Ok(words) = line.Words() {
            let wn = words.Size().unwrap_or(0);
            for j in 0..wn {
                let Ok(word) = words.GetAt(j) else { continue };
                let Ok(r) = word.BoundingRect() else { continue };
                min_x = min_x.min(r.X);
                min_y = min_y.min(r.Y);
                max_x = max_x.max(r.X + r.Width);
                max_y = max_y.max(r.Y + r.Height);
                out_words.push(OcrWord {
                    text: word.Text().map(|t| t.to_string()).unwrap_or_default(),
                    x: r.X,
                    y: r.Y,
                    w: r.Width,
                    h: r.Height,
                });
            }
        }

        if min_x == f32::MAX {
            // 没有词框就退化成整图
            min_x = 0.0;
            min_y = 0.0;
            max_x = w as f32;
            max_y = h as f32;
        }

        out_lines.push(OcrLine {
            text,
            x: min_x,
            y: min_y,
            w: max_x - min_x,
            h: max_y - min_y,
        });
    }

    Ok(OcrResultDto {
        lines: out_lines,
        words: out_words,
        width: w,
        height: h,
        engine_lang: lang,
        ms: started.elapsed().as_millis(),
    })
}
