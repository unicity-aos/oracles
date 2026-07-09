//! Atomic VFS file writes shared by every host provisioner.

use astrid_sdk::prelude::*;

const RANDOM_SUFFIX_LEN: usize = 4;

/// Write `bytes` to `path` via temp-file + rename (crash-safe).
pub fn write_atomic(path: &str, bytes: &[u8]) -> Result<(), SysError> {
    let temp = temp_sibling(path)?;
    fs::write(&temp, bytes)?;
    if let Err(e) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(e);
    }
    Ok(())
}

/// Best-effort scrub of temp siblings for `path`.
pub fn cleanup_temp(path: &str) {
    let Some(prefix) = temp_prefix(path) else {
        return;
    };
    let Some(parent) = parent_dir(path) else {
        return;
    };
    let Ok(entries) = fs::read_dir(&parent) else {
        return;
    };
    for entry in entries {
        if entry.file_name().starts_with(&prefix) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn parent_dir(path: &str) -> Option<String> {
    let idx = path.rfind('/')?;
    Some(path[..idx].to_string())
}

fn temp_prefix(path: &str) -> Option<String> {
    let basename = match path.rfind('/') {
        Some(idx) => path.get(idx + 1..)?,
        None => path,
    };
    if basename.is_empty() {
        return None;
    }
    Some(format!(".{basename}.tmp."))
}

fn temp_sibling(path: &str) -> Result<String, SysError> {
    let parent = parent_dir(path)
        .ok_or_else(|| SysError::ApiError(format!("target path has no parent dir: '{path}'")))?;
    let basename = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(path);
    if basename.is_empty() {
        return Err(SysError::ApiError(format!("invalid target path: '{path}'")));
    }
    Ok(format!(
        "{parent}/.{basename}.tmp.{}",
        random_hex_suffix()?
    ))
}

fn random_hex_suffix() -> Result<String, SysError> {
    runtime::random_bytes(RANDOM_SUFFIX_LEN).map(|bytes| hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}
