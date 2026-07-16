//! S7 shutdown plumbing: graceful session termination, identity-refresh
//! teardown, and the matching respawn sweep.
//!
//! Termination protocol (per slice):
//!   1. `Signal::Term`
//!   2. Spin-wait up to [`crate::GRACEFUL_SHUTDOWN_GRACE`] checking
//!      `read_logs().running`
//!   3. `kill()` fallback if still running
//!   4. Final `read_logs` drain
//!   5. Publish `claude.v1.event.<sid>.exited{exit_code,signal,reason}`
//!   6. Evict from the live session registry; `Process` drop reaps.
//!
//! Identity refresh follows the same termination path for every session
//! owned by the principal whose identity was just saved, then immediately
//! respawns them before returning from that principal-stamped IPC message.
//! The config and secret are resolved once before teardown and passed through
//! explicitly; a later message for another principal can never change the
//! context used by the respawn.

use astrid_sdk::prelude::*;
use oracle_host::ids::{PrincipalId, resolve_principal};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::{
    AuthMode, InteractionMode, PrincipalConfig, load_or_default as load_principal_config,
};
use crate::identity;
use crate::spawn::{self, SpawnInputs};
use crate::state::{self, RuntimeSession, SessionRecord, Sessions};
use crate::{GRACEFUL_SHUTDOWN_GRACE, PENDING_RESTART_PREFIX};

/// 50 ms checkpoint inside the SIGTERM grace window. Twenty checks
/// across a 2 s grace keeps the loop responsive while staying well
/// under the host's `sleep` ceiling.
const GRACE_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Wire shape of `tool.v1.execute.save_identity.result`.
/// `success` is the only field we read; everything else is forwarded
/// observability that we don't need here.
#[derive(Debug, Deserialize)]
struct SaveIdentityResult {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    principal_id: Option<String>,
}

fn identity_refresh_principal(
    result: &SaveIdentityResult,
    stamped: Option<&str>,
) -> Result<PrincipalId, SysError> {
    match result.principal_id.as_deref() {
        Some(body) => resolve_principal(body, stamped),
        None => PrincipalId::parse(
            stamped.ok_or_else(|| SysError::ApiError("caller principal is required".into()))?,
        ),
    }
}

/// KV-persisted respawn marker. Pending session ids per principal so a
/// supervisor tick can rebuild them with a fresh identity prompt.
#[derive(Debug, Serialize, Deserialize)]
struct PendingRestart {
    principal_id: String,
    /// Per-session metadata needed to rebuild. Carries the workspace
    /// root and prior identity_path so respawn can derive the new
    /// principal home without re-querying the install crate.
    sessions: Vec<PendingRestartSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingRestartSession {
    session_id: String,
    identity_path: String,
    started_at_ms: u64,
    /// Best-effort retry counter so an unresolvable api_key / spark
    /// outage doesn't pin a marker on the bus forever.
    #[serde(default)]
    attempts: u32,
}

/// Invocation-bound inputs captured while the verified identity result is
/// still the active message. Secrets remain in memory and are never written to
/// the pending-restart marker.
struct RespawnContext {
    principal_id: String,
    home_path: String,
    config: PrincipalConfig,
    api_key: Option<String>,
}

impl RespawnContext {
    fn capture(principal_id: &PrincipalId) -> Result<Self, SysError> {
        let config = load_principal_config();
        config.validate()?;
        if config.interaction_mode != InteractionMode::Headless {
            return Err(SysError::ApiError(format!(
                "identity refresh found a live headless session for repl principal {principal_id}"
            )));
        }
        let api_key = match config.auth_mode {
            AuthMode::ApiKey => {
                let key = env::var("api_key").unwrap_or_default();
                if key.is_empty() {
                    return Err(SysError::ApiError(format!(
                        "identity refresh has no api_key for principal {principal_id}"
                    )));
                }
                Some(key)
            }
            AuthMode::Subscription => None,
        };

        Ok(Self {
            principal_id: principal_id.to_string(),
            // Keep the kernel-resolved VFS scheme. Native child env/cwd
            // translation belongs to the Astrid process host, not this guest.
            home_path: "home://".to_string(),
            config,
            api_key,
        })
    }
}

/// Cap on consecutive respawn attempts before the marker is dropped
/// and an audit event is emitted. Tunable; keep small so observability
/// surfaces hard failures fast.
const MAX_RESPAWN_ATTEMPTS: u32 = 5;

/// Gracefully stop a single session by id. Idempotent: a stop on an
/// already-evicted session is a no-op + warn.
///
/// Race-tolerant against [`supervisor::drive_session`]: if the
/// supervisor observes `running:false` during the 2 s spin-wait window,
/// it publishes its own `claude.v1.event.<sid>.exited` and evicts the
/// session. In that case our phase-3 `map.remove` returns `None` and we
/// skip the publish to avoid a duplicate exit event on the bus.
pub(crate) fn stop_session(
    sessions: &Sessions,
    session_id: &str,
    reason: &str,
) -> Result<(), SysError> {
    // Phase 1: send SIGTERM under the lock.
    let initial = sessions.with(|map| -> bool {
        if let Some(session) = map.get(session_id) {
            let _ = session.process.signal(process::Signal::Term);
            true
        } else {
            false
        }
    })?;

    if !initial {
        log::warn(format!(
            "claude-runner: stop({session_id}) — no live session, dropping"
        ));
        return Ok(());
    }

    // Phase 2: spin-wait outside the lock so interceptors keep running.
    let exited_clean = wait_for_exit(sessions, session_id, GRACEFUL_SHUTDOWN_GRACE);

    // Phase 3: SIGKILL fallback + final drain, then publish + evict.
    //
    // INVARIANT (mirrors lib.rs::send_user_turn / supervisor::drive_session):
    // NO host calls under the Sessions lock. `process.kill()` and
    // `process.read_logs()` both cross the kernel resource boundary and
    // can block on back-pressure; holding the mutex across them would
    // serialise the whole supervisor loop and risks deadlock if the
    // host call re-enters the bus drain. Phase 3a (under lock): evict
    // the entry and clone the `PersistentProcess` out into a `PreparedKill`
    // hand-off. Phase 3b (lock released): issue the host kill/drain.
    //
    // We discriminate three outcomes:
    //   * `Some(summary)` — we still owned the session and just evicted
    //     it; publish.
    //   * `None` — the supervisor's `drive_session` evicted it under us
    //     during phase 2 (and already published its own `exited` event);
    //     skip our publish to avoid a duplicate.
    let prepared = sessions.with(|map| {
        map.remove(session_id)
            .map(|session| PreparedKill { session })
    })?;

    let final_exit = prepared.map(|p| {
        let mut summary = ExitSummary::default();
        // Drain the final tail FIRST: both `stop` and `release` discard the
        // buffered tail, and — unlike the ephemeral `Process` whose `Drop`
        // reaps — dropping a `PersistentProcess` never reaps, so the host
        // slot must be freed explicitly below (`release` if already exited,
        // `stop` otherwise).
        if let Ok(logs) = p.session.process.read_logs() {
            summary.exit_code = logs.exit.and_then(|e| e.exit_code);
            summary.signal = logs.exit.and_then(|e| e.signal);
            summary.stdout_tail = trailing(&logs.stdout);
            summary.stderr_tail = trailing(&logs.stderr);
        }
        let cleanup_error = if exited_clean {
            // Exited inside the grace window — just release the id (frees the
            // slot + drops the retained tail we already captured above).
            p.session
                .process
                .clone()
                .release()
                .err()
                .map(|error| format!("release failed: {error:?}"))
        } else {
            // Still running after the grace window: SIGTERM -> grace ->
            // SIGKILL and REMOVE the id. `stop` returns the real exit, which
            // supersedes whatever `read_logs` reported above.
            match p.session.process.clone().stop(None) {
                Ok(exit) => {
                    summary.exit_code = exit.exit_code;
                    summary.signal = exit.signal;
                    None
                }
                Err(error) => Some(format!("stop failed: {error:?}")),
            }
        };
        summary.detail.clone_from(&cleanup_error);
        (summary, p, cleanup_error)
    });

    match final_exit {
        Some((summary, p, None)) => {
            let principal_id = &p.session.record.principal_id;
            publish_exited(session_id, reason, &summary);
            // Persisted record cleanup is best-effort — a stale row gets
            // cleaned up by the next reload-recovery sweep if delete fails.
            if let Err(e) = state::delete_record(session_id) {
                log::warn(format!(
                    "claude-runner: KV record cleanup for {session_id} failed: {e:?}"
                ));
            }
            // Drop the per-(principal, session) hook token so a forged
            // `claude.v1.hook.*` event arriving after the
            // session is gone can no longer pass token validation. Best-
            // effort: log on failure (parallel to delete_record above).
            if let Err(e) = crate::hooks::forget_token(principal_id, session_id) {
                log::warn(format!(
                    "claude-runner: hook-token cleanup for {session_id} failed: {e:?}"
                ));
            }
        }
        Some((summary, p, Some(cleanup_error))) => {
            // The host did not confirm terminal cleanup. Put the exact runtime
            // session back and retain its KV record + hook token; deleting
            // them here would orphan a child that may still be running.
            sessions.with(|map| {
                map.insert(session_id.to_string(), p.session);
            })?;
            sessions.request_reload_recovery()?;
            let _ = ipc::publish_json(
                &format!("claude.v1.event.{session_id}.stop_failed"),
                &serde_json::json!({
                    "reason": reason,
                    "error": cleanup_error,
                    "detail": summary.detail,
                }),
            );
            return Err(SysError::ApiError(format!(
                "stop({session_id}) did not reach a terminal host state: {cleanup_error}"
            )));
        }
        None => {
            // Supervisor's drive_session beat us to the eviction and
            // has already published an `exited` event with its own
            // reason ("exited"/"buffer_overflow"/"capsule_reload").
            // Drop our publish — at-most-once on the bus. The matching
            // `evict()` path performs the hook-token + record cleanup.
            log::info(format!(
                "claude-runner: stop({session_id}) — already evicted by supervisor; skipping duplicate exited event"
            ));
        }
    }

    Ok(())
}

/// Handle a `tool.v1.execute.save_identity.result`.
/// On `success=true` for a principal with live sessions: capture the verified
/// principal's config and secret, gracefully terminate each session, and
/// respawn it immediately with a freshly fetched identity. The whole operation
/// completes before the run loop polls another subscription.
pub(crate) fn handle_identity_refresh(
    sessions: &Sessions,
    msg: &ipc::Message,
) -> Result<(), SysError> {
    let result: SaveIdentityResult = match serde_json::from_str(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            log::warn(format!(
                "claude-runner: save_identity payload parse failed: {e}"
            ));
            return Ok(());
        }
    };
    if !result.success {
        return Ok(());
    }

    let principal = match identity_refresh_principal(&result, msg.principal.verified()) {
        Ok(principal) => principal,
        Err(error) => {
            log::warn(format!(
                "claude-runner: rejected save_identity result with untrusted principal: {error}"
            ));
            return Ok(());
        }
    };
    let principal_id = principal.to_string();

    // Snapshot the per-principal session list.
    let targets: Vec<PendingRestartSession> = sessions.with(|map| {
        map.values()
            .filter(|s| s.record.principal_id == principal_id)
            .map(|s| PendingRestartSession {
                session_id: s.record.session_id.clone(),
                identity_path: s.record.identity_path.clone(),
                started_at_ms: s.record.started_at_ms,
                attempts: 0,
            })
            .collect()
    })?;

    let key = pending_restart_key(&principal_id);
    let existing = kv::get_json_opt::<PendingRestart>(&key)?;
    if targets.is_empty() && existing.is_none() {
        return Ok(());
    }

    // Capture every ambient, principal-scoped input before teardown and before
    // the run loop can poll a message for a different principal.
    let context = RespawnContext::capture(&principal)?;

    log::info(format!(
        "claude-runner: identity refresh for {principal_id}; recycling {} session(s)",
        targets.len()
    ));

    // Persist the marker BEFORE tearing sessions down, so a crash in the
    // tear-down loop still leaves a recoverable respawn list.
    //
    // Merge with any existing marker so two save_identity events in
    // close succession (or one firing mid-teardown) don't clobber the
    // first round's pending list. KV doesn't expose CAS — best we can
    // do is read-modify-write per principal; the principal is the only
    // logical writer for its own marker.
    let merged_sessions: Vec<PendingRestartSession> = match existing {
        Some(existing) => {
            let mut union = existing.sessions;
            for new in &targets {
                if !union.iter().any(|s| s.session_id == new.session_id) {
                    union.push(new.clone());
                }
            }
            union
        }
        None => targets.clone(),
    };
    let marker = PendingRestart {
        principal_id: principal_id.clone(),
        sessions: merged_sessions,
    };
    kv::set_json(&key, &marker)?;

    let mut ready = marker.sessions.clone();
    for t in targets {
        if let Err(e) = stop_session(sessions, &t.session_id, "identity_refresh") {
            log::warn(format!(
                "claude-runner: identity-refresh stop({}) failed: {e:?}",
                t.session_id
            ));
            // The old process and recovery state remain live. Do not create a
            // second child for the same session id.
            ready.retain(|pending| pending.session_id != t.session_id);
        }
    }

    if ready.is_empty() {
        kv::delete(&key)?;
        return Ok(());
    }
    kv::set_json(
        &key,
        &PendingRestart {
            principal_id: principal_id.clone(),
            sessions: ready,
        },
    )?;

    respawn_pending_with_context(sessions, &key, &context)
}

/// Respawn one principal's pending sessions using the context captured from
/// the same verified identity-result message. A failed item stays persisted,
/// but is retried only when another verified result for that principal arrives;
/// the generic supervisor tick must not re-resolve secrets under ambient state.
fn respawn_pending_with_context(
    sessions: &Sessions,
    key: &str,
    context: &RespawnContext,
) -> Result<(), SysError> {
    let Some(marker): Option<PendingRestart> = kv::get_json_opt(key)? else {
        return Ok(());
    };
    if marker.principal_id != context.principal_id {
        return Err(SysError::ApiError(format!(
            "pending restart principal {} does not match bound context {}",
            marker.principal_id, context.principal_id
        )));
    }

    let mut still_pending: Vec<PendingRestartSession> = Vec::new();
    for mut s in marker.sessions {
        match respawn_one(sessions, context, &s) {
            Ok(()) => log::info(format!(
                "claude-runner: respawned session {} for {} on identity refresh",
                s.session_id, context.principal_id
            )),
            Err(e) => {
                s.attempts = s.attempts.saturating_add(1);
                if s.attempts >= MAX_RESPAWN_ATTEMPTS {
                    log::warn(format!(
                        "claude-runner: respawn({}) for {} failed after {} attempts; giving up — {e:?}",
                        s.session_id, context.principal_id, s.attempts
                    ));
                    let _ = ipc::publish_json(
                        "claude.v1.audit.respawn_abandoned",
                        &serde_json::json!({
                            "principal_id": context.principal_id,
                            "session_id": s.session_id,
                            "attempts": s.attempts,
                            "error": format!("{e:?}"),
                        }),
                    );
                    // Drop the session from the marker — caller has
                    // to issue a fresh spawn request to recover.
                } else {
                    log::warn(format!(
                        "claude-runner: respawn({}) for {} failed (attempt {}); will retry — {e:?}",
                        s.session_id, context.principal_id, s.attempts
                    ));
                    still_pending.push(s);
                }
            }
        }
    }

    if still_pending.is_empty() {
        if let Err(e) = kv::delete(key) {
            log::warn(format!("claude-runner: clearing {key} failed: {e:?}"));
        }
    } else {
        let updated = PendingRestart {
            principal_id: marker.principal_id,
            sessions: still_pending,
        };
        if let Err(e) = kv::set_json(key, &updated) {
            log::warn(format!("claude-runner: updating {key} failed: {e:?}"));
        }
    }
    Ok(())
}

fn respawn_one(
    sessions: &Sessions,
    context: &RespawnContext,
    s: &PendingRestartSession,
) -> Result<(), SysError> {
    // Fetch fresh identity prompt from spark + materialize a new
    // append-system-prompt file under <home>/.claude/. The identity
    // crate writes through the `home://` VFS scheme. The same scheme is
    // passed to the process request; translating guest VFS env/cwd/argv into
    // sandbox-native paths is the Astrid process host's responsibility.
    let prompt = identity::fetch_prompt(&context.principal_id, &s.session_id, &context.home_path)?;
    let identity_path = identity::write_prompt_file(&context.home_path, &s.session_id, &prompt)?;

    // Mint a fresh per-(principal, session) hook token for the
    // respawned subprocess. The previous incarnation's token was deleted
    // by `stop_session` during identity-refresh teardown; without a new
    // token persisted to KV, `astrid-emit` invocations from the
    // respawned `claude -p` child would fail the runner's validator lookup and
    // get dropped as forgeries. Mirrors the mint+persist pattern in
    // `handle_spawn` for cold spawns.
    let hook_token = crate::hooks::mint_token()?;
    crate::hooks::persist_token(&context.principal_id, &s.session_id, &hook_token)?;

    // `SpawnInputs::api_key` is `Option<&str>`; the subscription path
    // threads `None` so `spawn::spawn_claude` omits the env export.
    let inputs = bound_spawn_inputs(context, &s.session_id, &identity_path, &hook_token);
    let spawned = match spawn::spawn_claude(&inputs) {
        Ok(spawned) => spawned,
        Err(error) => {
            let _ = crate::hooks::forget_token(&context.principal_id, &s.session_id);
            return Err(error);
        }
    };

    let now_ms = match time::now() {
        Ok(t) => t
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0),
        Err(_) => 0,
    };

    let record = SessionRecord {
        principal_id: context.principal_id.clone(),
        session_id: s.session_id.clone(),
        identity_path,
        started_at_ms: now_ms,
        os_pid: spawned.os_pid,
        process_id: spawned.process_id,
    };
    let cleanup_process = spawned.process.clone();
    let recovery_process = spawned.process.clone();
    if let Err(error) = state::save_record(&record) {
        if let Err(cleanup_error) =
            crate::cleanup_untracked_process(cleanup_process, &context.principal_id, &s.session_id)
        {
            crate::preserve_recovery_session(sessions, &record, recovery_process, &cleanup_error);
        }
        return Err(error);
    }

    if let Err(error) = sessions.with(|map| {
        map.insert(
            s.session_id.clone(),
            RuntimeSession {
                record: record.clone(),
                process: spawned.process,
                codec: crate::codec::LineDecoder::default(),
            },
        );
    }) {
        if let Err(cleanup_error) =
            crate::cleanup_untracked_process(cleanup_process, &context.principal_id, &s.session_id)
        {
            crate::preserve_recovery_session(sessions, &record, recovery_process, &cleanup_error);
        }
        return Err(error);
    }

    // Audit parity with handle_spawn: emit `claude.v1.audit.spawn` so
    // downstream consumers don't need a separate path for refreshed
    // sessions vs. fresh ones. Include `auth_mode` in its canonical
    // snake_case wire form so respawn audit lines carry the same
    // attribution tuple as cold-spawn entries — without this a
    // subscription-mode respawn would be indistinguishable from an
    // api_key one in the audit stream. `interaction_mode` is omitted
    // here: respawn is only reachable from sessions the runner spawned itself
    // (i.e. headless), so the mode is implicit.
    // `claude.v1.event.<sid>.respawned` is the session-scoped event for
    // UI / metrics.
    let auth_mode_str = match context.config.auth_mode {
        AuthMode::ApiKey => "api_key",
        AuthMode::Subscription => "subscription",
    };
    let _ = ipc::publish_json(
        "claude.v1.audit.spawn",
        &serde_json::json!({
            "principal_id": context.principal_id,
            "session_id": s.session_id,
            "pid": spawned.os_pid,
            "flags_hash": spawned.flags_hash,
            "auth_mode": auth_mode_str,
            "reason": "identity_refresh",
        }),
    );
    let _ = ipc::publish_json(
        &format!("claude.v1.event.{}.respawned", s.session_id),
        &serde_json::json!({
            "principal_id": context.principal_id,
            "reason": "identity_refresh",
            "flags_hash": spawned.flags_hash,
        }),
    );
    Ok(())
}

fn bound_spawn_inputs<'a>(
    context: &'a RespawnContext,
    session_id: &'a str,
    identity_path: &'a str,
    hook_token: &'a str,
) -> SpawnInputs<'a> {
    SpawnInputs {
        principal_id: &context.principal_id,
        session_id,
        home_path: &context.home_path,
        identity_path,
        api_key: context.api_key.as_deref(),
        hook_token,
        model: context.config.model,
        max_turns: context.config.max_turns,
    }
}

// ---- helpers -----------------------------------------------------------

#[derive(Default)]
struct ExitSummary {
    exit_code: Option<i32>,
    signal: Option<i32>,
    stdout_tail: Option<String>,
    stderr_tail: Option<String>,
    detail: Option<String>,
}

/// Hand-off package collected under `Sessions::with` in
/// [`stop_session`]'s phase 3a and consumed in phase 3b outside the
/// lock. Carries the complete runtime session so a failed terminal host call
/// can restore the process handle, decoder, and persisted recovery identity.
struct PreparedKill {
    session: RuntimeSession,
}

/// Spin-wait outside the registry lock until either the session exits
/// or `grace` elapses. Returns `true` if it exited cleanly inside the
/// window. Tolerates the session being evicted under us (returns true
/// — nothing to kill). Breaks early if `time::sleep` starts erroring so
/// host shutdown doesn't pin us in a tight busy-loop on the clock.
///
/// INVARIANT (mirrors lib.rs::send_user_turn): `process.read_logs` is a
/// host call that crosses the kernel resource boundary. Holding the
/// `Sessions` mutex across it would serialise every other handler and
/// risks deadlock if the host call re-enters the bus drain. Clone the
/// `PersistentProcess` handle under the lock, drop the guard, then read.
fn wait_for_exit(sessions: &Sessions, session_id: &str, grace: Duration) -> bool {
    let deadline = time::monotonic() + grace;
    while time::monotonic() < deadline {
        // Phase 1: clone the Process handle out from under the lock.
        let proc_opt = match sessions.with(|map| map.get(session_id).map(|s| s.process.clone())) {
            Ok(p) => p,
            Err(_) => return false, // poisoned — fall through to kill()
        };
        let Some(process) = proc_opt else {
            return true; // already gone
        };

        // Phase 2: host call outside the lock.
        let still_running = match process.read_logs() {
            Ok(logs) => logs.running,
            Err(_) => return false,
        };
        if !still_running {
            return true;
        }
        if time::sleep(GRACE_POLL_INTERVAL).is_err() {
            // Host shutdown / unload — bail rather than spin on the
            // monotonic clock for the rest of the grace window.
            return false;
        }
    }
    false
}

fn publish_exited(session_id: &str, reason: &str, summary: &ExitSummary) {
    let mut payload = serde_json::json!({
        "reason": reason,
        "exit_code": summary.exit_code,
        "signal": summary.signal,
    });
    if let Some(obj) = payload.as_object_mut() {
        if let Some(tail) = &summary.stdout_tail {
            obj.insert(
                "stdout_tail".into(),
                serde_json::Value::String(tail.clone()),
            );
        }
        if let Some(tail) = &summary.stderr_tail {
            obj.insert(
                "stderr_tail".into(),
                serde_json::Value::String(tail.clone()),
            );
        }
        if let Some(detail) = &summary.detail {
            obj.insert("detail".into(), serde_json::Value::String(detail.clone()));
        }
    }
    let _ = ipc::publish_json(&format!("claude.v1.event.{session_id}.exited"), &payload);
}

/// Keep only the trailing 4 KiB of a drained log buffer. Bus payloads
/// are 1 MiB; truncating here keeps event size bounded without losing
/// the most-recent diagnostic context.
fn trailing(s: &str) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    const MAX: usize = 4 * 1024;
    if s.len() <= MAX {
        return Some(s.to_string());
    }
    // Slice on a char boundary by walking from the end.
    let mut idx = s.len().saturating_sub(MAX);
    while !s.is_char_boundary(idx) && idx < s.len() {
        idx += 1;
    }
    Some(s[idx..].to_string())
}

fn pending_restart_key(principal_id: &str) -> String {
    format!("{PENDING_RESTART_PREFIX}.{principal_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailing_returns_none_for_empty() {
        assert!(trailing("").is_none());
    }

    #[test]
    fn trailing_passes_short_unchanged() {
        let s = "short";
        assert_eq!(trailing(s).as_deref(), Some("short"));
    }

    #[test]
    fn trailing_truncates_to_cap() {
        let s = "a".repeat(5000);
        let t = trailing(&s).unwrap();
        assert!(t.len() <= 4 * 1024);
        assert_eq!(t.chars().next(), Some('a'));
    }

    #[test]
    fn pending_restart_key_format() {
        assert_eq!(pending_restart_key("p1"), "claude.pending_restart.p1");
    }

    #[test]
    fn respawn_inputs_remain_bound_after_an_interleaved_principal() {
        let alice = RespawnContext {
            principal_id: "alice".into(),
            home_path: "home://".into(),
            config: PrincipalConfig::default(),
            api_key: Some("alice-secret".into()),
        };
        let bob = RespawnContext {
            principal_id: "bob".into(),
            home_path: "home://".into(),
            config: PrincipalConfig::default(),
            api_key: Some("bob-secret".into()),
        };

        let alice_inputs = bound_spawn_inputs(&alice, "alice-session", "home://alice-id", "hook");
        let _bob_inputs = bound_spawn_inputs(&bob, "bob-session", "home://bob-id", "hook");

        assert_eq!(alice_inputs.principal_id, "alice");
        assert_eq!(alice_inputs.api_key, Some("alice-secret"));
        assert_eq!(alice_inputs.home_path, "home://");
    }

    #[test]
    fn identity_refresh_uses_the_stamped_principal_and_rejects_mismatch() {
        let omitted = SaveIdentityResult {
            success: true,
            principal_id: None,
        };
        assert_eq!(
            identity_refresh_principal(&omitted, Some("alice"))
                .expect("stamped principal")
                .as_str(),
            "alice"
        );

        let forged = SaveIdentityResult {
            success: true,
            principal_id: Some("bob".to_string()),
        };
        assert!(identity_refresh_principal(&forged, Some("alice")).is_err());
        assert!(identity_refresh_principal(&omitted, None).is_err());
    }
}
