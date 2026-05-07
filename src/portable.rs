//! Portable file format for export / backup.
//!
//! On disk: either plain JSON of `ExportPayload`, or AES-GCM encrypted bytes
//! whose plaintext is that same JSON (see `crypto.rs`).

use crate::crypto;
use crate::model::{Collection, Vault};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;
use uuid::Uuid;

const PAYLOAD_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ExportPayload {
    #[serde(rename = "kv-collection")]
    Collection { version: u32, collection: Collection },
    #[serde(rename = "kv-backup")]
    Backup { version: u32, vault: Vault },
}

impl ExportPayload {
    pub fn collection(c: Collection) -> Self {
        ExportPayload::Collection {
            version: PAYLOAD_VERSION,
            collection: c,
        }
    }
    pub fn backup(v: Vault) -> Self {
        ExportPayload::Backup {
            version: PAYLOAD_VERSION,
            vault: v,
        }
    }
}

/// Write the payload to disk, encrypting iff `password` is non-empty.
pub fn write(path: &Path, payload: &ExportPayload, password: &str) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let bytes = if password.is_empty() {
        json
    } else {
        crypto::encrypt(&json, password)?
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(
        path.extension()
            .map(|e| {
                let mut s = e.to_os_string();
                s.push(".tmp");
                s
            })
            .unwrap_or_else(|| std::ffi::OsString::from("tmp")),
    );
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Read raw bytes from disk and report whether they look encrypted.
pub fn read_raw(path: &Path) -> io::Result<(Vec<u8>, bool)> {
    let bytes = fs::read(path)?;
    let enc = crypto::looks_encrypted(&bytes);
    Ok((bytes, enc))
}

/// Decode bytes (decrypting first if a password is supplied) into an `ExportPayload`.
pub fn decode(bytes: &[u8], password: &str) -> io::Result<ExportPayload> {
    let json = if crypto::looks_encrypted(bytes) {
        if password.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "File is encrypted; password required",
            ));
        }
        crypto::decrypt(bytes, password)?
    } else {
        bytes.to_vec()
    };
    serde_json::from_slice::<ExportPayload>(&json).map_err(|e| {
        io::Error::new(io::ErrorKind::InvalidData, format!("Invalid file: {e}"))
    })
}

/// Generate fresh UUIDs for an imported collection so it doesn't collide with
/// or silently overwrite an existing one.
pub fn rekey_collection(c: &mut Collection) {
    c.id = Uuid::new_v4();
    for tab in &mut c.tabs {
        tab.id = Uuid::new_v4();
        for group in &mut tab.groups {
            group.id = Uuid::new_v4();
            for item in &mut group.items {
                use crate::model::VaultItem::*;
                match item {
                    Text { id, .. }
                    | KeyValue { id, .. }
                    | ReplaceText { id, .. }
                    | Note { id, .. } => {
                        *id = Uuid::new_v4();
                    }
                }
            }
        }
    }
}
