//! Capture the whole virtual desktop into memory. The image never touches disk.
//! Coordinates are physical pixels across all monitors; DPI scales are attached
//! per monitor so the overlay can map CSS pixels back to physical pixels.

use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject,
    EnumDisplayMonitors, GetDC, GetDIBits, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, DIB_RGB_COLORS, HDC, HMONITOR, MONITORINFOEXW, SRCCOPY, BI_RGB,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

/// Geometry and DPI of one monitor, in physical pixels.
#[derive(Clone, Debug)]
pub struct MonitorGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub dpi: u32,
}

/// A full-desktop capture held in memory.
pub struct ScreenCapture {
    pub width: i32,
    pub height: i32,
    pub origin_x: i32,
    pub origin_y: i32,
    /// RGBA, top-down, `width * height * 4` bytes.
    pub pixels: Vec<u8>,
    pub monitors: Vec<MonitorGeometry>,
}

pub fn capture() -> Result<ScreenCapture, String> {
    let origin_x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let origin_y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err("无法获取屏幕尺寸。".into());
    }

    let monitors = collect_monitors();

    unsafe {
        let screen_dc = GetDC(None);
        let mem_dc = CreateCompatibleDC(Some(screen_dc));
        let bitmap = CreateCompatibleBitmap(screen_dc, width, height);
        let old = SelectObject(mem_dc, bitmap.into());
        let copied = BitBlt(
            mem_dc,
            0,
            0,
            width,
            height,
            Some(screen_dc),
            origin_x,
            origin_y,
            SRCCOPY,
        );

        let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
        if copied.is_ok() {
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // top-down: first row is the top of the screen
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                bmiColors: [Default::default(); 1],
            };
            let _ = GetDIBits(
                mem_dc,
                bitmap,
                0,
                height as u32,
                Some(pixels.as_mut_ptr() as *mut _),
                &mut bmi,
                DIB_RGB_COLORS,
            );
        }

        let _ = SelectObject(mem_dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);

        // GetDIBits with BI_RGB yields BGRA; flip to RGBA for the front end.
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        Ok(ScreenCapture {
            width,
            height,
            origin_x,
            origin_y,
            pixels,
            monitors,
        })
    }
}

/// True when the capture is entirely black (a protected screen, e.g. DRM video
/// or a secure desktop).
pub fn is_black(pixels: &[u8]) -> bool {
    pixels.chunks_exact(4).all(|px| {
        // RGBA: treat any channel above a low threshold as "not black".
        px[0] <= 4 && px[1] <= 4 && px[2] <= 4
    })
}

/// Base64-encode RGBA pixel bytes for transfer to the overlay webview.
pub fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn collect_monitors() -> Vec<MonitorGeometry> {
    let mut out: Vec<MonitorGeometry> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum_proc),
            LPARAM(&mut out as *mut _ as isize),
        );
    }
    out
}

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    unsafe {
        let vec = &mut *(lparam.0 as *mut Vec<MonitorGeometry>);
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmonitor, &mut info.monitorInfo).as_bool() {
            let rc = info.monitorInfo.rcMonitor;
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            vec.push(MonitorGeometry {
                x: rc.left,
                y: rc.top,
                width: rc.right - rc.left,
                height: rc.bottom - rc.top,
                dpi: dpi_x,
            });
        }
    }
    BOOL(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn is_black_detects_all_black() {
        // One black pixel (RGBA) -> black.
        assert!(is_black(&[0, 0, 0, 255]));
        // Any channel above threshold -> not black.
        assert!(!is_black(&[10, 0, 0, 255]));
        assert!(!is_black(&[0, 255, 0, 255]));
        assert!(!is_black(&[0, 0, 5, 255]));
    }
}
