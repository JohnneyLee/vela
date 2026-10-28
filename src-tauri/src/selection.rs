//! Windows geometry helpers: cursor position and clamping into the work area.

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};

pub fn cursor_pos() -> Result<(i32, i32), String> {
    let mut pt = POINT::default();
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt)
            .map_err(|err| err.to_string())?;
    }
    Ok((pt.x, pt.y))
}

pub fn clamp_near_cursor(x: i32, y: i32, width: i32, height: i32) -> (i32, i32) {
    let mut px = x + 16;
    let mut py = y + 16;
    if let Some((left, top, right, bottom)) = work_area(x, y) {
        if px + width > right {
            px = (x - width - 16).max(left);
        }
        if py + height > bottom {
            py = (y - height - 16).max(top);
        }
        px = px.clamp(left, (right - width).max(left));
        py = py.clamp(top, (bottom - height).max(top));
    }
    (px, py)
}

pub fn clamp_into_work(x: i32, y: i32, width: i32, height: i32) -> (i32, i32) {
    let Some((left, top, right, bottom)) = work_area(x, y) else {
        return (x, y);
    };
    (
        x.clamp(left, (right - width).max(left)),
        y.clamp(top, (bottom - height).max(top)),
    )
}

fn work_area(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut info = std::mem::zeroed::<MONITORINFO>();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let rc = info.rcWork;
        Some((rc.left, rc.top, rc.right, rc.bottom))
    }
}
