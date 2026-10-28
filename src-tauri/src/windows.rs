//! Window management: the action bar (non-activating), the result panel, and
//! the settings window. The bar is created hidden at startup and only shown /
//! repositioned when a selection is detected.

use std::sync::atomic::{AtomicIsize, Ordering};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, GetWindowRect, IsWindowVisible, SetWindowLongW, GWL_EXSTYLE,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

const BAR_LABEL: &str = "bar";
const PANEL_LABEL: &str = "panel";
const SETTINGS_LABEL: &str = "settings";

static BAR_HWND: AtomicIsize = AtomicIsize::new(0);

/// Make a window non-activating at the Windows level so it never steals focus.
pub fn apply_no_activate(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        if let Ok(handle) = window.hwnd() {
            BAR_HWND.store(handle.0 as isize, Ordering::Relaxed);
            // tauri's HWND may come from a different `windows` crate version;
            // re-wrap through the raw pointer to match our own crate's type.
            let hwnd = windows::Win32::Foundation::HWND(handle.0 as *mut core::ffi::c_void);
            unsafe {
                let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
                let ex = ex | (WS_EX_NOACTIVATE.0 as i32) | (WS_EX_TOOLWINDOW.0 as i32);
                SetWindowLongW(hwnd, GWL_EXSTYLE, ex);
            }
        }
    }
}

/// Whether the action bar window is currently visible. Safe from any thread.
pub fn bar_visible() -> bool {
    let raw = BAR_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return false;
    }
    unsafe {
        IsWindowVisible(windows::Win32::Foundation::HWND(raw as *mut core::ffi::c_void))
            .as_bool()
    }
}

/// Whether a screen point lies inside the action bar window. Safe from any thread.
pub fn bar_contains(x: i32, y: i32) -> bool {
    let raw = BAR_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return false;
    }
    unsafe {
        let mut rect = RECT::default();
        let hwnd = windows::Win32::Foundation::HWND(raw as *mut core::ffi::c_void);
        if GetWindowRect(hwnd, &mut rect).is_ok() {
            x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
        } else {
            false
        }
    }
}

pub fn bar(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(BAR_LABEL)
}

pub fn panel(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(PANEL_LABEL)
}

pub fn settings(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(SETTINGS_LABEL)
}

/// Show the action bar near the given screen point, clamped into the work area.
/// Safe to call from any thread: window calls are dispatched to the main loop.
pub fn show_bar(app: &AppHandle, x: i32, y: i32, suggested: String) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn(async move {
        let Some(window) = bar(&app) else {
            return;
        };
        let (px, py) = crate::selection::clamp_near_cursor(x, y, 220, 72);
        let _ = window.set_position(PhysicalPosition::new(px, py));
        let _ = window.show();
        let _ = app.emit("bar:show", serde_json::json!({ "suggested": suggested }));
    });
}

pub fn hide_bar(app: &AppHandle) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn(async move {
        if let Some(window) = bar(&app) {
            let _ = window.hide();
        }
    });
}

pub fn open_settings(app: &AppHandle) {
    if let Some(window) = settings(app) {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn close_settings(app: &AppHandle) {
    if let Some(window) = settings(app) {
        let _ = window.hide();
    }
}
