//! Write method two: type the text character-by-character with Unicode key
//! events. Fallback for targets that block paste (remote desktop, admin apps).

use std::thread;
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, VIRTUAL_KEY, VK_SHIFT,
};

use super::policy::{Newline, Policy};
use crate::capture;

const MAX_TYPED: usize = 500;

pub fn type_text(text: &str, pol: &Policy) -> Result<(), String> {
    if text.chars().count() > MAX_TYPED {
        return Err("文字较长，已放入剪贴板，请手动粘贴。".into());
    }
    let mut batch = 0usize;
    for ch in text.chars() {
        match ch {
            '\n' => {
                match pol.newline {
                    Newline::ShiftEnter => send_shift_enter(),
                    Newline::JoinWithSpace => send_char(' '),
                    Newline::AsIs => send_char('\n'),
                }
            }
            _ => send_char(ch),
        }
        batch += 1;
        if batch % 20 == 0 {
            thread::sleep(Duration::from_millis(4));
        }
    }
    Ok(())
}

fn send_char(ch: char) {
    let mut buf = [0u16; 2];
    for unit in ch.encode_utf16(&mut buf) {
        send_unit(*unit);
    }
}

fn send_unit(unit: u16) {
    let down = unicode_input(unit, false);
    let up = unicode_input(unit, true);
    let inputs = [down, up];
    unsafe {
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_shift_enter() {
    let inputs = [
        key_event(VK_SHIFT, false),
        key_event(VIRTUAL_KEY(0x0D), false),
        key_event(VIRTUAL_KEY(0x0D), true),
        key_event(VK_SHIFT, true),
    ];
    unsafe {
        let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: unit,
                dwFlags: if up {
                    KEYEVENTF_UNICODE | KEYEVENTF_KEYUP
                } else {
                    KEYEVENTF_UNICODE
                },
                time: 0,
                dwExtraInfo: capture::input::VELA_TAG,
            },
        },
    }
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
                dwExtraInfo: capture::input::VELA_TAG,
            },
        },
    }
}
