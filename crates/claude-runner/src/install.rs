//! `#[astrid::install]` lifecycle hook business logic.
//!
//! Split out of `lib.rs` to keep that file under the 1000-line CI gate.
//! The `on_install` wrapper on `ClaudeRunner` (in `lib.rs`) delegates to
//! [`run`] here; this module owns the `[env]` read, the
//! [`config::PrincipalConfig`] construction, validation, KV save, and
//! the `claude.v1.audit.install_choices` audit publish.
//!
//! The macro signature is fixed at `fn(&self) -> Result<(), SysError>`
//! (see `sdk-rust/astrid-sdk-macros/src/lib.rs:222-226`) — no
//! additional typed args may be added. The kernel runs the hook
//! exactly once per (capsule, principal) install during a lifecycle
//! phase that admits `astrid:elicit@1.0.0` and `astrid:kv@1.0.0`. The
//! kernel has already walked the `[env]` block in `claude-runner/Capsule.toml`
//! by the time we run, so the operator's `interaction_mode` /
//! `auth_mode` picks are available via `astrid_sdk::env::var(...)` as
//! the literal `enum_values` strings declared there. The `api_key`
//! secret has likewise been collected by the kernel into the host
//! SecretStore — the runner does NOT touch it from the hook; the spawn path
//! reads it back at runtime via `env::var("api_key")`. Subscription-
//! mode operators leave the prompt blank, which the CLI install path
//! (`install_prompts.rs:93`) refuses to persist.
//!
//! Idempotency: calling this hook on an already-installed principal
//! overwrites the KV record with the operator's latest choices.
//! Install-time elicit takes precedence over any runtime patches that
//! `handle_settings_set` may have applied since the previous install
//! run.
//!
//! Failure mode: an unrecognised `interaction_mode` or `auth_mode`
//! returns [`SysError::ApiError`] so the install fails loud rather
//! than silently degrading to a default — an operator editing the
//! per-principal env JSON out-of-band must hit a hard stop.
//!
//! Audit: emits `claude.v1.audit.install_choices` (covered by the runner's
//! existing `claude.v1.audit.*` wildcard `[publish]` declaration).
//!
//! TODO(astrid-rfcs#TBD): mirror to a shared cross-capsule audit topic
//! once a convention lands; the kernel-side `astrid.v1.audit.entry`
//! is admin-action-shaped and not the right home for capsule-emitted
//! attribution.

use astrid_sdk::prelude::*;

use crate::config;

const AUDIT_TOPIC: &str = "claude.v1.audit.install_choices";

/// Read the kernel-elicited [env] picks, build a
/// [`config::PrincipalConfig`], validate, persist, and emit the
/// install-choices audit event. Called by the `#[astrid::install]`
/// wrapper in `lib.rs`.
pub(crate) fn run() -> Result<(), SysError> {
    // Empty string ⇒ kernel did not surface the key (older manifest
    // skew or operator skipped the prompt); fall back to the declared
    // `[env]` default rather than failing the install. Anything else
    // outside the known alphabet is a hard ApiError — operators
    // editing env JSON out-of-band must hit a stop sign rather than
    // silently degrading.
    let interaction_raw = env::var("interaction_mode").unwrap_or_default();
    let interaction_mode = match interaction_raw.as_str() {
        "headless" | "" => config::InteractionMode::Headless,
        "repl" => config::InteractionMode::Repl,
        other => {
            return Err(SysError::ApiError(format!(
                "claude-runner: unrecognised interaction_mode '{other}' (expected 'headless' or 'repl')"
            )));
        }
    };

    let auth_raw = env::var("auth_mode").unwrap_or_default();
    let auth_mode = match auth_raw.as_str() {
        "api_key" | "" => config::AuthMode::ApiKey,
        "subscription" => config::AuthMode::Subscription,
        other => {
            return Err(SysError::ApiError(format!(
                "claude-runner: unrecognised auth_mode '{other}' (expected 'api_key' or 'subscription')"
            )));
        }
    };

    let cfg = config::PrincipalConfig {
        interaction_mode,
        auth_mode,
        // Governance fields are not part of the install envelope; they
        // default at install and are set later via settings.set.
        model: config::ModelPreference::default(),
        max_turns: None,
        schema_version: config::SCHEMA_VERSION,
    };
    cfg.validate()?;
    config::save(&cfg)?;

    let _ = ipc::publish_json(
        AUDIT_TOPIC,
        &serde_json::json!({
            "interaction_mode": serde_json::to_value(interaction_mode)
                .unwrap_or(serde_json::Value::Null),
            "auth_mode": serde_json::to_value(auth_mode)
                .unwrap_or(serde_json::Value::Null),
            "schema_version": config::SCHEMA_VERSION,
        }),
    );

    Ok(())
}
