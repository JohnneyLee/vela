//! Capture diagnostics logging. Only records the process name, capture path,
//! result, duration, active-task count, and (on clipboard contention) the
//! holder's process name — never the selected text.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// How many captures are running right now.
static ACTIVE_CAPTURES: AtomicUsize = AtomicUsize::new(0);

/// RAII counter so every return path decrements the active count.
pub struct ActiveCapture;

impl ActiveCapture {
    pub fn start() -> Self {
        ACTIVE_CAPTURES.fetch_add(1, Ordering::SeqCst);
        ActiveCapture
    }
}

impl Drop for ActiveCapture {
    fn drop(&mut self) {
        ACTIVE_CAPTURES.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Append one capture log line. `holder` is the process holding the clipboard
/// open when contention was detected (empty otherwise).
pub fn log_capture(exe: &str, via: &str, result: &str, ms: u128, holder: &str) {
    let running = ACTIVE_CAPTURES.load(Ordering::SeqCst);
    let mut line = format!(
        "{} exe={} via={} result={} ms={} running={}",
        timestamp(),
        exe,
        via,
        result,
        ms,
        running
    );
    if !holder.is_empty() {
        line.push_str(&format!(" holder={holder}"));
    }
    append_line(&line);
}

/// Free-form diagnostic line (prefetch timings, focus snapshots). Never pass text content.
pub fn log_diag(line: &str) {
    append_line(&format!("{} {line}", timestamp()));
}

fn append_line(line: &str) {
    let dir = log_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("capture-{}.log", date_str()));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
    prune_logs(&dir);
}

fn log_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(base).join("com.vela.desktop").join("logs")
}

/// Local-free UTC timestamp "YYYY-MM-DD HH:MM:SS", built from the epoch.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let tod = secs.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn date_str() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's civil-from-days algorithm, returns (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Remove capture log files older than 7 days.
fn prune_logs(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age.as_secs() > 7 * 86_400)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10957), (2000, 1, 1));
        assert_eq!(civil_from_days(20454), (2026, 1, 1));
        // A leap day must fall on 2000-02-29.
        assert_eq!(civil_from_days(11016), (2000, 2, 29));
    }
}
