//! Singleton Unicity AOS oracle identity over the neutral Astrid wire.
//!
//! Every host shares this. Host-specific data lives on [`crate::HostProfile`].

use crate::newtypes::{AuditTopicPrefix, CapsuleName, LogTag, McpNamespace, McpToolPrefix, Topic};

/// Product and runtime identities for the shared oracle broker.
///
/// The product-facing MCP namespace is AOS. Published capsule names and
/// transport topics remain Astrid identifiers so existing artifacts and wire
/// contracts retain their provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OracleIdentity {
    /// Capsule component id.
    pub capsule_name: CapsuleName,
    /// MCP namespace segment.
    pub mcp_namespace: McpNamespace,
    /// Full MCP tool prefix including trailing `__`.
    pub mcp_tool_prefix: McpToolPrefix,
    /// Log tag.
    pub log_tag: LogTag,
    /// `astrid.v1.tools.list`
    pub tools_list_topic: Topic,
    /// `astrid.v1.tools.describe`
    pub tools_describe_topic: Topic,
    /// `astrid.v1.audit.`
    pub audit_topic_prefix: AuditTopicPrefix,
}

impl OracleIdentity {
    /// The only oracle identity. Host adapters do not get their own.
    pub const AOS: Self = Self {
        capsule_name: CapsuleName("astrid-mcp"),
        mcp_namespace: McpNamespace("aos"),
        mcp_tool_prefix: McpToolPrefix("mcp__aos__"),
        log_tag: LogTag("astrid-mcp"),
        tools_list_topic: Topic("astrid.v1.tools.list"),
        tools_describe_topic: Topic("astrid.v1.tools.describe"),
        audit_topic_prefix: AuditTopicPrefix("astrid.v1.audit."),
    };

    /// Build a full audit topic: `{prefix}{event}`.
    #[must_use]
    pub fn audit_topic(&self, event: &str) -> String {
        let mut out = String::with_capacity(self.audit_topic_prefix.as_str().len() + event.len());
        out.push_str(self.audit_topic_prefix.as_str());
        out.push_str(event);
        out
    }

    /// Allowed-tools glob (`mcp__aos__*`).
    #[must_use]
    pub fn allowed_tools_glob(&self) -> String {
        let mut out = String::with_capacity(self.mcp_tool_prefix.as_str().len() + 1);
        out.push_str(self.mcp_tool_prefix.as_str());
        out.push('*');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aos_identity_preserves_the_astrid_runtime_wire() {
        let id = OracleIdentity::AOS;
        assert_eq!(id.capsule_name.as_str(), "astrid-mcp");
        assert_eq!(id.mcp_namespace.as_str(), "aos");
        assert_eq!(id.mcp_tool_prefix.as_str(), "mcp__aos__");
        assert_eq!(id.tools_list_topic.as_str(), "astrid.v1.tools.list");
        assert_eq!(id.tools_describe_topic.as_str(), "astrid.v1.tools.describe");
        assert_eq!(id.audit_topic("policy_deny"), "astrid.v1.audit.policy_deny");
        assert_eq!(id.allowed_tools_glob(), "mcp__aos__*");
    }
}
