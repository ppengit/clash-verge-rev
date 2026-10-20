use super::manifest::EncryptionInfo;
use aes_gcm::{
    Aes256Gcm,
    aead::{Aead as _, KeyInit as _},
};
use anyhow::{Context as _, Result, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use pbkdf2::pbkdf2_hmac;
use sha2::Sha256;

pub const KDF_ITERATIONS: u32 = 200_000;
pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 12;

fn derive_key(password: &str, salt: &[u8], iterations: u32) -> Result<[u8; 32]> {
    if iterations == 0 {
        bail!("payload encryption iteration count is invalid");
    }
    let mut key = [0_u8; 32];
    pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iterations, &mut key);
    Ok(key)
}

pub fn encrypt_payload(plaintext: &[u8], password: &str) -> Result<(Vec<u8>, EncryptionInfo)> {
    if password.is_empty() {
        bail!("payload encryption requires a password");
    }
    let mut salt = [0_u8; SALT_LEN];
    getrandom::fill(&mut salt)?;
    let key = derive_key(password, &salt, KDF_ITERATIONS)?;
    let cipher = Aes256Gcm::new_from_slice(&key)?;

    let mut nonce = [0_u8; NONCE_LEN];
    getrandom::fill(&mut nonce)?;
    let ciphertext = cipher
        .encrypt((&nonce).into(), plaintext)
        .map_err(|_| anyhow::anyhow!("failed to encrypt sync payload"))?;

    let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&ciphertext);

    Ok((
        payload,
        EncryptionInfo {
            algo: "aes-256-gcm".into(),
            kdf: "pbkdf2-sha256".into(),
            iterations: KDF_ITERATIONS,
            salt: STANDARD.encode(salt),
        },
    ))
}

pub fn decrypt_payload(payload: &[u8], password: &str, info: &EncryptionInfo) -> Result<Vec<u8>> {
    if password.is_empty() {
        bail!("the cloud payload is encrypted; set the sync password to continue");
    }
    if info.algo != "aes-256-gcm" || info.kdf != "pbkdf2-sha256" {
        bail!("the cloud payload uses unsupported encryption parameters");
    }
    if payload.len() <= NONCE_LEN {
        bail!("the encrypted sync payload is truncated");
    }
    let salt = STANDARD
        .decode(info.salt.as_bytes())
        .context("the sync manifest encryption salt is invalid")?;
    let key = derive_key(password, &salt, info.iterations)?;
    let cipher = Aes256Gcm::new_from_slice(&key)?;

    let (nonce, ciphertext) = payload.split_at(NONCE_LEN);
    let nonce: &[u8; NONCE_LEN] = nonce
        .try_into()
        .map_err(|_| anyhow::anyhow!("the encrypted sync payload nonce is invalid"))?;
    cipher
        .decrypt(nonce.into(), ciphertext)
        .map_err(|_| anyhow::anyhow!("failed to decrypt the sync payload; the password may be wrong"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_rejects_wrong_passwords_or_tampering() {
        let plaintext = b"payload contents";
        let (encrypted, info) = encrypt_payload(plaintext, "correct horse").unwrap();
        assert_ne!(&encrypted[..], plaintext);

        let decrypted = decrypt_payload(&encrypted, "correct horse", &info).unwrap();
        assert_eq!(decrypted, plaintext);

        assert!(decrypt_payload(&encrypted, "wrong", &info).is_err());

        let mut tampered = encrypted;
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(decrypt_payload(&tampered, "correct horse", &info).is_err());
    }

    #[test]
    fn manifests_never_contain_the_password() {
        let (_, info) = encrypt_payload(b"data", "secret-value").unwrap();
        let encoded = serde_json::to_string(&info).unwrap();
        assert!(!encoded.contains("secret-value"));
    }
}
