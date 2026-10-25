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
    #[serde(default = "default_source_height")]
    pub source_height: u32,
    #[serde(default = "default_direction")]
    pub direction: String,
    #[serde(default = "default_style")]
    pub style: String,
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

impl Default for Settings {
    fn default() -> Self {
        Self {
            mt_source: default_mt(),
            ai_source: default_ai(),
            deepl_key: String::new(),
            deepseek_key: String::new(),
            source_height: default_source_height(),
            direction: default_direction(),
            style: default_style(),
        }
    }
}

impl Settings {
    pub fn normalize(mut self) -> Self {
        if self.mt_source == "deepl" {
            self.mt_source = default_mt();
            if self.ai_source == "free" || self.ai_source == "gemini" {
                self.ai_source = "deepl".into();
            }
        }
        self.mt_source = match self.mt_source.as_str() {
            "google" | "mymemory" | "offline" => self.mt_source,
            _ => default_mt(),
        };
        self.ai_source = match self.ai_source.as_str() {
            "free" | "deepseek" | "deepl" => self.ai_source,
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
        self.deepl_key = self.deepl_key.trim().to_string();
        self.deepseek_key = self.deepseek_key.trim().to_string();
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
    let file_had_secret = raw.contains("\"deeplKey\"") || raw.contains("\"deepseekKey\"");
    overlay_secret(&mut settings.deepl_key, "deepl");
    overlay_secret(&mut settings.deepseek_key, "deepseek");
    if file_had_secret {
        let _ = write_file(dir, &settings);
    }
    settings
}

pub fn save(dir: &Path, settings: &Settings) -> Result<(), String> {
    let clean = settings.clone().normalize();
    secrets::store("deepl", &clean.deepl_key)?;
    secrets::store("deepseek", &clean.deepseek_key)?;
    write_file(dir, &clean)
}

fn overlay_secret(file_value: &mut String, name: &str) {
    let stored = secrets::load(name);
    if stored.is_empty() && !file_value.is_empty() {
        let _ = secrets::store(name, file_value);
    } else if !stored.is_empty() {
        *file_value = stored;
    }
}

fn write_file(dir: &Path, settings: &Settings) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let raw = serde_json::to_string_pretty(settings).map_err(|err| err.to_string())?;
    if raw.contains("\"deeplKey\"") || raw.contains("\"deepseekKey\"") {
        return Err("设置文件仍包含 Key。".into());
    }
    fs::write(dir.join("settings.json"), raw).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_file_does_not_contain_api_keys() {
        let previous_deepl = secrets::load("deepl");
        let previous_deepseek = secrets::load("deepseek");
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.test-settings");
        let _ = fs::remove_dir_all(&dir);
        let settings = Settings {
            deepl_key: "deepl-secret-value".into(),
            deepseek_key: "deepseek-secret-value".into(),
            ..Settings::default()
        };
        let saved = save(&dir, &settings);
        let raw = fs::read_to_string(dir.join("settings.json")).unwrap_or_default();
        let loaded = load(&dir);
        let _ = secrets::store("deepl", &previous_deepl);
        let _ = secrets::store("deepseek", &previous_deepseek);
        let _ = fs::remove_dir_all(&dir);
        saved.unwrap();
        assert!(!raw.contains("deepl-secret-value"));
        assert!(!raw.contains("deepseek-secret-value"));
        assert!(!raw.contains("\"deeplKey\""));
        assert!(!raw.contains("\"deepseekKey\""));
        assert_eq!(loaded.deepl_key, "deepl-secret-value");
        assert_eq!(loaded.deepseek_key, "deepseek-secret-value");
    }
}
