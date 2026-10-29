//! Clipboard backup/restore with format enumeration and sequence-number checks.
//! Text, files, and DIB images are copied byte-for-byte; GDI handle formats
//! cannot be copied and mark the snapshot as non-restorable.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardSequenceNumber, GetOpenClipboardWindow, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GHND,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Formats the system derives from others; restoring their source is enough.
const DERIVED: [u32; 3] = [1 /*CF_TEXT*/, 7 /*CF_OEMTEXT*/, 16 /*CF_LOCALE*/];

/// GDI/handle formats we cannot copy byte-for-byte. These are almost always
/// redundant previews of content that is also present in a byte-copyable format
/// (CF_DIB for bitmaps, CF_UNICODETEXT/CF_HTML for text), so we skip them
/// instead of failing the whole capture.
const HANDLE_FORMATS: [u32; 13] = [
    2,  // CF_BITMAP
    3,  // CF_METAFILEPICT
    4,  // CF_SYLK
    5,  // CF_DIF
    6,  // CF_TIFF
    9,  // CF_PALETTE
    10, // CF_PENDATA
    11, // CF_RIFF
    30|    12, // CF_WAVE
    14, // CF_ENHMETAFILE
    0x0080, // CF_DSPTEXT
    0x0082, // CF_DSPBITMAP
    0x008E, // CF_DSPENHMETAFILE
];

const CF_BITMAP: u32 = 2;
const CF_DIB: u32 = 8;

/// Formats whose loss during restore would be real damage: text, file list,
/// and bitmap bytes. Any *other* format that fails to read is a private /
/// delayed-render / decoration format (Office, WPS, WeChat, browsers put these
/// on the clipboard alongside text) and is dropped instead of failing capture.
const CRITICAL: [u32; 3] = [
    13, // CF_UNICODETEXT
    15, // CF_HDROP (file list)
    8,  // CF_DIB
];

pub struct Snapshot {
    items: Vec<(u32, Vec<u8>)>,
    pub restorable: bool,
}

impl Snapshot {
    /// 备份里的纯文本（若有），供复制前后比对。
    pub fn unicode_text(&self) -> Option<String> {
        let fmt = CF_UNICODETEXT.0 as u32;
        let bytes = self.items.iter().find(|(f, _)| *f == fmt)?.1.as_slice();
        if bytes.len() < 2 {
            return None;
        }
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_ne_bytes([c[0], c[1]]))
            .collect();
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let text = String::from_utf16_lossy(&units[..end]);
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

pub fn backup() -> Result<Snapshot, String> {
    // Backoff so a briefly-held clipboard (clipboard managers, IMEs) can finish.
    let backoff_ms = [10u64, 20, 40, 80, 160, 320];
    let mut attempts = 0usize;
    loop {
        if unsafe { OpenClipboard(None) }.is_ok() {
            break;
        }
        if attempts >= backoff_ms.len() {
            let holder = open_holder_name();
            super::log::log_capture("", "copy", "occupied", 0, &holder);
            return Err(if holder.is_empty() {
                "剪贴板被其他程序占用，请稍后重试。".to_string()
            } else {
                format!("剪贴板正被 {holder} 占用，请稍后重试。")
            });
        }
        std::thread::sleep(Duration::from_millis(backoff_ms[attempts]));
        attempts += 1;
    }
    let mut items = Vec::new();
    let mut restorable = true;
    // 安全网：枚举+读取通常毫秒级完成，但个别延迟渲染格式可能阻塞。
    // 给一个宽松上限，超时只丢弃剩余格式，不再把整个取词判死。
    let deadline = Instant::now() + Duration::from_millis(2000);
    let mut saw_bitmap = false;
    let mut saw_dib = false;

    let mut fmt = 0u32;
    loop {
        if Instant::now() > deadline {
            break;
        }
        fmt = unsafe { EnumClipboardFormats(fmt) };
        if fmt == 0 {
            break;
        }
        if DERIVED.contains(&fmt) {
            continue;
        }
        if HANDLE_FORMATS.contains(&fmt) {
            if fmt == CF_BITMAP {
                saw_bitmap = true;
            }
            continue;
        }
        if fmt == CF_DIB {
            saw_dib = true;
        }
        match read_format(fmt) {
            Some(bytes) => items.push((fmt, bytes)),
            // 只有关键格式（文本/文件/图片）读不出才算无法还原；
            // 私有、装饰、延迟渲染格式读不出时丢弃即可，不阻塞取词。
            None => {
                if CRITICAL.contains(&fmt) {
                    restorable = false;
                }
            }
        }
    }
    // A bitmap with no DIB counterpart is real content we cannot copy; treat it
    // as non-restorable rather than silently dropping an image.
    if saw_bitmap && !saw_dib {
        restorable = false;
    }
    let _ = unsafe { CloseClipboard() };
    Ok(Snapshot { items, restorable })
}

/// Name of the process currently holding the clipboard open, if detectable.
fn open_holder_name() -> String {
    let Ok(hwnd) = (unsafe { GetOpenClipboardWindow() }) else {
        return String::new();
    };
    if hwnd.0.is_null() {
        return String::new();
    }
    unsafe {
        let mut pid = 0u32;
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
        super::context::process_name(pid)
    }
}

fn read_format(fmt: u32) -> Option<Vec<u8>> {
    let handle = unsafe { GetClipboardData(fmt) }.ok()?;
    let hg = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(hg) };
    if size == 0 {
        return None;
    }
    let ptr = unsafe { GlobalLock(hg) };
    if ptr.is_null() {
        return None;
    }
    let mut out = vec![0u8; size];
    unsafe {
        std::ptr::copy_nonoverlapping(ptr as *const u8, out.as_mut_ptr(), size);
        let _ = GlobalUnlock(hg);
    }
    Some(out)
}

/// Restore only if the clipboard sequence number still matches the value
/// observed right after we finished reading.
pub fn restore(snap: Snapshot, seq_after: u32) {
    if !snap.restorable {
        return;
    }
    if unsafe { GetClipboardSequenceNumber() } != seq_after {
        return;
    }
    if unsafe { OpenClipboard(None) }.is_err() {
        return;
    }
    let _ = unsafe { EmptyClipboard() };
    for (fmt, bytes) in &snap.items {
        set_bytes(*fmt, bytes);
    }
    // Mark our restore so it does not enter clipboard history or cloud sync.
    mark_exclude();
    let _ = unsafe { CloseClipboard() };
}

fn set_bytes(fmt: u32, bytes: &[u8]) {
    let Ok(hg) = (unsafe { GlobalAlloc(GHND, bytes.len().max(1)) }) else {
        return;
    };
    let ptr = unsafe { GlobalLock(hg) };
    if ptr.is_null() {
        let _ = unsafe { GlobalFree(Some(hg)) };
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        let _ = GlobalUnlock(hg);
        let _ = SetClipboardData(fmt, Some(HANDLE(hg.0)));
    }
}

fn mark_exclude() {
    for name in [
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ] {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let fmt = unsafe { RegisterClipboardFormatW(windows::core::PCWSTR(wide.as_ptr())) };
        if fmt != 0 {
            let zero = 0u32;
            set_bytes(fmt, &zero.to_ne_bytes());
        }
    }
}

/// Put plain text on the clipboard, marked as excluded from history.
#[allow(dead_code)]
pub fn set_text(text: &str) -> Result<(), String> {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let bytes: Vec<u8> = wide
        .iter()
        .flat_map(|u| u.to_ne_bytes())
        .chain([0u16, 0u16].iter().flat_map(|u| u.to_ne_bytes()))
        .collect();
    if unsafe { OpenClipboard(None) }.is_err() {
        return Err("剪贴板被占用，请再试一次。".into());
    }
    let _ = unsafe { EmptyClipboard() };
    set_bytes(CF_UNICODETEXT.0 as u32, &bytes);
    mark_exclude();
    let _ = unsafe { CloseClipboard() };
    Ok(())
}

pub fn read_text() -> Option<String> {
    if unsafe { OpenClipboard(None) }.is_err() {
        return None;
    }
    let out = read_text_locked();
    let _ = unsafe { CloseClipboard() };
    out
}

fn read_text_locked() -> Option<String> {
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok()?;
    let hg = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(hg) };
    if size < 2 {
        return None;
    }
    let ptr = unsafe { GlobalLock(hg) };
    if ptr.is_null() {
        return None;
    }
    let units = unsafe { std::slice::from_raw_parts(ptr as *const u16, size / 2) };
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    let text = String::from_utf16_lossy(&units[..end]);
    let _ = unsafe { GlobalUnlock(hg) };
    Some(text)
}
