//! Host-scoped bus topic builders.
//!
//! Host events live under `{host}.v1.*` (e.g. `claude.v1.event.*`).
//! The shared MCP broker stays on `astrid.v1.*` via [`oracle_core::OracleIdentity`].

use oracle_core::Host;

/// Topic helpers for one host adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostTopics {
    /// Bus namespace segment (`claude`, `grok`, `codex`).
    pub namespace: &'static str,
}

impl HostTopics {
    /// Topics for a host.
    #[must_use]
    pub const fn for_host(host: Host) -> Self {
        let namespace = match host {
            Host::Claude => "claude",
            Host::Grok => "grok",
            Host::Codex => "codex",
            // non_exhaustive: new hosts must pick a bus namespace explicitly.
            _ => "unknown",
        };
        Self { namespace }
    }

    /// `{ns}.v1.install.complete`
    #[must_use]
    pub fn install_complete(&self) -> String {
        format!("{}.v1.install.complete", self.namespace)
    }

    /// `{ns}.v1.install.status`
    #[must_use]
    pub fn install_status(&self) -> String {
        format!("{}.v1.install.status", self.namespace)
    }

    /// `{ns}.v1.install.run`
    #[must_use]
    pub fn install_run(&self) -> String {
        format!("{}.v1.install.run", self.namespace)
    }

    /// KV marker prefix `{ns}.install.complete`
    #[must_use]
    pub fn install_marker_prefix(&self) -> String {
        format!("{}.install.complete", self.namespace)
    }

    /// Config KV key `{ns}.principal.config`
    #[must_use]
    pub fn principal_config_key(&self) -> String {
        format!("{}.principal.config", self.namespace)
    }

    /// Session KV prefix `{ns}.session`
    #[must_use]
    pub fn session_key_prefix(&self) -> String {
        format!("{}.session", self.namespace)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_topics() {
        let t = HostTopics::for_host(Host::Claude);
        assert_eq!(t.install_complete(), "claude.v1.install.complete");
        assert_eq!(t.principal_config_key(), "claude.principal.config");
    }
}
