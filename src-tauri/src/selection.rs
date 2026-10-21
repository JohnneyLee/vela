//! Windows-only: read the current selection by sending Ctrl+C, then restore the clipboard.

use std::thread;
use std::time::Duration;

use arboard::Clipboard;
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_SHIFT,
};

pub fn capture_selection() -> Result<String, String> {
    read_selection(15, 40, true)
}

pub(crate) fn probe_selection() -> Option<String> {
    read_selection(4, 20, false).ok()
}

fn read_selection(attempts: u32, delay_ms: u64, allow_copy_message: bool) -> Result<String, String> {
    let mut clipboard = Clipboard::new().map_err(|err| err.to_string())?;
    let previous = clipboard.get_text().ok();
    let token = format!(
        "__vela_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );

    clipboard
        .set_text(&token)
        .map_err(|err| format!("无法写入剪贴板: {err}"))?;

    // The hotkey modifiers may still be held. Wait so the synthetic Ctrl+C is a real copy.
    wait_modifiers_released();
    thread::sleep(Duration::from_millis(30));
    send_ctrl_c()?;

    let mut found = None;
    let mut attempts_left = attempts;
    for _ in 0..attempts / 2 {
        attempts_left = attempts_left.saturating_sub(1);
        thread::sleep(Duration::from_millis(delay_ms));
        if let Some(text) = changed_clipboard(&mut clipboard, &token) {
            found = Some(text);
            break;
        }
    }
    if found.is_none() && allow_copy_message {
        send_wm_copy();
        for _ in 0..attempts_left.max(4) {
            thread::sleep(Duration::from_millis(delay_ms));
            if let Some(text) = changed_clipboard(&mut clipboard, &token) {
                found = Some(text);
                break;
            }
        }
    }

    if let Some(prev) = previous {
        let _ = clipboard.set_text(prev);
    } else {
        let _ = clipboard.clear();
    }

    let text = found
        .ok_or_else(|| "没有取到选中文字。请先在其他窗口选中文本，再按 Ctrl+Alt+T。".to_string())?;
    let text = text.trim();
    if text.is_empty() {
        return Err("选中内容为空。".into());
    }
    const MAX_CHARS: usize = 4000;
    let trimmed: String = text.chars().take(MAX_CHARS).collect();
    Ok(trimmed)
}

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

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) < 0 }
}

fn wait_modifiers_released() {
    for _ in 0..40 {
        if !key_down(VK_CONTROL) && !key_down(VK_MENU) && !key_down(VK_SHIFT) {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn changed_clipboard(clipboard: &mut Clipboard, token: &str) -> Option<String> {
    let text = clipboard.get_text().ok()?;
    if text == token { None } else { Some(text) }
}

fn send_wm_copy() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SendMessageW};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return;
        }
        let _ = SendMessageW(hwnd, 0x0301, Some(WPARAM(0)), Some(LPARAM(0)));
    }
}

fn send_ctrl_c() -> Result<(), String> {
    let inputs = [
        key_event(VK_CONTROL, false),
        key_event(VIRTUAL_KEY(0x43), false),
        key_event(VIRTUAL_KEY(0x43), true),
        key_event(VK_CONTROL, true),
    ];
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent == 0 {
        return Err("发送 Ctrl+C 失败。".into());
    }
    Ok(())
}

fn key_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS::default()
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
