//! API keys live in the Windows Credential Manager, not in a file that can be copied or uploaded.

use std::ffi::c_void;

use windows::core::PCWSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

pub fn load(name: &str) -> String {
    let current = read_target(&target_name(name));
    let legacy = read_target(&legacy_target_name(name));
    if !legacy.is_empty() {
        if current.is_empty() {
            let _ = write_target(&target_name(name), &legacy);
        }
        delete_target(&legacy_target_name(name));
    }
    if current.is_empty() { legacy } else { current }
}

pub fn store(name: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        delete_target(&target_name(name));
        delete_target(&legacy_target_name(name));
        return Ok(());
    }
    write_target(&target_name(name), value)?;
    delete_target(&legacy_target_name(name));
    Ok(())
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
        CredFree(cred as *const c_void);
        text
    }
}

fn write_target(target: &str, value: &str) -> Result<(), String> {
    let mut blob = value.as_bytes().to_vec();
    let mut target_buf = wide(target);
    let cred = CREDENTIALW {
        Flags: Default::default(),
        Type: CRED_TYPE_GENERIC,
        TargetName: windows::core::PWSTR(target_buf.as_mut_ptr()),
        Comment: windows::core::PWSTR::null(),
        LastWritten: FILETIME::default(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: windows::core::PWSTR::null(),
        UserName: windows::core::PWSTR::null(),
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|err| format!("无法保存 Key：{err}"))
}

fn delete_target(target: &str) {
    let target = wide(target);
    let _ = unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) };
}

pub fn discard(name: &str) {
    delete_target(&target_name(name));
    delete_target(&legacy_target_name(name));
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
    fn credential_roundtrip_uses_a_test_name() {
        let name = "selftest";
        store(name, "sample-key").unwrap();
        assert_eq!(load(name), "sample-key");
        store(name, "").unwrap();
        assert!(load(name).is_empty());
    }
}
