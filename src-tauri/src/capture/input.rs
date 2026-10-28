//! Low-level keyboard input. Every synthetic event is tagged with VELA_TAG so
//! Vela's own hooks can ignore its own keystrokes.

use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
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
    let mut inputs = Vec::with_capacity((mods.len() + 1) * 2);
    for &m in mods {
        inputs.push(key_event(m, false));
    }
    inputs.push(key_event(key, false));
    inputs.push(key_event(key, true));
    for &m in mods.iter().rev() {
        inputs.push(key_event(m, true));
    }
    let sent = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    sent == inputs.len() as u32
}

fn key_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
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
                dwExtraInfo: VELA_TAG,
            },
        },
    }
}
