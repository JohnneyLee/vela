//! Snapshot of the trigger moment: which window, process, and whether it is elevated.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, POINT};
use windows::Win32::Security::{
    GetTokenInformation, TokenElevation, TokenElevationTypeFull, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId,
};

pub struct Ctx {
    #[allow(dead_code)]
    pub hwnd: HWND,
    #[allow(dead_code)]
    pub pid: u32,
    pub exe: String,
    pub class: String,
    pub elevated: bool,
    #[allow(dead_code)]
    pub cursor: POINT,
}

pub fn collect() -> Ctx {
    let hwnd = unsafe { GetForegroundWindow() };
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    let class = class_name(hwnd);
    let exe = process_name(pid);
    let elevated = is_elevated(pid);
    let cursor = cursor_pos();
    Ctx {
        hwnd,
        pid,
        exe,
        class,
        elevated,
        cursor,
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

pub(crate) fn process_name(pid: u32) -> String {
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return String::new();
    };
    let mut buf = [0u16; 260];
    let mut size = (buf.len() * 2) as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
        .is_ok()
    };
    let _ = unsafe { CloseHandle(handle) };
    if !ok {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_string()
}

fn is_elevated(pid: u32) -> bool {
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        // Cannot even query: treat as possibly elevated.
        return true;
    };
    let mut token = HANDLE::default();
    let opened =
        unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) }.is_ok();
    let mut elevated = true;
    if opened {
        let mut info = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some(&mut info as *mut _ as *mut core::ffi::c_void),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
            .is_ok()
        };
        if ok {
            elevated = info.TokenIsElevated != 0;
        }
        let _ = unsafe { CloseHandle(token) };
    }
    let _ = unsafe { CloseHandle(handle) };
    elevated
}

fn cursor_pos() -> POINT {
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    pt
}

#[allow(dead_code)]
fn _elevation_type_guard() {
    // TokenElevationTypeFull is referenced to document intent; the runtime
    // check above uses TokenElevation for the actual elevated flag.
    let _ = TokenElevationTypeFull;
    let _: fn() -> windows::Win32::Security::TOKEN_INFORMATION_CLASS = || TokenElevation;
    let _: PROCESS_NAME_FORMAT = PROCESS_NAME_WIN32;
}
