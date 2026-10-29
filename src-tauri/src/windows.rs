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

const ICON_W: i32 = 52;
const ICON_H: i32 = 52;
const EXPANDED_W: i32 = 220;
const EXPANDED_H: i32 = 72;

/// Show the collapsed entry icon near the selection, offset so it does not sit
/// under the cursor tip. Safe from any thread.
pub fn show_bar(app: &AppHandle, x: i32, y: i32, suggested: String) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn(async move {
        let Some(window) = bar(&app) else {
            return;
        };
        apply_no_activate(&window);
        let _ = window.set_size(tauri::LogicalSize::new(ICON_W as f64, ICON_H as f64));
        // 离光标再远一点，避免落在右下角一两像素处。
        let (px, py) = crate::selection::clamp_near_cursor(x + 12, y + 8, ICON_W, ICON_H);
        let _ = window.set_position(PhysicalPosition::new(px, py));
        let _ = window.show();
        let _ = app.emit(
            "bar:show",
            serde_json::json!({ "suggested": suggested, "collapsed": true }),
        );
    });
}

pub fn set_bar_layout(app: &AppHandle, mode: &str) {
    let expanded = mode == "expanded";
    let app = app.clone();
    let _ = tauri::async_runtime::spawn(async move {
        let Some(window) = bar(&app) else {
            return;
        };
        apply_no_activate(&window);
        let (w, h) = if expanded {
            (EXPANDED_W, EXPANDED_H)
        } else {
            (ICON_W, ICON_H)
        };
        // Keep top-left fixed so the recommended action sits where the icon was.
        let pos = window.outer_position().ok();
        let _ = window.set_size(tauri::LogicalSize::new(w as f64, h as f64));
        if let Some(p) = pos {
            let _ = window.set_position(p);
        }
    });
}

pub fn hide_bar(app: &AppHandle) {
    let app = app.clone();
    let _ = tauri::async_runtime::spawn(async move {
        let _ = app.emit("bar:hide", ());
        if let Some(window) = bar(&app) {
            let _ = window.hide();
            let _ = window.set_size(tauri::LogicalSize::new(ICON_W as f64, ICON_H as f64));
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
