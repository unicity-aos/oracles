//! Per-session state: persisted KV records and live in-memory runtime.
//!
//! Two layers of state coexist:
//!
//! - [`SessionRecord`] — JSON, persisted to KV under `claude.session.<sid>`
//!   keyed by session_id. Survives capsule unload; the records are the
//!   recovery list scanned on supervisor start.
//! - [`RuntimeSession`] — live in-memory state: the
//!   [`process::PersistentProcess`] resource handle (a kernel resource
//!   NOT serializable to KV) plus the stream-json line decoder buffer.
//!   Held in a [`Mutex<HashMap>`] inside the capsule singleton; dies on
//!   capsule reload.
//!
//! On reload the persisted records survive but their in-memory
//! [`PersistentProcess`](process::PersistentProcess) handles do not.
//! Because the `claude -p` children run on the persistent tier they
//! usually OUTLIVE the reload, so the supervisor's first-tick reconcile
//! re-[`attach`](astrid_sdk::process::attach)es by
//! [`SessionRecord::process_id`] and resumes driving each survivor;
//! genuinely-dead children reconcile to a real `exited` (or `lost`) and
//! their records are cleared. See [`crate::supervisor`].

use crate::codec;
use astrid_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// KV key prefix for Claude runner session records.
pub(crate) const SESSION_KEY_PREFIX: &str = "claude.session";

/// Per-principal cap on concurrently active sessions.
///
/// Set to 4 — well under the 8-process-per-capsule host ceiling so
/// `claude-install` scratch spawns still have headroom even at full
/// session occupancy.
pub(crate) const MAX_SESSIONS_PER_PRINCIPAL: usize = 4;

/// KV key for a session's persisted record.
pub(crate) fn session_key(session_id: &str) -> String {
    format!("{SESSION_KEY_PREFIX}.{session_id}")
}

// NOTE: There is intentionally no runner-side install-complete key here.
// Earlier revisions defined `install_complete_key` and had
// `ensure_install` short-circuit on it, but the kernel scopes KV by
// `{principal}:capsule:{capsule_id}` — runner and claude-install are
// distinct capsule_ids with disjoint KV namespaces. The marker lives in
// claude-install's namespace exclusively; the runner gates on the published
// `claude.v1.install.complete` event whose `already_installed: true`
// flag is the cache-hit signal. See `lib.rs::ensure_install`.

/// Persisted session bookkeeping. Restored from KV on capsule load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRecord {
    pub principal_id: String,
    pub session_id: String,
    pub identity_path: String,
    pub started_at_ms: u64,
    /// Stamped at spawn for audit correlation; not load-bearing for
    /// behaviour, just observability.
    pub os_pid: u32,
    /// Host-owned persistent process id. The `claude -p` child is spawned
    /// on the persistent tier (`spawn_persistent`), so it survives a runner
    /// capsule reload; this id is how the supervisor's reload reconcile
    /// re-[`attach`](astrid_sdk::process::attach)es to keep driving the
    /// SAME process instead of abandoning the session. Empty only on
    /// records written by a pre-durable capsule incarnation — those are
    /// reconciled as un-attachable orphans (a real exit, not a resume).
    #[serde(default)]
    pub process_id: String,
}

/// Live, non-serializable session state held in capsule memory.
///
/// `process` is a [`process::PersistentProcess`] — an opaque, cheap,
/// `Clone` handle keyed by [`ProcessId`](astrid_sdk::process::ProcessId)
/// over a HOST-owned child that survives this capsule instance being
/// reset. The handle itself dies on reload (it lives in capsule memory),
/// but the underlying child does NOT: the reload reconcile re-derives an
/// equivalent handle via [`attach`](astrid_sdk::process::attach) from the
/// [`SessionRecord::process_id`] persisted in KV. Dropping the handle is a
/// no-op on the child (unlike the ephemeral `Process`, whose `Drop`
/// reaps) — reaping is explicit via `stop` / `release` / host TTLs.
///
/// The handle is cloned out of the [`Sessions`] mutex critical section
/// before issuing host calls (`write_stdin`, `read_logs`, …) so the
/// mutex is never held across a host call — that would serialise the
/// whole supervisor loop and risk deadlock if the call re-enters the bus
/// drain. `PersistentProcess: Clone` is the supported escape (no `Arc`
/// needed; the handle is just an id wrapper).
pub(crate) struct RuntimeSession {
    pub record: SessionRecord,
    pub process: process::PersistentProcess,
    pub codec: codec::LineDecoder,
}

/// Singleton in-memory registry of live sessions. Behind a `Mutex` for
/// interior mutability since handler entry points take `&self` (the
/// `#[capsule]` macro requires non-mutable receivers when no method
/// uses `&mut self`).
#[derive(Default)]
pub(crate) struct Sessions {
    inner: Mutex<HashMap<String, RuntimeSession>>,
    /// One-shot flag: the supervisor's first tick after capsule load
    /// scans KV for orphaned records and emits synthetic
    /// `capsule_reload` exit events. We don't want to repeat that scan
    /// on every tick.
    reload_recovered: Mutex<bool>,
}

impl Sessions {
    /// Borrow the live session map for direct mutation.
    pub(crate) fn with<R>(
        &self,
        f: impl FnOnce(&mut HashMap<String, RuntimeSession>) -> R,
    ) -> Result<R, SysError> {
        let mut guard = self.inner.lock().map_err(|_| {
            SysError::ApiError("claude-runner session registry lock poisoned".into())
        })?;
        Ok(f(&mut guard))
    }

    /// Count active sessions for a principal — enforces the per-principal cap.
    pub(crate) fn count_for_principal(&self, principal_id: &str) -> Result<usize, SysError> {
        self.with(|map| {
            map.values()
                .filter(|s| s.record.principal_id == principal_id)
                .count()
        })
    }

    /// True the first time it is called after capsule load; false thereafter.
    /// Used to gate the orphan-record recovery scan to a single execution.
    pub(crate) fn take_reload_recovered_flag(&self) -> Result<bool, SysError> {
        let mut guard = self.reload_recovered.lock().map_err(|_| {
            SysError::ApiError("claude-runner reload-recovery flag lock poisoned".into())
        })?;
        if *guard {
            Ok(false)
        } else {
            *guard = true;
            Ok(true)
        }
    }

    /// Request another persisted-record recovery sweep on the next tick.
    ///
    /// Used when a post-spawn bookkeeping failure could not stop the
    /// persistent child. Keeping the record and re-arming recovery is the only
    /// safe alternative to deleting the last handle to a still-live process.
    pub(crate) fn request_reload_recovery(&self) -> Result<(), SysError> {
        let mut guard = self.reload_recovered.lock().map_err(|_| {
            SysError::ApiError("claude-runner reload-recovery flag lock poisoned".into())
        })?;
        *guard = false;
        Ok(())
    }
}

/// Persist a session record to KV.
pub(crate) fn save_record(rec: &SessionRecord) -> Result<(), SysError> {
    kv::set_json(&session_key(&rec.session_id), rec)
}

/// Delete a session's KV record.
pub(crate) fn delete_record(session_id: &str) -> Result<(), SysError> {
    kv::delete(&session_key(session_id))
}

/// Enumerate every persisted session record by scanning the KV prefix.
pub(crate) fn list_all_records() -> Result<Vec<SessionRecord>, SysError> {
    let keys = kv::list_keys(&format!("{SESSION_KEY_PREFIX}."))?;
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(rec) = kv::get_json_opt::<SessionRecord>(&key)? {
            out.push(rec);
        }
    }
    Ok(out)
}
