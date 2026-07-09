//! Product enum and the static profiles every broker/plugin binds to.

use crate::newtypes::{
    AuditTopicPrefix, BusNamespace, CapsuleName, DistroId, HostDisplayName, LogTag, McpNamespace,
    McpToolPrefix, PrincipalFamily, Topic,
};

/// Which Astrid-governed agent product is running.
///
/// Discriminant only — wire strings live on [`ProductProfile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Product {
    /// Claude Code on Astrid.
    Sage,
    /// Grok Build on Astrid.
    Mimir,
    /// Codex on Astrid.
    Sibyl,
}

impl Product {
    /// All known products, in stable order.
    pub const ALL: [Product; 3] = [Product::Sage, Product::Mimir, Product::Sibyl];

    /// Static profile for this product.
    #[inline]
    #[must_use]
    pub const fn profile(self) -> ProductProfile {
        match self {
            Product::Sage => ProductProfile::SAGE,
            Product::Mimir => ProductProfile::MIMIR,
            Product::Sibyl => ProductProfile::SIBYL,
        }
    }

    /// Parse a product from a distro id or capsule stem (`sage`, `sage-mcp`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        let stem = id.strip_suffix("-mcp").unwrap_or(id);
        match stem {
            "sage" => Some(Product::Sage),
            "mimir" => Some(Product::Mimir),
            "sibyl" => Some(Product::Sibyl),
            _ => None,
        }
    }
}

impl core::fmt::Display for Product {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.profile().distro_id.as_str())
    }
}

/// Complete static identity for one product.
///
/// Precomputes every hot-path wire string so the broker never formats
/// product names at runtime on the common path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductProfile {
    /// Discriminant.
    pub product: Product,
    /// `astrid init --distro` id.
    pub distro_id: DistroId,
    /// Pretty product name for humans.
    pub pretty_name: &'static str,
    /// Default principal family for the host plugin.
    pub principal_family: PrincipalFamily,
    /// MCP namespace segment (`sage` in `mcp__sage__*`).
    pub mcp_namespace: McpNamespace,
    /// Full MCP tool prefix including trailing `__`.
    pub mcp_tool_prefix: McpToolPrefix,
    /// Capsule component id (`sage-mcp`).
    pub capsule_name: CapsuleName,
    /// Bus namespace (`sage` in `sage.v1.*`).
    pub bus_namespace: BusNamespace,
    /// Host display name (`Claude`).
    pub host_display: HostDisplayName,
    /// Log tag (`sage-mcp`).
    pub log_tag: LogTag,
    /// `{ns}.v1.tools.list`
    pub tools_list_topic: Topic,
    /// `{ns}.v1.tools.describe`
    pub tools_describe_topic: Topic,
    /// `{ns}.v1.audit.`
    pub audit_topic_prefix: AuditTopicPrefix,
}

impl ProductProfile {
    /// Sage — Claude Code.
    pub const SAGE: Self = Self::const_new(
        Product::Sage,
        "sage",
        "Sage (Claude on Astrid)",
        "claude-code",
        "Claude",
    );

    /// Mimir — Grok Build.
    pub const MIMIR: Self = Self::const_new(
        Product::Mimir,
        "mimir",
        "Mimir (Grok on Astrid)",
        "grok-code",
        "Grok",
    );

    /// Sibyl — Codex.
    pub const SIBYL: Self = Self::const_new(
        Product::Sibyl,
        "sibyl",
        "Sibyl (Codex on Astrid)",
        "sibyl-code",
        "Codex",
    );

    const fn const_new(
        product: Product,
        id: &'static str,
        pretty_name: &'static str,
        principal_family: &'static str,
        host_display: &'static str,
    ) -> Self {
        // Capsule name and topics are derived from `id` via concat at const time.
        // We cannot format! in const, so each product must pass id that matches
        // the static topic strings below — verified in unit tests.
        let (capsule_name, tools_list, tools_describe, audit_prefix, mcp_prefix, log_tag) =
            match product {
                Product::Sage => (
                    "sage-mcp",
                    "sage.v1.tools.list",
                    "sage.v1.tools.describe",
                    "sage.v1.audit.",
                    "mcp__sage__",
                    "sage-mcp",
                ),
                Product::Mimir => (
                    "mimir-mcp",
                    "mimir.v1.tools.list",
                    "mimir.v1.tools.describe",
                    "mimir.v1.audit.",
                    "mcp__mimir__",
                    "mimir-mcp",
                ),
                Product::Sibyl => (
                    "sibyl-mcp",
                    "sibyl.v1.tools.list",
                    "sibyl.v1.tools.describe",
                    "sibyl.v1.audit.",
                    "mcp__sibyl__",
                    "sibyl-mcp",
                ),
            };

        // id must match the bus namespace used in the match arms above.
        let _ = id;

        Self {
            product,
            distro_id: DistroId(id),
            pretty_name,
            principal_family: PrincipalFamily(principal_family),
            mcp_namespace: McpNamespace(id),
            mcp_tool_prefix: McpToolPrefix(mcp_prefix),
            capsule_name: CapsuleName(capsule_name),
            bus_namespace: BusNamespace(id),
            host_display: HostDisplayName(host_display),
            log_tag: LogTag(log_tag),
            tools_list_topic: Topic(tools_list),
            tools_describe_topic: Topic(tools_describe),
            audit_topic_prefix: AuditTopicPrefix(audit_prefix),
        }
    }

    /// Build a full audit topic: `{prefix}{event}` (e.g. `sage.v1.audit.policy_deny`).
    ///
    /// `event` must be a single clean segment (no dots). Allocates.
    #[must_use]
    pub fn audit_topic(&self, event: &str) -> String {
        let mut out = String::with_capacity(self.audit_topic_prefix.as_str().len() + event.len());
        out.push_str(self.audit_topic_prefix.as_str());
        out.push_str(event);
        out
    }

    /// Allowed-tools glob for the host (`mcp__sage__*`).
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
    fn profiles_are_internally_consistent() {
        for product in Product::ALL {
            let p = product.profile();
            assert_eq!(p.product, product);
            assert_eq!(p.distro_id.as_str(), p.bus_namespace.as_str());
            assert_eq!(p.mcp_namespace.as_str(), p.bus_namespace.as_str());
            assert!(p.mcp_tool_prefix.as_str().starts_with("mcp__"));
            assert!(p.mcp_tool_prefix.as_str().ends_with("__"));
            assert!(p.mcp_tool_prefix.as_str().contains(p.mcp_namespace.as_str()));
            assert!(p.tools_list_topic.as_str().starts_with(p.bus_namespace.as_str()));
            assert!(p.tools_describe_topic.as_str().starts_with(p.bus_namespace.as_str()));
            assert!(p.audit_topic_prefix.as_str().starts_with(p.bus_namespace.as_str()));
            assert_eq!(p.capsule_name.as_str(), p.log_tag.as_str());
            assert_eq!(
                p.audit_topic("policy_deny"),
                format!("{}policy_deny", p.audit_topic_prefix.as_str())
            );
            assert_eq!(
                p.allowed_tools_glob(),
                format!("{}*", p.mcp_tool_prefix.as_str())
            );
        }
    }

    #[test]
    fn from_id_accepts_distro_and_capsule_stems() {
        assert_eq!(Product::from_id("sage"), Some(Product::Sage));
        assert_eq!(Product::from_id("sage-mcp"), Some(Product::Sage));
        assert_eq!(Product::from_id("mimir-mcp"), Some(Product::Mimir));
        assert_eq!(Product::from_id("sibyl"), Some(Product::Sibyl));
        assert_eq!(Product::from_id("unknown"), None);
    }

    #[test]
    fn principal_families_are_distinct() {
        let families: Vec<_> = Product::ALL
            .iter()
            .map(|p| p.profile().principal_family.as_str())
            .collect();
        assert_eq!(families, ["claude-code", "grok-code", "sibyl-code"]);
    }
}
