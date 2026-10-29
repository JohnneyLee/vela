//! Remembers the last confirmed selection briefly. Electron editors (Cursor /
//! VS Code) often drop the selection when the action bar is clicked, so we
//! arm the cache while the selection is still live.

use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Entry {
    text: String,
    exe: String,
    at: Instant,
}

static LAST: Mutex<Option<Entry>> = Mutex::new(None);

const TTL: Duration = Duration::from_secs(20);

pub fn store(text: String, exe: String) {
    if text.trim().is_empty() {
        return;
    }
    if let Ok(mut guard) = LAST.lock() {
        *guard = Some(Entry {
            text,
            exe,
            at: Instant::now(),
        });
    }
}

/// Peek without consuming — caller clears after a successful use.
pub fn get_if_fresh(current_exe: &str) -> Option<String> {
    let guard = LAST.lock().ok()?;
    let entry = guard.as_ref()?;
    if entry.at.elapsed() > TTL {
        return None;
    }
    let cur = current_exe.to_ascii_lowercase();
    let was = entry.exe.to_ascii_lowercase();
    // exe 为空时放宽匹配（切回失败时仍可用缓存）。
    if !cur.is_empty() && !was.is_empty() && cur != was {
        return None;
    }
    Some(entry.text.clone())
}

pub fn clear() {
    if let Ok(mut guard) = LAST.lock() {
        *guard = None;
    }
}
