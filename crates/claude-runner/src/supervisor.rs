//! Cooperative supervisor for live `claude -p` subprocesses.
//!
//! The supervisor is the body of `#[astrid::run]`. Every ~50 ms it:
//!
//! 1. Drains stdout from every active session, feeding bytes into the
//!    session's [`codec::LineDecoder`].
//! 2. Dispatches every decoded envelope to the matching
//!    `claude.v1.event.<sid>.*` IPC topic.
//! 3. Detects process crash / buffer overflow / capsule reload and
//!    publishes synthetic `exited` events so consumers don't hang.
//!
//! Tool execution is NOT the runner's job. Claude calls the registered `astrid
//! mcp serve` MCP server directly over the MCP protocol, so tool calls
//! never round-trip through the supervisor. The supervisor only relays
//! the conversation stream (init / text / done / partial) onto the
//! `claude.v1.event.<sid>.*` topics; `tool_use` blocks and control requests
//! are observe-only.

use crate::codec::{AssistantBlock, CodecError, Decoded};
use crate::state::{RuntimeSession, SessionRecord, Sessions, delete_record, list_all_records};
use astrid_sdk::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// Supervisor tick cadence. Conservative enough to keep idle-capsule
/// CPU near zero, fast enough that interactive token streams from the
/// model feel live.
pub(crate) const TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Run one supervisor tick across every active session. Returns
/// `Ok(())` even if individual sessions hit errors — errors are
/// surfaced as IPC events, never bubbled up to abort the tick.
pub(crate) fn tick(sessions: &Sessions) -> Result<(), SysError> {
    // First-tick reconcile: persisted records with no live handle are
    // re-attached by process_id — survivors resume, dead children exit.
    if sessions.take_reload_recovered_flag()? {
        reload_reconcile(sessions)?;
    }

    // Collect session ids without holding the lock across the per-
    // session work (some of which publishes IPC, which can in turn
    // poll our subscriptions).
    let session_ids: Vec<String> = sessions.with(|map| map.keys().cloned().collect())?;

    for sid in session_ids {
        drive_session(sessions, &sid)?;
    }

    Ok(())
}

/// Pull new stdout from one session and dispatch its events.
///
/// INVARIANT: NO host calls under the Sessions lock. The drain runs in
/// three phases around the single blocking host call:
///
/// 1. Phase 1a (under lock): clone the `PersistentProcess` handle.
/// 2. Phase 2 (lock released): call `process.read_logs()` — a host
///    call that may block and could re-enter the bus drain.
/// 3. Phase 1b (re-lock): feed the bytes into the session's codec and
///    decode any completed lines into conversation events.
///
/// Holding the sessions mutex across the host call would serialise the
/// whole supervisor loop and risks deadlock; cloning the
/// `PersistentProcess` is cheap (it is just an id wrapper) and `read_logs`
/// only needs `&self`.
fn drive_session(sessions: &Sessions, session_id: &str) -> Result<(), SysError> {
    // Phase 1a: clone what we need out from under the lock.
    let prep = sessions.with(|map| -> Option<DrivePrep> {
        let session = map.get(session_id)?;
        Some(DrivePrep {
            process: session.process.clone(),
        })
    })?;

    let Some(prep) = prep else { return Ok(()) };

    // Phase 2: host call with NO sessions lock held.
    let logs = match prep.process.read_logs() {
        Ok(l) => l,
        Err(e) => {
            log::warn(format!(
                "claude-runner: read_logs failed for {session_id}: {e}"
            ));
            return Ok(());
        }
    };

    // Phase 1b: re-take the lock to feed the codec and update in-flight
    // tool-call bookkeeping. The session may have been evicted in
    // between the two lock acquisitions (e.g. shutdown on another
    // path); treat that as Skip.
    let outcome = sessions.with(|map| -> Option<DriveOutcome> {
        let session = map.get_mut(session_id)?;

        let mut events: Vec<PendingEvent> = Vec::new();
        let mut buffer_overflow = false;
        if !logs.stdout.is_empty() {
            let decoded = session
                .codec
                .feed(logs.stdout.as_bytes())
                .collect::<Vec<_>>();
            for item in decoded {
                match item {
                    Ok(d) => collect_events(session_id, d, &mut events),
                    Err(CodecError::LineTooLong) => {
                        buffer_overflow = true;
                    }
                    Err(CodecError::Malformed(msg)) => {
                        log::warn(format!(
                            "claude-runner: malformed stream-json on {session_id}: {msg}"
                        ));
                    }
                }
            }
        }

        if buffer_overflow {
            return Some(DriveOutcome::BufferOverflow);
        }

        let exited = if logs.running { None } else { Some(logs.exit) };

        Some(DriveOutcome::Drained { events, exited })
    })?;

    match outcome {
        None => Ok(()),
        Some(DriveOutcome::BufferOverflow) => {
            publish_exit(session_id, "buffer_overflow", None, None);
            evict(sessions, session_id)?;
            Ok(())
        }
        Some(DriveOutcome::Drained { events, exited }) => {
            for ev in events {
                ev.publish();
            }
            if let Some(exit_info) = exited {
                let (code, sig) = match exit_info {
                    Some(info) => (info.exit_code, info.signal),
                    None => (None, None),
                };
                publish_exit(session_id, "exited", code, sig);
                evict(sessions, session_id)?;
            }
            Ok(())
        }
    }
}

enum DriveOutcome {
    Drained {
        events: Vec<PendingEvent>,
        exited: Option<Option<process::ExitInfo>>,
    },
    BufferOverflow,
}

/// Hand-off package collected under `Sessions::with` and consumed
/// outside the lock by [`drive_session`]. Carries the cloned
/// [`PersistentProcess`](process::PersistentProcess) handle so the host
/// `read_logs` runs with the sessions mutex released.
struct DrivePrep {
    process: process::PersistentProcess,
}

/// IPC events queued while the session lock is held; published after
/// we release the lock so subscribers' handlers can re-enter freely.
struct PendingEvent {
    topic: String,
    payload: Value,
}

impl PendingEvent {
    fn publish(self) {
        if let Err(e) = ipc::publish_json(&self.topic, &self.payload) {
            log::warn(format!("publish {} failed: {e}", self.topic));
        }
    }
}

/// Translate one [`Decoded`] envelope into the corresponding
/// `claude.v1.event.<sid>.*` conversation payloads. Tool calls and approvals
/// no longer flow through here — claude drives those against the registered
/// MCP server — so this only relays init / text / done / partial events.
fn collect_events(session_id: &str, decoded: Decoded, out: &mut Vec<PendingEvent>) {
    match decoded {
        Decoded::SystemInit { model, .. } => {
            out.push(PendingEvent {
                topic: format!("claude.v1.event.{session_id}.init"),
                payload: serde_json::json!({ "model": model }),
            });
        }
        Decoded::Assistant { content_blocks } => {
            // Tool execution is owned by the registered `aos mcp serve`
            // MCP server — claude calls it directly over MCP, so the runner never
            // dispatches `mcp__aos__*` tool calls itself (doing so would
            // double-execute). Here the runner only relays the assistant's text;
            // tool_use blocks are observe-only and dropped at decode.
            for block in content_blocks {
                let AssistantBlock::Text { text } = block;
                out.push(PendingEvent {
                    topic: format!("claude.v1.event.{session_id}.text"),
                    payload: serde_json::json!({ "delta": text }),
                });
            }
        }
        Decoded::Result {
            subtype,
            is_error,
            usage,
            total_cost_usd,
            permission_denials,
        } => {
            out.push(PendingEvent {
                topic: format!("claude.v1.event.{session_id}.done"),
                payload: serde_json::json!({
                    "subtype": subtype,
                    "is_error": is_error,
                    "usage": usage,
                    "total_cost_usd": total_cost_usd,
                    "permission_denials": permission_denials,
                }),
            });
        }
        Decoded::StreamEvent { event } => {
            // Token-level events; consumer opts in.
            out.push(PendingEvent {
                topic: format!("claude.v1.event.{session_id}.partial"),
                payload: serde_json::json!({ "event": event }),
            });
        }
        Decoded::ControlRequest { .. } | Decoded::UserToolResultEcho { .. } | Decoded::Ping => {
            // No-op. Control requests (permission gating, MCP transport)
            // are handled by claude itself against the registered MCP
            // server, never by the runner; tool_result echoes are
            // observability-only; ping is keepalive.
        }
        Decoded::Unknown(value) => {
            log::warn(format!(
                "claude-runner: unknown stream-json envelope on {session_id}: {}",
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("<no type>")
            ));
        }
    }
}

fn publish_exit(session_id: &str, reason: &str, exit_code: Option<i32>, signal: Option<i32>) {
    let _ = ipc::publish_json(
        &format!("claude.v1.event.{session_id}.exited"),
        &serde_json::json!({
            "reason": reason,
            "exit_code": exit_code,
            "signal": signal,
        }),
    );
}

/// Drop a runtime session AND its persisted record.
fn evict(sessions: &Sessions, session_id: &str) -> Result<(), SysError> {
    // Snapshot the principal_id BEFORE removing the runtime entry so we
    // can build the `claude.hook_token.<principal>.<session>` KV key for
    // cleanup. After `map.remove` returns, the only place the
    // principal binding lives is the persisted SessionRecord — and
    // `delete_record` may run before we'd otherwise have a chance to
    // look it up, so capture it here in one critical section.
    let principal_id = sessions.with(|map| -> Option<String> {
        let session = map.remove(session_id)?;
        Some(session.record.principal_id)
    })?;
    let _ = delete_record(session_id);
    // Drop the per-(principal, session) hook token alongside the
    // persisted record so a forged `claude.v1.hook.*` event
    // arriving after eviction cannot pass the validator lookup. Best-
    // effort: a missing principal (session already gone) is a no-op.
    if let Some(principal_id) = principal_id {
        let _ = crate::hooks::forget_token(&principal_id, session_id);
    }
    Ok(())
}

/// First-tick reconcile: every persisted record with no live runtime
/// session is an orphan from the previous capsule incarnation. Because the
/// `claude -p` child runs on the PERSISTENT tier it usually OUTLIVED the
/// reload — only the in-memory handle died — so we re-[`attach`] by the
/// recorded [`process_id`](SessionRecord::process_id) and reconcile:
///
/// * still running → REBUILD the runtime session and keep driving the same
///   child (the conversation is unbroken). Emit `…​.resumed`; keep the
///   record + hook token (same child, same `ASTRID_HOOK_TOKEN`).
/// * already exited (within `exit_retention`) → publish the REAL exit,
///   `release` the id, drop the record + token.
/// * reaped / unknown / pre-durable record (no `process_id`) → unattachable;
///   publish a synthetic exit and drop the record + token.
fn reload_reconcile(sessions: &Sessions) -> Result<(), SysError> {
    let live_ids: HashMap<String, ()> =
        sessions.with(|map| map.keys().cloned().map(|k| (k, ())).collect())?;
    let records: Vec<SessionRecord> = match list_all_records() {
        Ok(r) => r,
        Err(e) => {
            log::warn(format!("claude-runner: reload-reconcile list failed: {e}"));
            return Ok(());
        }
    };
    for rec in records {
        if live_ids.contains_key(&rec.session_id) {
            continue;
        }
        // Defense in depth: refuse to format!() an unvalidated id into an
        // IPC topic, even from our own KV store. A stale record from before
        // validation was deployed (or a future bug that writes a tainted id)
        // must not publish on an attacker-chosen topic. Drop it either way.
        if crate::validate_id("session_id", &rec.session_id).is_err() {
            log::warn("claude-runner: reload-reconcile dropping record with invalid session_id");
            let _ = delete_record(&rec.session_id);
            // The matching hook token is keyed by the same id; sweep it.
            let _ = crate::hooks::forget_token(&rec.principal_id, &rec.session_id);
            continue;
        }

        // Probe the recorded id (a pre-durable record has none), then let
        // the pure decision table choose the action — see
        // [`reconcile_decision`]. Attaching is a cheap id-wrapper; `status`
        // is the only host round-trip.
        let proc = if rec.process_id.is_empty() {
            None
        } else {
            Some(process::attach(rec.process_id.clone()))
        };
        let probe = match &proc {
            Some(p) => match p.status() {
                Ok(info) => Probe::Known(info.exit.map(|e| (e.exit_code, e.signal))),
                Err(_) => Probe::Unresolved,
            },
            None => Probe::NoId,
        };

        match reconcile_decision(probe) {
            // The child outlived the reload — keep driving it.
            Reconcile::Resume => {
                if let Some(p) = proc {
                    resume_session(sessions, rec, p)?;
                }
            }
            // Exited during the reload gap (still retained) → truthful exit,
            // then free the host slot (a `PersistentProcess` never reaps on
            // drop).
            Reconcile::Exited { code, signal } => {
                if let Some(p) = proc
                    && let Err(e) = p.release()
                {
                    log::warn(format!(
                        "claude-runner: reconcile release({}) failed: {e:?}",
                        rec.session_id
                    ));
                }
                abandon_orphan(&rec, "exited", code, signal);
            }
            // Unattachable (reaped TTL / unknown / pre-durable) — synthetic
            // exit, nothing to release.
            Reconcile::Abandon { reason } => abandon_orphan(&rec, reason, None, None),
        }
    }
    Ok(())
}

/// Outcome of probing an orphan record's recorded persistent id.
enum Probe {
    /// No id on the record (a pre-durable incarnation) — nothing to attach.
    NoId,
    /// The host could not resolve the id — reaped by a TTL, or unknown.
    Unresolved,
    /// The id resolved; the inner value is the recorded exit `(code, signal)`
    /// — `None` while the child is still running.
    Known(Option<(Option<i32>, Option<i32>)>),
}

/// Reconcile verdict for one orphan. Factored out of the host calls in
/// [`reload_reconcile`] so the decision table is unit-testable without a
/// live persistent process.
#[derive(Debug, PartialEq, Eq)]
enum Reconcile {
    /// Child is alive — rebuild the runtime session and resume.
    Resume,
    /// Child has terminated — publish this real exit and free the slot.
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
    },
    /// Nothing to resume — publish a synthetic exit under `reason`.
    Abandon { reason: &'static str },
}

/// Pure decision table for [`reload_reconcile`]. A resolved id with no
/// recorded exit is a live child → [`Resume`](Reconcile::Resume); a resolved
/// id WITH an exit is reconciled to that real exit; an unresolved id is
/// `lost`; a record carrying no id is a pre-durable orphan reported as the
/// legacy `capsule_reload`.
fn reconcile_decision(probe: Probe) -> Reconcile {
    match probe {
        Probe::Known(None) => Reconcile::Resume,
        Probe::Known(Some((code, signal))) => Reconcile::Exited { code, signal },
        Probe::Unresolved => Reconcile::Abandon { reason: "lost" },
        Probe::NoId => Reconcile::Abandon {
            reason: "capsule_reload",
        },
    }
}

/// Rebuild the in-memory [`RuntimeSession`] for a child that survived the
/// reload, re-attaching by id. The record + hook token are kept untouched:
/// it is the SAME running child under the same `ASTRID_HOOK_TOKEN`, so its
/// in-flight `astrid-emit` hooks keep validating. The codec starts empty —
/// the supervisor picks up cleanly from the next decoded line. KNOWN
/// LIMITATION: a stream-json line straddling the reload boundary can be lost
/// under the draining `read_logs` path; moving to cursor-addressed
/// `read_since` closes that gap (follow-up). Tool calls survive the reload on
/// their own — claude owns them against the registered MCP server, off this
/// stream entirely.
fn resume_session(
    sessions: &Sessions,
    rec: SessionRecord,
    process: process::PersistentProcess,
) -> Result<(), SysError> {
    let session_id = rec.session_id.clone();
    let principal_id = rec.principal_id.clone();
    let process_id = rec.process_id.clone();
    sessions.with(|map| {
        map.insert(
            session_id.clone(),
            RuntimeSession {
                record: rec,
                process,
                codec: crate::codec::LineDecoder::default(),
            },
        );
    })?;
    let _ = ipc::publish_json(
        &format!("claude.v1.event.{session_id}.resumed"),
        &serde_json::json!({
            "principal_id": principal_id,
            "process_id": process_id,
            "reason": "capsule_reload",
        }),
    );
    log::info(format!(
        "claude-runner: resumed session {session_id} after capsule reload (re-attached {process_id})"
    ));
    Ok(())
}

/// Publish a terminal `exited` event for an orphan we cannot keep, then drop
/// its persisted record and hook token. Sweeping the token closes the window
/// where a forged `claude.v1.hook.*` arriving after the session is gone could
/// still pass the validator lookup.
fn abandon_orphan(rec: &SessionRecord, reason: &str, code: Option<i32>, sig: Option<i32>) {
    publish_exit(&rec.session_id, reason, code, sig);
    let _ = delete_record(&rec.session_id);
    let _ = crate::hooks::forget_token(&rec.principal_id, &rec.session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_resumes_a_running_child() {
        // Resolved id, no recorded exit → the child outlived the reload.
        assert_eq!(reconcile_decision(Probe::Known(None)), Reconcile::Resume);
    }

    #[test]
    fn reconcile_reports_the_real_exit() {
        // Resolved id WITH an exit → publish that exit, not a synthetic one.
        assert_eq!(
            reconcile_decision(Probe::Known(Some((Some(137), Some(9))))),
            Reconcile::Exited {
                code: Some(137),
                signal: Some(9),
            }
        );
        // A clean exit carries through too (code 0, no signal).
        assert_eq!(
            reconcile_decision(Probe::Known(Some((Some(0), None)))),
            Reconcile::Exited {
                code: Some(0),
                signal: None,
            }
        );
    }

    #[test]
    fn reconcile_marks_an_unresolved_id_lost() {
        // Host could not resolve the id (reaped by a TTL during the gap).
        assert_eq!(
            reconcile_decision(Probe::Unresolved),
            Reconcile::Abandon { reason: "lost" }
        );
    }

    #[test]
    fn reconcile_treats_a_pre_durable_record_as_capsule_reload() {
        // No id to attach to → keep the legacy reason so existing consumers
        // of `capsule_reload` still see it for un-upgradable records.
        assert_eq!(
            reconcile_decision(Probe::NoId),
            Reconcile::Abandon {
                reason: "capsule_reload",
            }
        );
    }
}
