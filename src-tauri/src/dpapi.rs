//! Windows DPAPI helpers. A blob is encrypted with the current user's master
//! key and can only be decrypted by this same user on this machine. It survives
//! reboot and is not cleared by tools that wipe the Credential Manager.

use std::ffi::c_void;

use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

pub fn protect(data: &[u8]) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        CryptProtectData(
            &input,
            windows::core::PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|err| format!("无法加密 Key：{err}"))?;
    Ok(copy_out(&output))
}

pub fn unprotect(data: &[u8]) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    }
    .map_err(|err| format!("无法解密 Key：{err}"))?;
    Ok(copy_out(&output))
}

fn copy_out(blob: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() || blob.cbData == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(blob.cbData as usize);
    unsafe {
        std::ptr::copy_nonoverlapping(blob.pbData, out.as_mut_ptr(), blob.cbData as usize);
        out.set_len(blob.cbData as usize);
        LocalFree(Some(HLOCAL(blob.pbData as *mut c_void)));
    }
    out
}
