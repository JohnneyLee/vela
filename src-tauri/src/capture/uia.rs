//! Method one: read the selection through UI Automation's TextPattern, without
//! touching the clipboard.

use std::sync::mpsc;
use std::time::Duration;

use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationTextPattern, CUIAutomation, UIA_TextPatternId,
};

use super::error::{CaptureError, Outcome, Via};

pub fn uia_selection() -> Outcome {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run());
    });
    match rx.recv_timeout(Duration::from_millis(120)) {
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

    let element = match unsafe { auto.GetFocusedElement() } {
        Ok(el) => el,
        Err(_) => return Outcome::Unsupported,
    };

    if unsafe { element.CurrentIsPassword() }
        .map(|b| b.as_bool())
        .unwrap_or(false)
    {
        return Outcome::Failed(CaptureError::PasswordField);
    }

    let pattern: IUIAutomationTextPattern = match unsafe {
        element.GetCurrentPatternAs(UIA_TextPatternId)
    } {
        Ok(p) => p,
        Err(_) => return Outcome::Unsupported,
    };

    let ranges = match unsafe { pattern.GetSelection() } {
        Ok(r) => r,
        Err(_) => return Outcome::Unsupported,
    };
    let length = unsafe { ranges.Length() }.unwrap_or(0);
    if length <= 0 {
        return Outcome::Unsupported;
    }
    let range = match unsafe { ranges.GetElement(0) } {
        Ok(r) => r,
        Err(_) => return Outcome::Unsupported,
    };
    let bstr = match unsafe { range.GetText(5000) } {
        Ok(t) => t,
        Err(_) => return Outcome::Unsupported,
    };
    let raw = bstr.to_string();
    match super::error::sanitize(&raw) {
        Some(text) => Outcome::Text {
            text,
            via: Via::Uia,
        },
        None => Outcome::Unsupported,
    }
}
