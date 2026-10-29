//! Method one: read the selection through UI Automation's TextPattern, without
//! touching the clipboard. Tries focused element, parents, descendants, then
//! cursor point — Electron/Monaco often exposes TextPattern on a descendant,
//! not on the focused chrome element.

use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTreeWalker,
    CUIAutomation, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use super::error::{CaptureError, Outcome, Via};
use super::log;

pub fn uia_selection() -> Outcome {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run());
    });
    // Electron 唤醒无障碍 + 子树查找可能更慢。
    match rx.recv_timeout(Duration::from_millis(600)) {
        Ok(outcome) => outcome,
        Err(_) => Outcome::Failed(CaptureError::Timeout),
    }
}

fn run() -> Outcome {
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let auto: IUIAutomation = match unsafe {
        CoCreateInstance::<Option<&windows::core::IUnknown>, IUIAutomation>(
            &CUIAutomation,
            None,
            CLSCTX_INPROC_SERVER,
        )
    } {
        Ok(auto) => auto,
        Err(_) => return Outcome::Unsupported,
    };

    let walker = unsafe { auto.RawViewWalker() }.ok();

    // 1) 焦点元素
    if let Ok(el) = unsafe { auto.GetFocusedElement() } {
        if let Some(out) = try_element(&el) {
            log::log_diag("uia hit=focus");
            return out;
        }
        if let Some(w) = walker.as_ref() {
            if let Some(found) = walk_parents(w, &el, 12) {
                log::log_diag("uia hit=focus_parent");
                return found;
            }
        }
        // 子树中带 TextPattern 的元素（Monaco 常见）
        if let Some(w) = walker.as_ref() {
            if let Some(out) = walk_descendants(w, &el, 40) {
                log::log_diag("uia hit=focus_descendant");
                return out;
            }
        }
    }

    // 2) 光标下
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_ok() {
        if let Ok(el) = unsafe { auto.ElementFromPoint(pt) } {
            if let Some(out) = try_element(&el) {
                log::log_diag("uia hit=point");
                return out;
            }
            if let Some(w) = walker.as_ref() {
                if let Some(found) = walk_parents(w, &el, 12) {
                    log::log_diag("uia hit=point_parent");
                    return found;
                }
                if let Some(out) = walk_descendants(w, &el, 40) {
                    log::log_diag("uia hit=point_descendant");
                    return out;
                }
            }
        }
    }

    log::log_diag("uia hit=none");
    Outcome::Unsupported
}

fn walk_descendants(
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    max: usize,
) -> Option<Outcome> {
    let mut stack = vec![root.clone()];
    let mut seen = 0usize;
    while let Some(node) = stack.pop() {
        if seen >= max {
            break;
        }
        seen += 1;
        if seen > 1 {
            if let Some(out) = try_element(&node) {
                return Some(out);
            }
        }
        let Ok(child) = (unsafe { walker.GetFirstChildElement(&node) }) else {
            continue;
        };
        let mut sib = Some(child);
        while let Some(cur) = sib {
            stack.push(cur.clone());
            sib = unsafe { walker.GetNextSiblingElement(&cur) }.ok();
        }
    }
    None
}

fn walk_parents(
    walker: &IUIAutomationTreeWalker,
    start: &IUIAutomationElement,
    max: usize,
) -> Option<Outcome> {
    let mut current = start.clone();
    for _ in 0..max {
        let parent = unsafe { walker.GetParentElement(&current) }.ok()?;
        if let Some(out) = try_element(&parent) {
            return Some(out);
        }
        current = parent;
    }
    None
}

fn try_element(element: &IUIAutomationElement) -> Option<Outcome> {
    if unsafe { element.CurrentIsPassword() }
        .map(|b| b.as_bool())
        .unwrap_or(false)
    {
        return Some(Outcome::Failed(CaptureError::PasswordField));
    }

    let pattern: IUIAutomationTextPattern =
        match unsafe { element.GetCurrentPatternAs(UIA_TextPatternId) } {
            Ok(p) => p,
            Err(_) => return None,
        };

    let ranges = match unsafe { pattern.GetSelection() } {
        Ok(r) => r,
        Err(_) => return None,
    };
    let length = unsafe { ranges.Length() }.unwrap_or(0);
    if length <= 0 {
        return None;
    }
    let range = match unsafe { ranges.GetElement(0) } {
        Ok(r) => r,
        Err(_) => return None,
    };
    let bstr = match unsafe { range.GetText(5000) } {
        Ok(t) => t,
        Err(_) => return None,
    };
    let raw = bstr.to_string();
    match super::error::sanitize(&raw) {
        Some(text) => Some(Outcome::Text {
            text,
            via: Via::Uia,
        }),
        None => None,
    }
}
