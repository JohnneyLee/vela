//! Read-aloud via the built-in Windows SAPI voices. The text is written to a
//! temporary UTF-8 file and read by a short PowerShell script, so no characters
//! ever pass through shell quoting or argument encoding.

use std::os::windows::process::CommandExt;
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 按指定语言朗读。`lang` 传 "zh"/"en"/"ja" 时用对应语音；
/// 传 "auto" 时按文字内容自动判断（含日文假名 → 日语）。
pub fn speak_with_lang(text: &str, lang: &str) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("没有可朗读的文字。".into());
    }
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
           $v = $s.GetInstalledVoices() | Where-Object {{ $_.VoiceInfo.Culture.Name -like '{lang}*' }} | Select-Object -First 1; \
           if ($v) {{ $s.SelectVoice($v.VoiceInfo.Name) }}; \
           $s.Speak($t) \
         }} catch {{ \
           [Console]::Error.WriteLine($_.Exception.Message); \
           exit 1 \
         }}"
    );

    let encoded = b64_encode(&encode_utf16le(&script));
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-EncodedCommand",
            &encoded,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    // The temp file is deleted here as a fallback (the script no longer deletes it).
    let _ = std::fs::remove_file(&path);

    match output {
        Ok(out) => {
            // Only a non-zero exit code is a failure. stderr may carry harmless
            // progress/noise even on success, so ignore it unless the script
            // actually failed (try/catch exits 1 on a real error).
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
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
        Err(err) => Err(format!("无法启动朗读：{err}")),
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
