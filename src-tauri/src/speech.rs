//! Read-aloud via the built-in Windows SAPI voices. The text is written to a
//! temporary UTF-8 file and read by a short PowerShell script, so no characters
//! ever pass through shell quoting or argument encoding.

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// PID of the in-flight read-aloud process. Held so a second tap can stop the
/// previous voice before starting a new one, instead of both talking at once.
static PLAYER_PID: Mutex<Option<u32>> = Mutex::new(None);

/// Set when the user (or close/Esc) asked to stop. The read-aloud then returns
/// Ok silently instead of reporting a fake error for its killed process.
static STOPPED: AtomicBool = AtomicBool::new(false);

/// Stop the currently-playing voice. Called by the panel's stop button, the
/// close/Esc path, and before a new read-aloud starts.
pub fn stop() {
    STOPPED.store(true, Ordering::SeqCst);
    stop_previous();
}

/// Kill the currently-playing voice (if any) before starting a new one.
fn stop_previous() {
    let pid = match PLAYER_PID.lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => None,
    };
    if let Some(pid) = pid {
        let _ = Command::new("taskkill.exe")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }
}

/// 按指定语言朗读。`lang` 传 "zh"/"en"/"ja" 时用对应语音；
/// 传 "auto" 时按文字内容自动判断（含日文假名 → 日语）。
pub fn speak_with_lang(text: &str, lang: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("没有可朗读的文字。".into());
    }
    STOPPED.store(false, Ordering::SeqCst);
    let lang = match lang {
        "zh" | "en" | "ja" => lang,
        _ => detect_voice_lang(text),
    };

    let path = std::env::temp_dir().join(format!("vela_speak_{}.txt", std::process::id()));
    std::fs::write(&path, text).map_err(|err| format!("无法写入临时文件：{err}"))?;
    let path_str = path.to_string_lossy().replace('\'', "''");

    // Fully ASCII script: the spoken text is read from the file, not embedded.
    // $ProgressPreference suppresses the "Preparing modules" progress records
    // that -NonInteractive otherwise writes to stderr. try/catch surfaces any
    // real speech error as a non-zero exit plus a message.
    let script = format!(
        "$ProgressPreference = 'SilentlyContinue'; \
         $ErrorActionPreference = 'Stop'; \
         try {{ \
           $t = Get-Content -Raw -Encoding UTF8 -LiteralPath '{path_str}'; \
           if ([string]::IsNullOrWhiteSpace($t)) {{ exit 0 }}; \
           Add-Type -AssemblyName System.Speech; \
           $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
           $voices = @($s.GetInstalledVoices()); \
           if ($voices.Count -eq 0) {{ exit 2 }}; \
           $v = $voices | Where-Object {{ $_.VoiceInfo.Culture.Name -like '{lang}*' }} | Select-Object -First 1; \
           if ($v) {{ $s.SelectVoice($v.VoiceInfo.Name) }}; \
           $parts = $t -split '(?<=[。！？.!?；;\\n])' | Where-Object {{ $_.Trim() -ne '' }}; \
           foreach ($p in $parts) {{ $s.Speak($p) }} \
         }} catch {{ \
           [Console]::Error.WriteLine($_.Exception.Message); \
           exit 1 \
         }}"
    );

    let encoded = b64_encode(&encode_utf16le(&script));

    stop_previous();

    let mut child = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("无法启动朗读：{err}"))?;

    let pid = child.id();
    {
        let mut guard = PLAYER_PID.lock().unwrap();
        *guard = Some(pid);
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("朗读出错：{err}"))?;

    {
        let mut guard = PLAYER_PID.lock().unwrap();
        if *guard == Some(pid) {
            *guard = None;
        }
    }

    // The temp file is deleted here as a fallback (the script no longer deletes it).
    let _ = std::fs::remove_file(&path);

    // User asked to stop: the killed process exits non-zero, but that is not a
    // failure to report.
    if STOPPED.load(Ordering::SeqCst) {
        return Ok(());
    }

    if !output.status.success() {
        // Exit code 2 = no installed voices at all.
        if output.status.code() == Some(2) {
            return Err(
                "系统没有安装语音。请打开「设置 → 时间和语言 → 语音」添加语音后重试。".into(),
            );
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr
            .lines()
            .filter(|l| !l.trim().is_empty())
            .last()
            .map(|l| l.trim().to_string())
            .unwrap_or_else(|| "朗读失败。请检查系统的音频输出设备。".into());
        Err(format!("朗读失败：{msg}"))
    } else {
        Ok(())
    }
}

/// 按文字内容粗判朗读语言：日文假名 → ja，汉字 → zh，否则 en。
fn detect_voice_lang(text: &str) -> &'static str {
    let mut has_cjk = false;
    let mut has_kana = false;
    for ch in text.chars() {
        if ('\u{3040}'..='\u{30FF}').contains(&ch) {
            has_kana = true;
        } else if ('\u{4E00}'..='\u{9FFF}').contains(&ch) {
            has_cjk = true;
        }
    }
    if has_kana {
        "ja"
    } else if has_cjk {
        "zh"
    } else {
        "en"
    }
}

fn encode_utf16le(text: &str) -> Vec<u8> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::with_capacity(units.len() * 2);
    for unit in units {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}

fn b64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn utf16le_is_little_endian() {
        let units = encode_utf16le("A");
        assert_eq!(units, vec![0x41, 0x00]);
    }

    #[test]
    fn speak_accepts_text_without_panicking() {
        // No audio assertion; just ensure the command path does not panic and
        // returns a Result (Ok if a voice is present, Err otherwise).
        let result = speak_with_lang("hello test", "auto");
        let _ = result;
    }
}
