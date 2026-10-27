//! API keys are DPAPI-encrypted (tied to the current user) and written to files
//! under the app config directory. They are not plaintext and not in the
//! Credential Manager, so system cleanup tools that wipe Credential Manager
//! will not lose them. Any key left in Credential Manager by an older build is
//! migrated into the file on first read and then removed from Credential Manager.

use std::fs;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CRED_TYPE_GENERIC,
};

use crate::dpapi;

pub fn load(dir: &Path, name: &str) -> String {
    if let Some(text) = read_file(dir, name) {
        return text;
    }
    migrate_credential(dir, name)
}

pub fn store(dir: &Path, name: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    let path = key_path(dir, name);
    if value.is_empty() {
        let _ = fs::remove_file(&path);
        return Ok(());
    }
    let encrypted = dpapi::protect(value.as_bytes())?;
    fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    fs::write(&path, encrypted).map_err(|err| format!("无法保存 Key：{err}"))
}

pub fn discard(dir: &Path, name: &str) {
    let _ = fs::remove_file(key_path(dir, name));
    delete_target(&target_name(name));
    delete_target(&legacy_target_name(name));
}

fn key_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.key"))
}

fn read_file(dir: &Path, name: &str) -> Option<String> {
    let blob = fs::read(key_path(dir, name)).ok()?;
    let decrypted = dpapi::unprotect(&blob).ok()?;
    let text = String::from_utf8_lossy(&decrypted).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn migrate_credential(dir: &Path, name: &str) -> String {
    let current = read_target(&target_name(name));
    let legacy = read_target(&legacy_target_name(name));
    let source = if current.is_empty() { legacy } else { current };
    if source.is_empty() {
        return String::new();
    }
    if let Ok(encrypted) = dpapi::protect(source.as_bytes()) {
        if fs::create_dir_all(dir).is_ok() {
            let _ = fs::write(key_path(dir, name), encrypted);
        }
    }
    delete_target(&target_name(name));
    delete_target(&legacy_target_name(name));
    source
}

fn read_target(target: &str) -> String {
    let target = wide(target);
    let mut cred = std::ptr::null_mut();
    let read = unsafe { CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut cred) };
    if read.is_err() || cred.is_null() {
        return String::new();
    }
    unsafe {
        let blob = std::slice::from_raw_parts(
            (*cred).CredentialBlob,
            (*cred).CredentialBlobSize as usize,
        );
        let text = String::from_utf8_lossy(blob).trim().to_string();
        CredFree(cred as *const std::ffi::c_void);
        text
    }
}

fn delete_target(target: &str) {
    let target = wide(target);
    let _ = unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) };
}

fn target_name(name: &str) -> String {
    format!("Vela:{name}")
}

fn legacy_target_name(name: &str) -> String {
    format!("QuickTrans:{name}")
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpapi_file_roundtrip_uses_a_test_name() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.test-secrets");
        let _ = fs::remove_dir_all(&dir);
        store(&dir, "selftest", "sample-key").unwrap();
        assert_eq!(load(&dir, "selftest"), "sample-key");
        let blob = fs::read(dir.join("selftest.key")).unwrap();
        assert!(!String::from_utf8_lossy(&blob).contains("sample-key"));
        store(&dir, "selftest", "").unwrap();
        assert!(load(&dir, "selftest").is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
