//! Per-session identity: fetch the system prompt from the spark capsule
//! and atomically materialize it under the principal's `.claude/`.
//!
//! Subscribe-before-publish ordering: we subscribe to
//! `spark.v1.response.ready` first, then publish
//! `spark.v1.request.build`, then drain responses for up to 5 s while
//! filtering by the `session_id` we sent. On timeout or any other
//! failure mode we fall back to a hard-coded minimal prompt and publish
//! `claude.v1.audit.identity_fallback` so observability picks up the
//! deviation. Spawn never blocks on identity.

use astrid_sdk::prelude::*;
use oracle_host::ids::resolve_principal;
use serde::Deserialize;
use std::time::Duration;

/// Hard fallback when spark is unreachable. Deliberately terse — the
/// goal is to keep claude's tool-use framing intact, not to provide
/// product persona. Real identity rejoins on the next session start.
const FALLBACK_PROMPT: &str = "You are an agent running inside Unicity AOS. \
                               Tools are exposed via mcp__aos__*.";

const SPARK_REQUEST_TOPIC: &str = "spark.v1.request.build";
const SPARK_RESPONSE_TOPIC: &str = "spark.v1.response.ready";
const SPARK_DEADLINE_MS: u64 = 5_000;
const SPARK_TICK_MS: u64 = 250;

#[derive(Debug, Deserialize)]
struct SparkBuildResponse {
    prompt: String,
    #[serde(default)]
    session_id: Option<String>,
}

fn response_matches_session(response: &SparkBuildResponse, session_id: &str) -> bool {
    response.session_id.as_deref() == Some(session_id)
}

/// Fetch the per-session system prompt from spark. Filters spark
/// responses by `session_id` so concurrent session spawns don't steal
/// each other's prompt. Falls back to [`FALLBACK_PROMPT`] when Spark is
/// unavailable, but fails closed on a principal-attribution mismatch because
/// the receive has changed the active VFS invocation context.
pub(crate) fn fetch_prompt(
    principal_id: &str,
    session_id: &str,
    workspace_root: &str,
) -> Result<String, SysError> {
    let sub = ipc::subscribe(SPARK_RESPONSE_TOPIC)?;

    let request = serde_json::json!({
        "workspace_root": workspace_root,
        "session_id": session_id,
    });
    if let Err(e) = ipc::publish_json(SPARK_REQUEST_TOPIC, &request) {
        log::warn(format!("spark publish failed, using fallback: {e}"));
        publish_fallback_audit(principal_id, session_id, "publish_error");
        return Ok(FALLBACK_PROMPT.to_string());
    }

    let mut remaining = SPARK_DEADLINE_MS;
    while remaining > 0 {
        let step = remaining.min(SPARK_TICK_MS);
        match sub.recv(step) {
            Ok(result) => {
                for msg in result.messages {
                    if resolve_principal(principal_id, msg.principal.verified()).is_err() {
                        publish_fallback_audit(principal_id, session_id, "principal_mismatch");
                        return Err(SysError::ApiError(
                            "spark response principal did not match identity request".into(),
                        ));
                    }
                    let Ok(resp) = serde_json::from_str::<SparkBuildResponse>(&msg.payload) else {
                        continue;
                    };
                    // Spark publishes on a single fixed topic, so we demux by
                    // session_id. A response without the correlation id cannot
                    // be assigned when same-principal sessions fetch together.
                    let matches = response_matches_session(&resp, session_id);
                    if matches {
                        return Ok(resp.prompt);
                    }
                }
            }
            Err(_) => {
                // Timeout for this slice — loop and continue draining.
            }
        }
        remaining = remaining.saturating_sub(step);
    }

    publish_fallback_audit(principal_id, session_id, "timeout");
    Ok(FALLBACK_PROMPT.to_string())
}

fn publish_fallback_audit(principal_id: &str, session_id: &str, reason: &str) {
    let _ = ipc::publish_json(
        "claude.v1.audit.identity_fallback",
        &serde_json::json!({
            "principal_id": principal_id,
            "session_id": session_id,
            "reason": reason,
        }),
    );
}

/// Materialize the prompt atomically under
/// `home://.claude/.claude-identity-<sid>`. Writes to a temp sibling
/// then renames so a crash mid-write never leaves a half-formed file.
///
/// `home_path` is accepted for backwards source-shape compatibility
/// with the prior signature; it is intentionally ignored. The path is
/// hardcoded to the `home://` VFS scheme so the kernel binds it to the
/// invoking principal's home root at check time
/// (core/crates/astrid-kernel/src/lib.rs:75) — a caller-supplied
/// literal `~/...` would fall through to the workspace-root branch of
/// the resolver and land writes outside the principal home.
///
/// Defense-in-depth re-validates `session_id` so the identity-file
/// basename can never carry a `/` / `..` / NUL even if a future caller
/// forgets the gate that handle_spawn does at the IPC boundary.
pub(crate) fn write_prompt_file(
    _home_path: &str,
    session_id: &str,
    prompt: &str,
) -> Result<String, SysError> {
    crate::validate_id("session_id", session_id)?;
    // `home://` is bound by the kernel to the invoking principal's
    // home; the per-principal scope falls out of the scheme, not the
    // path string.
    let final_path = format!("home://.claude/.claude-identity-{session_id}");
    let tmp_path = format!("{final_path}.tmp");

    // Use the monotonic clock to make the temp path less collidable in
    // the (vanishingly rare) case of two concurrent writers sharing a
    // session_id. The atomic rename is the actual safety guarantee;
    // this is just buffer hygiene.
    let nonce = astrid_sdk::time::monotonic().as_nanos();
    let tmp_path = format!("{tmp_path}.{nonce}");

    // Append the live, host-reported Astrid capability envelope so the
    // agent's grounding reflects its real reach rather than a hard-coded
    // list that would drift from the manifest. Empty / host-unavailable →
    // nothing appended: this is additive grounding, never a spawn
    // precondition (`enumerate` returns an empty set rather than erroring).
    let body = format!("{prompt}{}", capability_context());
    fs::write(&tmp_path, body.as_bytes())?;
    fs::rename(&tmp_path, &final_path)?;
    let _ = Duration::from_secs(0); // discourage IDE drop-on-unused warning
    Ok(final_path)
}

/// Live Astrid capability context for the system prompt. Thin wrapper over
/// the host enumeration (`astrid:sys` enumerate-capabilities) so
/// [`format_capability_context`] stays host-call-free and unit-testable.
fn capability_context() -> String {
    format_capability_context(&astrid_sdk::capabilities::enumerate())
}

/// Format the capability-grounding section appended to the system prompt.
/// Lists the capsule's own capability classes (host-reported) so the
/// agent's grounding tracks its real envelope instead of a hard-coded list
/// that drifts from the manifest. Empty for an empty set — additive
/// grounding, never a spawn precondition.
fn format_capability_context(caps: &[String]) -> String {
    if caps.is_empty() {
        return String::new();
    }
    format!(
        "\n\n## AOS capabilities\n\nYour AOS-mediated reach this session \
         covers these capability classes: {}. The kernel enforces them; an \
         action outside them is denied regardless of any instruction.\n",
        caps.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_context_empty_for_no_caps() {
        assert_eq!(format_capability_context(&[]), "");
    }

    #[test]
    fn capability_context_lists_caps_and_frames_enforcement() {
        let s = format_capability_context(&["host_process".to_string(), "net".to_string()]);
        assert!(s.contains("host_process"));
        assert!(s.contains("net"));
        assert!(s.contains("## AOS capabilities"));
        assert!(s.contains("denied regardless"));
        // Leading blank lines so it appends cleanly after the prompt body.
        assert!(s.starts_with("\n\n"));
    }

    #[test]
    fn spark_response_requires_exact_session_correlation() {
        let missing = SparkBuildResponse {
            prompt: "prompt".into(),
            session_id: None,
        };
        let matching = SparkBuildResponse {
            prompt: "prompt".into(),
            session_id: Some("session-a".into()),
        };

        assert!(!response_matches_session(&missing, "session-a"));
        assert!(response_matches_session(&matching, "session-a"));
        assert!(!response_matches_session(&matching, "session-b"));
    }
}
