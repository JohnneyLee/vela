//! Error and outcome types for the capture chain.

use std::fmt;

/// How a piece of text was obtained. Used to remember which method works per app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Uia,
    CopyKey,
}

/// Result of trying one capture method.
pub enum Outcome {
    Text {
        text: String,
        #[allow(dead_code)]
        via: Via,
    },
    NoSelection,
    Unsupported,
    Failed(CaptureError),
}

/// Internal error, mapped to a user-facing message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    PasswordField,
    ElevatedTarget,
    Timeout,
    InputBlocked,
    NoSelection,
    Unsupported,
    Other(String),
}

impl CaptureError {
    pub fn user_message(&self) -> String {
        match self {
            CaptureError::ElevatedTarget => {
                "目标窗口以管理员运行，Vela 无法读取。可以在设置里以管理员身份运行 Vela，或改用截图翻译。".into()
            }
            CaptureError::NoSelection => "没有检测到选中的文字。".into(),
            CaptureError::PasswordField => "这是密码框，已跳过。".into(),
            CaptureError::Timeout => "这个软件响应太慢。可以试试截图翻译。".into(),
            CaptureError::InputBlocked => {
                "无法向这个窗口发送按键。可以试试截图翻译。".into()
            }
            CaptureError::Unsupported => "这个软件不支持直接取词，建议用截图翻译。".into(),
            CaptureError::Other(msg) => msg.clone(),
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

/// Clean up text no matter where it came from.
pub fn sanitize(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '\r' => out.push('\n'),
            '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' => {}
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    let text = out.trim().to_string();
    if text.is_empty() {
        None
    } else {
        const MAX: usize = 5000;
        let truncated: String = text.chars().take(MAX).collect();
        Some(truncated)
    }
}
