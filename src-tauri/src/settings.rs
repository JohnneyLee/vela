//! Local engine choice. API keys live in Windows Credential Manager, never in this JSON file.
//! Translation text is never stored.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::secrets;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default = "default_mt")]
    pub mt_source: String,
    #[serde(default = "default_ai")]
    pub ai_source: String,
    #[serde(default, skip_serializing)]
    pub deepl_key: String,
    #[serde(default, skip_serializing)]
    pub deepseek_key: String,
    #[serde(default, skip_serializing)]
    pub openai_key: String,
    #[serde(default, skip_serializing)]
    pub moonshot_key: String,
    #[serde(default, skip_serializing)]
    pub gemini_key: String,
    #[serde(default)]
    pub has_deepl_key: bool,
    #[serde(default)]
    pub has_deepseek_key: bool,
    #[serde(default)]
    pub has_openai_key: bool,
    #[serde(default)]
    pub has_moonshot_key: bool,
    #[serde(default)]
    pub has_gemini_key: bool,
    #[serde(default = "default_source_height")]
    pub source_height: u32,
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(default = "default_style")]
    pub style: String,
    #[serde(default = "default_display_mode")]
    pub display_mode: String,
    #[serde(default)]
    pub allow_partial_clipboard: bool,
    #[serde(default)]
    pub local_only: bool,
    #[serde(default)]
    pub ollama_model: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_native")]
    pub native: String,
}

fn default_mt() -> String {
    "google".into()
}

fn default_ai() -> String {
    "free".into()
}

fn default_source_height() -> u32 {
    72
}

fn default_direction() -> String {
    "auto".into()
}

fn default_style() -> String {
    "none".into()
}

fn default_display_mode() -> String {
    "compare".into()
}

fn default_theme() -> String {
    "system".into()
}

/// 母语默认值：取系统界面语言，取不到（非 zh/en/ja）就回退中文。
fn default_native() -> String {
    #[cfg(windows)]
    {
        let langid = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        match langid & 0x3FF {
            0x09 => return "en".into(),
            0x11 => return "ja".into(),
            _ => return "zh".into(),
        }
    }
    #[cfg(not(windows))]
    {
        "zh".into()
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mt_source: default_mt(),
            ai_source: default_ai(),
            deepl_key: String::new(),
            deepseek_key: String::new(),
            openai_key: String::new(),
            moonshot_key: String::new(),
            gemini_key: String::new(),
            has_deepl_key: false,
            has_deepseek_key: false,
            has_openai_key: false,
            has_moonshot_key: false,
            has_gemini_key: false,
            source_height: default_source_height(),
            direction: default_direction(),
            style: default_style(),
            display_mode: default_display_mode(),
            allow_partial_clipboard: false,
            local_only: false,
            ollama_model: String::new(),
            theme: default_theme(),
            native: default_native(),
        }
    }
}

impl Settings {
    pub fn normalize(mut self) -> Self {
        self.mt_source = match self.mt_source.as_str() {
            "google" | "mymemory" | "offline" | "deepl" => self.mt_source,
            _ => default_mt(),
        };
        self.ai_source = match self.ai_source.as_str() {
            "free" | "deepseek" | "openai" | "moonshot" | "gemini" | "ollama" => {
                self.ai_source
            }
            _ => default_ai(),
        };
        self.direction = match self.direction.as_str() {
            "auto" | "zh-en" | "en-zh" => self.direction,
            _ => default_direction(),
        };
        self.style = match self.style.as_str() {
            "none" | "academic" | "colloquial" | "formal" | "idiomatic" | "literal" => self.style,
            _ => default_style(),
        };
        self.display_mode = match self.display_mode.as_str() {
            "auto" | "ai-first" | "mt-first" | "compare" => self.display_mode,
            _ => default_display_mode(),
        };
        self.deepl_key = self.deepl_key.trim().to_string();
        self.deepseek_key = self.deepseek_key.trim().to_string();
        self.openai_key = self.openai_key.trim().to_string();
        self.moonshot_key = self.moonshot_key.trim().to_string();
        self.gemini_key = self.gemini_key.trim().to_string();
        self.ollama_model = self.ollama_model.trim().to_string();
        self.theme = match self.theme.as_str() {
            "system" | "light" | "dark" | "green" | "sepia" => self.theme,
            _ => default_theme(),
        };
        self.native = match self.native.as_str() {
            "zh" | "en" | "ja" => self.native,
            _ => default_native(),
        };
        self.source_height = self.source_height.clamp(48, 400);
        self
    }
}

pub fn load(dir: &Path) -> Settings {
    let path = dir.join("settings.json");
    let raw = fs::read_to_string(&path).unwrap_or_default();
    let mut settings = serde_json::from_str::<Settings>(&raw)
        .unwrap_or_default()
        .normalize();
    let file_had_secret = ["deeplKey", "deepseekKey", "openaiKey", "moonshotKey", "geminiKey"]
        .iter()
        .any(|name| raw.contains(&format!("\"{name}\"")));
    overlay_secret(dir, &mut settings.deepl_key, "deepl");
    overlay_secret(dir, &mut settings.deepseek_key, "deepseek");
    overlay_secret(dir, &mut settings.openai_key, "openai");
    overlay_secret(dir, &mut settings.moonshot_key, "moonshot");
    overlay_secret(dir, &mut settings.gemini_key, "gemini");
    // The "has key" flags reflect DPAPI state, recomputed on every load.
    settings.has_deepl_key = !settings.deepl_key.is_empty();
    settings.has_deepseek_key = !settings.deepseek_key.is_empty();
    settings.has_openai_key = !settings.openai_key.is_empty();
    settings.has_moonshot_key = !settings.moonshot_key.is_empty();
    settings.has_gemini_key = !settings.gemini_key.is_empty();
    if file_had_secret {
        let _ = write_file(dir, &settings);
    }
    settings
}

/// Save engine/style settings. Never touches API keys — keys are stored and
/// cleared only through `save_key`, so switching style/source can not wipe them.
pub fn save(dir: &Path, settings: &Settings) -> Result<(), String> {
    let mut clean = settings.clone().normalize();
    // Preserve the key-present flags from the last load (they reflect DPAPI,
    // not the values this caller happened to send).
    let existing = load(dir);
    clean.has_deepl_key = existing.has_deepl_key;
    clean.has_deepseek_key = existing.has_deepseek_key;
    clean.has_openai_key = existing.has_openai_key;
    clean.has_moonshot_key = existing.has_moonshot_key;
    clean.has_gemini_key = existing.has_gemini_key;
    write_file(dir, &clean)
}

/// Persist one API key with DPAPI. Called only when the user actually enters a key.
pub fn save_key(dir: &Path, name: &str, key: &str) -> Result<(), String> {
    secrets::store(dir, name, key)
}

/// Remove a stored API key (both the DPAPI file and any migrated credential).
pub fn delete_key(dir: &Path, name: &str) {
    secrets::discard(dir, name)
}

fn overlay_secret(dir: &Path, file_value: &mut String, name: &str) {
    let stored = secrets::load(dir, name);
    if stored.is_empty() && !file_value.is_empty() {
        let _ = secrets::store(dir, name, file_value);
    } else if !stored.is_empty() {
        *file_value = stored;
    }
}

fn write_file(dir: &Path, settings: &Settings) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let raw = serde_json::to_string_pretty(settings).map_err(|err| err.to_string())?;
    if ["deeplKey", "deepseekKey", "openaiKey", "moonshotKey", "geminiKey"]
        .iter()
        .any(|name| raw.contains(&format!("\"{name}\"")))
    {
        return Err("设置文件仍包含 Key。".into());
    }
    fs::write(dir.join("settings.json"), raw).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_file_does_not_contain_api_keys() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.test-settings");
        let _ = fs::remove_dir_all(&dir);
        // Keys are saved through the dedicated path, never through save().
        save_key(&dir, "deepl", "deepl-secret-value").unwrap();
        save_key(&dir, "deepseek", "deepseek-secret-value").unwrap();
        // Saving a style change must not wipe the keys.
        let settings = Settings {
            style: "academic".into(),
            ..Settings::default()
        };
        save(&dir, &settings).unwrap();
        let raw = fs::read_to_string(dir.join("settings.json")).unwrap_or_default();
        assert!(!raw.contains("deepl-secret-value"));
        assert!(!raw.contains("deepseek-secret-value"));
        assert!(!raw.contains("\"deeplKey\""));
        assert!(!raw.contains("\"deepseekKey\""));
        // The keys survive the style save.
        let loaded = load(&dir);
        assert_eq!(loaded.deepl_key, "deepl-secret-value");
        assert_eq!(loaded.deepseek_key, "deepseek-secret-value");
        assert!(loaded.has_deepl_key);
        assert!(loaded.has_deepseek_key);
        let _ = fs::remove_dir_all(&dir);
    }
}
