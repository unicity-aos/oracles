//! Principal and session id NewTypes.

use astrid_sdk::prelude::*;
use core::fmt;
use serde::{Deserialize, Serialize};

/// Hard cap on principal / session id length (wire + KV keys).
pub const MAX_ID_LEN: usize = 128;

/// Validated principal id. Construction is the only validation gate.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrincipalId(String);

impl PrincipalId {
    /// Parse and validate an untrusted principal id from IPC / config.
    pub fn parse(raw: &str) -> Result<Self, SysError> {
        validate_id("principal_id", raw)?;
        Ok(Self(raw.to_string()))
    }

    /// Borrow as `str`.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for PrincipalId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PrincipalId").field(&self.0).finish()
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Validated session id.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// Parse and validate an untrusted session id.
    pub fn parse(raw: &str) -> Result<Self, SysError> {
        validate_id("session_id", raw)?;
        Ok(Self(raw.to_string()))
    }

    /// Borrow as `str`.
    #[inline]
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SessionId").field(&self.0).finish()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Shared charset gate for principal and session ids.
///
/// Rejects empty, `.` / `..`, over-length, and anything outside
/// `[A-Za-z0-9._-]`. Used by installers and runners so a host cannot
/// invent a looser parser.
pub fn validate_id(field: &str, id: &str) -> Result<(), SysError> {
    if id.is_empty() {
        return Err(SysError::ApiError(format!("{field} must not be empty")));
    }
    if id == "." || id == ".." {
        return Err(SysError::ApiError(format!("{field} is reserved")));
    }
    if id.len() > MAX_ID_LEN {
        return Err(SysError::ApiError(format!(
            "{field} exceeds {MAX_ID_LEN} characters"
        )));
    }
    for c in id.chars() {
        if !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            return Err(SysError::ApiError(format!(
                "{field} contains disallowed character '{c}'"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_principal() {
        assert!(PrincipalId::parse("claude-code").is_ok());
        assert!(PrincipalId::parse("a.b_c-1").is_ok());
    }

    #[test]
    fn rejects_path_and_empty() {
        assert!(PrincipalId::parse("").is_err());
        assert!(PrincipalId::parse("..").is_err());
        assert!(PrincipalId::parse("a/b").is_err());
        assert!(PrincipalId::parse(&"x".repeat(MAX_ID_LEN + 1)).is_err());
    }
}
