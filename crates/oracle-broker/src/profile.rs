//! Active [`ProductProfile`] for this broker instance.
//!
//! Thin product capsules call [`install`] once (or at each handler entry)
//! before invoking broker logic. The guest is single-threaded; `OnceLock`
//! is only to reject silent double-install of a *different* product.

use std::sync::OnceLock;

use oracle_core::ProductProfile;

static PROFILE: OnceLock<&'static ProductProfile> = OnceLock::new();

/// Bind this process to a product profile.
///
/// Idempotent when the same profile is installed again. Panics if a
/// different product is installed after the first call — that would mean
/// two product capsules linked into one binary, which is a build bug.
pub fn install(profile: &'static ProductProfile) {
    match PROFILE.set(profile) {
        Ok(()) => {}
        Err(existing) => {
            assert_eq!(
                existing.product, profile.product,
                "oracle-broker: product profile already installed as {:?}, cannot reinstall as {:?}",
                existing.product, profile.product
            );
        }
    }
}

/// The installed product profile.
///
/// # Panics
/// If [`install`] has not been called.
#[inline]
#[must_use]
pub(crate) fn profile() -> &'static ProductProfile {
    PROFILE
        .get()
        .copied()
        .expect("oracle-broker: ProductProfile not installed; call oracle_broker::install first")
}

/// Log-line tag for the active product (`sage-mcp`, …).
#[inline]
#[must_use]
pub(crate) fn log_tag() -> &'static str {
    profile().log_tag.as_str()
}

/// MCP tool name prefix for the active product (`mcp__sage__`).
#[inline]
#[must_use]
pub(crate) fn mcp_tool_prefix() -> &'static str {
    profile().mcp_tool_prefix.as_str()
}

/// Tools list publish topic (`sage.v1.tools.list`).
#[inline]
#[must_use]
pub(crate) fn tools_list_topic() -> &'static str {
    profile().tools_list_topic.as_str()
}

/// Build an audit topic for `event` (single segment).
#[inline]
#[must_use]
pub(crate) fn audit_topic(event: &str) -> String {
    profile().audit_topic(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oracle_core::ProductProfile;

    #[test]
    fn install_is_idempotent_for_same_product() {
        install(&ProductProfile::SAGE);
        install(&ProductProfile::SAGE);
        assert_eq!(profile().product, oracle_core::Product::Sage);
    }
}
