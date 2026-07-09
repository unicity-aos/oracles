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
//! owned by the principal whose identity was just saved, then writes a
//! `claude.pending_restart.<principal_id>` KV marker carrying the
//! terminated session_ids. The supervisor's tick respawns them with a
//! freshly-fetched identity prompt on the next pass.

use astrid_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::config::{AuthMode, load_or_default as load_principal_config};
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
    let prepared = sessions.with(|map| -> Option<PreparedKill> {
        let session = map.remove(session_id)?;
        Some(PreparedKill {
            process: session.process.clone(),
            principal_id: session.record.principal_id.clone(),
        })
    })?;

    let final_exit = prepared.map(|p| {
        let mut summary = ExitSummary::default();
        // Drain the final tail FIRST: both `stop` and `release` discard the
        // buffered tail, and — unlike the ephemeral `Process` whose `Drop`
        // reaps — dropping a `PersistentProcess` never reaps, so the host
        // slot must be freed explicitly below (`release` if already exited,
        // `stop` otherwise).
        if let Ok(logs) = p.process.read_logs() {
            summary.exit_code = logs.exit.and_then(|e| e.exit_code);
            summary.signal = logs.exit.and_then(|e| e.signal);
            summary.stdout_tail = trailing(&logs.stdout);
            summary.stderr_tail = trailing(&logs.stderr);
        }
        if exited_clean {
            // Exited inside the grace window — just release the id (frees the
            // slot + drops the retained tail we already captured above).
            if let Err(e) = p.process.release() {
                summary.detail = Some(format!("release failed: {e:?}"));
            }
        } else {
            // Still running after the grace window: SIGTERM -> grace ->
            // SIGKILL and REMOVE the id. `stop` returns the real exit, which
            // supersedes whatever `read_logs` reported above.
            match p.process.stop(None) {
                Ok(exit) => {
                    summary.exit_code = exit.exit_code;
                    summary.signal = exit.signal;
                }
                Err(e) => {
                    summary.detail = Some(format!("stop failed: {e:?}"));
                }
            }
        }
        (summary, p.principal_id)
    });

    match final_exit {
        Some((summary, principal_id)) => {
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
            if let Err(e) = crate::hooks::forget_token(&principal_id, session_id) {
                log::warn(format!(
                    "claude-runner: hook-token cleanup for {session_id} failed: {e:?}"
                ));
            }
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
/// On `success=true` for a principal with live sessions: gracefully
/// terminate each, persist a `PendingRestart` marker so the next
/// supervisor tick respawns them with a freshly fetched identity.
pub(crate) fn handle_identity_refresh(
    sessions: &Sessions,
    msg: &ipc::Message,
) -> Result<(), SysError> {
    let result: SaveIdentityResult = match serde_json::from_str(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            log::warn(format!("claude-runner: save_identity payload parse failed: {e}"));
            return Ok(());
        }
    };
    if !result.success {
        return Ok(());
    }

    // Principal source preference: payload field > IPC envelope principal.
    let principal_id = result
        .principal_id
        .or_else(|| msg.principal.verified().map(str::to_string))
        .unwrap_or_default();
    if principal_id.is_empty() {
        log::warn("claude-runner: save_identity success without resolvable principal; ignoring");
        return Ok(());
    }

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

    if targets.is_empty() {
        return Ok(());
    }

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
    let key = pending_restart_key(&principal_id);
    let merged_sessions: Vec<PendingRestartSession> =
        match kv::get_json_opt::<PendingRestart>(&key)? {
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

    for t in targets {
        if let Err(e) = stop_session(sessions, &t.session_id, "identity_refresh") {
            log::warn(format!(
                "claude-runner: identity-refresh stop({}) failed: {e:?}",
                t.session_id
            ));
        }
    }
    Ok(())
}

/// Tick-driven respawn sweep. Reads every
/// `claude.pending_restart.<principal_id>` marker, attempts to rebuild
/// each listed session, and clears the marker on success.
///
/// If respawn fails for a given session the marker is left in place so
/// the next tick retries — but the session_id is removed from the
/// marker if it succeeded for one but not another, so we don't
/// double-spawn the survivors.
pub(crate) fn respawn_pending(sessions: &Sessions) -> Result<(), SysError> {
    let keys = kv::list_keys(&format!("{PENDING_RESTART_PREFIX}."))?;
    if keys.is_empty() {
        return Ok(());
    }

    for key in keys {
        let Some(marker): Option<PendingRestart> = kv::get_json_opt(&key)? else {
            continue;
        };

        let mut still_pending: Vec<PendingRestartSession> = Vec::new();
        for mut s in marker.sessions {
            match respawn_one(sessions, &marker.principal_id, &s) {
                Ok(()) => log::info(format!(
                    "claude-runner: respawned session {} for {} on identity refresh",
                    s.session_id, marker.principal_id
                )),
                Err(e) => {
                    s.attempts = s.attempts.saturating_add(1);
                    if s.attempts >= MAX_RESPAWN_ATTEMPTS {
                        log::warn(format!(
                            "claude-runner: respawn({}) for {} failed after {} attempts; giving up — {e:?}",
                            s.session_id, marker.principal_id, s.attempts
                        ));
                        let _ = ipc::publish_json(
                            "claude.v1.audit.respawn_abandoned",
                            &serde_json::json!({
                                "principal_id": marker.principal_id,
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
                            s.session_id, marker.principal_id, s.attempts
                        ));
                        still_pending.push(s);
                    }
                }
            }
        }

        if still_pending.is_empty() {
            // Clear the marker.
            if let Err(e) = kv::delete(&key) {
                log::warn(format!("claude-runner: clearing {key} failed: {e:?}"));
            }
        } else {
            let updated = PendingRestart {
                principal_id: marker.principal_id,
                sessions: still_pending,
            };
            if let Err(e) = kv::set_json(&key, &updated) {
                log::warn(format!("claude-runner: updating {key} failed: {e:?}"));
            }
        }
    }
    Ok(())
}

fn respawn_one(
    sessions: &Sessions,
    principal_id: &str,
    s: &PendingRestartSession,
) -> Result<(), SysError> {
    // Resolve the principal's home from the prior identity_path:
    // ".../.claude/.claude-identity-<sid>" -> "..." (the home dir).
    let home_scheme = home_from_identity_path(&s.identity_path).ok_or_else(|| {
        SysError::ApiError(format!(
            "respawn({}): can't derive home from identity_path {}",
            s.session_id, s.identity_path
        ))
    })?;

    // Identity fetch + write go through the VFS scheme so the kernel
    // resolves them per-invocation-principal. The subprocess `HOME` /
    // cwd, by contrast, must be a real OS path — `claude` does not
    // interpret `home://` and would fall back to ambient `$HOME`,
    // silently breaking per-principal isolation. Canonicalise here
    // (mirrors claude-install's resolution on cold spawn) so the
    // refreshed session is wired up the same way handle_spawn wires a
    // fresh one. If canonicalisation isn't supported by the host (or
    // the kernel rejects the scheme), fall back to the scheme string;
    // the worst case is the original silent-isolation-break, which is
    // no regression from current behaviour.
    let resolved_home = fs::canonicalize(&home_scheme).unwrap_or_else(|_| home_scheme.clone());

    // Auth mode branch — mirrors `lib.rs::handle_spawn` so respawn
    // honours the per-principal AuthMode and the two-axis matrix
    // (Headless|Repl × ApiKey|Subscription) is consistent across the
    // cold-spawn and identity-refresh pipelines. In ApiKey mode the
    // kernel-elicited `api_key` secret is required; an empty value
    // hard-errors (the prior single-mode behaviour). In Subscription
    // mode we skip the env::var read entirely so the cleartext never
    // lands in this stack frame and `spawn::spawn_claude` omits the
    // `.env("ANTHROPIC_API_KEY")` call — Claude falls back to its
    // keychain OAuth path written by `claude /login`.
    //
    // NOTE: `respawn_pending` runs in the runner's supervisor loop under
    // the runner's capsule principal — same context as `handle_spawn`'s
    // interceptor — so `load_or_default()` reads the same canonical
    // `claude.principal.config` record that the cold-spawn pipeline
    // reads. The `principal_id` argument here is threaded through to
    // the spawned subprocess inputs / audit fields, not the KV lookup.
    let cfg = load_principal_config();
    let api_key: Option<String> = match cfg.auth_mode {
        AuthMode::ApiKey => {
            let key = env::var("api_key").unwrap_or_default();
            if key.is_empty() {
                return Err(SysError::ApiError(format!(
                    "respawn({}): no api_key configured for principal {principal_id}",
                    s.session_id
                )));
            }
            Some(key)
        }
        AuthMode::Subscription => None,
    };

    // Fetch fresh identity prompt from spark + materialize a new
    // append-system-prompt file under <home>/.claude/. The identity
    // crate writes through the `home://` VFS scheme regardless of
    // what we pass for `home_path`, so the scheme string is fine
    // here — only `spawn_claude` needs the resolved OS path.
    let prompt = identity::fetch_prompt(principal_id, &s.session_id, &home_scheme)?;
    let identity_path = identity::write_prompt_file(&home_scheme, &s.session_id, &prompt)?;

    // Mint a fresh per-(principal, session) hook token for the
    // respawned subprocess. The previous incarnation's token was deleted
    // by `stop_session` during identity-refresh teardown; without a new
    // token persisted to KV, `astrid-emit` invocations from the
    // respawned `claude -p` child would fail the runner's validator lookup and
    // get dropped as forgeries. Mirrors the mint+persist pattern in
    // `handle_spawn` for cold spawns.
    let hook_token = crate::hooks::mint_token()?;
    crate::hooks::persist_token(principal_id, &s.session_id, &hook_token)?;

    // `SpawnInputs::api_key` is `Option<&str>`; the subscription path
    // threads `None` so `spawn::spawn_claude` omits the env export.
    let spawned = spawn::spawn_claude(&SpawnInputs {
        principal_id,
        session_id: &s.session_id,
        home_path: &resolved_home,
        identity_path: &identity_path,
        api_key: api_key.as_deref(),
        hook_token: &hook_token,
        model: cfg.model,
        max_turns: cfg.max_turns,
    })?;

    let now_ms = match time::now() {
        Ok(t) => t
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0),
        Err(_) => 0,
    };

    let record = SessionRecord {
        principal_id: principal_id.to_string(),
        session_id: s.session_id.clone(),
        identity_path,
        started_at_ms: now_ms,
        os_pid: spawned.os_pid,
        process_id: spawned.process_id,
    };
    state::save_record(&record)?;

    sessions.with(|map| {
        map.insert(
            s.session_id.clone(),
            RuntimeSession {
                record,
                process: spawned.process,
                codec: crate::codec::LineDecoder::default(),
            },
        );
    })?;

    // Audit parity with handle_spawn: emit `claude.v1.audit.spawn` so
    // downstream consumers don't need a separate path for refreshed
    // sessions vs. fresh ones. Include `auth_mode` in its canonical
    // snake_case wire form so respawn audit lines carry the same
    // attribution tuple as cold-spawn entries — without this a
    // subscription-mode respawn would be indistinguishable from an
    // api_key one in the audit stream. `interaction_mode` is omitted
    // here: respawn is only reachable from sessions sage spawned itself
    // (i.e. headless), so the mode is implicit.
    // `claude.v1.event.<sid>.respawned` is the session-scoped event for
    // UI / metrics.
    let auth_mode_str = match cfg.auth_mode {
        AuthMode::ApiKey => "api_key",
        AuthMode::Subscription => "subscription",
    };
    let _ = ipc::publish_json(
        "claude.v1.audit.spawn",
        &serde_json::json!({
            "principal_id": principal_id,
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
            "principal_id": principal_id,
            "reason": "identity_refresh",
            "flags_hash": spawned.flags_hash,
        }),
    );
    Ok(())
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
/// lock. Carries the cloned [`PersistentProcess`](process::PersistentProcess)
/// handle so the host `read_logs` / `stop` / `release` calls can run with
/// the sessions mutex released — same lock-discipline shape as the
/// supervisor's `read_logs` drain in [`crate::supervisor`].
struct PreparedKill {
    process: process::PersistentProcess,
    /// Snapshotted under the lock in phase 3a so phase 3b can build the
    /// `claude.hook_token.<principal>.<session>` KV key without re-locking
    /// the sessions map. Required for hook-token cleanup on session end
    /// (the per-(principal, session) token minted at spawn time).
    principal_id: String,
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

fn home_from_identity_path(identity_path: &str) -> Option<String> {
    // Convention: identity paths live at "home://.claude/.claude-identity-<sid>".
    // The home root is the `home://` VFS scheme — the kernel binds it
    // to the invoking principal's home (`~/.astrid/home/<principal>/`,
    // see core/crates/astrid-kernel/src/lib.rs:75). Validate the shape
    // before returning so a stray legacy record from a pre-home://
    // capsule incarnation surfaces as `None` instead of silently
    // producing an unresolvable path.
    let trimmed = identity_path.trim_end_matches('/');
    let no_id = trimmed.rsplit_once('/')?.0;
    if !no_id.ends_with(".claude") {
        return None;
    }
    Some("home://".to_string())
}

fn pending_restart_key(principal_id: &str) -> String {
    format!("{PENDING_RESTART_PREFIX}.{principal_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_path_derivation_round_trip() {
        let h = home_from_identity_path("home://.claude/.claude-identity-abc").unwrap();
        assert_eq!(h, "home://");
    }

    #[test]
    fn home_path_short_returns_none() {
        assert!(home_from_identity_path("nope").is_none());
    }

    #[test]
    fn home_path_rejects_legacy_tilde_record() {
        // A SessionRecord left behind by a pre-home:// capsule version
        // must NOT be silently respawned against an unresolvable path —
        // the literal-tilde scheme falls through to workspace root in
        // the kernel, which is exactly the failure mode this fix
        // closes. Surfacing `None` forces the respawn sweep to abandon
        // the record instead.
        assert!(
            home_from_identity_path(
                "~/.astrid/principals/p1/something_other_than_dot_claude/.claude-identity-abc",
            )
            .is_none()
        );
    }

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
}
