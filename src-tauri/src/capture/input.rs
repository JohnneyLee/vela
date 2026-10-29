//! Low-level keyboard input. Every synthetic event is tagged with VELA_TAG so
//! Vela's own hooks can ignore its own keystrokes — except Electron copy, which
//! uses untagged scancode events (Chromium often ignores tagged/injected VK-only
//! chords).

use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MAP_VIRTUAL_KEY_TYPE, VIRTUAL_KEY,
    VK_CONTROL, VK_INSERT, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

/// Tag written into `dwExtraInfo` of every synthetic key event.
pub const VELA_TAG: usize = 0x5645_4C41;

pub fn wait_modifiers_released(max: Duration) -> bool {
    let keys = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];
    let start = Instant::now();
    while start.elapsed() < max {
        let held = keys
            .iter()
            .any(|&vk| (unsafe { GetAsyncKeyState(vk.0 as i32) } as i16) < 0);
        if !held {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// Send a chord: modifiers down, key down, key up, modifiers up.
/// Returns true if every event was injected.
pub fn send_chord(mods: &[VIRTUAL_KEY], key: VIRTUAL_KEY) -> bool {
    send_chord_ex(mods, key, true)
}

/// Electron / Chromium: scancode chord with no VELA_TAG so the renderer treats
/// it closer to a physical keypress. Also used for Ctrl+Insert.
pub fn send_chord_scancode_untagged(mods: &[VIRTUAL_KEY], key: VIRTUAL_KEY) -> bool {
    let mut inputs = Vec::with_capacity((mods.len() + 1) * 2);
    for &m in mods {
        inputs.push(scancode_event(m, false, 0));
    }
    inputs.push(scancode_event(key, false, 0));
    inputs.push(scancode_event(key, true, 0));
    for &m in mods.iter().rev() {
        inputs.push(scancode_event(m, true, 0));
    }
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    sent == inputs.len() as u32
}

pub fn send_ctrl_c_scancode() -> bool {
    send_chord_scancode_untagged(&[VK_CONTROL], VIRTUAL_KEY(0x43))
}

pub fn send_ctrl_insert_scancode() -> bool {
    send_chord_scancode_untagged(&[VK_CONTROL], VK_INSERT)
}

fn send_chord_ex(mods: &[VIRTUAL_KEY], key: VIRTUAL_KEY, tagged: bool) -> bool {
    let extra = if tagged { VELA_TAG } else { 0 };
    let mut inputs = Vec::with_capacity((mods.len() + 1) * 2);
    for &m in mods {
        inputs.push(key_event(m, false, extra));
    }
    inputs.push(key_event(key, false, extra));
    inputs.push(key_event(key, true, extra));
    for &m in mods.iter().rev() {
        inputs.push(key_event(m, true, extra));
    }
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    sent == inputs.len() as u32
}

fn key_event(vk: VIRTUAL_KEY, up: bool, extra: usize) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS::default()
                },
                time: 0,
                dwExtraInfo: extra,
            },
        },
    }
}

fn scancode_event(vk: VIRTUAL_KEY, up: bool, extra: usize) -> INPUT {
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAP_VIRTUAL_KEY_TYPE(0)) } as u16;
    let mut flags = KEYEVENTF_SCANCODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: extra,
            },
        },
    }
}
