//! Host adapters — Claude Code / Grok Build / Codex.
//!
//! Hosts are not product brands. They are external runtimes Unicity AOS
//! integrates. Wire identity is always [`crate::OracleIdentity::AOS`].

use crate::newtypes::{HostDisplayName, PackId, PrincipalFamily};

/// Which external coding host this oracle pack targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Host {
    /// Anthropic Claude Code.
    Claude,
    /// xAI Grok Build.
    Grok,
    /// OpenAI Codex.
    Codex,
}

impl Host {
    /// All known hosts, stable order.
    pub const ALL: [Host; 3] = [Host::Claude, Host::Grok, Host::Codex];

    /// Static profile for this host.
    #[inline]
    #[must_use]
    pub const fn profile(self) -> HostProfile {
        match self {
            Host::Claude => HostProfile::CLAUDE,
            Host::Grok => HostProfile::GROK,
            Host::Codex => HostProfile::CODEX,
        }
    }

    /// Parse from a pack id or short host name (`claude`, `grok`, `codex`).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        let stem = id
            .strip_suffix("-mcp")
            .or_else(|| id.strip_suffix("-install"))
            .unwrap_or(id);
        match stem {
            "claude" => Some(Host::Claude),
            "grok" => Some(Host::Grok),
            "codex" => Some(Host::Codex),
            _ => None,
        }
    }
}

impl core::fmt::Display for Host {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.profile().pack_id.as_str())
    }
}

/// Host-only install/plugin profile. Does **not** carry MCP wire identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostProfile {
    /// Discriminant.
    pub host: Host,
    /// Additive oracle-pack id.
    pub pack_id: PackId,
    /// Pretty pack name.
    pub pretty_name: &'static str,
    /// Default principal family for this host's plugin.
    pub principal_family: PrincipalFamily,
    /// Host product display name.
    pub host_display: HostDisplayName,
}

impl HostProfile {
    /// Claude Code host.
    pub const CLAUDE: Self = Self {
        host: Host::Claude,
        pack_id: PackId("claude"),
        pretty_name: "Unicity AOS for Claude Code",
        principal_family: PrincipalFamily("claude-code"),
        host_display: HostDisplayName("Claude Code"),
    };

    /// Grok Build host.
    pub const GROK: Self = Self {
        host: Host::Grok,
        pack_id: PackId("grok"),
        pretty_name: "Unicity AOS for Grok Build",
        principal_family: PrincipalFamily("grok-code"),
        host_display: HostDisplayName("Grok Build"),
    };

    /// Codex host.
    pub const CODEX: Self = Self {
        host: Host::Codex,
        pack_id: PackId("codex"),
        pretty_name: "Unicity AOS for Codex",
        principal_family: PrincipalFamily("codex-code"),
        host_display: HostDisplayName("Codex"),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_have_distinct_principals() {
        let families: Vec<_> = Host::ALL
            .iter()
            .map(|h| h.profile().principal_family.as_str())
            .collect();
        assert_eq!(families, ["claude-code", "grok-code", "codex-code"]);
    }

    #[test]
    fn from_id_accepts_only_live_host_ids() {
        assert_eq!(Host::from_id("claude"), Some(Host::Claude));
        assert_eq!(Host::from_id("grok"), Some(Host::Grok));
        assert_eq!(Host::from_id("codex-mcp"), Some(Host::Codex));
        assert_eq!(Host::from_id("codex"), Some(Host::Codex));
        assert_eq!(Host::from_id("unknown"), None);
    }
}
