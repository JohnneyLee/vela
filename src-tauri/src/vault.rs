//! API keys are sealed with a master password that is never written to disk.
//! The vault file stores an Argon2id salt, an AES-GCM nonce, and ciphertext.

use std::fs;
use std::path::Path;

use aes_gcm::aead::rand_core::{OsRng, RngCore};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const MEMORY_KIB: u32 = 19_456;
const TIME_COST: u32 = 2;
const LANES: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SecretSet {
    deepl: String,
    deepseek: String,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    kdf: String,
    m: u32,
    t: u32,
    p: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}

pub fn save_key(dir: &Path, name: &str, key: &str, password: &str) -> Result<(), String> {
    let mut secrets = match read_envelope(dir) {
        Ok(envelope) => open_envelope(&envelope, password)?,
        Err(err) if err == missing_message() => SecretSet::default(),
        Err(err) => return Err(err),
    };
    match name {
        "deepl" => secrets.deepl = key.trim().to_string(),
        "deepseek" => secrets.deepseek = key.trim().to_string(),
        _ => return Err("不认识的 Key。".into()),
    }
    write_secrets(dir, &secrets, password)?;
    secrets.deepl.zeroize();
    secrets.deepseek.zeroize();
    Ok(())
}

pub fn load_keys(dir: &Path, password: &str) -> Result<(String, String), String> {
    let envelope = read_envelope(dir)?;
    let mut secrets = open_envelope(&envelope, password)?;
    let pair = (std::mem::take(&mut secrets.deepl), std::mem::take(&mut secrets.deepseek));
    Ok(pair)
}

fn missing_message() -> String {
    "还没有加密保存的 Key。".into()
}

fn read_envelope(dir: &Path) -> Result<Envelope, String> {
    let path = dir.join("vault.json");
    if !path.is_file() {
        return Err(missing_message());
    }
    let raw = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    serde_json::from_str(&raw).map_err(|err| err.to_string())
}

fn write_secrets(dir: &Path, secrets: &SecretSet, password: &str) -> Result<(), String> {
    let mut password = require_password(password)?;
    let plain = serde_json::to_vec(secrets).map_err(|err| err.to_string())?;
    let mut salt = [0u8; 16];
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce_bytes);
    let mut derived = derive_key(password.as_bytes(), &salt, MEMORY_KIB, TIME_COST, LANES)?;
    let cipher = Aes256Gcm::new_from_slice(&derived).map_err(|err| err.to_string())?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &plain,
                aad: b"vela-keys-v1",
            },
        )
        .map_err(|err| err.to_string())?;
    derived.zeroize();
    password.zeroize();
    let envelope = Envelope {
        kdf: "argon2id".into(),
        m: MEMORY_KIB,
        t: TIME_COST,
        p: LANES,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce_bytes),
        ciphertext: B64.encode(ciphertext),
    };
    fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    let raw = serde_json::to_string(&envelope).map_err(|err| err.to_string())?;
    fs::write(dir.join("vault.json"), raw).map_err(|err| err.to_string())
}

fn open_envelope(envelope: &Envelope, password: &str) -> Result<SecretSet, String> {
    if envelope.kdf != "argon2id" {
        return Err("加密格式无法识别。".into());
    }
    let mut password = require_password(password)?;
    let salt = B64.decode(&envelope.salt).map_err(|err| err.to_string())?;
    let nonce_bytes = B64.decode(&envelope.nonce).map_err(|err| err.to_string())?;
    let ciphertext = B64
        .decode(&envelope.ciphertext)
        .map_err(|err| err.to_string())?;
    let mut derived = derive_key(password.as_bytes(), &salt, envelope.m, envelope.t, envelope.p)?;
    password.zeroize();
    let cipher = Aes256Gcm::new_from_slice(&derived).map_err(|err| err.to_string())?;
    derived.zeroize();
    if nonce_bytes.len() != 12 {
        return Err("主密码不正确。".into());
    }
    let nonce = Nonce::from_slice(&nonce_bytes);
    let mut plain = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &ciphertext,
                aad: b"vela-keys-v1",
            },
        )
        .map_err(|_| "主密码不正确。".to_string())?;
    let secrets = serde_json::from_slice(&plain).map_err(|err| err.to_string());
    plain.zeroize();
    secrets
}

fn require_password(password: &str) -> Result<String, String> {
    let password = password.trim();
    if password.is_empty() {
        return Err("请输入主密码。主密码不会保存。".into());
    }
    Ok(password.to_string())
}

fn derive_key(password: &[u8], salt: &[u8], memory: u32, time: u32, lanes: u32) -> Result<[u8; 32], String> {
    let params = Params::new(memory, time, lanes, Some(32)).map_err(|err| err.to_string())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(password, salt, &mut key)
        .map_err(|err| err.to_string())?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_file_does_not_contain_the_password_or_the_key() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.test-vault");
        let _ = fs::remove_dir_all(&dir);
        save_key(&dir, "deepseek", "sk-test-value", "only-the-user-knows").unwrap();
        let raw = fs::read_to_string(dir.join("vault.json")).unwrap();
        assert!(!raw.contains("sk-test-value"));
        assert!(!raw.contains("only-the-user-knows"));
        let (deepl, deepseek) = load_keys(&dir, "only-the-user-knows").unwrap();
        assert!(deepl.is_empty());
        assert_eq!(deepseek, "sk-test-value");
        assert!(load_keys(&dir, "wrong-password").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
