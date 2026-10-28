//! Text recognition via the built-in Windows.Media.Ocr engine. Results stay in
//! memory. The `Recognizer` trait keeps the engine swappable for a local model
//! later.

use std::sync::Once;

use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

static COM_INIT: Once = Once::new();

fn init_com() {
    COM_INIT.call_once(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    });
}

/// One recognized line plus its bounding box (in the *original* image's pixel
/// coordinates, before the 2x upscale).
#[derive(Clone, Debug)]
pub struct OcrLineBox {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Swappable recognition interface.
pub trait Recognizer: Send + Sync {
    fn recognize(
        &self,
        pixels: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Vec<OcrLineBox>, String>;
    fn available_languages(&self) -> Result<Vec<String>, String>;
}

/// The built-in Windows OCR engine.
pub struct WindowsOcr;

impl Recognizer for WindowsOcr {
    fn recognize(
        &self,
        pixels: &[u8],
        width: i32,
        height: i32,
    ) -> Result<Vec<OcrLineBox>, String> {
        recognize_windows(pixels, width, height)
    }

    fn available_languages(&self) -> Result<Vec<String>, String> {
        available_languages()
    }
}

/// Recognize RGBA pixels, upscaled 2x first for small text.
fn recognize_windows(
    pixels: &[u8],
    width: i32,
    height: i32,
) -> Result<Vec<OcrLineBox>, String> {
    init_com();
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
        format!("无法创建识别引擎：{e}。请在「设置 → 时间和语言 → 语言」里安装识别语言包。")
    })?;

    let (scaled, sw, sh) = scale2x(pixels, width, height);
    let max_dim = OcrEngine::MaxImageDimension().unwrap_or(10000);
    let (scaled, sw, sh, factor) = if sw > max_dim as i32 || sh > max_dim as i32 {
        let f = (max_dim as f32 / sw.max(sh) as f32).max(0.01);
        let (img, nw, nh) = scale(pixels, width, height, f);
        (img, nw, nh, 1.0 / f)
    } else {
        (scaled, sw, sh, 2.0)
    };

    let bitmap = to_software_bitmap(&scaled, sw, sh)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(|e| format!("识别请求失败：{e}"))?
        .get()
        .map_err(|e| format!("识别失败：{e}"))?;
    let lines = result.Lines().map_err(|e| format!("读取识别结果失败：{e}"))?;

    let mut out = Vec::new();
    for i in 0..lines.Size().map_err(|e| e.to_string())? {
        let line = lines.GetAt(i).map_err(|e| e.to_string())?;
        let text = line.Text().map_err(|e| e.to_string())?.to_string();
        let words = line.Words().map_err(|e| e.to_string())?;
        let mut rect: Option<windows::Foundation::Rect> = None;
        for j in 0..words.Size().map_err(|e| e.to_string())? {
            let word = words.GetAt(j).map_err(|e| e.to_string())?;
            let r = word.BoundingRect().map_err(|e| e.to_string())?;
            rect = Some(match rect {
                None => r,
                Some(acc) => {
                    let left = acc.X.min(r.X);
                    let top = acc.Y.min(r.Y);
                    let right = (acc.X + acc.Width).max(r.X + r.Width);
                    let bottom = (acc.Y + acc.Height).max(r.Y + r.Height);
                    windows::Foundation::Rect {
                        X: left,
                        Y: top,
                        Width: right - left,
                        Height: bottom - top,
                    }
                }
            });
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        let (x, y, w, h) = match rect {
            Some(r) => (r.X / factor, r.Y / factor, r.Width / factor, r.Height / factor),
            None => (0.0, 0.0, width as f32, height as f32),
        };
        out.push(OcrLineBox { text, x, y, width: w, height: h });
    }
    Ok(out)
}

/// Languages the installed OCR engine can recognize (BCP-47 tags).
fn available_languages() -> Result<Vec<String>, String> {
    init_com();
    let langs = OcrEngine::AvailableRecognizerLanguages().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for i in 0..langs.Size().map_err(|e| e.to_string())? {
        let lang = langs.GetAt(i).map_err(|e| e.to_string())?;
        out.push(lang.LanguageTag().map_err(|e| e.to_string())?.to_string());
    }
    Ok(out)
}

fn to_software_bitmap(pixels: &[u8], width: i32, height: i32) -> Result<SoftwareBitmap, String> {
    let writer = DataWriter::new().map_err(|e| e.to_string())?;
    writer.WriteBytes(pixels).map_err(|e| e.to_string())?;
    let buffer = writer.DetachBuffer().map_err(|e| e.to_string())?;
    SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Rgba8, width, height)
        .map_err(|e| e.to_string())
}

/// Nearest-neighbour 2x upscale of RGBA pixels.
fn scale2x(pixels: &[u8], w: i32, h: i32) -> (Vec<u8>, i32, i32) {
    scale(pixels, w, h, 2.0)
}

/// Nearest-neighbour scale of RGBA pixels by `factor`.
fn scale(pixels: &[u8], w: i32, h: i32, factor: f32) -> (Vec<u8>, i32, i32) {
    let nw = (w as f32 * factor).round().max(1.0) as i32;
    let nh = (h as f32 * factor).round().max(1.0) as i32;
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    for y in 0..nh {
        let sy = ((y as f32 / factor) as i32).clamp(0, h - 1);
        for x in 0..nw {
            let sx = ((x as f32 / factor) as i32).clamp(0, w - 1);
            let src = ((sy * w + sx) * 4) as usize;
            let dst = ((y * nw + x) * 4) as usize;
            out[dst..dst + 4].copy_from_slice(&pixels[src..src + 4]);
        }
    }
    (out, nw, nh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale2x_doubles_dimensions_and_copies_centers() {
        // 2x2 RGBA, all red.
        let px = [255u8, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255, 255, 0, 0, 255];
        let (out, w, h) = scale2x(&px, 2, 2);
        assert_eq!((w, h), (4, 4));
        assert_eq!(out.len(), 4 * 4 * 4);
        // Every pixel stays red.
        assert!(out.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
    }
}
