#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

//! Claude runner — Claude headless agent runner on Unicity AOS.
//!
//! Supervises one `claude -p --input-format stream-json --output-format
//! stream-json` subprocess per principal session. Streams the user's
//! turns in, parses Claude's stream-json events out, and relays the
//! conversation onto `claude.v1.event.<sid>.*`. The subprocess is long-
//! lived so Anthropic-side prompt caching stays warm turn-to-turn.
//!
//! Tool execution is NOT the runner's job. Claude is configured with the
//! registered `aos mcp serve` MCP server (`--mcp-config`), so it
//! invokes `mcp__aos__*` tools directly against that server over the
//! MCP protocol. The runner never sees, dispatches, or writes back tool calls
//! — doing so on top of the registered server would double-execute every
//! tool. The supervisor relays only conversation text / lifecycle events.
//!
//! Bills against the user's Anthropic Agent SDK credit (per Anthropic's
//! June 15, 2026 billing model). For per-turn API completion mode that
//! bypasses the SDK credit, see the sibling crate `claude-completion`.
//!
//! # Lifecycle paths
//!
//! * `handle_spawn` — provision a `claude -p` subprocess, fetch identity
//!   from spark with fallback, write the system-prompt file, spawn with
//!   the hardened argv set, persist the `state::SessionRecord`.
//! * `handle_send` — encode a user-turn stream-json envelope and write
//!   it to the session's stdin in one call.
//! * `#[astrid::run]` — supervisor tick at `supervisor::TICK_INTERVAL`
//!   cadence driving `supervisor::tick`: drain stdout, decode stream-
//!   json, publish `claude.v1.event.<sid>.*` for init / text / result /
//!   partial, and detect crash / buffer-overflow / capsule-reload.

use astrid_sdk::prelude::*;
use oracle_host::ids::{resolve_principal, stamped_principal};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

// `codec` — stream-json wire encode/decode. Outbound is just the user
// turn now; tool-result / control-response write-back is gone because
// claude drives tools against the registered MCP server, off this stream.
mod codec;
// `config` is purely additive plumbing in this slice — the
// `PrincipalConfig` type and KV helpers (`load_or_default`, `save`)
// exist so the install hook (`#[astrid::install]`) and the
// `handle_settings_set` IPC interceptor in upcoming slices can wire to
// a single canonical record. No call site exercises it yet, so allow
// dead_code at the module boundary to keep the slice buildable without
// loosening warnings crate-wide.
#[allow(dead_code)]
mod config;
// `hooks` — hook-event-bridge primitives (token mint / persist /
// lookup / forget / compare + canonical topic map). Surface-only in
// this slice; spawn-env wiring, run-loop subscriber, and shutdown
// cleanup land in follow-on slices. The module itself carries an
// `#![allow(dead_code)]` until those wirings appear.
pub(crate) mod hooks;
mod identity;
// `#[astrid::install]` body — split out of `lib.rs` to keep that file
// under the 1000-line CI gate.
mod install;
// `handle_settings_set` business logic — split out of `lib.rs` to keep
// that file under the 1000-line CI gate. The interceptor wiring lands
// in a later slice; until then the module's items are dead at the
// crate boundary.
#[allow(dead_code)]
mod settings;
mod shutdown;
mod spawn;
mod state;
mod supervisor;

use codec::{Outbound, encode};
// The `handle_spawn` interceptor consumes `AuthMode`,
// `InteractionMode`, and `load_status` (re-exported under the legacy
// `load_principal_config_status` alias) to branch the spawn pipeline on
// the per-principal config — including the schema_version migration /
// rejection split. `PrincipalConfig` and `save` remain plumbing for the
// install hook and `handle_settings_set` interceptor.
use config::{AuthMode, InteractionMode, LoadOutcome, load_status as load_principal_config_status};
use state::{MAX_SESSIONS_PER_PRINCIPAL, RuntimeSession, SessionRecord, Sessions, save_record};

/// Graceful-shutdown grace before falling back to SIGKILL.
/// [`shutdown::stop_session`] reads this.
pub(crate) const GRACEFUL_SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// KV prefix for the "respawn this principal's sessions on next tick"
/// marker dropped by identity-refresh teardown. Scanned in the
/// supervisor loop by [`shutdown::respawn_pending`].
pub(crate) const PENDING_RESTART_PREFIX: &str = "claude.pending_restart";

/// Validate an untrusted id (principal_id or session_id) from an IPC
/// payload before it flows into path construction, KV keys, or topic
/// formatting.
///
/// Rejects:
///   * empty / pure-`.` / pure-`..` reserved segments,
///   * anything longer than [`oracle_host::MAX_ID_LEN`],
///   * any character outside `[A-Za-z0-9._-]` (catches `/`, `\`, NUL,
///     quotes, whitespace, and topic delimiters all in one rule).
///
/// Mirrors the alphabet enforced by
/// `claude-install::layout::sanitize_principal_id` so a value that
/// successfully provisions a principal home is also a valid spawn
/// input. `field` is the logical name (used only in error text).
pub(crate) fn validate_id(field: &str, id: &str) -> Result<(), SysError> {
    oracle_host::ids::validate_id(field, id)
}

/// Claude host runner — capsule singleton.
///
/// Holds the live `Sessions` registry directly. The `#[capsule]`
/// macro stores a single `OnceLock<ClaudeRunner>` and gives every handler the
/// same `&self` for the duration of one capsule incarnation. That
/// satisfies the requirement to share `Process` resource handles
/// across IPC dispatches (KV-backed state would not — `Process` is a
/// component-model `resource` and is not serializable). Durable
/// metadata still rides in KV per-session via `state::SessionRecord`;
/// the runtime map is rebuilt on reload by the supervisor's first-tick
/// recovery sweep.
#[derive(Default)]
pub struct ClaudeRunner {
    pub(crate) sessions: Sessions,
}

/// `claude.v1.request.spawn` payload.
#[derive(Debug, Deserialize)]
pub struct SpawnRequest {
    /// Astrid principal this session belongs to.
    pub principal_id: String,
    /// Optional caller-provided session id — generated UUIDv4 if absent.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Optional first turn to write after spawn completes.
    #[serde(default)]
    pub initial_message: Option<String>,
}

/// `claude.v1.request.send.<sid>` payload — `session_id` is duplicated
/// in-body so dispatch handlers don't have to parse the topic suffix
/// themselves.
#[derive(Debug, Deserialize)]
pub struct SendRequest {
    /// Target session id.
    pub session_id: String,
    /// Plain-text user turn body.
    pub text: String,
}

/// `claude.v1.request.settings.set` payload.
///
/// Partial-update semantics: every field is `Option<...>`. Absent fields
/// preserve the current persisted value; present fields overwrite it. The
/// merged record is validated before persistence as a defence-in-depth
/// gate against forged IPC input (the serde enum alphabet already rejects
/// unknown variants).
///
/// [`interaction_mode`]: Self::interaction_mode
/// [`auth_mode`]: Self::auth_mode
#[derive(Debug, Deserialize)]
pub struct SettingsSetRequest {
    /// Principal whose config we're mutating. Validated via
    /// `validate_id` before reaching KV.
    pub principal_id: String,
    /// New interaction mode. When `None` the persisted value is kept.
    #[serde(default)]
    pub interaction_mode: Option<config::InteractionMode>,
    /// New auth mode. When `None` the persisted value is kept.
    #[serde(default)]
    pub auth_mode: Option<config::AuthMode>,
    /// New model tier. When `None` the persisted value is kept.
    #[serde(default)]
    pub model: Option<config::ModelPreference>,
    /// New per-session turn cap. `Some(n)` sets the cap; `None` keeps the
    /// persisted value. (v0 limitation: a patch cannot clear a cap back to
    /// uncapped — that takes a re-install. Set semantics, not unset.)
    #[serde(default)]
    pub max_turns: Option<u32>,
}

#[capsule]
impl ClaudeRunner {
    /// Spawn a new `claude -p` subprocess for a principal session.
    #[astrid::interceptor("handle_spawn")]
    pub fn handle_spawn(&self, mut req: SpawnRequest) -> Result<(), SysError> {
        req.principal_id = stamped_principal(&req.principal_id)?.to_string();
        // Untrusted input gate. principal_id flows into KV keys and
        // topic strings (the fs path is `home://...`, kernel-scoped per
        // invocation — principal_id no longer reaches the path);
        // session_id flows into the identity file basename and per-
        // session topics. Reject anything outside `[A-Za-z0-9._-]`
        // before the value escapes into formatted strings.
        validate_id("principal_id", &req.principal_id)?;
        if let Some(sid) = &req.session_id {
            validate_id("session_id", sid)?;
        }

        // Load the per-principal config BEFORE the session-cap check so
        // a Repl-mode principal doesn't burn a session slot just to be
        // rejected.
        //
        // Schema-version split (see `config::LoadOutcome`):
        //   * Current  — use the record verbatim. Common path.
        //   * NeedsMigration — known-older record; auto-migrate forward
        //     by publishing `claude.v1.audit.schema_migrated` + a relink
        //     with the patched config, then proceed with the patched
        //     cfg as the effective config. Safe because known-older
        //     → newer fields are filled fail-secure (Headless/ApiKey)
        //     and the operator's persisted modes are preserved.
        //   * Unknown — strictly-newer record this binary doesn't
        //     understand. Reject the spawn with a structured event
        //     rather than silently demoting to default (a binary
        //     downgrade must NOT overwrite operator-persisted
        //     settings).
        //
        // Missing / malformed records still fall back fail-secure to
        // the default `{Headless, ApiKey}` at the current `SCHEMA_VERSION`
        // via the `Current` arm of `load_status` — preserves current
        // behaviour for any principal that hasn't run the runner's
        // `#[astrid::install]` hook yet.
        let cfg = match load_principal_config_status() {
            LoadOutcome::Current(cfg) => cfg,
            LoadOutcome::NeedsMigration {
                patched,
                previous_version,
            } => {
                // Audit the auto-migration before persisting so a crash
                // mid-relink still leaves a forensic trail.
                let _ = ipc::publish_json(
                    "claude.v1.audit.schema_migrated",
                    &serde_json::json!({
                        "principal_id": req.principal_id,
                        "previous_version": previous_version,
                        "current": config::SCHEMA_VERSION,
                    }),
                );
                // Persist the patched record to the runner's own KV namespace
                // BEFORE publishing the relink. runner and claude-install live
                // in separate per-capsule KV namespaces (the canonical
                // record the runner reads on the next handle_spawn lives at
                // `claude.principal.config` here, while claude-install only
                // rewrites its own `.claude/` artifacts and its own
                // install-complete marker). Without this `save`, every
                // subsequent spawn would re-detect the old schema_version
                // and re-trigger a relink — the migration would never
                // terminate. Best-effort: a KV write failure must not
                // abort the spawn (the patched config is valid in-memory
                // for this turn), but it MUST be logged so the next
                // spawn's repeat-migration is diagnosable.
                if let Err(e) = config::save(&patched) {
                    log::warn(format!(
                        "claude-runner: failed to persist migrated principal config \
                         for {}: {e:?}; the next spawn will re-trigger \
                         migration",
                        req.principal_id
                    ));
                }
                // Publish a relink so claude-install rewrites the on-disk
                // `.claude/` artifacts in its own per-capsule namespace.
                // We do NOT block on the relink-complete reply here: the
                // patched config is already valid for this spawn (the
                // fields needed by the spawn pipeline are populated), and
                // the save above is what the next handle_spawn observes.
                let _ = ipc::publish_json(
                    "claude.v1.install.relink",
                    &serde_json::json!({
                        "principal_id": req.principal_id,
                        "config": patched,
                    }),
                );
                patched
            }
            LoadOutcome::Unknown(got) => {
                // Strictly-newer schema. Refuse to spawn. Mirrors the
                // structured rejection envelope used by `principal_limit`
                // and `interaction_mode_is_repl` below — fail-secure-
                // loud rather than silently downgrading.
                let _ = ipc::publish_json(
                    "claude.v1.event.session_rejected",
                    &serde_json::json!({
                        "principal_id": req.principal_id,
                        "reason": "schema_version_unsupported",
                        "got": got,
                        "expected": config::SCHEMA_VERSION,
                        "hint": "upgrade the runner or run --force install to rewrite the config",
                    }),
                );
                return Ok(());
            }
        };

        if let Err(error) = cfg.validate() {
            let _ = ipc::publish_json(
                "claude.v1.event.session_rejected",
                &serde_json::json!({
                    "principal_id": req.principal_id,
                    "reason": "invalid_config",
                    "error": error.to_string(),
                }),
            );
            return Ok(());
        }

        // Repl mode short-circuit. No spawn, no identity fetch, no env
        // read — the user drives `claude` directly inside the principal
        // folder and the runner just publishes a structured rejection so the
        // CLI / uplink can hint at the right next step. Note we omit
        // `session_id` from the payload: in repl mode no session is
        // minted, so reflecting the (possibly caller-provided) id would
        // imply a binding that doesn't exist.
        if cfg.interaction_mode == InteractionMode::Repl {
            let _ = ipc::publish_json(
                "claude.v1.event.session_rejected",
                &serde_json::json!({
                    "principal_id": req.principal_id,
                    "reason": "interaction_mode_is_repl",
                    "hint": "user drives `claude` directly in principal folder",
                }),
            );
            return Ok(());
        }

        // Per-principal session cap.
        let in_use = self.sessions.count_for_principal(&req.principal_id)?;
        if in_use >= MAX_SESSIONS_PER_PRINCIPAL {
            let _ = ipc::publish_json(
                "claude.v1.event.session_rejected",
                &serde_json::json!({
                    "principal_id": req.principal_id,
                    "reason": "principal_limit",
                    "active": in_use,
                    "limit": MAX_SESSIONS_PER_PRINCIPAL,
                }),
            );
            return Ok(());
        }

        let session_id = req.session_id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let principal_id = req.principal_id;
        if self
            .sessions
            .with(|sessions| sessions.contains_key(&session_id))?
        {
            publish_spawn_error(&session_id, &principal_id, "session_already_exists");
            return Ok(());
        }

        // Block until claude-install confirms the principal's `.claude/`
        // is provisioned. claude-install is the source of truth: it
        // performs its own idempotency check (returns `already_installed:
        // true` on a fast-reply cache-hit) and publishes
        // `success: false` with an `error` field on hard failure. On a
        // hard failure we surface the error to the spawn-error topic and
        // abort the spawn — proceeding would just spawn `claude -p`
        // against an unprovisioned principal home.
        //
        // On success claude-install returns the principal's `home://` VFS
        // scheme. The guest must not discover a host path; the process host
        // resolves the scheme when it applies child env/cwd/argv.
        let home_path = match ensure_install(&principal_id, &cfg) {
            EnsureInstall::Ok(home) => home,
            EnsureInstall::Failed(reason) => {
                publish_spawn_error(
                    &session_id,
                    &principal_id,
                    &format!("install_failed: {reason}"),
                );
                return Ok(());
            }
        };

        // Auth mode branch. In ApiKey mode the kernel-elicited secret
        // is required; an empty value aborts the spawn (the current
        // behaviour). In Subscription mode we SKIP the env::var read
        // entirely so the cleartext never lands in this stack frame and
        // a blank-on-purpose secret doesn't trigger `api_key_missing`.
        // The subscription path then threads `None` into SpawnInputs
        // and `spawn::spawn_claude` omits the `.env("ANTHROPIC_API_KEY")`
        // call so Claude falls back to its keychain OAuth path written
        // by `claude /login`.
        let api_key: Option<String> = match cfg.auth_mode {
            AuthMode::ApiKey => {
                let key = env::var("api_key").unwrap_or_default();
                if key.is_empty() {
                    publish_spawn_error(&session_id, &principal_id, "api_key_missing");
                    return Ok(());
                }
                Some(key)
            }
            AuthMode::Subscription => None,
        };

        // Mint the per-(principal, session) hook token AFTER every
        // up-front rejection path (schema_version, Repl, principal_limit,
        // install_failed, api_key_missing) so we never persist a token
        // for a session that won't actually spawn. The token is the
        // shared secret `astrid-emit` (shipping in core via astrid#814)
        // echoes back in every `claude.v1.hook.*` envelope; the
        // run loop verifies it against KV before republishing on
        // `hook.v1.event.<name>`. Two distinct failure modes:
        //
        //   * `hook_token_mint_failed` — host CSPRNG unavailable. Fatal:
        //     spawning without a token would let any forged hook
        //     publish republish under the runner's vouching.
        //   * `hook_token_persist_failed` — KV write failed. Also fatal:
        //     without a persisted token the run loop has nothing to
        //     match the envelope against, so every hook fire would be
        //     dropped as a spoof.
        let hook_token = match crate::hooks::mint_token() {
            Ok(t) => t,
            Err(e) => {
                log::warn(format!("claude-runner: hook token mint failed: {e:?}"));
                publish_spawn_error(&session_id, &principal_id, "hook_token_mint_failed");
                return Ok(());
            }
        };
        if let Err(e) = crate::hooks::persist_token(&principal_id, &session_id, &hook_token) {
            log::warn(format!("claude-runner: hook token persist failed: {e:?}"));
            publish_spawn_error(&session_id, &principal_id, "hook_token_persist_failed");
            return Ok(());
        }

        // Fetch identity prompt from spark with a 5 s budget. Falls
        // back to a hard-coded minimal prompt + audit on timeout.
        //
        // `home_path` remains a kernel-resolved VFS scheme for both file and
        // process requests; no host filesystem path crosses into the capsule.
        let prompt = match identity::fetch_prompt(&principal_id, &session_id, &home_path) {
            Ok(prompt) => prompt,
            Err(error) => {
                let _ = crate::hooks::forget_token(&principal_id, &session_id);
                publish_spawn_error(
                    &session_id,
                    &principal_id,
                    &format!("identity_context_failed: {error}"),
                );
                return Ok(());
            }
        };
        let identity_path = match identity::write_prompt_file(&home_path, &session_id, &prompt) {
            Ok(p) => p,
            Err(e) => {
                let _ = crate::hooks::forget_token(&principal_id, &session_id);
                publish_spawn_error(
                    &session_id,
                    &principal_id,
                    &format!("identity_write_failed: {e}"),
                );
                return Ok(());
            }
        };

        let started_at_ms = astrid_sdk::time::now()
            .ok()
            .and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_millis())
            })
            .and_then(|m| u64::try_from(m).ok())
            .unwrap_or(0);

        // `api_key` is `Some(...)` in api_key auth mode and would be `None`
        // in subscription mode once the dual-auth-mode runtime branch
        // lands. Today the empty-key short-circuit above guarantees we
        // reach this site with a non-empty `api_key`, so we always thread
        // `Some(...)`. The `Option<&str>` argument shape is retained at
        // the spawn boundary so the subscription path can land without
        // churning the call sites again. See `SpawnInputs::api_key`'s
        // doc-comment for the audit-stability invariant.
        let spawned = match spawn::spawn_claude(&spawn::SpawnInputs {
            principal_id: &principal_id,
            session_id: &session_id,
            home_path: &home_path,
            identity_path: &identity_path,
            api_key: api_key.as_deref(),
            hook_token: &hook_token,
            model: cfg.model,
            max_turns: cfg.max_turns,
        }) {
            Ok(s) => s,
            Err(e) => {
                // Best-effort: drop the just-persisted hook token so it
                // doesn't strand in KV. The session never started so
                // letting it leak is non-fatal — the next spawn for
                // (principal, session) would overwrite anyway — but
                // cleanup keeps the key space tidy. Ignore the delete
                // result; nothing to do if KV is wedged.
                let _ = crate::hooks::forget_token(&principal_id, &session_id);
                publish_spawn_error(&session_id, &principal_id, &format!("spawn_failed: {e}"));
                return Ok(());
            }
        };

        let record = SessionRecord {
            principal_id: principal_id.clone(),
            session_id: session_id.clone(),
            identity_path,
            started_at_ms,
            os_pid: spawned.os_pid,
            process_id: spawned.process_id,
        };
        let cleanup_process = spawned.process.clone();
        let recovery_process = spawned.process.clone();
        if let Err(error) = save_record(&record) {
            if let Err(cleanup_error) =
                cleanup_untracked_process(cleanup_process, &principal_id, &session_id)
            {
                preserve_recovery_session(
                    &self.sessions,
                    &record,
                    recovery_process,
                    &cleanup_error,
                );
            }
            publish_spawn_error(
                &session_id,
                &principal_id,
                &format!("session_record_failed: {error}"),
            );
            return Ok(());
        }

        if let Err(error) = self.sessions.with(|map| {
            map.insert(
                session_id.clone(),
                RuntimeSession {
                    record: record.clone(),
                    process: spawned.process,
                    codec: codec::LineDecoder::new(),
                },
            );
        }) {
            if let Err(cleanup_error) =
                cleanup_untracked_process(cleanup_process, &principal_id, &session_id)
            {
                preserve_recovery_session(
                    &self.sessions,
                    &record,
                    recovery_process,
                    &cleanup_error,
                );
            }
            publish_spawn_error(
                &session_id,
                &principal_id,
                &format!("session_registry_failed: {error}"),
            );
            return Ok(());
        }

        // TODO(astrid-rfcs#TBD): mirror to a shared cross-capsule audit
        // topic once a convention lands; the kernel-side
        // `astrid.v1.audit.entry` is admin-action-shaped and not the
        // right home for capsule-emitted attribution. The runner stays on
        // `claude.v1.audit.*` exclusively today — see README "Known
        // deficiencies" for the open RFC-class gap.
        //
        // `auth_mode` and `interaction_mode` are emitted in their
        // canonical snake_case wire form (matching the [env] select
        // enum_values) so downstream consumers can attribute the spawn
        // to a mode tuple without re-deriving it from the config KV.
        // `flags_hash` is intentionally stable across auth modes (see
        // [`spawn::SpawnInputs::api_key`]); the auth attribution comes
        // from this field, not the argv fingerprint.
        let auth_mode_str = match cfg.auth_mode {
            AuthMode::ApiKey => "api_key",
            AuthMode::Subscription => "subscription",
        };
        let interaction_mode_str = match cfg.interaction_mode {
            InteractionMode::Headless => "headless",
            InteractionMode::Repl => "repl",
        };
        let _ = ipc::publish_json(
            "claude.v1.audit.spawn",
            &serde_json::json!({
                "principal_id": principal_id,
                "session_id": session_id,
                "pid": spawned.os_pid,
                "flags_hash": spawned.flags_hash,
                "auth_mode": auth_mode_str,
                "interaction_mode": interaction_mode_str,
            }),
        );
        let _ = ipc::publish_json(
            &format!("claude.v1.event.{session_id}.spawned"),
            &serde_json::json!({
                "principal_id": principal_id,
                "session_id": session_id,
                "pid": spawned.os_pid,
            }),
        );

        // Optional first turn.
        if let Some(text) = req.initial_message
            && !text.is_empty()
        {
            send_user_turn(&self.sessions, &session_id, &text)?;
        }

        Ok(())
    }

    /// Per-principal install hook: persist the operator's interaction
    /// + auth choices into the runner's KV namespace and emit
    /// `claude.v1.audit.install_choices`. Implementation lives in
    /// `install::run`; see that module's doc-comment for the
    /// `[env]` read contract, failure mode, idempotency semantics,
    /// and audit shape.
    #[astrid::install]
    pub fn on_install(&self) -> Result<(), SysError> {
        install::run()
    }

    /// Send a user turn into an existing session's stdin.
    #[astrid::interceptor("handle_send")]
    pub fn handle_send(&self, req: SendRequest) -> Result<(), SysError> {
        // Validate before the id reaches any format!/IPC topic.
        validate_id("session_id", &req.session_id)?;
        if let Some(principal) = self.sessions.with(|sessions| {
            sessions
                .get(&req.session_id)
                .map(|session| session.record.principal_id.clone())
        })? {
            stamped_principal(&principal)?;
        }
        send_user_turn(&self.sessions, &req.session_id, &req.text)
    }

    /// Settings-set handler. Topic `claude.v1.request.settings.set`.
    /// Applies a partial patch to the per-principal
    /// `config::PrincipalConfig`, persists, and publishes — in order —
    /// `claude.v1.audit.settings_changed`, `claude.v1.install.relink`,
    /// `claude.v1.settings.changed`. Implementation lives in
    /// `settings::apply`; see that module's doc-comment for the
    /// publish-ordering contract and error semantics.
    #[astrid::interceptor("handle_settings_set")]
    pub fn handle_settings_set(&self, req: SettingsSetRequest) -> Result<(), SysError> {
        settings::apply(req)
    }

    /// Supervisor run loop. Each tick (~50 ms):
    /// 1. Drains every active session's stdout via [`supervisor::tick`].
    /// 2. Drains `claude.v1.request.stop.*` and gracefully terminates
    ///    matching sessions ([`shutdown::stop_session`]).
    /// 3. Drains `tool.v1.execute.save_identity.result` for identity-
    ///    refresh teardown ([`shutdown::handle_identity_refresh`]).
    /// 4. Drains `claude.v1.hook.*`, validates each per-session hook token
    ///    against KV, and republishes on the canonical
    ///    `hook.v1.event.<name>` ([`crate::hooks::validate_and_route`]).
    /// Identity refresh captures its principal-bound home/config/secret and
    /// completes the matching respawn inline before this loop polls hooks.
    ///
    /// Tool dispatch and approval routing are deliberately absent: claude
    /// drives `mcp__aos__*` tools against the registered `aos mcp
    /// serve` MCP server, and that server owns its own approval / timeout
    /// handling — the runner would only double-execute if it intervened.
    #[astrid::run]
    fn run(&self) -> Result<(), SysError> {
        let stop_sub = ipc::subscribe("claude.v1.request.stop.*")?;
        let identity_sub = ipc::subscribe("tool.v1.execute.save_identity.result")?;
        // Hook validator (runner-as-CA): `astrid-emit` (shipping in core
        // via astrid#814) stamps a per-session token onto every Claude
        // hook fire and publishes on `claude.v1.hook.<name>`.
        // The run loop drains those, verifies the token against KV,
        // and republishes the validated payload on the canonical
        // `hook.v1.event.<name>` topic. Mismatches are dropped with an
        // audit on `claude.v1.audit.hook_spoof_attempt`.
        let hook_sub = ipc::subscribe("claude.v1.hook.*")?;
        let _ = runtime::signal_ready();
        log::info("claude-runner: supervisor loop starting");

        loop {
            if let Err(e) = supervisor::tick(&self.sessions) {
                log::warn(format!("claude-runner: supervisor tick errored: {e}"));
            }

            if let Ok(poll) = stop_sub.poll() {
                for msg in poll.messages {
                    let sid = topic_tail(&msg.topic)
                        .map(str::to_string)
                        .unwrap_or_default();
                    if sid.is_empty() {
                        continue;
                    }
                    // The topic tail is attacker-controlled (anyone with
                    // ipc::publish to `claude.v1.request.stop.*` can pick
                    // it). Validate before it reaches log lines or
                    // downstream `format!()`s.
                    if validate_id("session_id", &sid).is_err() {
                        log::warn("claude-runner: stop request with invalid session_id; dropping");
                        continue;
                    }
                    let owner = match self
                        .sessions
                        .with(|map| map.get(&sid).map(|s| s.record.principal_id.clone()))
                    {
                        Ok(owner) => owner,
                        Err(error) => {
                            log::warn(format!(
                                "claude-runner: cannot authorize stop({sid}): {error}"
                            ));
                            continue;
                        }
                    };
                    let Some(owner) = owner else {
                        continue;
                    };
                    if resolve_principal(&owner, msg.principal.verified()).is_err() {
                        log::warn(format!(
                            "claude-runner: rejected cross-principal stop({sid})"
                        ));
                        let _ = ipc::publish_json(
                            "claude.v1.audit.stop_rejected",
                            &serde_json::json!({
                                "session_id": sid,
                                "reason": "principal_mismatch",
                            }),
                        );
                        continue;
                    }
                    if let Err(e) = shutdown::stop_session(&self.sessions, &sid, "requested") {
                        log::warn(format!("claude-runner: stop({sid}) failed: {e:?}"));
                    }
                }
            }

            if let Ok(poll) = identity_sub.poll() {
                for msg in poll.messages {
                    if let Err(e) = shutdown::handle_identity_refresh(&self.sessions, &msg) {
                        log::warn(format!("claude-runner: identity-refresh failed: {e:?}"));
                    }
                }
            }

            // Hook validator drain. `validate_and_route` parses, looks up
            // the per-session token in KV, strips the transport envelope
            // (principal_id / session_id / token), and republishes on
            // `hook.v1.event.<name>` (canonical) or `claude.v1.notification`
            // (no canonical equivalent yet). Token mismatches drop the
            // event and publish `claude.v1.audit.hook_spoof_attempt`.
            // Errors are swallowed (helper logs internally) so a bad
            // poll doesn't tear down the run loop.
            if let Ok(poll) = hook_sub.poll() {
                let _ = crate::hooks::validate_and_route(poll.messages);
            }

            if astrid_sdk::time::sleep(supervisor::TICK_INTERVAL).is_err() {
                // Sleep returning Err implies host shutdown / unload.
                break;
            }
        }
        Ok(())
    }
}

/// Write a `user`-role stream-json envelope to a session's stdin.
///
/// INVARIANT: `process.write_stdin` is a host call that may block on
/// kernel-side back-pressure and could re-enter the bus drain. Holding
/// the `Sessions` mutex across it would serialise the entire supervisor
/// loop and risks deadlock. The pattern here — encode + clone the
/// `PersistentProcess` handle under the lock, drop the guard, then write —
/// is the canonical lock-discipline shape mirrored by the supervisor's
/// `read_logs` drain in [`supervisor::drive_session`].
fn send_user_turn(sessions: &Sessions, session_id: &str, text: &str) -> Result<(), SysError> {
    let line = encode(&Outbound::UserTurn { text });
    if line.len() > 1024 * 1024 {
        let _ = ipc::publish_json(
            &format!("claude.v1.event.{session_id}.error"),
            &serde_json::json!({ "reason": "stdin_quota" }),
        );
        return Ok(());
    }

    // Phase 1: clone the Process handle out from under the lock.
    let process = sessions.with(|map| map.get(session_id).map(|s| s.process.clone()))?;
    let Some(process) = process else {
        log::warn(format!("send to unknown session {session_id}"));
        return Ok(());
    };

    // Phase 2: host call outside the lock.
    match process.write_stdin(line.as_bytes()) {
        Ok(_) => Ok(()),
        Err(e) => {
            let msg = format!("{e}");
            if msg.contains("TooLarge") {
                let _ = ipc::publish_json(
                    &format!("claude.v1.event.{session_id}.error"),
                    &serde_json::json!({ "reason": "stdin_quota" }),
                );
            } else {
                log::warn(format!("write_stdin send_user_turn failed: {msg}"));
            }
            Ok(())
        }
    }
}

/// Result of [`ensure_install`].
///
/// We deliberately do NOT model "timeout" or "internal SDK error" as a
/// distinct variant — both end up surfaced through `Failed` so the spawn
/// path uniformly aborts and the operator sees one stream of spawn-
/// error events instead of chasing two divergent topics.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EnsureInstall {
    /// claude-install confirmed the principal is provisioned. Either a
    /// fresh `success: true` event arrived, or the cache-hit fast-reply
    /// (`already_installed: true`) landed. The string is always the guest VFS
    /// scheme `home://`; host filesystem paths never cross into the capsule.
    Ok(String),
    /// Either claude-install published `success: false` (carrying its
    /// `error` field), or the 30 s deadline elapsed with no response,
    /// or an SDK call blew up. Carries an operator-readable reason
    /// string forwarded to `claude.v1.event.<sid>.error`.
    Failed(String),
}

/// Classification of one `claude.v1.install.complete` envelope when
/// matched against a target `principal_id`. Pure function — extracted
/// from [`ensure_install`] so the success / failure / skip branching
/// is unit-testable on the host without standing up the IPC bus.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InstallEnvelope {
    /// Envelope matched our principal and reported `success: true`.
    /// The installer's informational `home_path` is deliberately ignored;
    /// the runner always supplies `home://` to the process host.
    Success,
    /// Envelope matched our principal and reported `success: false`.
    /// The string is the install error reason, lifted verbatim from the
    /// `error` field if present, otherwise `"unknown"`.
    Failure(String),
    /// Envelope was for a different principal, was unparseable, or had
    /// no `principal_id` field. The caller should keep waiting.
    Skip,
}

/// Pure classifier for a single `claude.v1.install.complete` payload
/// against `principal_id`.
///
/// Returns [`InstallEnvelope::Skip`] for any payload that does not
/// match `principal_id` (including malformed JSON), [`Success`] for a
/// matching `success: true`, and [`Failure(reason)`] for a matching
/// `success: false` (using the `error` field, or `"unknown"` if the
/// envelope omitted it). Treated as failure rather than success-by-
/// default so a malformed claude-install envelope cannot silently
/// progress the spawn.
///
/// [`Success`]: InstallEnvelope::Success
/// [`Failure(reason)`]: InstallEnvelope::Failure
pub(crate) fn classify_install_complete(payload: &str, principal_id: &str) -> InstallEnvelope {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return InstallEnvelope::Skip;
    };
    if value
        .get("principal_id")
        .and_then(Value::as_str)
        .is_none_or(|p| p != principal_id)
    {
        return InstallEnvelope::Skip;
    }
    if value.get("success").and_then(Value::as_bool) == Some(true) {
        return InstallEnvelope::Success;
    }
    let reason = value
        .get("error")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string();
    InstallEnvelope::Failure(reason)
}

/// Drive claude-install for `principal_id` and block until it terminates.
///
/// Source-of-truth contract:
///
/// * claude-install owns idempotency. It writes the install-complete
///   marker into its OWN per-capsule KV namespace; the kernel scopes
///   KV by `{principal}:capsule:{capsule_id}` so the runner cannot read it
///   from here. The previous implementation tried to and silently
///   missed every time, forcing a full publish/subscribe round-trip on
///   every spawn — including for already-provisioned principals. That
///   short-circuit is now claude-install's job: it returns
///   `already_installed: true` on a cache-hit, which is fast because
///   the publish/subscribe traffic is loopback within the bus.
///
/// * claude-install signals success with `success: true`, hard failure
///   with `success: false` + an `error` field. A hard failure here
///   must abort the spawn — proceeding to fork `claude -p` against an
///   unprovisioned `.claude/` directory would just produce a noisier
///   downstream failure.
///
/// * A 30 s deadline with no matching reply is treated as a hard
///   failure too. The host may have unloaded claude-install, or the
///   capsule registry may be missing it — in either case the spawn
///   has no useful work to do.
fn ensure_install(principal_id: &str, cfg: &config::PrincipalConfig) -> EnsureInstall {
    let sub = match ipc::subscribe("claude.v1.install.complete") {
        Ok(s) => s,
        Err(e) => return EnsureInstall::Failed(format!("subscribe_failed: {e}")),
    };
    // Thread the per-principal config in the install.run envelope so
    // claude-install can branch its `.claude/settings.local.json` +
    // `.mcp.json` writers without a cross-namespace KV read (claude-install
    // lives in its own per-capsule KV namespace and can't peek at the runner's
    // `claude.principal.config` record directly — the kernel scopes KV
    // by `{principal}:capsule:{capsule_id}`). The receiver defaults to
    // `PrincipalConfig::default()` (i.e. `{Headless, ApiKey, v1}`) when
    // the field is absent, preserving back-compat with older runner
    // envelopes; see `claude-install::run_install` for that fallback.
    if let Err(e) = ipc::publish_json(
        "claude.v1.install.run",
        &serde_json::json!({
            "principal_id": principal_id,
            "config": cfg,
        }),
    ) {
        return EnsureInstall::Failed(format!("publish_failed: {e}"));
    }

    let deadline = Duration::from_secs(30);
    let mut remaining_ms = u64::try_from(deadline.as_millis()).unwrap_or(30_000);
    while remaining_ms > 0 {
        let step = remaining_ms.min(2_000);
        if let Ok(result) = sub.recv(step) {
            for msg in result.messages {
                if resolve_principal(principal_id, msg.principal.verified()).is_err() {
                    continue;
                }
                // Multiple principals may be installing concurrently on
                // the bus — `classify_install_complete` filters to our
                // own principal_id and folds the success/failure shape
                // into a single decision. Failure includes the `error`
                // string verbatim so the operator sees the real reason.
                match classify_install_complete(&msg.payload, principal_id) {
                    InstallEnvelope::Success => {
                        return EnsureInstall::Ok("home://".to_string());
                    }
                    InstallEnvelope::Failure(reason) => return EnsureInstall::Failed(reason),
                    InstallEnvelope::Skip => {}
                }
            }
        }
        remaining_ms = remaining_ms.saturating_sub(step);
    }

    EnsureInstall::Failed(format!(
        "install_timeout: no claude.v1.install.complete for principal {principal_id} within 30s"
    ))
}

fn publish_spawn_error(session_id: &str, principal_id: &str, reason: &str) {
    let _ = ipc::publish_json(
        &format!("claude.v1.event.{session_id}.error"),
        &serde_json::json!({
            "principal_id": principal_id,
            "session_id": session_id,
            "reason": reason,
        }),
    );
}

pub(crate) fn cleanup_untracked_process(
    process: process::PersistentProcess,
    principal_id: &str,
    session_id: &str,
) -> Result<(), SysError> {
    cleanup_after_stop(process.stop(None).map(|_| ()), || {
        let _ = hooks::forget_token(principal_id, session_id);
        let _ = state::delete_record(session_id);
    })
    .inspect_err(|error| {
        log::error(format!(
            "claude-runner: failed to stop untracked process for {session_id}: {error:?}"
        ));
    })
}

fn cleanup_after_stop<E>(
    stop_result: Result<(), E>,
    cleanup_state: impl FnOnce(),
) -> Result<(), E> {
    stop_result?;
    cleanup_state();
    Ok(())
}

pub(crate) fn preserve_recovery_session(
    sessions: &Sessions,
    record: &SessionRecord,
    process: process::PersistentProcess,
    cleanup_error: &SysError,
) {
    log::error(format!(
        "claude-runner: preserving recovery state for {} after cleanup failure: {cleanup_error:?}",
        record.session_id
    ));
    if let Err(error) = state::save_record(record) {
        log::error(format!(
            "claude-runner: failed to preserve recovery record for {}: {error:?}",
            record.session_id
        ));
    }
    if let Err(error) = sessions.with(|map| {
        map.entry(record.session_id.clone())
            .or_insert_with(|| RuntimeSession {
                record: record.clone(),
                process,
                codec: codec::LineDecoder::default(),
            });
    }) {
        log::error(format!(
            "claude-runner: failed to preserve runtime handle for {}: {error:?}",
            record.session_id
        ));
    }
    if let Err(error) = sessions.request_reload_recovery() {
        log::error(format!(
            "claude-runner: failed to re-arm recovery for {}: {error:?}",
            record.session_id
        ));
    }
}

/// Pull the trailing segment out of an IPC topic (the bit after the
/// last `.`). Used by the run loop to recover the `session_id` from a
/// `claude.v1.request.stop.<sid>` wildcard-subscription envelope.
pub(crate) fn topic_tail(topic: &str) -> Option<&str> {
    topic.rsplit('.').next().filter(|s| !s.is_empty())
}

// Host-side unit tests live in a sibling file — see `lib_tests.rs` for
// the bodies. `#[path]` keeps the test module attached to `lib.rs` via
// `super::*` while moving the line-count weight out of this file.
#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
