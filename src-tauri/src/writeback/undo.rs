//! Undo slot: keep the original text in memory so "还原" can revert the last
//! writeback. Cleared after 30 seconds, on window change, or when consumed.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY;

use super::target;
use crate::capture;

struct Slot {
    original: String,
    at: Instant,
}

static SLOT: OnceLock<Mutex<Option<Slot>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Slot>> {
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn store(original: String) {
    let mut guard = slot().lock().unwrap();
    *guard = Some(Slot {
        original,
        at: Instant::now(),
    });
}

/// Send Ctrl+Z and, if the undo slot is still fresh, return its original text.
pub fn undo() -> Result<Option<String>, String> {
    let fresh = {
        let guard = slot().lock().unwrap();
        match guard.as_ref() {
            Some(s) if s.at.elapsed() < Duration::from_secs(30) => Some(s.original.clone()),
            _ => None,
        }
    };
    // Bring the source window back before undoing, so Ctrl+Z lands in the app
    // the text was written into, not in Vela.
    target::switch_back_and_verify()?;
    let chord = [windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL];
    if !capture::input::send_chord(&chord, VIRTUAL_KEY(0x5A)) {
        return Err("无法发送撤销。".into());
    }
    if fresh.is_some() {
        *slot().lock().unwrap() = None;
    }
    Ok(fresh)
}

pub fn clear() {
    *slot().lock().unwrap() = None;
}
