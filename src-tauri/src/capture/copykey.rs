//! Method two: simulated copy. Backs up the full clipboard, sends Ctrl+C (or
//! Ctrl+Insert in consoles), polls the sequence number, reads text, restores.
//!
//! Electron/Cursor diagnosis (2026-10-28 logs): PostMessage WM_COPY and tagged
//! SendInput Ctrl+C both return success but never bump the clipboard sequence.
//! Fixes tried here: SendMessageTimeout WM_COPY to Chrome_RenderWidgetHostHWND,
//! untagged scancode Ctrl+C / Ctrl+Insert with AttachThreadInput.

use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_CONTROL, VK_INSERT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, GetCursorPos, GetForegroundWindow, GetGUIThreadInfo,
    GetWindowThreadProcessId, PostMessageW, SendMessageTimeoutW, WindowFromPoint, GUITHREADINFO,
    SEND_MESSAGE_TIMEOUT_FLAGS, SMTO_ABORTIFHUNG, WM_COPY,
};

use super::clipboard;
use super::context::Ctx;
use super::error::{CaptureError, Outcome, Via};
use super::input;
use super::log;

pub fn copykey_selection(ctx: &Ctx, allow_partial: bool) -> Outcome {
    if ctx.elevated {
        return Outcome::Failed(CaptureError::ElevatedTarget);
    }
    if is_console(ctx) {
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
        return finish(ctx, snap, before, "ctrl_insert");
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

    let electron = super::is_electron_editor(&ctx.exe);
    if electron {
        return electron_copy(ctx, snap, allow_partial);
    }

    let before = seq();
    if !input::send_chord(&[VK_CONTROL], VIRTUAL_KEY(0x43)) {
        return Outcome::Failed(CaptureError::InputBlocked);
    }
    finish(ctx, snap, before, "ctrl_c")
}

fn electron_copy(ctx: &Ctx, snap: clipboard::Snapshot, allow_partial: bool) -> Outcome {
    let prev = snap.unicode_text();

    // 1) SendMessageTimeout WM_COPY to Chromium render widget(s) under cursor /
    //    focus — PostMessage to the top-level frame was a no-op (seq unchanged).
    let before = seq();
    let targets = copy_target_hwnds();
    let mut any_ok = false;
    for (hwnd, class) in &targets {
        let mut result = 0usize;
        let ok = unsafe {
            SendMessageTimeoutW(
                *hwnd,
                WM_COPY,
                WPARAM(0),
                LPARAM(0),
                SEND_MESSAGE_TIMEOUT_FLAGS(SMTO_ABORTIFHUNG.0),
                200,
                Some(&mut result),
            )
            .0 != 0
        };
        any_ok |= ok;
        log::log_diag(&format!(
            "copykey method=wm_copy_send class={class} ok={ok} seq_before={before} exe={}",
            ctx.exe
        ));
        if ok {
            if let Some(text) = poll_text(before, &prev, 30, 30) {
                let after = seq();
                log_finish("wm_copy_send", before, after, true, &ctx.exe);
                clipboard::restore(snap, after);
                return sanitize_text(text);
            }
        }
    }
    if !any_ok {
        // Fallback PostMessage to focus (logged previously as posted but no-op).
        let _ = post_wm_copy_focus();
    }
    log_finish("wm_copy_send", before, seq(), false, &ctx.exe);

    // 2) Untagged scancode Ctrl+C with AttachThreadInput.
    std::thread::sleep(Duration::from_millis(30));
    let before = seq();
    let sent = with_attached_input(|| input::send_ctrl_c_scancode());
    log::log_diag(&format!(
        "copykey method=ctrl_c_scancode sent={sent} seq_before={before} exe={}",
        ctx.exe
    ));
    if sent {
        if let Some(text) = poll_text(before, &prev, 40, 60) {
            let after = seq();
            log_finish("ctrl_c_scancode", before, after, true, &ctx.exe);
            clipboard::restore(snap, after);
            return sanitize_text(text);
        }
        log_finish("ctrl_c_scancode", before, seq(), false, &ctx.exe);
    }

    // 3) Ctrl+Insert scancode — often bypasses IME's Ctrl+C handling.
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
    let sent = with_attached_input(|| input::send_ctrl_insert_scancode());
    log::log_diag(&format!(
        "copykey method=ctrl_insert_scancode sent={sent} seq_before={before} exe={}",
        ctx.exe
    ));
    if !sent {
        clipboard::restore(snap, before);
        return Outcome::Failed(CaptureError::InputBlocked);
    }
    finish(ctx, snap, before, "ctrl_insert_scancode")
}

fn sanitize_text(text: String) -> Outcome {
    match super::error::sanitize(&text) {
        Some(text) => Outcome::Text {
            text,
            via: Via::CopyKey,
        },
        None => Outcome::NoSelection,
    }
}

fn log_finish(method: &str, before: u32, after: u32, has_text: bool, exe: &str) {
    log::log_diag(&format!(
        "copykey finish method={method} seq_before={before} seq_after={after} seq_changed={} has_text={has_text} exe={exe}",
        after != before
    ));
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

fn copy_target_hwnds() -> Vec<(HWND, String)> {
    let mut out: Vec<(HWND, String)> = Vec::new();
    let mut push = |hwnd: HWND| {
        if hwnd.0.is_null() {
            return;
        }
        let class = class_name(hwnd);
        if out.iter().any(|(h, _)| h.0 == hwnd.0) {
            return;
        }
        out.push((hwnd, class));
    };

    unsafe {
        let mut pt = windows::Win32::Foundation::POINT::default();
        let _ = GetCursorPos(&mut pt);
        let under = WindowFromPoint(pt);
        push(under);

        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(0, &mut info).is_ok() {
            push(info.hwndFocus);
            push(info.hwndCaret);
        }
        push(GetForegroundWindow());

        // Prefer Chromium render widgets: collect children of the root under cursor.
        let root = {
            let mut hwnd = under;
            for _ in 0..6 {
                match windows::Win32::UI::WindowsAndMessaging::GetParent(hwnd) {
                    Ok(parent) if !parent.0.is_null() => hwnd = parent,
                    _ => break,
                }
            }
            hwnd
        };
        let mut found: Vec<HWND> = Vec::new();
        let _ = EnumChildWindows(
            Some(root),
            Some(enum_render_proc),
            LPARAM(&mut found as *mut Vec<HWND> as isize),
        );
        for hwnd in found {
            push(hwnd);
        }
    }

    // Prefer render-widget classes first.
    out.sort_by(|a, b| {
        let score = |c: &str| {
            let l = c.to_ascii_lowercase();
            if l.contains("chrome_renderwidgethosthwnd") {
                0
            } else if l.contains("chrome_") {
                1
            } else {
                2
            }
        };
        score(&a.1).cmp(&score(&b.1))
    });
    out.truncate(8);
    out
}

unsafe extern "system" fn enum_render_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let list = &mut *(lparam.0 as *mut Vec<HWND>);
    let class = class_name(hwnd).to_ascii_lowercase();
    if class.contains("chrome_renderwidgethosthwnd") || class.contains("intermediate d3d") {
        list.push(hwnd);
    }
    windows::core::BOOL(1) // continue
}

fn post_wm_copy_focus() -> bool {
    unsafe {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let focus = if GetGUIThreadInfo(0, &mut info).is_ok() && !info.hwndFocus.0.is_null() {
            info.hwndFocus
        } else {
            GetForegroundWindow()
        };
        if focus.0.is_null() {
            return false;
        }
        PostMessageW(Some(focus), WM_COPY, WPARAM(0), LPARAM(0)).is_ok()
    }
}

fn with_attached_input<F: FnOnce() -> bool>(f: F) -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        let fg_tid = GetWindowThreadProcessId(fg, None);
        let cur = GetCurrentThreadId();
        let attached = fg_tid != 0 && fg_tid != cur;
        if attached {
            let _ = AttachThreadInput(cur, fg_tid, true);
        }
        let sent = f();
        if attached {
            let _ = AttachThreadInput(cur, fg_tid, false);
        }
        sent
    }
}

fn poll_text(before: u32, prev: &Option<String>, first: u32, second: u32) -> Option<String> {
    for _ in 0..first {
        std::thread::sleep(Duration::from_millis(10));
        if seq() != before {
            if let Some(t) = clipboard::read_text() {
                return Some(t);
            }
        }
    }
    for _ in 0..second {
        std::thread::sleep(Duration::from_millis(10));
        if seq() != before {
            if let Some(t) = clipboard::read_text() {
                return Some(t);
            }
        }
    }
    if let Some(now) = clipboard::read_text() {
        let changed = match prev {
            Some(old) => old != &now,
            None => !now.trim().is_empty(),
        };
        if changed {
            return Some(now);
        }
    }
    None
}

fn finish(ctx: &Ctx, snap: clipboard::Snapshot, before: u32, method: &str) -> Outcome {
    let electron = super::is_electron_editor(&ctx.exe);
    let prev = snap.unicode_text();
    let first = if electron { 40 } else { 25 };
    let second = if electron { 80 } else { 60 };
    let text = poll_text(before, &prev, first, second);
    let after = seq();
    log_finish(
        method,
        before,
        after,
        text.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false),
        &ctx.exe,
    );
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
    unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() }
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
