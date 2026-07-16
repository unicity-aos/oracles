//! `handle_settings_set` business logic.
//!
//! Split out of `lib.rs` to keep that file under the 1000-line CI gate.
//! The interceptor wrapper lives on the `ClaudeRunner` impl in `lib.rs`; this
//! module owns the partial-patch / validate / persist / publish chain
//! and emits both the new settings-changed topic and the relink trigger.
//!
//! Contract / ordering (see the wrapper's doc-comment for the canonical
//! version):
//!
//! 1. `claude.v1.audit.settings_changed` — covered by the existing
//!    `claude.v1.audit.*` wildcard `[publish]` declaration; emitted BEFORE
//!    the relink trigger so an audit consumer sees the intent even if
//!    the relink reply never arrives.
//! 2. `claude.v1.install.relink` — triggers claude-install's `handle_relink`;
//!    the file rewrite happens in claude-install's per-capsule KV
//!    namespace (distinct from the runner's), so the new config travels
//!    in-payload (see S2/S4 — `RelinkRequest` carries an optional
//!    `config` field for cross-namespace propagation).
//! 3. Bounded wait (≤ 5 s) for the matching `claude.v1.install.complete`
//!    envelope before publishing `settings.changed`. The subscription is
//!    opened BEFORE the relink publish to avoid race-loss against a
//!    synchronous interceptor turn (mirrors `ensure_install` ordering
//!    in `lib.rs`). On `Success`/`Failure` we proceed to step 4; on
//!    timeout we publish anyway (fail-open — the on-disk relink may
//!    still complete and downstream consumers can refetch) and emit an
//!    audit warning. Failure also emits an audit entry before
//!    publishing `settings.changed`. The deadline is operator-facing
//!    (vs `ensure_install`'s 30 s spawn budget) so a short ceiling
//!    matters more than wait-it-out completeness. Correlation is by
//!    `principal_id` alone — a concurrent fresh install for the same
//!    principal is rare and either path leaves the KV record consistent,
//!    so the ambiguity is documented and accepted.
//! 4. `claude.v1.settings.changed` — emitted after the bounded wait
//!    terminates (any outcome).
//!
//! TODO(astrid-rfcs#TBD): mirror to a shared cross-capsule audit topic
//! once a convention lands; kernel-side `astrid.v1.audit.entry` is
//! admin-action-shaped and not the right home.

use astrid_sdk::prelude::*;
use oracle_host::ids::{resolve_principal, stamped_principal};
use std::time::Duration;

use crate::config;
use crate::{InstallEnvelope, SettingsSetRequest, classify_install_complete, validate_id};

const AUDIT_TOPIC: &str = "claude.v1.audit.settings_changed";
const RELINK_TOPIC: &str = "claude.v1.install.relink";
const CHANGED_TOPIC: &str = "claude.v1.settings.changed";
const INSTALL_COMPLETE_TOPIC: &str = "claude.v1.install.complete";
/// Bounded wait budget for the `claude.v1.install.complete` echo after
/// publishing `claude.v1.install.relink`. Kept tight (5 s) because the
/// `settings.changed` propagation latency is operator-perceptible —
/// contrast with `ensure_install`'s 30 s spawn budget, which the
/// operator never sees directly.
const RELINK_COMPLETE_DEADLINE: Duration = Duration::from_secs(5);

/// Apply a `SettingsSetRequest` end-to-end. Never propagates errors:
/// internal failures publish an `error` field on the audit topic and
/// return Ok (interceptor contract — publishers don't see handler
/// errors). The caller (the interceptor in `lib.rs`) just returns the
/// `Ok(())` we return here.
pub(crate) fn apply(mut req: SettingsSetRequest) -> Result<(), SysError> {
    req.principal_id = stamped_principal(&req.principal_id)?.to_string();
    // Untrusted input gate. principal_id flows into the relink payload
    // and audit publish; reject anything outside the standard alphabet
    // before the value escapes into formatted strings.
    if let Err(e) = validate_id("principal_id", &req.principal_id) {
        let _ = ipc::publish_json(
            AUDIT_TOPIC,
            &serde_json::json!({
                "principal_id": req.principal_id,
                "error": format!("invalid_principal_id: {e}"),
            }),
        );
        return Ok(());
    }

    // Load existing config with schema-version classification so a
    // future-schema record on disk (e.g. left behind by a newer runner
    // binary that the operator has just downgraded from) is NOT
    // silently flattened into our default and overwritten on save —
    // that would invert the fail-secure intent ITEM 3 / handle_spawn's
    // Unknown arm enforces on the spawn path. Mirrors that arm here so
    // the other write path agrees on the rejection contract.
    let previous = match config::load_status() {
        config::LoadOutcome::Current(cfg) => cfg,
        config::LoadOutcome::NeedsMigration {
            patched,
            previous_version,
        } => {
            // Audit the auto-migration so the relink-driven save below
            // is traceable. Mirrors handle_spawn's schema_migrated
            // event shape for cross-handler consistency.
            let _ = ipc::publish_json(
                "claude.v1.audit.schema_migrated",
                &serde_json::json!({
                    "principal_id": req.principal_id,
                    "previous_version": previous_version,
                    "current": config::SCHEMA_VERSION,
                }),
            );
            patched
        }
        config::LoadOutcome::Unknown(got) => {
            // Strictly-newer record. Refuse to overwrite operator-
            // persisted settings on a binary downgrade. Mirrors the
            // structured rejection in `handle_spawn`'s Unknown arm
            // (lib.rs) — fail-secure-loud rather than silently
            // demoting to default.
            let _ = ipc::publish_json(
                AUDIT_TOPIC,
                &serde_json::json!({
                    "principal_id": req.principal_id,
                    "error": format!(
                        "schema_version_unsupported: got {got}, supported {}",
                        config::SCHEMA_VERSION
                    ),
                }),
            );
            return Ok(());
        }
    };

    // Apply partial patch: absent Option fields preserve the current
    // value; present fields overwrite.
    let merged = config::PrincipalConfig {
        interaction_mode: req.interaction_mode.unwrap_or(previous.interaction_mode),
        auth_mode: req.auth_mode.unwrap_or(previous.auth_mode),
        model: req.model.unwrap_or(previous.model),
        max_turns: req.max_turns.or(previous.max_turns),
        schema_version: config::SCHEMA_VERSION,
    };

    // Defence-in-depth: validate the merged record even though the
    // serde enum alphabet has already filtered unknown variants —
    // `schema_version` is a u32 and could be forged by a future
    // upstream change.
    if let Err(e) = merged.validate() {
        let _ = ipc::publish_json(
            AUDIT_TOPIC,
            &serde_json::json!({
                "principal_id": req.principal_id,
                "previous_config": previous,
                "error": format!("validate_failed: {e}"),
            }),
        );
        return Ok(());
    }

    // Persist. KV write errors surface as audit-error + Ok.
    if let Err(e) = config::save(&merged) {
        let _ = ipc::publish_json(
            AUDIT_TOPIC,
            &serde_json::json!({
                "principal_id": req.principal_id,
                "previous_config": previous,
                "error": format!("kv_save_failed: {e:?}"),
            }),
        );
        return Ok(());
    }

    // Ordered publish chain — see module doc-comment for the rationale
    // on this exact ordering.

    // (1) claude.v1.audit.settings_changed FIRST.
    let _ = ipc::publish_json(
        AUDIT_TOPIC,
        &serde_json::json!({
            "principal_id": req.principal_id,
            "previous_config": previous,
            "new_config": merged,
        }),
    );

    // (2a) Subscribe to claude.v1.install.complete BEFORE publishing the
    // relink. claude-install runs as a synchronous interceptor under the
    // publishing kernel turn and may drain through `publish_complete`
    // before control returns here; subscribing after the publish would
    // race-lose. Same ordering as `ensure_install` in `lib.rs`.
    //
    // If the subscribe itself fails (kernel out of subscription slots,
    // capability missing, etc.) we fall back to the legacy fire-and-
    // forget path with an audit warning rather than hard-erroring out
    // of settings.set — consumers may still observe the on-disk write
    // and the relink trigger itself was best-effort already.
    let install_sub = match ipc::subscribe(INSTALL_COMPLETE_TOPIC) {
        Ok(sub) => Some(sub),
        Err(e) => {
            let _ = ipc::publish_json(
                AUDIT_TOPIC,
                &serde_json::json!({
                    "principal_id": req.principal_id,
                    "error": format!("relink_subscribe_failed: {e}"),
                }),
            );
            None
        }
    };

    // (2b) claude.v1.install.relink: trigger claude-install's rewrite. The
    // merged config is threaded into the payload so claude-install (in a
    // different KV namespace) does not need to read the runner's KV.
    let _ = ipc::publish_json(
        RELINK_TOPIC,
        &serde_json::json!({
            "principal_id": req.principal_id,
            "config": merged,
        }),
    );

    // (2c) Bounded wait for claude-install to ack the relink. Reuses the
    // crate-internal `classify_install_complete` classifier so the
    // success / failure / skip branching stays in lockstep with
    // `ensure_install`. Correlation is by `principal_id` alone — see
    // module doc-comment for the accepted ambiguity vs a concurrent
    // fresh install for the same principal.
    if let Some(sub) = install_sub {
        let mut remaining_ms = u64::try_from(RELINK_COMPLETE_DEADLINE.as_millis()).unwrap_or(5_000);
        let mut outcome: Option<InstallEnvelope> = None;
        while remaining_ms > 0 && outcome.is_none() {
            let step = remaining_ms.min(2_000);
            if let Ok(result) = sub.recv(step) {
                for msg in result.messages {
                    if resolve_principal(&req.principal_id, msg.principal.verified()).is_err() {
                        continue;
                    }
                    match classify_install_complete(&msg.payload, &req.principal_id) {
                        InstallEnvelope::Skip => {}
                        env => {
                            outcome = Some(env);
                            break;
                        }
                    }
                }
            }
            remaining_ms = remaining_ms.saturating_sub(step);
        }

        match outcome {
            Some(InstallEnvelope::Success) => {
                // Happy path — claude-install finished the rewrite before
                // we publish settings.changed.
            }
            Some(InstallEnvelope::Failure(reason)) => {
                // claude-install hit a hard failure. Surface it on the
                // audit topic so the operator sees the real reason, but
                // still publish settings.changed — the canonical KV
                // record is already written and downstream consumers
                // may want to know the value changed even if the on-
                // disk projection didn't.
                let _ = ipc::publish_json(
                    AUDIT_TOPIC,
                    &serde_json::json!({
                        "principal_id": req.principal_id,
                        "error": format!("relink_failed: {reason}"),
                    }),
                );
            }
            Some(InstallEnvelope::Skip) => {
                // Loop only stores Skip when no other variant was seen
                // — unreachable in practice because the inner match
                // filters Skip out. Treated as timeout for safety.
                let _ = ipc::publish_json(
                    AUDIT_TOPIC,
                    &serde_json::json!({
                        "principal_id": req.principal_id,
                        "error": "relink_timeout",
                    }),
                );
            }
            None => {
                // Deadline elapsed with no matching envelope. Fail-open:
                // publish settings.changed anyway; the on-disk relink
                // may still complete asynchronously and consumers can
                // refetch the KV record.
                let _ = ipc::publish_json(
                    AUDIT_TOPIC,
                    &serde_json::json!({
                        "principal_id": req.principal_id,
                        "error": "relink_timeout",
                    }),
                );
            }
        }
    }

    // (3) claude.v1.settings.changed. Emitted after the bounded wait
    // terminates (success, failure, or timeout); the audit topic above
    // captures the wait outcome separately for operators.
    let _ = ipc::publish_json(
        CHANGED_TOPIC,
        &serde_json::json!({
            "principal_id": req.principal_id,
            "config": merged,
            "schema_version": config::SCHEMA_VERSION,
        }),
    );

    Ok(())
}
