//! Per-application writeback rules, keyed by process name. The built-in table
//! is a starting point; users can override it later through settings.

pub enum Newline {
    AsIs,
    ShiftEnter,
    JoinWithSpace,
}

pub enum WriteMode {
    Paste,
    Type,
}

pub struct Policy {
    pub paste_chord: Vec<windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY>,
    pub paste_key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    pub restore_ms: u64,
    pub select_all: bool,
    pub newline: Newline,
    pub mode: WriteMode,
    pub style: &'static str,
}

impl Default for Policy {
    fn default() -> Self {
        use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VIRTUAL_KEY};
        Policy {
            paste_chord: vec![VK_CONTROL],
            paste_key: VIRTUAL_KEY(0x56),
            restore_ms: 300,
            select_all: false,
            newline: Newline::AsIs,
            mode: WriteMode::Paste,
            style: "natural",
        }
    }
}

pub fn for_exe(exe: &str) -> Policy {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VIRTUAL_KEY};
    let exe = exe.to_ascii_lowercase();
    let mut p = Policy::default();
    match exe.as_str() {
        "wechat.exe" | "weixin.exe" | "qq.exe" | "dingtalk.exe" => {
            p.select_all = true;
            p.newline = Newline::JoinWithSpace;
            p.style = "colloquial";
        }
        "slack.exe" => {
            p.select_all = true;
            p.newline = Newline::ShiftEnter;
            p.style = "colloquial";
        }
        "outlook.exe" | "winword.exe" | "wps.exe" => {
            p.select_all = false;
            p.style = "formal";
        }
        "code.exe" => {
            p.select_all = false;
        }
        "windowsterminal.exe" | "cmd.exe" | "powershell.exe" | "pwsh.exe" => {
            p.paste_chord = vec![VK_CONTROL];
            p.paste_key = VIRTUAL_KEY(0x56);
            p.select_all = false;
        }
        "mstsc.exe" => {
            p.restore_ms = 1200;
            p.mode = WriteMode::Type;
        }
        _ => {}
    }
    p
}
