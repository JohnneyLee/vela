//! Write method one: paste. Back up the clipboard, set the translation, send
//! the paste chord, wait for the target to read it, then restore.

use std::thread;
use std::time::Duration;

use super::policy::Policy;
use crate::capture;

pub fn paste_text(text: &str, pol: &Policy) -> Result<(), String> {
    let snap = capture::clipboard::backup().map_err(|e| e)?;
    if !snap.restorable {
        return Err("剪贴板无法完整备份，已取消替换。".into());
    }
    capture::clipboard::set_text(text)?;

    thread::sleep(Duration::from_millis(80));

    if !capture::input::send_chord(&pol.paste_chord, pol.paste_key) {
        return Err("无法向这个窗口输入。".into());
    }

    thread::sleep(Duration::from_millis(pol.restore_ms));
    let after = unsafe {
        windows::Win32::System::DataExchange::GetClipboardSequenceNumber()
    };
    capture::clipboard::restore(snap, after);
    Ok(())
}
