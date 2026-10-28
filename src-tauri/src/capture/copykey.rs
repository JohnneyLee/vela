//! Method two: simulated copy. Backs up the full clipboard, sends Ctrl+C (or
//! Ctrl+Insert in consoles), polls the sequence number, reads text, restores.

use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_CONTROL, VK_INSERT};

use super::clipboard;
use super::context::Ctx;
use super::error::{CaptureError, Outcome, Via};
use super::input;

pub fn copykey_selection(ctx: &Ctx, allow_partial: bool) -> Outcome {
    if ctx.elevated {
        return Outcome::Failed(CaptureError::ElevatedTarget);
    }
    if is_console(ctx) {
        // Ctrl+C would interrupt a running command. Ctrl+Insert copies without
        // that risk, and does nothing if there is no selection.
        if !input::wait_modifiers_released(Duration::from_millis(500)) {
            return Outcome::Failed(CaptureError::Other("请松开按键。".into()));
        }
        let snap = match clipboard::backup() {
            Ok(s) => s,
            Err(e) => return Outcome::Failed(CaptureError::Other(e)),
        };
        if !snap.restorable && !allow_partial {
            return Outcome::Failed(CaptureError::Other(
                "剪贴板里有无法完整备份的格式，已放弃以免弄丢内容。".into(),
            ));
        }
        let before = seq();
        if !input::send_chord(&[VK_CONTROL], VK_INSERT) {
            return Outcome::Failed(CaptureError::InputBlocked);
        }
        return finish(ctx, snap, before);
    }

    if !input::wait_modifiers_released(Duration::from_millis(500)) {
        return Outcome::Failed(CaptureError::Other("请松开按键。".into()));
    }
    let snap = match clipboard::backup() {
        Ok(s) => s,
        Err(e) => return Outcome::Failed(CaptureError::Other(e)),
    };
    if !snap.restorable && !allow_partial {
        return Outcome::Failed(CaptureError::Other(
            "剪贴板里有无法完整备份的格式，已放弃以免弄丢内容。".into(),
        ));
    }
    let before = seq();
    if !input::send_chord(&[VK_CONTROL], VIRTUAL_KEY(0x43)) {
        return Outcome::Failed(CaptureError::InputBlocked);
    }
    finish(ctx, snap, before)
}

fn finish(_ctx: &Ctx, snap: clipboard::Snapshot, before: u32) -> Outcome {
    let mut after = before;
    let mut text = None;
    // First pass: up to ~250ms.
    for _ in 0..25 {
        std::thread::sleep(Duration::from_millis(10));
        after = seq();
        if after != before {
            text = clipboard::read_text();
            break;
        }
    }
    // Second, slower pass for slow applications.
    if text.is_none() {
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(10));
            after = seq();
            if after != before {
                text = clipboard::read_text();
                break;
            }
        }
    }
    clipboard::restore(snap, after);
    match text.and_then(|t| super::error::sanitize(&t)) {
        Some(text) => Outcome::Text {
            text,
            via: Via::CopyKey,
        },
        None => Outcome::NoSelection,
    }
}

fn seq() -> u32 {
    unsafe {
        windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
    }
}

fn is_console(ctx: &Ctx) -> bool {
    let class = ctx.class.to_ascii_lowercase();
    let exe = ctx.exe.to_ascii_lowercase();
    class.contains("console")
        || exe.contains("cmd")
        || exe.contains("powershell")
        || exe.contains("pwsh")
        || exe.contains("windowsterminal")
        || exe.contains("mintty")
        || exe.contains("putty")
}
