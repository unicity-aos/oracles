//! Per-principal config — the dual-axis (interaction × auth) shape
//! threaded through every `.claude/` writer in this crate.
//!
//! # Wire-shape mirror of claude-runner
//!
//! [`PrincipalConfig`] is mirrored here so install JSON writers can branch
//! without a WASM edge to the runner. Keep serde fields byte-compatible with
//! `claude-runner::config::PrincipalConfig`. Shared axes ([`InteractionMode`])
//! come from `oracle-host`.
//!
//! When the canonical type changes, update both crates and bump
//! [`PrincipalConfig::SCHEMA_VERSION`].

use serde::{Deserialize, Serialize};

/// Shared headless/repl axis (see `oracle_host::InteractionMode`).
pub(crate) use oracle_host::InteractionMode;

/// How Claude authenticates. Wire-form is `"api_key" | "subscription"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Host SecretStore-backed Anthropic API key (default).
    #[default]
    ApiKey,
    /// User runs `claude /login` in the principal folder; macOS Keychain
    /// path is HOME-blind and not cryptographically principal-isolated.
    Subscription,
}

/// Which Anthropic model tier `claude` runs under. Mirror of
/// `claude_runner` model preference — wire-form
/// `"default" | "opus" | "sonnet" | "haiku"`. The CLI-alias mapping
/// lives in `claude-runner` (claude-install never builds argv); only the
/// wire shape is mirrored here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreference {
    /// No `--model` flag; claude uses its own default.
    #[default]
    Default,
    Opus,
    Sonnet,
    Haiku,
}

/// Per-principal Claude config. Mirror of the canonical
/// `claude_runner` PrincipalConfig — see module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalConfig {
    /// How the user drives Claude (headless vs repl).
    #[serde(default)]
    pub interaction_mode: InteractionMode,
    /// How Claude authenticates (api_key vs subscription).
    #[serde(default)]
    pub auth_mode: AuthMode,
    /// Model tier `claude` runs under (governance). Mirror field — used
    /// by claude-runner to build argv; carried here only to keep the wire shape
    /// byte-identical.
    #[serde(default)]
    pub model: ModelPreference,
    /// Optional per-session agentic-turn cap (governance). Mirror field.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Forward-compat tag. Bumped when the shape changes incompatibly.
    #[serde(default = "PrincipalConfig::default_schema_version")]
    pub schema_version: u32,
}

impl Default for PrincipalConfig {
    fn default() -> Self {
        Self {
            interaction_mode: InteractionMode::default(),
            auth_mode: AuthMode::default(),
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: Self::SCHEMA_VERSION,
        }
    }
}

impl PrincipalConfig {
    /// Wire-format version. Persisted so older runner payloads can be
    /// detected and migrated; bump on incompatible shape changes.
    pub const SCHEMA_VERSION: u32 = 2;

    fn default_schema_version() -> u32 {
        Self::SCHEMA_VERSION
    }

    /// Best-effort sanity check. Today the serde enum variants are
    /// closed (anything else is a deserialize error), so the only thing
    /// left to enforce is `schema_version <= SCHEMA_VERSION` — a future
    /// payload from a newer runner would fail loudly here rather than be
    /// silently truncated to the default.
    #[allow(dead_code)]
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version > Self::SCHEMA_VERSION {
            return Err(format!(
                "PrincipalConfig.schema_version {} exceeds supported {} — upgrade claude-install",
                self.schema_version,
                Self::SCHEMA_VERSION
            ));
        }
        if self.max_turns == Some(0) {
            return Err(
                "PrincipalConfig.max_turns must be >= 1 when set; 0 forbids all work".to_string(),
            );
        }
        if self.interaction_mode == InteractionMode::Headless
            && self.auth_mode == AuthMode::Subscription
        {
            return Err(
                "subscription authentication requires repl interaction mode; headless Claude requires an API key"
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_headless_api_key_v2() {
        let cfg = PrincipalConfig::default();
        assert_eq!(cfg.interaction_mode, InteractionMode::Headless);
        assert_eq!(cfg.auth_mode, AuthMode::ApiKey);
        assert_eq!(cfg.model, ModelPreference::Default);
        assert_eq!(cfg.max_turns, None);
        assert_eq!(cfg.schema_version, PrincipalConfig::SCHEMA_VERSION);
    }

    #[test]
    fn serde_wire_format_uses_snake_case() {
        let cfg = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::Subscription,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: 1,
        };
        let v = serde_json::to_value(cfg).unwrap();
        assert_eq!(v["interaction_mode"], "repl");
        assert_eq!(v["auth_mode"], "subscription");
        assert_eq!(v["schema_version"], 1);
    }

    #[test]
    fn deserialise_accepts_missing_fields_via_defaults() {
        let cfg: PrincipalConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, PrincipalConfig::default());
    }

    #[test]
    fn validate_rejects_future_schema_version() {
        let cfg = PrincipalConfig {
            schema_version: u32::MAX,
            ..PrincipalConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_current_schema_version() {
        assert!(PrincipalConfig::default().validate().is_ok());
    }

    #[test]
    fn validate_rejects_headless_subscription() {
        let cfg = PrincipalConfig {
            auth_mode: AuthMode::Subscription,
            ..PrincipalConfig::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn validate_accepts_repl_subscription() {
        let cfg = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::Subscription,
            ..PrincipalConfig::default()
        };
        assert!(cfg.validate().is_ok());
    }

    /// Canonical fully-populated wire payload for schema_version=2. The
    /// sibling `claude-runner` crate's test module declares an identical literal
    /// — keep the two strings byte-for-byte equal. Bump alongside
    /// [`PrincipalConfig::SCHEMA_VERSION`] when the wire shape changes.
    /// Avoiding a shared crate by design (two consumers only); the
    /// reciprocal serialize/deserialize tests in both crates pin the
    /// contract.
    const WIRE_FORMAT_V2: &str = r#"{"interaction_mode":"headless","auth_mode":"api_key","model":"default","max_turns":null,"schema_version":2}"#;

    #[test]
    fn default_serializes_to_wire_format_v2() {
        let json = serde_json::to_string(&PrincipalConfig::default()).unwrap();
        assert_eq!(json, WIRE_FORMAT_V2);
    }

    #[test]
    fn wire_format_v2_round_trips_to_default() {
        let cfg: PrincipalConfig = serde_json::from_str(WIRE_FORMAT_V2).unwrap();
        assert_eq!(cfg, PrincipalConfig::default());
    }
}
