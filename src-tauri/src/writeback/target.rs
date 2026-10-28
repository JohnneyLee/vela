//! Target bookkeeping: remember the source window (and its process) before
//! Vela's popup takes focus, then switch back and verify before writing.

use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};

static SOURCE_HWND: AtomicIsize = AtomicIsize::new(0);
static SOURCE_PID: AtomicU32 = AtomicU32::new(0);
static SOURCE_EXE: Mutex<String> = Mutex::new(String::new());

pub fn remember() {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        SOURCE_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        SOURCE_PID.store(pid, Ordering::Relaxed);
    }
    let exe = crate::capture::context::process_name(SOURCE_PID.load(Ordering::Relaxed));
    if let Ok(mut slot) = SOURCE_EXE.lock() {
        *slot = exe;
    }
}

pub fn has_source() -> bool {
    SOURCE_HWND.load(Ordering::Relaxed) != 0
}

pub fn source_exe() -> String {
    SOURCE_EXE.lock().map(|s| s.clone()).unwrap_or_default()
}

pub fn source() -> Option<HWND> {
    let raw = SOURCE_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        None
    } else {
        Some(HWND(raw as *mut core::ffi::c_void))
    }
}

pub fn clear() {
    SOURCE_HWND.store(0, Ordering::Relaxed);
    SOURCE_PID.store(0, Ordering::Relaxed);
    if let Ok(mut slot) = SOURCE_EXE.lock() {
        slot.clear();
    }
}

/// Bring the source window back to the foreground and confirm it is still the
/// same process. The user just clicked a Vela button, so Vela is allowed to
/// hand focus back to the source window.
pub fn switch_back_and_verify() -> Result<u32, String> {
    let Some(hwnd) = source() else {
        return Err("没有找到原来的输入框，译文可以复制。".into());
    };
    let pid = SOURCE_PID.load(Ordering::Relaxed);

    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }

    // Wait up to 300ms for the foreground window to become the target again.
    for _ in 0..30 {
        unsafe {
            let fg = GetForegroundWindow();
            let mut fg_pid = 0u32;
            GetWindowThreadProcessId(fg, Some(&mut fg_pid));
            if fg.0 == hwnd.0 || (pid != 0 && fg_pid == pid) {
                return Ok(pid);
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err("无法切回原来的窗口，译文可以复制。".into())
}
