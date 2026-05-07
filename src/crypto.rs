//! Password-based encryption for export/backup files.
//!
//! Format:
//!   MAGIC(8 bytes "KVENC1\0\0") | salt(16) | nonce(12) | ciphertext+tag
//!
//! KDF: Argon2id (default params)  →  32-byte key
//! AEAD: AES-256-GCM

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use argon2::Argon2;
use std::io;

pub const MAGIC: &[u8; 8] = b"KVENC1\0\0";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = MAGIC.len() + SALT_LEN + NONCE_LEN;

fn io_err(msg: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.to_string())
}

fn derive_key(password: &str, salt: &[u8]) -> io::Result<[u8; 32]> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| io_err(format!("Argon2 KDF: {e}")))?;
    Ok(key)
}

pub fn encrypt(plaintext: &[u8], password: &str) -> io::Result<Vec<u8>> {
    use rand::RngCore;
    let mut salt = [0u8; SALT_LEN];
    rand::rngs::OsRng.fill_bytes(&mut salt);

    let key_bytes = derive_key(password, &salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| io_err(format!("AES-GCM encrypt: {e}")))?;

    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn decrypt(data: &[u8], password: &str) -> io::Result<Vec<u8>> {
    if data.len() < HEADER_LEN || &data[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a KeyVault encrypted file",
        ));
    }
    let salt = &data[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce_bytes = &data[MAGIC.len() + SALT_LEN..HEADER_LEN];
    let ct = &data[HEADER_LEN..];

    let key_bytes = derive_key(password, salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher.decrypt(nonce, ct).map_err(|_| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Decryption failed (wrong password or corrupt file)",
        )
    })
}

pub fn looks_encrypted(data: &[u8]) -> bool {
    data.len() >= MAGIC.len() && &data[..MAGIC.len()] == MAGIC
}
