//! Capture chain: try UI Automation first (no clipboard), then simulated copy.

pub(crate) mod clipboard;
pub(crate) mod context;
mod copykey;
mod error;
pub(crate) mod input;
mod log;
pub(crate) mod prefetch_debug;
pub(crate) mod selection_cache;
pub(crate) mod uia;

pub use prefetch_debug::{PrefetchDebug, PrefetchEvent, PrefetchPhase};

pub use error::{sanitize, CaptureError, Outcome, Via};

use std::sync::atomic::{AtomicBool, Ordering};
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
    const EDITABLE: [&str; 16] = [
        "wechat.exe",
        "weixin.exe",
        "qq.exe",
        "dingtalk.exe",
        "slack.exe",
        "code.exe",
        "cursor.exe",
        "notepad.exe",
        "winword.exe",
        "wps.exe",
        "outlook.exe",
        "windowsterminal.exe",
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
        "devenv.exe",
    ];
    EDITABLE.iter().any(|e| exe.contains(e))
}

/// Electron / Chromium 编辑器：UIA 常不可靠，复制也需要更长等待。
pub fn is_electron_editor(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    // 用后缀匹配，避免误伤其它进程名。
    exe.ends_with("cursor.exe")
        || exe.ends_with("code.exe")
        || exe.ends_with("code - insiders.exe")
        || exe.ends_with("windsurf.exe")
        || exe.ends_with("trae.exe")
}

/// 终端类进程：选区后不应自动出入口（Ctrl+C 会中断进程）。
pub fn is_terminal_exe(exe: &str) -> bool {
    let exe = exe.to_ascii_lowercase();
    exe.ends_with("windowsterminal.exe")
        || exe.ends_with("cmd.exe")
        || exe.ends_with("powershell.exe")
        || exe.ends_with("pwsh.exe")
        || exe.ends_with("windows terminal.exe")
        || exe.contains("mintty")
}

/// 选区后是否自动弹出入口图标。终端默认仅热键（避免误触 Ctrl+C 中断）。
/// 代码编辑器仍出小图标，但不默认静默预取词。
pub fn auto_show_on_selection(exe: &str) -> bool {
    !is_terminal_exe(exe)
}

/// 可编辑场景出条更晚，减少「选中为了删除」时的打扰。
pub fn selection_settle_ms(exe: &str) -> u32 {
    if is_editable_exe(exe) {
        500
    } else {
        300
    }
}

/// Probe with UI Automation only — never touches the clipboard. Used by the
/// action-bar trigger, which must not disturb the user's clipboard on selection.
pub fn probe_uia_only() -> Outcome {
    uia::uia_selection()
}

/// 后台静默取词是否仍在进行（Electron arm）。
static ARM_BUSY: AtomicBool = AtomicBool::new(false);

static ARM_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn outcome_reason(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Text { .. } => "success",
        Outcome::NoSelection => "no_selection",
        Outcome::Unsupported => "unsupported",
        Outcome::Failed(CaptureError::InputBlocked) => "input_blocked",
        Outcome::Failed(CaptureError::ElevatedTarget) => "elevated",
        Outcome::Failed(CaptureError::Timeout) => "timeout",
        Outcome::Failed(CaptureError::PasswordField) => "password",
        Outcome::Failed(CaptureError::NoSelection) => "no_selection",
        Outcome::Failed(CaptureError::Unsupported) => "unsupported",
        Outcome::Failed(CaptureError::Other(msg)) => {
            if msg.contains("占用") {
                "occupied"
            } else if msg.contains("备份") || msg.contains("无法完整") {
                "backup_fail"
            } else if msg.contains("松开") {
                "modifiers_held"
            } else {
                "other"
            }
        }
    }
}

/// Run one silent copy for Electron; stores cache on success. Never logs text.
pub fn arm_electron_selection_with(ev: &PrefetchEvent) -> Option<String> {
    let t0 = Instant::now();
    prefetch_debug::log_line(ev, "start", "begin", "", t0);
    let _guard = match CAPTURE_LOCK.lock() {
        Ok(g) => g,
        Err(_) => {
            prefetch_debug::log_line(ev, "copy", "lock_fail", "", t0);
            return None;
        }
    };
    let ctx = context::collect();
    if !is_electron_editor(&ctx.exe) {
        prefetch_debug::log_line(
            ev,
            "copy",
            "exe_mismatch",
            &format!("ctx_exe={}", ctx.exe),
            t0,
        );
        return None;
    }
    let budget = std::time::Duration::from_millis(ev.budget_ms);
    // 先 UIA（子树 TextPattern），再剪贴板。Cursor 上注入 Ctrl+C 常不改 seq。
    let uia = uia::uia_selection();
    if let Outcome::Text { text, .. } = uia {
        selection_cache::store(text.clone(), ctx.exe.clone());
        prefetch_debug::log_line(
            ev,
            "uia",
            "success",
            &format!("chars={} ms={}", text.chars().count(), t0.elapsed().as_millis()),
            t0,
        );
        return Some(text);
    }
    prefetch_debug::log_line(ev, "uia", outcome_reason(&uia), "", t0);

    let outcome = copykey::copykey_selection(&ctx, true);
    let reason = outcome_reason(&outcome);
    let extra = format!(
        "copy_ms={} over_budget={}",
        t0.elapsed().as_millis(),
        t0.elapsed() > budget
    );
    prefetch_debug::log_line(ev, "copy", reason, &extra, t0);
    match outcome {
        Outcome::Text { text, .. } => {
            selection_cache::store(text.clone(), ctx.exe);
            prefetch_debug::log_line(
                ev,
                "cache",
                "stored",
                &format!("chars={}", text.chars().count()),
                t0,
            );
            Some(text)
        }
        _ => None,
    }
}

/// Async arm: wait until mouseup+delay_ms, then copy, clear busy.
pub fn start_prefetch_async(ev: PrefetchEvent) {
    if !is_electron_editor(&ev.exe) {
        return;
    }
    let generation = ARM_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    ARM_BUSY.store(true, Ordering::SeqCst);
    let mouseup_wall = ev.mouseup_ms;
    let delay = ev.delay_ms;
    std::thread::spawn(move || {
        // Wait until mouse-up + delay (wall clock).
        loop {
            let now = prefetch_debug::now_ms();
            if now.saturating_sub(mouseup_wall) >= delay {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = arm_electron_selection_with(&ev);
        if ARM_GEN.load(Ordering::SeqCst) == generation {
            ARM_BUSY.store(false, Ordering::SeqCst);
        }
    });
}

/// Synchronous prefetch for before_bar: wait delay from mouseup, copy, return.
pub fn run_prefetch_before_bar(ev: &PrefetchEvent) -> Option<String> {
    let generation = ARM_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    ARM_BUSY.store(true, Ordering::SeqCst);
    let mouseup_wall = ev.mouseup_ms;
    let delay = ev.delay_ms;
    loop {
        let now = prefetch_debug::now_ms();
        if now.saturating_sub(mouseup_wall) >= delay {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let out = arm_electron_selection_with(ev);
    if ARM_GEN.load(Ordering::SeqCst) == generation {
        ARM_BUSY.store(false, Ordering::SeqCst);
    }
    out
}

pub fn arm_busy() -> bool {
    ARM_BUSY.load(Ordering::SeqCst)
}

pub fn has_fresh_electron_cache() -> bool {
    let remembered = crate::writeback::target::source_exe();
    let ctx_exe = context::collect().exe;
    selection_cache::get_if_fresh(&remembered).is_some()
        || selection_cache::get_if_fresh(&ctx_exe).is_some()
        || selection_cache::get_if_fresh("").is_some()
}

pub fn prefetch_config() -> &'static PrefetchDebug {
    prefetch_debug::config()
}

pub fn log_focus_stage(stage: &str, exe: &str) {
    if !prefetch_debug::config().log {
        return;
    }
    let (fg, focus) = prefetch_debug::focus_pair();
    log::log_diag(&format!(
        "focus stage={stage} exe={exe} fg={fg} focus={focus}"
    ));
}

/// 动作条点击时：Electron 优先消费探测阶段缓存，避免再切焦点/再复制。
pub fn take_electron_cache() -> Option<String> {
    let remembered = crate::writeback::target::source_exe();
    let ctx_exe = context::collect().exe;
    if !(is_electron_editor(&remembered) || is_electron_editor(&ctx_exe)) {
        return None;
    }
    let text = selection_cache::get_if_fresh(&remembered)
        .or_else(|| selection_cache::get_if_fresh(&ctx_exe))
        .or_else(|| selection_cache::get_if_fresh(""))?;
    selection_cache::clear();
    Some(text)
}

/// 若缓存尚未就绪且后台 arm 仍在跑，短等一会儿再取（点「译」太快时）。
pub fn take_electron_cache_wait(max: std::time::Duration) -> Option<String> {
    let remembered = crate::writeback::target::source_exe();
    let ctx_exe = context::collect().exe;
    if !(is_electron_editor(&remembered) || is_electron_editor(&ctx_exe)) {
        return None;
    }
    let started = Instant::now();
    loop {
        if let Some(text) = selection_cache::get_if_fresh(&remembered)
            .or_else(|| selection_cache::get_if_fresh(&ctx_exe))
            .or_else(|| selection_cache::get_if_fresh(""))
        {
            selection_cache::clear();
            return Some(text);
        }
        if started.elapsed() >= max {
            return None;
        }
        if !ARM_BUSY.load(Ordering::SeqCst) {
            // 防「刚写完缓存就清 busy」的竞态：再给一轮短读。
            std::thread::sleep(std::time::Duration::from_millis(20));
            if let Some(text) = selection_cache::get_if_fresh(&remembered)
                .or_else(|| selection_cache::get_if_fresh(&ctx_exe))
                .or_else(|| selection_cache::get_if_fresh(""))
            {
                selection_cache::clear();
                return Some(text);
            }
            // spawn 尚未置 busy：最多再等到 150ms。
            if started.elapsed() < std::time::Duration::from_millis(150) {
                std::thread::sleep(std::time::Duration::from_millis(15));
                continue;
            }
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
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
    let _active = log::ActiveCapture::start();
    let started = Instant::now();
    let ctx = context::collect();
    // 也用记住的源进程名匹配缓存（点按钮后前台可能已不是 Cursor）。
    let cache_exe = {
        let remembered = crate::writeback::target::source_exe();
        if !remembered.is_empty() {
            remembered
        } else {
            ctx.exe.clone()
        }
    };

    // Electron：点按钮后选区多半没了，优先用选中时缓存的文字。
    if is_electron_editor(&cache_exe) || is_electron_editor(&ctx.exe) {
        if let Some(text) = selection_cache::get_if_fresh(&cache_exe)
            .or_else(|| selection_cache::get_if_fresh(&ctx.exe))
            .or_else(|| selection_cache::get_if_fresh(""))
        {
            selection_cache::clear();
            log::log_capture(
                &cache_exe,
                "cache",
                "success",
                started.elapsed().as_millis(),
                "",
            );
            return Outcome::Text {
                text,
                via: Via::CopyKey,
            };
        }
    }

    let uia = uia::uia_selection();
    match uia {
        Outcome::Text { ref text, .. } => {
            selection_cache::store(text.clone(), ctx.exe.clone());
            log::log_capture(
                &ctx.exe,
                "uia",
                "success",
                started.elapsed().as_millis(),
                "",
            );
            return uia;
        }
        Outcome::Failed(CaptureError::PasswordField) => {
            log::log_capture(
                &ctx.exe,
                "uia",
                "password",
                started.elapsed().as_millis(),
                "",
            );
            return uia;
        }
        Outcome::Failed(_) => {} // fall through to copy
        Outcome::NoSelection | Outcome::Unsupported => {} // fall through to copy
    }

    let copy = copykey::copykey_selection(&ctx, allow_partial);
    if let Outcome::Text { ref text, .. } = copy {
        selection_cache::store(text.clone(), ctx.exe.clone());
        log::log_capture(
            &ctx.exe,
            "copy",
            "success",
            started.elapsed().as_millis(),
            "",
        );
        return copy;
    }

    if let Some(text) = selection_cache::get_if_fresh(&cache_exe)
        .or_else(|| selection_cache::get_if_fresh(&ctx.exe))
        .or_else(|| selection_cache::get_if_fresh(""))
    {
        selection_cache::clear();
        log::log_capture(
            &cache_exe,
            "cache",
            "success",
            started.elapsed().as_millis(),
            "",
        );
        return Outcome::Text {
            text,
            via: Via::CopyKey,
        };
    }

    log::log_capture(
        &ctx.exe,
        "none",
        outcome_result(&copy),
        started.elapsed().as_millis(),
        "",
    );
    copy
}

fn outcome_result(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Text { .. } => "success",
        Outcome::NoSelection => "no_selection",
        Outcome::Unsupported => "unsupported",
        Outcome::Failed(err) => match err {
            CaptureError::Timeout => "timeout",
            CaptureError::PasswordField => "password",
            CaptureError::ElevatedTarget => "elevated",
            CaptureError::InputBlocked => "input_blocked",
            CaptureError::NoSelection => "no_selection",
            CaptureError::Unsupported => "unsupported",
            CaptureError::Other(msg) => {
                if msg.contains("占用") {
                    "occupied"
                } else {
                    "other"
                }
            }
        },
    }
}
