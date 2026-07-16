//! Hook-event-bridge primitives: per-session token mint / persist /
//! lookup / forget, constant-time token compare, and the canonical
//! Claude-hook → Astrid-topic mapping.
//!
//! Wired end-to-end: `handle_spawn` (lib.rs) mints + persists tokens
//! and sets `ASTRID_HOOK_TOKEN`/`ASTRID_PRINCIPAL_ID`/`ASTRID_SESSION_ID`
//! on the `claude` child env; the `#[astrid::run]` supervisor subscribes
//! to `claude.v1.hook.*` and routes each event through
//! `lookup_token` + `tokens_match` + canonical-topic republish;
//! `shutdown::stop_session` and `supervisor::evict` call `forget_token`
//! on every session-end path. See astrid-runtime/astrid#814 (binary) and
//! rfcs#30 (capability-token forward direction) for the broader design
//! and the residual Linux env-stealability gap.
//!
//! ## Threat model
//!
//! The native `astrid-emit` binary (shipping separately in core) reads
//! a per-session token from the spawned `claude -p` process env
//! (`ASTRID_HOOK_TOKEN`) and includes it in every published
//! `claude.v1.hook.*` event. The runner's loop looks the token
//! up in KV at `claude.hook_token.<principal>.<session>` and only
//! republishes on the canonical `hook.v1.event.*` topics when the
//! claimed token matches the stored one.
//!
//! Token compare is constant-time to avoid a same-host timing oracle
//! even though the practical threat is bounded to other processes
//! sharing the same env (Linux residual env-stealability — see RFC#30
//! capability-token migration).

use astrid_sdk::prelude::*;
use serde::Deserialize;

use crate::topic_tail;

/// KV prefix for per-session hook tokens. Composed with
/// `(principal_id, session_id)` by [`hook_token_key`] to mirror the runner's
/// established `claude.<thing>.<id>.<id>` convention (see
/// `state::SESSION_KEY_PREFIX`).
pub(crate) const HOOK_TOKEN_KEY_PREFIX: &str = "claude.hook_token";

/// Number of random bytes drawn from the host CSPRNG per token. 32
/// bytes → 256 bits → 64-hex-char string, comfortably above the
/// guessing-attack budget for any plausible adversary.
const TOKEN_BYTES: usize = 32;

/// Mapping from Claude-side hook event names (as they appear in the
/// `claude.v1.hook.<name>` topic tail) to the topic the
/// validator republishes on after a successful token match.
///
/// Every tail except `notification` maps to a canonical Astrid
/// `hook.v1.event.<name>` topic — the session lifecycle (setup / start /
/// end), the prompt and tool-call turns (incl. failures, batches, prompt
/// expansion, permission denials), the subagent lifecycle, the compaction
/// window, and config / instructions / filesystem / MCP-elicitation
/// observability. `hook.v1.event.*` is a wildcard publish, so the table
/// widens with no cross-capsule contract change. `notification` has no
/// canonical equivalent yet — the runner republishes it on the claude-namespaced
/// `claude.v1.notification` instead.
///
/// SYNC: the SET of tails here MUST equal the set of `claude.v1.hook.<tail>`
/// values in `claude_install::layout::HOOK_TOPIC_MAP` (claude-install/src/layout.rs).
/// The two crates have no dependency edge, so the table is mirrored. A tail
/// authored on the spawn side but absent here is published by `astrid-emit`
/// yet dropped by the validator (`unknown_hook`); the reverse is dead. The
/// cross-crate set-equality is pinned by tests on both sides.
pub(crate) const HOOK_TOPIC_MAP: &[(&str, &str)] = &[
    ("session_start", "hook.v1.event.session_start"),
    ("session_end", "hook.v1.event.session_end"),
    ("session_setup", "hook.v1.event.session_setup"),
    ("message_received", "hook.v1.event.message_received"),
    ("message_expanded", "hook.v1.event.message_expanded"),
    ("before_tool_call", "hook.v1.event.before_tool_call"),
    ("after_tool_call", "hook.v1.event.after_tool_call"),
    (
        "after_tool_call_failed",
        "hook.v1.event.after_tool_call_failed",
    ),
    ("after_tool_batch", "hook.v1.event.after_tool_batch"),
    ("permission_requested", "hook.v1.event.permission_requested"),
    ("permission_denied", "hook.v1.event.permission_denied"),
    ("message_sent", "hook.v1.event.message_sent"),
    ("message_failed", "hook.v1.event.message_failed"),
    ("subagent_start", "hook.v1.event.subagent_start"),
    ("subagent_stop", "hook.v1.event.subagent_stop"),
    ("task_created", "hook.v1.event.task_created"),
    ("task_completed", "hook.v1.event.task_completed"),
    ("teammate_idle", "hook.v1.event.teammate_idle"),
    (
        "on_compaction_started",
        "hook.v1.event.on_compaction_started",
    ),
    (
        "on_compaction_completed",
        "hook.v1.event.on_compaction_completed",
    ),
    ("config_changed", "hook.v1.event.config_changed"),
    ("instructions_loaded", "hook.v1.event.instructions_loaded"),
    ("file_changed", "hook.v1.event.file_changed"),
    ("cwd_changed", "hook.v1.event.cwd_changed"),
    ("worktree_created", "hook.v1.event.worktree_created"),
    ("worktree_removed", "hook.v1.event.worktree_removed"),
    (
        "elicitation_requested",
        "hook.v1.event.elicitation_requested",
    ),
    ("elicitation_resolved", "hook.v1.event.elicitation_resolved"),
    ("message_displayed", "hook.v1.event.message_displayed"),
    ("notification", "claude.v1.notification"),
];

/// KV key for a session's hook token. The kernel scopes KV by
/// `{principal}:capsule:{capsule_id}`, so this key sits inside the runner's
/// per-principal namespace; embedding `principal_id` in the key
/// nonetheless lets the runner's run loop (which runs under the runner's own
/// principal, not the claimed one) demand-fetch by the envelope's
/// claimed `(principal, session)` pair.
pub(crate) fn hook_token_key(principal_id: &str, session_id: &str) -> String {
    format!("{HOOK_TOKEN_KEY_PREFIX}.{principal_id}.{session_id}")
}

/// Mint a fresh hook token using the host CSPRNG.
///
/// Returns the hex-encoded 256-bit token. The token is **not**
/// persisted — callers must follow up with [`persist_token`] before
/// threading the value into the child process's env.
pub(crate) fn mint_token() -> Result<String, SysError> {
    let bytes = runtime::random_bytes(TOKEN_BYTES)?;
    Ok(hex_encode(&bytes))
}

/// Persist a hook token at `claude.hook_token.<principal>.<session>`.
/// Stored as raw bytes (the hex digits) — no JSON envelope, since the
/// value is opaque to every consumer except `tokens_match`.
pub(crate) fn persist_token(
    principal_id: &str,
    session_id: &str,
    token: &str,
) -> Result<(), SysError> {
    kv::set_bytes(&hook_token_key(principal_id, session_id), token.as_bytes())
}

/// Look up a previously-persisted hook token for `(principal,
/// session)`. Returns `Ok(None)` when no token is registered —
/// callers should treat that as a spoof attempt indistinguishable
/// from a stale claim.
pub(crate) fn lookup_token(
    principal_id: &str,
    session_id: &str,
) -> Result<Option<String>, SysError> {
    let Some(bytes) = kv::get_bytes_opt(&hook_token_key(principal_id, session_id))? else {
        return Ok(None);
    };
    // Lossy is fine: tokens are lowercase hex by construction; any
    // non-UTF-8 byte means the slot was corrupted and the lookup
    // will deliberately mismatch against any well-formed claim.
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// Delete a session's hook token. Idempotent (per `kv::delete`'s
/// contract) — safe to call from every session-end path
/// (`shutdown::stop_session`, `supervisor::evict`,
/// `supervisor::reload_recover`) without coordinating which one
/// "owns" the cleanup.
pub(crate) fn forget_token(principal_id: &str, session_id: &str) -> Result<(), SysError> {
    kv::delete(&hook_token_key(principal_id, session_id))
}

/// Constant-time equality on token strings.
///
/// Length mismatch short-circuits to `false`; equal-length inputs are
/// compared byte-by-byte with XOR-accumulation so the timing channel
/// reveals only the length (already implicit from the protocol).
///
/// The practical threat model is local-process — the validator and
/// the producer both run under the same host — but using `==` on
/// secret material is a code-smell that propagates: future refactors
/// that move token compare across an IPC boundary, an HTTP probe, or
/// a profiler sample inherit the original primitive's properties.
pub(crate) fn tokens_match(claimed: &str, stored: &str) -> bool {
    let a = claimed.as_bytes();
    let b = stored.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// Wire shape of an `claude.v1.hook.<name>` envelope as
/// published by the native `astrid-emit` binary (core PR for
/// astrid-runtime/astrid#814). `principal_id`, `session_id`, and
/// `token` are claim-only transport fields — the runner trusts none of
/// them until [`tokens_match`] succeeds against the KV-stored token.
#[derive(Debug, Deserialize)]
struct HookEnvelope {
    hook: String,
    payload: String,
    #[serde(default)]
    correlation_id: Option<String>,
    principal_id: String,
    session_id: String,
    token: String,
}

/// Look up the canonical republish topic for a hook name. Returns
/// `None` for unknown hook names; the caller audits and drops.
fn canonical_topic_for(hook_name: &str) -> Option<&'static str> {
    HOOK_TOPIC_MAP.iter().find_map(|(name, topic)| {
        if *name == hook_name {
            Some(*topic)
        } else {
            None
        }
    })
}

/// Build the canonical republish body for a token-validated envelope.
///
/// **Strip-the-transport** invariant: the returned `serde_json::Value`
/// MUST NOT carry `session_id` or `token`. Those are transport-layer
/// fields the validator uses to authenticate the producer; subscribers
/// on the canonical `hook.v1.event.<name>` topic see only the validator's
/// vouched principal claim plus the canonical payload + correlation id.
///
/// `principal_id` rides INSIDE the body (not in kernel attribution
/// metadata) because the republish is attributed to the runner's own capsule
/// from the run-loop context — the runner acts as a CA, and the principal
/// claim has no other channel onto the wire.
///
/// Pure function: no host calls, no IPC, no KV. Lifted out of
/// [`validate_and_route`] so the strip-the-transport regression test
/// (`session_id` / `token` MUST NOT appear in the serialized JSON) is
/// host-call-free and deterministic.
fn build_canonical_body(envelope: &HookEnvelope) -> serde_json::Value {
    serde_json::json!({
        "hook": envelope.hook,
        "payload": envelope.payload,
        "correlation_id": envelope.correlation_id,
        "principal_id": envelope.principal_id,
    })
}

/// Hard cap on attacker-controlled string fields echoed onto the
/// audit topic. The producer of `claude.v1.hook.*` is
/// authenticated only by a token *we* mint, so a spoof attempt can
/// place arbitrary bytes in `principal_id` / `session_id` / `hook`.
/// Truncating before republish bounds 1:1 amplification onto
/// `claude.v1.audit.hook_spoof_attempt` to ~3*256 + reason overhead per
/// event regardless of the producer's payload size. 256 is comfortably
/// above any legitimate id (the runner's own `validate_id` caps ids at 128
/// — see `lib::MAX_ID_LEN`) while leaving headroom for diagnostic
/// suffixes a future producer might add.
const AUDIT_FIELD_CAP: usize = 256;

/// Truncate a string to at most `AUDIT_FIELD_CAP` bytes, splitting on
/// a char boundary so the resulting `&str` stays valid UTF-8. Returns
/// `s` unchanged when already short enough.
fn audit_truncate(s: &str) -> &str {
    if s.len() <= AUDIT_FIELD_CAP {
        return s;
    }
    // floor_char_boundary is unstable; walk back from AUDIT_FIELD_CAP
    // until we land on a char boundary. At most 3 bytes of slack for
    // UTF-8.
    let mut cap = AUDIT_FIELD_CAP;
    while !s.is_char_boundary(cap) {
        cap -= 1;
    }
    &s[..cap]
}

/// Publish a `claude.v1.audit.hook_spoof_attempt` carrying *only the
/// claim* — never the offending token bytes. Emitting the token on
/// the audit topic would defeat its purpose as a secret.
///
/// The claim fields (`claimed_principal`, `claimed_session`, `hook`)
/// originate from a claim-only envelope under attacker control, so
/// each is truncated via [`audit_truncate`] before publish to bound
/// the audit topic's amplification factor on adversarial input.
///
/// `reason` distinguishes:
///
/// * `"no_token"` — no entry registered for the claimed pair (stale,
///   forged, or pre-spawn).
/// * `"token_mismatch"` — entry exists but bytes differ.
/// * `"unknown_hook"` — the hook name has no canonical mapping in
///   [`HOOK_TOPIC_MAP`].
/// * `"tail_mismatch"` — the topic's trailing segment doesn't match
///   the envelope's `hook` field (producer is malformed or actively
///   adversarial).
fn publish_spoof_audit(claimed_principal: &str, claimed_session: &str, hook: &str, reason: &str) {
    let _ = ipc::publish_json(
        "claude.v1.audit.hook_spoof_attempt",
        &serde_json::json!({
            "claimed_principal": audit_truncate(claimed_principal),
            "claimed_session": audit_truncate(claimed_session),
            "hook": audit_truncate(hook),
            "reason": reason,
        }),
    );
}

/// Run-loop drain helper for `claude.v1.hook.*` events.
///
/// Single-phase — no `Sessions` lock is involved, only a KV lookup and a
/// bus republish. Per message:
///
/// 1. Parse the envelope. On parse failure, warn and continue (a
///    malformed payload is logged but never crashes the run loop).
/// 2. Sanity-check the topic tail equals the envelope's `hook` field.
///    A mismatch means the producer is malformed or actively
///    adversarial — audit and drop.
/// 3. Look up `claude.hook_token.<principal>.<session>` in KV. Missing
///    entry is audited as `"no_token"` and dropped.
/// 4. Constant-time compare the claimed token against the stored
///    value via [`tokens_match`]. Failure is audited as
///    `"token_mismatch"` (claim only — never the token bytes) and
///    dropped.
/// 5. Resolve the canonical republish topic via [`HOOK_TOPIC_MAP`].
///    Unknown hook names are audited and dropped.
/// 6. Build the canonical republish body. Per the workflow brief,
///    the `principal_id` rides INSIDE the payload because the runner
///    attributes the republish from its own run-loop context — the
///    principal claim has no other channel. The transport fields
///    `session_id` and `token` are stripped; subscribers never see
///    them.
pub(crate) fn validate_and_route(messages: Vec<ipc::Message>) -> Result<(), SysError> {
    if messages.is_empty() {
        return Ok(());
    }
    for msg in messages {
        let envelope: HookEnvelope = match serde_json::from_str(&msg.payload) {
            Ok(e) => e,
            Err(e) => {
                log::warn(format!(
                    "claude-runner: hook event parse failed on '{}': {e}",
                    msg.topic
                ));
                continue;
            }
        };

        // Sanity: topic tail must equal the envelope's declared hook
        // name. A producer that disagrees with itself is either
        // malformed or trying to slip an event in under a hook name
        // it didn't authenticate against.
        let tail = topic_tail(&msg.topic).unwrap_or("");
        if tail != envelope.hook {
            publish_spoof_audit(
                &envelope.principal_id,
                &envelope.session_id,
                &envelope.hook,
                "tail_mismatch",
            );
            continue;
        }

        // Look up the per-session token. The `principal_id` and
        // `session_id` here are claim-only — the token match below
        // is what makes the claim trustworthy. The kernel's
        // per-(principal, capsule) KV scoping means we can't read
        // another principal's namespace even if the envelope tries
        // to claim it, since the lookup runs under the runner's own
        // run-loop principal.
        //
        // KV transport errors are logged + skipped (NOT `?`-bubbled):
        // a single wedged KV call must not abort processing of the
        // remaining batch from this `poll()`. Mirrors the parse-error
        // branch above and the broader "validator never tears down the
        // run loop" contract.
        let stored = match lookup_token(&envelope.principal_id, &envelope.session_id) {
            Ok(Some(s)) => s,
            Ok(None) => {
                publish_spoof_audit(
                    &envelope.principal_id,
                    &envelope.session_id,
                    &envelope.hook,
                    "no_token",
                );
                continue;
            }
            Err(e) => {
                log::warn(format!(
                    "claude-runner: hook-token KV lookup failed for hook '{}': {e:?}",
                    envelope.hook
                ));
                continue;
            }
        };
        if !tokens_match(&envelope.token, &stored) {
            // Audit the claim — NOT the offending token bytes.
            publish_spoof_audit(
                &envelope.principal_id,
                &envelope.session_id,
                &envelope.hook,
                "token_mismatch",
            );
            continue;
        }

        // Token matches — claim is trusted. Resolve the canonical
        // republish topic.
        let Some(canonical) = canonical_topic_for(&envelope.hook) else {
            publish_spoof_audit(
                &envelope.principal_id,
                &envelope.session_id,
                &envelope.hook,
                "unknown_hook",
            );
            continue;
        };

        // Build the canonical body via the pure helper so the
        // strip-the-transport invariant has a single source of truth.
        // See [`build_canonical_body`] for the field-by-field contract.
        let body = build_canonical_body(&envelope);
        if let Err(e) = ipc::publish_json(canonical, &body) {
            log::warn(format!(
                "claude-runner: republish to '{canonical}' failed: {e:?}"
            ));
        }
    }
    Ok(())
}

/// Lowercase-hex encode a byte slice. Mirrors the helper at
/// `claude-install/src/atomic.rs:99-107`. Duplicated rather than shared
/// across crates because (a) hex encode is six lines, (b) the runner and
/// claude-install have no other shared utility crate, and (c) carving
/// out an extra shared crate for this single helper is the wrong
/// trade.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

// Host-side unit tests live in a sibling file -- see `hooks_tests.rs`
// for the bodies. `#[path]` keeps the test module attached to this
// file via `super::*` while matching the `lib.rs` / `lib_tests.rs`
// convention used throughout the crate.
#[cfg(test)]
#[path = "hooks_tests.rs"]
mod tests;
