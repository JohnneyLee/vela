//! Prefetch (silent Ctrl+C) diagnostics and timing knobs for Cursor experiments.
//! Never logs selected text — only timings, window/exe names, and result codes.
//!
//! Config file (created with defaults on first read):
//!   %APPDATA%/com.vela.desktop/prefetch-debug.json
//!
//! Env overrides (optional):
//!   VELA_PREFETCH_PHASE=on_mouseup|before_bar|after_bar
//!   VELA_PREFETCH_DELAY_MS=0..2000
//!   VELA_PREFETCH_BUDGET_MS=50..5000
//!   VELA_PREFETCH_LOG=0|1

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::context;
use super::log;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrefetchPhase {
    /// Start copy `delay_ms` after mouse-up; bar shows on settle independently.
    OnMouseUp,
    /// After mouse-up + `delay_ms`, copy (up to `budget_ms`), then show bar.
    #[default]
    BeforeBar,
    /// Show bar first, then after `delay_ms` start copy.
    AfterBar,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefetchDebug {
    /// Silent Ctrl+C on selection. Default off (privacy + delete-selection UX).
    #[serde(default)]
    pub enabled: bool,
    /// When relative to bar show the silent copy runs.
    #[serde(default)]
    pub phase: PrefetchPhase,
    /// Delay after mouse-up (or after bar for AfterBar) before starting copy.
    #[serde(default = "default_delay")]
    pub delay_ms: u64,
    /// Max time spent in one silent-copy attempt before giving up.
    #[serde(default = "default_budget")]
    pub budget_ms: u64,
    /// Write prefetch diagnostic lines (no text content).
    #[serde(default = "default_true")]
    pub log: bool,
}

fn default_delay() -> u64 {
    300
}

fn default_budget() -> u64 {
    2500
}

fn default_true() -> bool {
    true
}

impl Default for PrefetchDebug {
    fn default() -> Self {
        Self {
            enabled: false,
            phase: PrefetchPhase::BeforeBar,
            delay_ms: 300,
            budget_ms: 2500,
            log: true,
        }
    }
}

static CACHED: OnceLock<PrefetchDebug> = OnceLock::new();

pub fn config() -> &'static PrefetchDebug {
    CACHED.get_or_init(load)
}

/// Reload from disk/env on next call — for tests / live tweak without restart
/// we expose an explicit refresh used by a tiny tauri command if needed later.
#[allow(dead_code)]
pub fn reload() -> PrefetchDebug {
    let cfg = load();
    let _ = CACHED.set(cfg.clone());
    cfg
}

fn load() -> PrefetchDebug {
    let path = config_path();
    let mut cfg = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<PrefetchDebug>(&raw).ok())
            .unwrap_or_default()
    } else {
        let cfg = PrefetchDebug::default();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(raw) = serde_json::to_string_pretty(&cfg) {
            let _ = std::fs::write(&path, raw);
        }
        cfg
    };
    apply_env(&mut cfg);
    cfg.delay_ms = cfg.delay_ms.min(2000);
    cfg.budget_ms = cfg.budget_ms.clamp(50, 5000);
    cfg
}

fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base)
        .join("com.vela.desktop")
        .join("prefetch-debug.json")
}

fn apply_env(cfg: &mut PrefetchDebug) {
    if let Ok(v) = std::env::var("VELA_PREFETCH_ENABLED") {
        cfg.enabled = matches!(v.trim(), "1" | "true" | "yes" | "on");
    }
    if let Ok(v) = std::env::var("VELA_PREFETCH_PHASE") {
        cfg.phase = match v.trim().to_ascii_lowercase().as_str() {
            "on_mouseup" | "mouseup" => PrefetchPhase::OnMouseUp,
            "after_bar" | "after" => PrefetchPhase::AfterBar,
            "before_bar" | "before" => PrefetchPhase::BeforeBar,
            _ => cfg.phase,
        };
    }
    if let Ok(v) = std::env::var("VELA_PREFETCH_DELAY_MS") {
        if let Ok(n) = v.trim().parse::<u64>() {
            cfg.delay_ms = n;
        }
    }
    if let Ok(v) = std::env::var("VELA_PREFETCH_BUDGET_MS") {
        if let Ok(n) = v.trim().parse::<u64>() {
            cfg.budget_ms = n;
        }
    }
    if let Ok(v) = std::env::var("VELA_PREFETCH_LOG") {
        cfg.log = matches!(v.trim(), "1" | "true" | "yes" | "on");
    }
}

/// Monotonic-ish wall clock ms for correlating events in one gesture.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Foreground + focus window process names (never titles / text).
pub fn focus_pair() -> (String, String) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };
    unsafe {
        let fg = GetForegroundWindow();
        let mut fg_pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut fg_pid));
        let fg_exe = context::process_name(fg_pid);

        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        let focus_exe = if GetGUIThreadInfo(0, &mut info).is_ok() && !info.hwndFocus.0.is_null() {
            let mut pid = 0u32;
            GetWindowThreadProcessId(info.hwndFocus, Some(&mut pid));
            context::process_name(pid)
        } else {
            String::new()
        };
        (fg_exe, focus_exe)
    }
}

#[derive(Debug, Clone)]
pub struct PrefetchEvent {
    pub mouseup_ms: u64,
    pub gesture: String,
    pub drag_px: i32,
    pub exe: String,
    pub phase: PrefetchPhase,
    pub delay_ms: u64,
    pub budget_ms: u64,
}

pub fn log_line(
    ev: &PrefetchEvent,
    stage: &str,
    result: &str,
    extra: &str,
    t0: Instant,
) {
    if !config().log {
        return;
    }
    let (fg, focus) = focus_pair();
    let mut line = format!(
        "prefetch stage={stage} result={result} exe={} gesture={} drag={} phase={:?} delay={} budget={} mouseup={} elapsed_ms={} fg={} focus={}",
        ev.exe,
        ev.gesture,
        ev.drag_px,
        ev.phase,
        ev.delay_ms,
        ev.budget_ms,
        ev.mouseup_ms,
        t0.elapsed().as_millis(),
        fg,
        focus
    );
    if !extra.is_empty() {
        line.push(' ');
        line.push_str(extra);
    }
    log::log_diag(&line);
}

pub fn sleep_delay(from_mouseup: Instant, delay_ms: u64) {
    let target = Duration::from_millis(delay_ms);
    if let Some(remain) = target.checked_sub(from_mouseup.elapsed()) {
        if !remain.is_zero() {
            std::thread::sleep(remain);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_phase_is_before_bar() {
        let cfg = PrefetchDebug::default();
        assert_eq!(cfg.phase, PrefetchPhase::BeforeBar);
        assert_eq!(cfg.delay_ms, 300);
    }

    #[test]
    fn phase_serde_snake() {
        let raw = r#"{"phase":"after_bar","delayMs":120,"budgetMs":250,"log":true}"#;
        let cfg: PrefetchDebug = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.phase, PrefetchPhase::AfterBar);
        assert_eq!(cfg.delay_ms, 120);
    }
}
