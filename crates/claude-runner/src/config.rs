//! Per-principal Claude host runtime configuration.
//!
//! Two orthogonal axes, both serialised in snake_case wire form so the
//! values match the `[env]` `select` options the operator sees at install
//! time:
//!
//! * [`InteractionMode`] — `headless` (Astrid drives `claude -p`) versus
//!   `repl` (user runs `claude` directly in the principal folder).
//! * [`AuthMode`] — `api_key` (kernel-elicited secret, exported to the
//!   subprocess as `ANTHROPIC_API_KEY`) versus `subscription` (user runs
//!   `claude /login` in Claude Code's interactive REPL). Subscription auth is
//!   invalid in headless mode.
//!
//! Persisted in the runner's per-capsule per-principal KV namespace at the
//! single canonical key [`KV_KEY`]. The `#[astrid::install]` lifecycle
//! hook is the first writer; the `handle_settings_set` IPC interceptor
//! is the runtime writer thereafter. Both call
//! [`PrincipalConfig::validate`] before persisting — IPC input is
//! untrusted and the install env values pass through host elicitation
//! that does NOT enforce the snake_case alphabet.
//!
//! `schema_version` is set to 1 from day one so a future schema bump can
//! migrate or reject stale records without ambiguity.

use astrid_sdk::prelude::*;
use serde::{Deserialize, Serialize};

/// KV key under which the per-principal config record is stored. The
/// kernel scopes KV per capsule per principal, so the key needs no
/// principal suffix — the runner's own KV namespace is already
/// principal-scoped at the host boundary.
pub(crate) const KV_KEY: &str = "claude.principal.config";

/// Current schema version. Bump only when the wire shape changes.
/// v2 added the `model` + `max_turns` governance fields (both additive
/// with `#[serde(default)]`, so v1 records migrate forward by filling
/// the fail-secure defaults — see [`classify`]).
pub(crate) const SCHEMA_VERSION: u32 = 2;

/// Shared headless/repl axis.
pub(crate) use oracle_host::InteractionMode;

/// How `claude` authenticates against Anthropic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// The runner exports `ANTHROPIC_API_KEY` from the kernel-elicited
    /// `[env].api_key` secret on every spawn. Fail-secure default —
    /// fails closed on a missing secret rather than silently joining
    /// the user's keychain OAuth.
    #[default]
    ApiKey,
    /// User runs `claude /login` manually inside the principal folder.
    /// The runner never sets `ANTHROPIC_API_KEY`. On macOS, OAuth tokens are
    /// stored in a keychain entry keyed by service+account, NOT by
    /// `HOME` — two principal folders on the same macOS user share
    /// the credential. Document the caveat to the operator.
    Subscription,
}

/// Which Anthropic model tier `claude` runs under for this principal.
/// Astrid governance lever: an operator pins a principal to a tier.
/// `Default` omits `--model` entirely (claude uses its configured
/// default); the others map to the stable CLI aliases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreference {
    /// Don't pass `--model`; claude picks its own default. Fail-open by
    /// design — absence of a governance choice must not break spawns.
    #[default]
    Default,
    Opus,
    Sonnet,
    Haiku,
}

impl ModelPreference {
    /// The `--model` CLI alias, or `None` for [`ModelPreference::Default`]
    /// (which omits the flag). A closed alias set keeps an operator-set
    /// value from ever reaching argv as an unvalidated string.
    pub(crate) fn cli_alias(self) -> Option<&'static str> {
        match self {
            ModelPreference::Default => None,
            ModelPreference::Opus => Some("opus"),
            ModelPreference::Sonnet => Some("sonnet"),
            ModelPreference::Haiku => Some("haiku"),
        }
    }
}

/// Per-principal Claude runtime configuration. Written by the runner's install
/// hook on first run, by `handle_settings_set` thereafter. Read by
/// `handle_spawn` at the top of the spawn pipeline. Crate-private so
/// the canonical record never leaks across the capsule boundary.
///
/// Wire shape is intentionally mirrored byte-for-byte with the
/// `claude-install` crate's [`PrincipalConfig`] copy: all five fields
/// carry `#[serde(default)]` so an empty `{}` envelope round-trips on
/// both ends. The canonical fully-populated payload is asserted by the
/// `WIRE_FORMAT_V2` test constant in both crates — keep the two
/// literals identical when bumping the schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PrincipalConfig {
    /// Interaction mode (see [`InteractionMode`]).
    #[serde(default)]
    pub(crate) interaction_mode: InteractionMode,
    /// Auth mode (see [`AuthMode`]).
    #[serde(default)]
    pub(crate) auth_mode: AuthMode,
    /// Model tier `claude` runs under (see [`ModelPreference`]). Astrid
    /// governance: pin a principal to a tier. Defaults to `Default`
    /// (claude's own model, no `--model` flag), so an empty `{}` and
    /// every pre-v2 record resolve fail-open to claude's default.
    #[serde(default)]
    pub(crate) model: ModelPreference,
    /// Optional cap on claude's agentic turns per session, threaded to
    /// `--max-turns`. `None` (the default) leaves it uncapped. Astrid
    /// governance: bound a principal's per-session work.
    #[serde(default)]
    pub(crate) max_turns: Option<u32>,
    /// Schema version. Always [`SCHEMA_VERSION`] for records written by
    /// this version of the runner.
    #[serde(default = "default_schema_version")]
    pub(crate) schema_version: u32,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl Default for PrincipalConfig {
    /// Fail-secure default for missing / malformed KV records:
    /// headless (the more restricted execution path) plus api_key (the
    /// auth mode that fails-closed on a missing secret rather than
    /// silently joining the user's keychain OAuth).
    fn default() -> Self {
        Self {
            interaction_mode: InteractionMode::default(),
            auth_mode: AuthMode::default(),
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: SCHEMA_VERSION,
        }
    }
}

impl PrincipalConfig {
    /// Validate the record. IPC input passes through here as a defence-
    /// in-depth gate against forged payloads (the serde enum alphabet
    /// already rejects unknown variants; this checks the numeric field
    /// since serde happily accepts any u32 there).
    ///
    /// Rejects strictly-future schema versions (a payload from a newer
    /// runner that this binary cannot understand) while admitting
    /// equal-or-older records so the caller can drive forward migration.
    /// Matches `claude-install::PrincipalConfig::validate`'s `>` check so
    /// the two crates agree on which records are accepted.
    pub(crate) fn validate(&self) -> Result<(), SysError> {
        if self.schema_version > SCHEMA_VERSION {
            return Err(SysError::ApiError(format!(
                "unsupported schema_version: got {}, supported {}",
                self.schema_version, SCHEMA_VERSION
            )));
        }
        if self.max_turns == Some(0) {
            return Err(SysError::ApiError(
                "max_turns must be >= 1 when set; 0 would forbid all work".to_string(),
            ));
        }
        if self.interaction_mode == InteractionMode::Headless
            && self.auth_mode == AuthMode::Subscription
        {
            return Err(SysError::ApiError(
                "subscription authentication requires repl interaction mode; headless Claude requires an API key"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Build the initial runtime record from the principal-scoped install env
    /// overlay. The native installer validates this closed alphabet before it
    /// writes the overlay; an out-of-band invalid value degrades to the safer
    /// default and is logged rather than broadening execution.
    fn from_install_env() -> Self {
        let interaction = env::var("interaction_mode").unwrap_or_default();
        let auth = env::var("auth_mode").unwrap_or_default();
        Self::from_install_values(&interaction, &auth)
    }

    fn from_install_values(interaction: &str, auth: &str) -> Self {
        let interaction_mode = match interaction {
            "" | "headless" => InteractionMode::Headless,
            "repl" => InteractionMode::Repl,
            other => {
                log::warn(format!(
                    "claude-runner: invalid installed interaction_mode {other:?}; using headless"
                ));
                InteractionMode::Headless
            }
        };
        let auth_mode = match auth {
            "" | "api_key" => AuthMode::ApiKey,
            "subscription" => AuthMode::Subscription,
            other => {
                log::warn(format!(
                    "claude-runner: invalid installed auth_mode {other:?}; using api_key"
                ));
                AuthMode::ApiKey
            }
        };
        let cfg = Self {
            interaction_mode,
            auth_mode,
            ..Self::default()
        };
        if cfg.validate().is_err() {
            log::warn(
                "claude-runner: installed headless subscription config is invalid; using headless api_key",
            );
            Self::default()
        } else {
            cfg
        }
    }
}

/// Outcome of loading a principal config record from KV. Lets callers
/// distinguish three cases the prior [`load_or_default`] collapsed into
/// "use default":
///
/// * [`Current`] — record present and at the current schema version. The
///   common path; the caller uses the config verbatim.
/// * [`NeedsMigration`] — record present at a known-older schema
///   version. The struct has been patched into the current shape by
///   filling new fields from [`PrincipalConfig::default`]; callers that
///   want to persist the migration (e.g. [`handle_spawn`]) should
///   re-publish the patched config via `claude.v1.install.relink` and
///   emit `claude.v1.audit.schema_migrated`. Lenient callers (e.g. the
///   respawn sweep in `shutdown.rs`) may use the patched config
///   directly without persisting — orphaning an in-flight identity-
///   refresh session because of a schema bump is worse than running it
///   on a migrated record.
/// * [`Unknown`] — record present at a strictly-newer schema version
///   this binary doesn't understand. Carries the raw version number for
///   the caller's error envelope. The fail-secure choice here is
///   spawn-rejection rather than silently demoting to default, which
///   would overwrite operator-persisted settings on a binary downgrade.
///
/// [`SCHEMA_VERSION`] is `2` today, so the `NeedsMigration` branch IS
/// live: a record persisted at schema 1 migrates forward on its next
/// spawn, publishing `claude.v1.install.relink`. That relink and the
/// install-driven artifact reconcile in claude-install can BOTH rewrite the
/// same `.claude/` files on a single spawn — which is safe, because both
/// write the identical patched-headless shape via atomic rename. The
/// `Unknown` branch stays cold until some binary persists a
/// strictly-newer schema. (schema_version tracks the CONFIG shape,
/// independently of claude-install's `artifact_version` for the on-disk
/// FILE shape — a file change can ship without a schema bump.)
///
/// [`Current`]: LoadOutcome::Current
/// [`NeedsMigration`]: LoadOutcome::NeedsMigration
/// [`Unknown`]: LoadOutcome::Unknown
/// [`handle_spawn`]: crate::ClaudeRunner::handle_spawn
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoadOutcome {
    /// Record at the current [`SCHEMA_VERSION`]; use as-is.
    Current(PrincipalConfig),
    /// Record at a known-older schema; the carried config has been
    /// patched to the current shape using defaults for any new fields.
    /// The raw `previous_version` is preserved so the caller can emit a
    /// structured audit event before persisting via relink.
    NeedsMigration {
        /// Patched config in the current schema shape.
        patched: PrincipalConfig,
        /// Schema version observed on disk.
        previous_version: u32,
    },
    /// Record at an unknown (strictly-newer) schema version. Carries
    /// the observed version so the caller can surface it to the
    /// operator.
    Unknown(u32),
}

/// Load the current config from KV with schema-version classification.
///
/// Branching rules:
///
/// * Missing record / parse / host error → [`LoadOutcome::Current`] with
///   [`PrincipalConfig::default`]. Fail-secure: a corrupted or absent
///   KV row cannot crash the spawn path, and the safer-default surface
///   (`{Headless, ApiKey}`) keeps the principal restricted until an
///   operator runs the install hook again.
/// * `schema_version == SCHEMA_VERSION` → [`LoadOutcome::Current`].
/// * `schema_version < SCHEMA_VERSION` → [`LoadOutcome::NeedsMigration`].
///   The carried struct has been re-shaped into the current schema (any
///   genuinely new fields would be filled from `PrincipalConfig::default`
///   here — today the struct is unchanged across versions, so this is
///   forward-looking machinery).
/// * `schema_version > SCHEMA_VERSION` → [`LoadOutcome::Unknown`]. Do
///   NOT silently default; the caller must decide whether to reject the
///   spawn (the safe choice in [`handle_spawn`]) or fall back to
///   default (the documented choice in `shutdown.rs::respawn_one` via
///   [`load_or_default`], where orphaning the session is worse than
///   running it on defaults).
///
/// [`handle_spawn`]: crate::ClaudeRunner::handle_spawn
pub(crate) fn load_status() -> LoadOutcome {
    match kv::get_json_opt::<PrincipalConfig>(KV_KEY) {
        Ok(Some(cfg)) => classify(cfg),
        Ok(None) => LoadOutcome::Current(PrincipalConfig::from_install_env()),
        Err(e) => {
            log::warn(format!(
                "claude-runner: failed to load principal config: {e:?}; using default"
            ));
            LoadOutcome::Current(PrincipalConfig::default())
        }
    }
}

/// Pure classifier — split out of [`load_status`] so the version-branching
/// is unit-testable on the host without a KV round-trip. The migration
/// arm is a copy-with-current-version today (no fields changed across
/// schema versions yet); when the schema next bumps, this is the seam
/// to fill any new fields with fail-secure defaults derived from the
/// old record.
fn classify(cfg: PrincipalConfig) -> LoadOutcome {
    use std::cmp::Ordering;
    match cfg.schema_version.cmp(&SCHEMA_VERSION) {
        Ordering::Equal => LoadOutcome::Current(cfg),
        Ordering::Less => {
            let previous_version = cfg.schema_version;
            // Migration seam. When SCHEMA_VERSION next bumps, replace
            // the spread below with a per-version match that fills any
            // new fields from `PrincipalConfig::default` (fail-secure
            // defaults).
            let patched = PrincipalConfig {
                schema_version: SCHEMA_VERSION,
                ..cfg
            };
            LoadOutcome::NeedsMigration {
                patched,
                previous_version,
            }
        }
        Ordering::Greater => LoadOutcome::Unknown(cfg.schema_version),
    }
}

/// Lenient wrapper retained for callers that cannot fail loudly on a
/// schema mismatch — specifically `shutdown::respawn_one`, where
/// orphaning an in-flight identity-refresh session because of a schema
/// downgrade would be worse than running it on the migrated or default
/// config. Maps:
///
/// * [`LoadOutcome::Current`] → the carried config
/// * [`LoadOutcome::NeedsMigration`] → the patched config (silent — does
///   NOT publish a relink; the supervisor-path caller is expected to log
///   the asymmetry and proceed)
/// * [`LoadOutcome::Unknown`] → [`PrincipalConfig::default`] + warn
///
/// Interceptors that CAN fail loudly (notably [`handle_spawn`]) should
/// call [`load_status`] directly and branch on the outcome so unknown-
/// future records reject the spawn rather than silently downgrading the
/// operator-persisted settings.
///
/// [`handle_spawn`]: crate::ClaudeRunner::handle_spawn
pub(crate) fn load_or_default() -> PrincipalConfig {
    let cfg = match load_status() {
        LoadOutcome::Current(cfg) => cfg,
        LoadOutcome::NeedsMigration {
            patched,
            previous_version,
        } => {
            log::warn(format!(
                "claude-runner: principal config schema_version {previous_version} is older than \
                 current {SCHEMA_VERSION}; using migrated record without persistence"
            ));
            patched
        }
        LoadOutcome::Unknown(v) => {
            log::warn(format!(
                "claude-runner: principal config schema_version {v} is newer than supported \
                 {SCHEMA_VERSION}; using default"
            ));
            PrincipalConfig::default()
        }
    };
    if let Err(error) = cfg.validate() {
        log::warn(format!(
            "claude-runner: invalid principal config ({error}); using headless api_key"
        ));
        PrincipalConfig::default()
    } else {
        cfg
    }
}

/// Persist a config record to KV under [`KV_KEY`]. Callers MUST call
/// [`PrincipalConfig::validate`] before invoking this.
pub(crate) fn save(cfg: &PrincipalConfig) -> Result<(), SysError> {
    kv::set_json(KV_KEY, cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_headless_api_key() {
        let cfg = PrincipalConfig::default();
        assert_eq!(cfg.interaction_mode, InteractionMode::Headless);
        assert_eq!(cfg.auth_mode, AuthMode::ApiKey);
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn initial_config_inherits_validated_install_modes() {
        let cfg = PrincipalConfig::from_install_values("repl", "subscription");
        assert_eq!(cfg.interaction_mode, InteractionMode::Repl);
        assert_eq!(cfg.auth_mode, AuthMode::Subscription);
    }

    #[test]
    fn snake_case_wire_form() {
        let cfg = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::Subscription,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: SCHEMA_VERSION,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(json.contains("\"interaction_mode\":\"repl\""));
        assert!(json.contains("\"auth_mode\":\"subscription\""));
    }

    #[test]
    fn round_trips_through_json() {
        let cfg = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::Subscription,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: SCHEMA_VERSION,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let parsed: PrincipalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, parsed);
    }

    #[test]
    fn validate_rejects_future_schema_version() {
        // `validate()` uses `>` (matching claude-install) so a strictly
        // newer record fails loudly. Equal-or-older is admitted so the
        // load path can drive forward migration without smashing the
        // operator's persisted record.
        let cfg = PrincipalConfig {
            interaction_mode: InteractionMode::Headless,
            auth_mode: AuthMode::ApiKey,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: u32::MAX,
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

    // Partial-update round-trip: the SettingsSetRequest shape uses
    // Option<...> for each field so absent fields preserve the current
    // value. Verified at the handler level — here we mirror the merge
    // semantic explicitly to lock the contract in. The Option<_> patch
    // values are sourced from real JSON deserialization (rather than
    // constructed as literals) so the merge expression operates on
    // opaque-to-the-compiler Options — locking the semantic without
    // tripping `clippy::unnecessary_literal_unwrap`.
    #[test]
    fn partial_update_preserves_absent_interaction_mode() {
        // Helper takes opaque Option<_> by-value so clippy can't see
        // through to a literal None/Some at the call site.
        fn merge(
            current: PrincipalConfig,
            patch_interaction: Option<InteractionMode>,
            patch_auth: Option<AuthMode>,
        ) -> PrincipalConfig {
            PrincipalConfig {
                interaction_mode: patch_interaction.unwrap_or(current.interaction_mode),
                auth_mode: patch_auth.unwrap_or(current.auth_mode),
                model: ModelPreference::default(),
                max_turns: None,
                schema_version: SCHEMA_VERSION,
            }
        }

        let current = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::ApiKey,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: SCHEMA_VERSION,
        };

        // Simulate handle_settings_set with a partial IPC payload:
        // interaction_mode absent, auth_mode present. Deserialize the
        // real wire shape so the Option<_> values come from serde, not
        // from literal `Some(...)` / `None`.
        #[derive(Deserialize)]
        struct PartialPatch {
            #[serde(default)]
            interaction_mode: Option<InteractionMode>,
            #[serde(default)]
            auth_mode: Option<AuthMode>,
        }
        let payload = r#"{"auth_mode":"subscription"}"#;
        let patch: PartialPatch = serde_json::from_str(payload).unwrap();
        // Confirm the patch shape before exercising the merge — guards
        // against a future serde change silently breaking the contract.
        assert!(patch.interaction_mode.is_none());
        assert_eq!(patch.auth_mode, Some(AuthMode::Subscription));

        let merged = merge(current, patch.interaction_mode, patch.auth_mode);

        // interaction_mode preserved from current, auth_mode taken from
        // the patch.
        assert_eq!(merged.interaction_mode, InteractionMode::Repl);
        assert_eq!(merged.auth_mode, AuthMode::Subscription);
    }

    // Defence: serde must reject unknown enum variants at deserialize
    // time. This is the closed-alphabet guarantee `validate()` relies on
    // — without it, IPC input with `"interaction_mode":"hybrid"` would
    // round-trip as a valid record. Pairs with
    // `load_or_default`'s fail-secure-on-Err path: a malformed KV
    // record produces an Err here, which `load_or_default` catches and
    // degrades to default with no panic.
    #[test]
    fn deserialize_rejects_unknown_interaction_mode() {
        let bad = r#"{"interaction_mode":"hybrid","auth_mode":"api_key","schema_version":1}"#;
        let parsed: Result<PrincipalConfig, _> = serde_json::from_str(bad);
        assert!(parsed.is_err());
    }

    #[test]
    fn deserialize_rejects_unknown_auth_mode() {
        let bad = r#"{"interaction_mode":"headless","auth_mode":"oauth_pkce","schema_version":1}"#;
        let parsed: Result<PrincipalConfig, _> = serde_json::from_str(bad);
        assert!(parsed.is_err());
    }

    #[test]
    fn malformed_json_fails_to_deserialize() {
        // Garbage payload — the kv::get_json_opt host call would return
        // Err on this shape, which load_or_default catches and degrades
        // to default. Verify the shape itself does fail to parse so the
        // fail-secure path is actually reached.
        let parsed: Result<PrincipalConfig, _> = serde_json::from_str("not json at all");
        assert!(parsed.is_err());
    }

    #[test]
    fn empty_object_deserializes_to_default() {
        // `{}` must round-trip to the default record — every field
        // carries `#[serde(default)]` so older / partial envelopes
        // produced before all three fields shipped still parse. Mirrors
        // claude-install's `deserialise_accepts_missing_fields_via_defaults`
        // so the two crates accept the same wire shape.
        let cfg: PrincipalConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg, PrincipalConfig::default());
    }

    /// Canonical fully-populated wire payload for schema_version=2. The
    /// `claude-install` crate's test module declares an identical literal
    /// — keep the two strings byte-for-byte equal. Bump alongside
    /// `SCHEMA_VERSION` when the wire shape changes.
    const WIRE_FORMAT_V2: &str = r#"{"interaction_mode":"headless","auth_mode":"api_key","model":"default","max_turns":null,"schema_version":2}"#;

    #[test]
    fn default_serializes_to_wire_format_v2() {
        // Locks the canonical serialize output for the default record.
        // Field-order matters: serde emits struct fields in declaration
        // order, so the literal must follow the source order
        // (interaction_mode, auth_mode, model, max_turns, schema_version).
        let json = serde_json::to_string(&PrincipalConfig::default()).unwrap();
        assert_eq!(json, WIRE_FORMAT_V2);
    }

    #[test]
    fn wire_format_v2_round_trips_to_default() {
        // Reciprocal: the canonical literal deserializes to the default
        // record. Combined with `default_serializes_to_wire_format_v2`,
        // this pins the contract on both sides — any drift in field
        // names, ordering, or enum variants breaks one of the two.
        let cfg: PrincipalConfig = serde_json::from_str(WIRE_FORMAT_V2).unwrap();
        assert_eq!(cfg, PrincipalConfig::default());
    }

    // ---- LoadOutcome / classify tests ---------------------------------
    //
    // `classify` is the pure half of `load_status` (the IO half is the
    // `kv::get_json_opt` call). Testing the classifier directly here lets
    // us exercise the three schema-version branches without standing up
    // the host KV.

    #[test]
    fn classify_current_returns_current() {
        // Equal schema_version → Current(cfg).
        let cfg = PrincipalConfig::default();
        assert_eq!(classify(cfg.clone()), LoadOutcome::Current(cfg));
    }

    #[test]
    fn classify_older_returns_needs_migration() {
        // Strictly-older schema_version → NeedsMigration with the
        // version bumped to current. `previous_version` carries the raw
        // observed value so the caller can audit the migration.
        //
        // We construct an artificially-older record by setting
        // schema_version=0 manually — strictly below the current
        // SCHEMA_VERSION (2), so it classifies as NeedsMigration.
        let older = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::Subscription,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: 0,
        };
        match classify(older) {
            LoadOutcome::NeedsMigration {
                patched,
                previous_version,
            } => {
                assert_eq!(previous_version, 0);
                assert_eq!(patched.schema_version, SCHEMA_VERSION);
                // Carries the operator-persisted choices forward — the
                // migration does NOT smash them with defaults.
                assert_eq!(patched.interaction_mode, InteractionMode::Repl);
                assert_eq!(patched.auth_mode, AuthMode::Subscription);
            }
            other => panic!("expected NeedsMigration, got {other:?}"),
        }
    }

    // Round-trip the `patched` config from a NeedsMigration outcome
    // through the same JSON shape `save` writes (via `kv::set_json` →
    // `serde_json::to_vec`) and back through `classify` (the pure half
    // of `load_status`). Asserts the result is `Current(patched)` —
    // i.e. once `handle_spawn`'s NeedsMigration arm calls
    // `config::save(&patched)`, the next spawn's `load_status` lands
    // on `Current` and does NOT re-trigger migration. Locks in the
    // termination property: without the `save`, this test would still
    // pass at the classifier level, but the live system would loop
    // forever (the missing `save` is the bug; this test guards the
    // serialize/classify contract those two host calls rely on).
    #[test]
    fn migration_save_then_classify_terminates_at_current() {
        let older = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::Subscription,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: 0,
        };
        let patched = match classify(older) {
            LoadOutcome::NeedsMigration { patched, .. } => patched,
            other => panic!("expected NeedsMigration, got {other:?}"),
        };
        // Mirror `save`'s wire path exactly: serde_json::to_vec is what
        // `kv::set_json` does under the hood.
        let bytes = serde_json::to_vec(&patched).unwrap();
        // Mirror `load_status`'s wire path: deserialize the bytes a
        // hypothetical `kv::get_json_opt` would return, then run the
        // same classifier the live load path runs.
        let reloaded: PrincipalConfig = serde_json::from_slice(&bytes).unwrap();
        match classify(reloaded) {
            LoadOutcome::Current(cfg) => assert_eq!(cfg, patched),
            other => panic!(
                "expected Current(patched) after save+reload (migration must \
                 terminate), got {other:?}"
            ),
        }
    }

    #[test]
    fn classify_newer_returns_unknown() {
        // Strictly-newer schema_version → Unknown(v). The caller (i.e.
        // handle_spawn) is expected to reject the spawn rather than
        // silently demote to default — a binary downgrade would
        // otherwise overwrite an operator's newer record with this
        // (older) binary's default schema_version.
        let newer = PrincipalConfig {
            interaction_mode: InteractionMode::Headless,
            auth_mode: AuthMode::ApiKey,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: u32::MAX,
        };
        assert_eq!(classify(newer), LoadOutcome::Unknown(u32::MAX));
    }

    // settings::apply (the runtime write path) reads `previous` via the
    // same classifier and MUST reject — not flatten-to-default — when
    // the on-disk record is at a strictly-newer schema. This pins the
    // contract: a synthetic future-schema `previous` carrying an
    // operator-persisted AuthMode::Subscription must classify as
    // `Unknown`, NOT as a borrowable `Current` whose values
    // settings::apply could merge against. Guards against a regression
    // where settings::apply falls back to `load_or_default` and
    // overwrites the persisted subscription with the default ApiKey on
    // binary downgrade.
    #[test]
    fn settings_apply_rejects_future_schema_rather_than_overwriting() {
        let persisted_future = PrincipalConfig {
            interaction_mode: InteractionMode::Repl,
            auth_mode: AuthMode::Subscription,
            model: ModelPreference::default(),
            max_turns: None,
            schema_version: u32::MAX,
        };
        match classify(persisted_future) {
            LoadOutcome::Unknown(v) => assert_eq!(v, u32::MAX),
            other => panic!(
                "future-schema record must classify Unknown so settings::apply \
                 rejects rather than overwriting persisted modes, got {other:?}"
            ),
        }
    }

    // Serde shape for the IPC request payload: both fields optional.
    #[test]
    fn settings_set_request_partial_payload_round_trips() {
        // Only auth_mode set; interaction_mode absent from the JSON.
        let payload = r#"{"principal_id":"p1","auth_mode":"subscription"}"#;
        #[derive(Deserialize)]
        struct Probe {
            principal_id: String,
            #[serde(default)]
            interaction_mode: Option<InteractionMode>,
            #[serde(default)]
            auth_mode: Option<AuthMode>,
        }
        let probe: Probe = serde_json::from_str(payload).unwrap();
        assert_eq!(probe.principal_id, "p1");
        assert!(probe.interaction_mode.is_none());
        assert_eq!(probe.auth_mode, Some(AuthMode::Subscription));
    }
}
