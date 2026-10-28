//! Capture chain: try UI Automation first (no clipboard), then simulated copy.

pub(crate) mod clipboard;
pub(crate) mod context;
mod copykey;
mod error;
pub(crate) mod input;
pub(crate) mod uia;

pub use error::{sanitize, CaptureError, Outcome};

use std::sync::Mutex;
use std::time::Instant;

/// Serializes captures so a second trigger never races the first on the clipboard.
static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

/// Process name (e.g. "chrome.exe") of the current foreground window.
pub fn current_exe() -> String {
    context::collect().exe
}

/// Whether the given process is known to host editable text fields. Cheap
/// process-name heuristic; a full UIA ValuePattern check can replace it later.
/// Used to decide whether the "替换" (write back) button should be shown.
pub fn is_editable_exe(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    const EDITABLE: [&str; 14] = [
        "wechat.exe",
        "weixin.exe",
        "qq.exe",
        "dingtalk.exe",
        "slack.exe",
        "code.exe",
        "notepad.exe",
        "winword.exe",
        "wps.exe",
        "outlook.exe",
        "windowsterminal.exe",
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
    ];
    EDITABLE.iter().any(|e| exe.contains(e))
}

/// Probe with UI Automation only — never touches the clipboard. Used by the
/// action-bar trigger, which must not disturb the user's clipboard on selection.
pub fn probe_uia_only() -> Outcome {
    uia::uia_selection()
}

/// Full capture used by the hotkey. Returns user-facing text or an error message.
pub fn capture() -> Result<String, String> {
    capture_allow_partial(false)
}

/// Full capture with the caller's preference for whether a non-restorable
/// clipboard snapshot is allowed to proceed (default: fail rather than risk
/// losing the user's clipboard contents).
pub fn capture_allow_partial(allow_partial: bool) -> Result<String, String> {
    let _guard = match CAPTURE_LOCK.try_lock() {
        Ok(guard) => guard,
        Err(_) => {
            return Err(CaptureError::Other("上一次取词还没结束，请稍后再试。".into())
                .user_message())
        }
    };
    match try_capture(allow_partial) {
        Outcome::Text { text, .. } => Ok(text),
        Outcome::NoSelection => Err(CaptureError::NoSelection.user_message()),
        Outcome::Failed(err) => Err(err.user_message()),
        Outcome::Unsupported => Err(CaptureError::Unsupported.user_message()),
    }
}

/// Lightweight probe for the floating icon: no error messages, no slow retries.
pub fn probe() -> Option<String> {
    match try_capture(false) {
        Outcome::Text { text, .. } => Some(text),
        _ => None,
    }
}

fn try_capture(allow_partial: bool) -> Outcome {
    let started = Instant::now();
    let ctx = context::collect();

    let uia = uia::uia_selection();
    match uia {
        Outcome::Text { .. } => {
            eprintln!(
                "[vela-capture] via=uia exe={} ms={}",
                ctx.exe,
                started.elapsed().as_millis()
            );
            return uia;
        }
        Outcome::Failed(CaptureError::PasswordField) => return uia,
        Outcome::Failed(_) => {} // fall through to copy
        Outcome::NoSelection | Outcome::Unsupported => {} // fall through to copy
    }

    let copy = copykey::copykey_selection(&ctx, allow_partial);
    let via = if matches!(copy, Outcome::Text { .. }) {
        "copy"
    } else {
        "none"
    };
    eprintln!(
        "[vela-capture] via={via} exe={} ms={}",
        ctx.exe,
        started.elapsed().as_millis()
    );
    copy
}
