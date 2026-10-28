//! Small icon that appears beside a text selection, like Eudic.
//! The window never activates, so the original app keeps the selection and Ctrl+C still copies it.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EndPaint, GetDC,
    MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO,
    BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
    RGBQUAD,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetDoubleClickTime, VK_CONTROL, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, LoadCursorW,
    PostMessageW, RegisterClassExW, SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage,
    HHOOK, HWND_TOPMOST, IDC_HAND, KBDLLHOOKSTRUCT, MA_NOACTIVATE, MSG, MSLLHOOKSTRUCT,
    SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, UpdateLayeredWindow,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WNDCLASSEXW, WM_APP, WM_DESTROY, WM_ERASEBKGND, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_PAINT, WM_TIMER, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const WM_PICKER_HIDE: u32 = WM_APP + 2;
const WM_PICKER_CLICK: u32 = WM_APP + 3;
const WM_PICKER_PROBE: u32 = WM_APP + 4;
const HIDE_TIMER_ID: usize = 1;
const PROBE_TIMER_ID: usize = 2;
const PROBE_SETTLE_MS: u32 = 300;

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
static PICKER_HWND: AtomicIsize = AtomicIsize::new(0);
static STARTED: OnceLock<Instant> = OnceLock::new();

thread_local! {
    static HWND_SLOT: Cell<isize> = const { Cell::new(0) };
    static MOUSE_HOOK: Cell<isize> = const { Cell::new(0) };
    static KEY_HOOK: Cell<isize> = const { Cell::new(0) };
    static DOWN: Cell<bool> = const { Cell::new(false) };
    static DOWN_X: Cell<i32> = const { Cell::new(0) };
    static DOWN_Y: Cell<i32> = const { Cell::new(0) };
    static LAST_UP_MS: Cell<u32> = const { Cell::new(0) };
    static LAST_UP_X: Cell<i32> = const { Cell::new(0) };
    static LAST_UP_Y: Cell<i32> = const { Cell::new(0) };
    static VISIBLE: Cell<bool> = const { Cell::new(false) };
    static ICON_X: Cell<i32> = const { Cell::new(0) };
    static ICON_Y: Cell<i32> = const { Cell::new(0) };
    static ICON_S: Cell<i32> = const { Cell::new(32) };
    static SHOW_X: Cell<i32> = const { Cell::new(0) };
    static SHOW_Y: Cell<i32> = const { Cell::new(0) };
    static PROBE_TICK: Cell<u32> = const { Cell::new(0) };
    static PENDING: RefCell<String> = RefCell::new(String::new());
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ARMED_TEXT: RefCell<String> = RefCell::new(String::new());
}

pub fn start(app: tauri::AppHandle) {
    if APP.set(app).is_err() {
        return;
    }
    let _ = STARTED.set(Instant::now());
    std::thread::Builder::new()
        .name("vela-picker".into())
        .spawn(|| unsafe { run_picker() })
        .ok();
}

pub fn hide() {
    let hwnd = PICKER_HWND.load(Ordering::Acquire);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd as *mut _)),
                WM_PICKER_HIDE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

fn hwnd_handle() -> HWND {
    HWND(HWND_SLOT.with(|slot| slot.get()) as *mut _)
}

fn now_ms() -> u32 {
    STARTED
        .get()
        .map(|start| start.elapsed().as_millis() as u32)
        .unwrap_or(0)
}

fn foreground_is_us() -> bool {
    unsafe {
        let hwnd = GetForegroundWindow();
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid == std::process::id()
    }
}

fn dpi_at(x: i32, y: i32) -> u32 {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut dx = 96u32;
        let mut dy = 96u32;
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() {
            dx.max(96)
        } else {
            96
        }
    }
}

fn icon_contains(x: i32, y: i32, pad: i32) -> bool {
    let origin_x = ICON_X.with(|slot| slot.get());
    let origin_y = ICON_Y.with(|slot| slot.get());
    let side = ICON_S.with(|slot| slot.get());
    x >= origin_x - pad
        && y >= origin_y - pad
        && x < origin_x + side + pad
        && y < origin_y + side + pad
}

fn point_in_icon(x: i32, y: i32) -> bool {
    VISIBLE.with(|slot| slot.get()) && icon_contains(x, y, 16)
}

fn arm_pending() {
    let text = PENDING.with(|slot| slot.borrow().clone());
    ARMED_TEXT.with(|slot| *slot.borrow_mut() = text);
    ARMED.with(|slot| slot.set(true));
}

fn clear_armed() {
    ARMED.with(|slot| slot.set(false));
    ARMED_TEXT.with(|slot| *slot.borrow_mut() = String::new());
}

fn post(msg: u32) {
    let hwnd = hwnd_handle();
    if !hwnd.0.is_null() {
        unsafe {
            let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
        }
    }
}

fn remember_show(x: i32, y: i32) {
    SHOW_X.with(|slot| slot.set(x));
    SHOW_Y.with(|slot| slot.set(y));
    PROBE_TICK.with(|slot| slot.set(slot.get().wrapping_add(1)));
    let hwnd = hwnd_handle();
    if hwnd.0.is_null() {
        return;
    }
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), PROBE_TIMER_ID);
        let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(
            Some(hwnd),
            PROBE_TIMER_ID,
            PROBE_SETTLE_MS,
            None,
        );
    }
}

fn probe_and_show() {
    let tick = PROBE_TICK.with(|slot| slot.get());
    if foreground_is_us() {
        return;
    }
    let ctrl_down = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
    let c_down = unsafe { GetAsyncKeyState(0x43) } < 0;
    if ctrl_down && c_down {
        return;
    }
    if PROBE_TICK.with(|slot| slot.get()) != tick {
        return;
    }
    // Only probe with UI Automation — never touch the clipboard on a selection.
    // If UIA can't confirm text but a selection gesture happened, still show
    // the bar; the actual capture runs when the user clicks an action.
    let suggested = match crate::capture::probe_uia_only() {
        crate::capture::Outcome::Text { text, .. } => recommend(&text),
        crate::capture::Outcome::Failed(crate::capture::CaptureError::PasswordField) => return,
        _ => "translate".to_string(),
    };
    let x = SHOW_X.with(|slot| slot.get());
    let y = SHOW_Y.with(|slot| slot.get());
    if let Some(app) = APP.get() {
        // Record the source window now, while it is still the foreground window
        // (the bar itself never takes focus). bar_pick will switch back to it
        // before capturing, so UIA/copy target the app the text was selected in.
        crate::writeback::target::remember();
        crate::windows::show_bar(app, x, y, suggested);
    }
}

fn recommend(text: &str) -> String {
    use crate::suggest::{self, Lang};
    let editable = crate::capture::is_editable_exe(&crate::capture::current_exe());
    let hint = suggest::Hint {
        editable,
        lang: suggest::detect_lang(text),
        word_count: suggest::word_count(text),
        symbol_ratio: suggest::symbol_ratio(text),
        chars: text.chars().count(),
    };
    // Native language defaults to Chinese; wire to settings later.
    suggest::suggest(&hint, Lang::Zh).id().to_string()
}

fn on_mouse(wparam: WPARAM, lparam: LPARAM) {
    let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
    let x = info.pt.x;
    let y = info.pt.y;
    match wparam.0 as u32 {
        WM_LBUTTONDOWN => {
            DOWN.with(|slot| slot.set(true));
            DOWN_X.with(|slot| slot.set(x));
            DOWN_Y.with(|slot| slot.set(y));
            // If the action bar is up and the click is outside it, dismiss it.
            if crate::windows::bar_visible() && !crate::windows::bar_contains(x, y) {
                if let Some(app) = APP.get() {
                    crate::windows::hide_bar(app);
                }
            }
            if point_in_icon(x, y) {
                arm_pending();
            } else if VISIBLE.with(|slot| slot.get()) {
                post(WM_PICKER_HIDE);
            }
        }
        WM_LBUTTONUP => {
            let was_down = DOWN.with(|slot| slot.replace(false));
            let dx = x - DOWN_X.with(|slot| slot.get());
            let dy = y - DOWN_Y.with(|slot| slot.get());
            let moved = dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy)) > 12 * 12;
            let previous = LAST_UP_MS.with(|slot| slot.get());
            let double = now_ms().wrapping_sub(previous) < unsafe { GetDoubleClickTime() }
                && (x - LAST_UP_X.with(|slot| slot.get())).abs() < 8
                && (y - LAST_UP_Y.with(|slot| slot.get())).abs() < 8;
            LAST_UP_MS.with(|slot| slot.set(now_ms()));
            LAST_UP_X.with(|slot| slot.set(x));
            LAST_UP_Y.with(|slot| slot.set(y));
            if ARMED.with(|slot| slot.get()) && icon_contains(x, y, 24) {
                post(WM_PICKER_CLICK);
            } else if was_down && (moved || double) && !foreground_is_us() {
                clear_armed();
                remember_show(x, y);
            } else {
                clear_armed();
            }
        }
        _ => {}
    }
}

fn on_key(wparam: WPARAM, lparam: LPARAM) {
    if wparam.0 as u32 != WM_KEYUP || foreground_is_us() {
        return;
    }
    let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
    let vk = info.vkCode;
    let shift = unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) } < 0;
    let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } < 0;
    let arrows = matches!(vk, 0x21..=0x28);
    let select_all = ctrl && vk == 0x41;
    let copying = ctrl && vk == 0x43;
    if !copying && ((shift && arrows) || select_all) {
        let mut pt = POINT::default();
        if unsafe { GetCursorPos(&mut pt) }.is_ok() {
            remember_show(pt.x, pt.y);
        }
    }
    if vk == 0x1B {
        // Esc hides both the legacy icon and the action bar.
        if crate::windows::bar_visible() {
            if let Some(app) = APP.get() {
                crate::windows::hide_bar(app);
            }
        }
        post(WM_PICKER_HIDE);
    }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        on_mouse(wparam, lparam);
    }
    let hook = HHOOK(MOUSE_HOOK.with(|slot| slot.get()) as *mut _);
    unsafe { CallNextHookEx(Some(hook), code, wparam, lparam) }
}

unsafe extern "system" fn key_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 {
        on_key(wparam, lparam);
    }
    let hook = HHOOK(KEY_HOOK.with(|slot| slot.get()) as *mut _);
    unsafe { CallNextHookEx(Some(hook), code, wparam, lparam) }
}

fn show_icon() {
    let x = SHOW_X.with(|slot| slot.get());
    let y = SHOW_Y.with(|slot| slot.get());
    let size = ((32 * dpi_at(x, y) as i32) / 96).clamp(32, 64);
    let gap = (4 * dpi_at(x, y) as i32 / 96).max(2);
    let (px, py) = crate::selection::clamp_into_work(x + gap, y - size / 2, size, size);
    let hwnd = hwnd_handle();
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            px,
            py,
            size,
            size,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        present_icon(hwnd, px, py, size);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    ICON_X.with(|slot| slot.set(px));
    ICON_Y.with(|slot| slot.set(py));
    ICON_S.with(|slot| slot.set(size));
    VISIBLE.with(|slot| slot.set(true));
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(
            Some(hwnd),
            HIDE_TIMER_ID,
            5000,
            None,
        );
    }
}

fn forget_pending() {
    PENDING.with(|slot| *slot.borrow_mut() = String::new());
}

fn hide_icon() {
    VISIBLE.with(|slot| slot.set(false));
    forget_pending();
    let hwnd = hwnd_handle();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(Some(hwnd), HIDE_TIMER_ID);
        let _ = ShowWindow(hwnd, SW_HIDE);
    }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut paint);
        if !hdc.is_invalid() {
            let _ = EndPaint(hwnd, &paint);
        }
    }
}

fn icon_pixels(size: i32) -> Vec<u8> {
    let n = size.max(1) as usize;
    let mut buf = vec![0u8; n * n * 4];
    let s = n as f32;
    let center = s * 0.5;
    let radius = s * 0.5 - 0.6;
    for y in 0..n {
        for x in 0..n {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - center;
            let dy = py - center;
            let cover = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let mark = bar_coverage(px, py, s);
            let red = 28.0 + (255.0 - 28.0) * mark;
            let green = 78.0 + (255.0 - 78.0) * mark;
            let blue = 115.0 + (255.0 - 115.0) * mark;
            let i = (y * n + x) * 4;
            buf[i] = (blue * cover) as u8;
            buf[i + 1] = (green * cover) as u8;
            buf[i + 2] = (red * cover) as u8;
            buf[i + 3] = (cover * 255.0) as u8;
        }
    }
    buf
}

fn bar_coverage(px: f32, py: f32, s: f32) -> f32 {
    let radius = s * 0.07;
    let top = capsule(px, py, s * 0.20, s * 0.37, s * 0.56, s * 0.37, radius);
    let bottom = capsule(px, py, s * 0.44, s * 0.63, s * 0.80, s * 0.63, radius);
    top.max(bottom)
}

fn capsule(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32, radius: f32) -> f32 {
    let abx = bx - ax;
    let aby = by - ay;
    let apx = px - ax;
    let apy = py - ay;
    let len2 = abx * abx + aby * aby;
    let t = if len2 == 0.0 {
        0.0
    } else {
        ((apx * abx + apy * aby) / len2).clamp(0.0, 1.0)
    };
    let dx = apx - abx * t;
    let dy = apy - aby * t;
    (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0)
}

fn present_icon(hwnd: HWND, x: i32, y: i32, size: i32) {
    let pixels = icon_pixels(size);
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return;
        }
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            let _ = ReleaseDC(None, screen);
            return;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            bmiColors: [RGBQUAD::default()],
        };
        let mut bits = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(None, screen);
            return;
        };
        if bits.is_null() {
            let _ = DeleteObject(dib.into());
            let _ = DeleteDC(mem);
            let _ = ReleaseDC(None, screen);
            return;
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
        let previous = SelectObject(mem, dib.into());
        let dst = POINT { x, y };
        let src = POINT { x: 0, y: 0 };
        let dim = SIZE { cx: size, cy: size };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let _ = UpdateLayeredWindow(
            hwnd,
            Some(screen),
            Some(&dst),
            Some(&dim),
            Some(mem),
            Some(&src),
            COLORREF(0),
            Some(&blend),
            ULW_ALPHA,
        );
        SelectObject(mem, previous);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        let _ = ReleaseDC(None, screen);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            paint(hwnd);
            LRESULT(0)
        }
        WM_PICKER_PROBE => {
            probe_and_show();
            LRESULT(0)
        }
        WM_PICKER_HIDE => {
            hide_icon();
            LRESULT(0)
        }
        WM_TIMER => {
            match wparam.0 {
                HIDE_TIMER_ID => hide_icon(),
                PROBE_TIMER_ID => {
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::KillTimer(
                            Some(hwnd),
                            PROBE_TIMER_ID,
                        );
                    }
                    probe_and_show();
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_PICKER_CLICK => {
            let pending = PENDING.with(|slot| std::mem::take(&mut *slot.borrow_mut()));
            let text = if pending.is_empty() {
                ARMED_TEXT.with(|slot| slot.borrow().clone())
            } else {
                pending
            };
            hide_icon();
            if !text.is_empty() {
                if let Some(app) = APP.get() {
                    crate::show_captured(app, text);
                }
            }
            clear_armed();
            LRESULT(0)
        }
        WM_DESTROY => LRESULT(0),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn run_picker() {
    let instance = match GetModuleHandleW(None) {
        Ok(module) => windows::Win32::Foundation::HINSTANCE(module.0),
        Err(err) => {
            eprintln!("Vela picker failed to get the module handle: {err}");
            return;
        }
    };
    let cursor = LoadCursorW(None, IDC_HAND).unwrap_or_default();
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: w!("VelaPicker"),
        ..Default::default()
    };
    if RegisterClassExW(&class) == 0 {
        eprintln!("Vela picker failed to register its window class");
        return;
    }
    let hwnd = match CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        w!("VelaPicker"),
        w!("Vela"),
        WS_POPUP,
        0,
        0,
        32,
        32,
        None,
        None,
        Some(instance),
        None,
    ) {
        Ok(hwnd) => hwnd,
        Err(err) => {
            eprintln!("Vela picker failed to create its window: {err}");
            return;
        }
    };
    HWND_SLOT.with(|slot| slot.set(hwnd.0 as isize));
    PICKER_HWND.store(hwnd.0 as isize, Ordering::Release);

    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0);
    let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), None, 0);
    match (mouse, keyboard) {
        (Ok(mouse), Ok(keyboard)) => {
            MOUSE_HOOK.with(|slot| slot.set(mouse.0 as isize));
            KEY_HOOK.with(|slot| slot.set(keyboard.0 as isize));
        }
        (mouse, keyboard) => {
            eprintln!(
                "Vela picker hooks were not installed ({:?} / {:?})",
                mouse.err(),
                keyboard.err()
            );
            return;
        }
    }

    let mut message = MSG::default();
    while GetMessageW(&mut message, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}

#[cfg(test)]
mod tests {
    use super::icon_pixels;

    #[test]
    fn selection_icon_is_a_circle_with_two_offset_bars() {
        let size = 64i32;
        let px = icon_pixels(size);
        let at = |x: i32, y: i32| {
            let i = ((y * size + x) as usize) * 4;
            (px[i], px[i + 1], px[i + 2], px[i + 3])
        };
        assert_eq!(at(0, 0).3, 0);
        let center = at(32, 32);
        assert!(center.3 > 250);
        assert!(center.2 < 80, "center should stay blue, got {center:?}");
        let top = at(24, 24);
        assert!(top.0 > 220 && top.1 > 220 && top.2 > 220, "top bar should be white, got {top:?}");
        let bottom = at(40, 40);
        assert!(
            bottom.0 > 220 && bottom.1 > 220 && bottom.2 > 220,
            "bottom bar should be white, got {bottom:?}"
        );
        let before_bottom = at(14, 40);
        assert!(
            before_bottom.2 < 80,
            "bottom bar should start further right, got {before_bottom:?}"
        );
        let after_top = at(48, 24);
        assert!(after_top.2 < 80, "top bar should end further left, got {after_top:?}");
    }
}
