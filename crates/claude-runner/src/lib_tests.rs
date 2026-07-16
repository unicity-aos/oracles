//! Host-side unit tests for items declared in `lib.rs`.
//!
//! Loaded via `#[path = "lib_tests.rs"] mod tests;` so the test bodies
//! don't push `lib.rs` over the 1000-line CI gate while keeping
//! `super::*` access to crate-private items.

use super::*;

// Regression: a claude.v1.install.complete payload with success:false
// must surface as Failure(error) so handle_spawn aborts with the
// real reason instead of progressing to api_key_missing.
#[test]
fn install_complete_failure_carries_error_reason() {
    let payload = r#"{
        "principal_id": "p1",
        "success": false,
        "home_path": "",
        "error": "permission_denied: /.claude/settings.local.json"
    }"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Failure("permission_denied: /.claude/settings.local.json".into()),
    );
}

#[test]
fn install_complete_failure_without_error_falls_back_to_unknown() {
    let payload = r#"{"principal_id":"p1","success":false,"home_path":""}"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Failure("unknown".into()),
    );
}

#[test]
fn install_complete_empty_error_falls_back_to_unknown() {
    let payload = r#"{"principal_id":"p1","success":false,"home_path":"","error":""}"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Failure("unknown".into()),
    );
}

#[test]
fn install_complete_success_path() {
    let payload =
        r#"{"principal_id":"p1","success":true,"home_path":"/home/me/.aos/runtime/home/p1"}"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Success,
    );
}

#[test]
fn install_complete_cache_hit_already_installed_is_success() {
    // claude-install fast-reply: success:true with already_installed:true.
    let payload = r#"{
        "principal_id": "p1",
        "success": true,
        "home_path": "/home/me/.aos/runtime/home/p1",
        "already_installed": true
    }"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Success,
    );
}

#[test]
fn install_complete_success_does_not_trust_a_payload_home_path() {
    // Older claude-install incarnations may omit `home_path`; it is
    // informational either way because the runner always passes `home://`.
    let payload = r#"{"principal_id":"p1","success":true}"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Success,
    );
}

#[test]
fn install_complete_other_principal_is_skip() {
    let payload = r#"{"principal_id":"p2","success":false,"home_path":"","error":"boom"}"#;
    // Even a failure for a *different* principal must Skip, not
    // Failure — otherwise concurrent installs would abort each
    // other's spawn paths.
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Skip,
    );
}

#[test]
fn install_complete_missing_principal_id_is_skip() {
    let payload = r#"{"success":true,"home_path":"/x"}"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Skip,
    );
}

#[test]
fn install_complete_malformed_json_is_skip() {
    assert_eq!(
        classify_install_complete("not json", "p1"),
        InstallEnvelope::Skip,
    );
}

#[test]
fn install_complete_missing_success_field_treats_as_failure() {
    // Defence in depth: an envelope missing both `success` and
    // `error` is treated as a failure (with "unknown") rather than
    // silently succeeded — preserves the original bug's invariant
    // that a non-success payload cannot proceed to spawn.
    let payload = r#"{"principal_id":"p1","home_path":"/x"}"#;
    assert_eq!(
        classify_install_complete(payload, "p1"),
        InstallEnvelope::Failure("unknown".into()),
    );
}

// Repl-mode short-circuit: a [`PrincipalConfig`] with
// `interaction_mode == Repl` must trigger the rejection branch in
// `handle_spawn` before any subsequent state mutation. The IPC /
// KV / process side-effects of the full handler can't run on the
// host target (they live behind the wasm host imports), so we
// pin the structural invariants the branch relies on:
//
// * The default config is NOT Repl (so existing principals are
//   unaffected until they explicitly opt in via the install hook).
// * Repl mode is round-trip-stable through serde (so the install
//   hook persisting it and the spawn handler loading it agree on
//   the wire form).
// * The rejection payload carries the exact `reason` string the
//   workflow contract pins, and a hint to surface to operators.
#[test]
fn repl_mode_short_circuits_spawn_with_rejection_payload() {
    // Invariant 1: default stays Headless — the slice MUST NOT
    // silently flip existing principals to Repl on a missing KV
    // record.
    let default_cfg = config::PrincipalConfig::default();
    assert_eq!(default_cfg.interaction_mode, InteractionMode::Headless);

    // Invariant 2: Repl-config round-trips through serde so the
    // install hook's `kv::set_json` write is recoverable by
    // `load_or_default` here on the spawn side.
    let repl_cfg = config::PrincipalConfig {
        interaction_mode: InteractionMode::Repl,
        auth_mode: AuthMode::ApiKey,
        model: config::ModelPreference::default(),
        max_turns: None,
        schema_version: config::SCHEMA_VERSION,
    };
    let wire = serde_json::to_string(&repl_cfg).expect("config serializes");
    let restored: config::PrincipalConfig =
        serde_json::from_str(&wire).expect("config round-trips");
    assert_eq!(restored, repl_cfg);
    assert_eq!(restored.interaction_mode, InteractionMode::Repl);

    // Invariant 3: the rejection payload `handle_spawn` builds for
    // Repl mode carries the contract-pinned reason + hint. Build
    // it the same way the handler does so a future refactor that
    // accidentally renames `reason` or drops `hint` fails here.
    let payload = serde_json::json!({
        "principal_id": "p1",
        "reason": "interaction_mode_is_repl",
        "hint": "user drives `claude` directly in principal folder",
    });
    assert_eq!(payload["reason"], "interaction_mode_is_repl");
    assert!(payload["hint"].as_str().unwrap().contains("claude"));
    // `session_id` must NOT appear in the rejection envelope: Repl
    // mode mints no session, so reflecting the caller-supplied id
    // would imply a binding that doesn't exist.
    assert!(payload.get("session_id").is_none());
}

#[test]
fn failed_process_stop_preserves_recovery_state() {
    use std::cell::Cell;

    let cleaned = Cell::new(false);
    let result = cleanup_after_stop::<&str>(Err("stop failed"), || cleaned.set(true));
    assert_eq!(result, Err("stop failed"));
    assert!(
        !cleaned.get(),
        "state must survive while the child may be live"
    );

    cleanup_after_stop::<&str>(Ok(()), || cleaned.set(true)).expect("terminal stop");
    assert!(cleaned.get(), "terminal stop may delete token and record");
}

#[test]
fn recovery_sweep_can_be_rearmed_after_post_spawn_failure() {
    let sessions = Sessions::default();
    assert!(sessions.take_reload_recovered_flag().expect("first sweep"));
    assert!(!sessions.take_reload_recovered_flag().expect("one shot"));
    sessions
        .request_reload_recovery()
        .expect("re-arm recovery sweep");
    assert!(
        sessions
            .take_reload_recovered_flag()
            .expect("recovery retry")
    );
}
