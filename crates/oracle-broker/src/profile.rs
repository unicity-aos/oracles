//! Active [`OracleIdentity`] for this broker instance.
//!
//! Thin capsule calls [`install`] once. The public namespace is always AOS;
//! hosts do not get separate wire namespaces.

use std::sync::OnceLock;

use oracle_core::OracleIdentity;

static IDENTITY: OnceLock<&'static OracleIdentity> = OnceLock::new();

/// Bind this process to the Unicity AOS oracle identity.
///
/// Idempotent. Panics only if a *different* identity pointer is installed
/// (should never happen — there is only [`OracleIdentity::AOS`]).
pub fn install(identity: &'static OracleIdentity) {
    match IDENTITY.set(identity) {
        Ok(()) => {}
        Err(existing) => {
            assert!(
                core::ptr::eq(existing, identity),
                "oracle-broker: identity already installed"
            );
        }
    }
}

/// Install the singleton AOS identity if not already set.
#[inline]
pub fn install_aos() {
    install(&OracleIdentity::AOS);
}

/// The installed oracle identity.
///
/// # Panics
/// If [`install`] / [`install_aos`] has not been called.
#[inline]
#[must_use]
pub(crate) fn identity() -> &'static OracleIdentity {
    IDENTITY.get().copied().expect(
        "oracle-broker: OracleIdentity not installed; call oracle_broker::install_aos first",
    )
}

/// Log-line tag (`astrid-mcp`).
#[inline]
#[must_use]
pub(crate) fn log_tag() -> &'static str {
    identity().log_tag.as_str()
}

/// MCP tool name prefix (`mcp__aos__`).
#[inline]
#[must_use]
pub(crate) fn mcp_tool_prefix() -> &'static str {
    identity().mcp_tool_prefix.as_str()
}

/// Tools list publish topic.
#[inline]
#[must_use]
pub(crate) fn tools_list_topic() -> &'static str {
    identity().tools_list_topic.as_str()
}

/// Build an audit topic for `event` (single segment).
#[inline]
#[must_use]
pub(crate) fn audit_topic(event: &str) -> String {
    identity().audit_topic(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_aos_is_idempotent() {
        install_aos();
        install_aos();
        assert_eq!(identity().capsule_name.as_str(), "astrid-mcp");
        assert_eq!(mcp_tool_prefix(), "mcp__aos__");
    }
}
