//! Per-principal Codex home provisioning handshake.

use astrid_sdk::prelude::*;
use oracle_host::ids::resolve_principal;
use serde_json::Value;
use std::time::Duration;

const INSTALL_RUN_TOPIC: &str = "codex.v1.install.run";
const INSTALL_COMPLETE_TOPIC: &str = "codex.v1.install.complete";
const INSTALL_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq)]
enum InstallResult {
    Success,
    Failure(String),
    Skip,
}

fn classify(payload: &str, principal_id: &str) -> InstallResult {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return InstallResult::Skip;
    };
    if value.get("principal_id").and_then(Value::as_str) != Some(principal_id) {
        return InstallResult::Skip;
    }
    if value.get("success").and_then(Value::as_bool) == Some(true) {
        return InstallResult::Success;
    }
    InstallResult::Failure(
        value
            .get("error")
            .and_then(Value::as_str)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("unknown")
            .to_string(),
    )
}

/// Provision the current stamped principal's Codex home before spawning Codex.
pub(crate) fn ensure(principal_id: &str) -> Result<(), String> {
    let subscription = ipc::subscribe(INSTALL_COMPLETE_TOPIC)
        .map_err(|error| format!("install_subscribe_failed: {error}"))?;
    ipc::publish_json(
        INSTALL_RUN_TOPIC,
        &serde_json::json!({
            "principal_id": principal_id,
            "force": false,
        }),
    )
    .map_err(|error| format!("install_publish_failed: {error}"))?;

    let mut remaining_ms = u64::try_from(INSTALL_DEADLINE.as_millis()).unwrap_or(30_000);
    while remaining_ms > 0 {
        let step = remaining_ms.min(2_000);
        if let Ok(batch) = subscription.recv(step) {
            for message in batch.messages {
                if resolve_principal(principal_id, message.principal.verified()).is_err() {
                    continue;
                }
                match classify(&message.payload, principal_id) {
                    InstallResult::Success => return Ok(()),
                    InstallResult::Failure(reason) => return Err(reason),
                    InstallResult::Skip => {}
                }
            }
        }
        remaining_ms = remaining_ms.saturating_sub(step);
    }

    Err(format!(
        "install_timeout: no {INSTALL_COMPLETE_TOPIC} for principal {principal_id} within 30s"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_classifier_is_principal_scoped_and_fail_closed() {
        assert_eq!(
            classify(r#"{"principal_id":"alice","success":true}"#, "alice"),
            InstallResult::Success
        );
        assert_eq!(
            classify(
                r#"{"principal_id":"alice","success":false,"error":"write failed"}"#,
                "alice"
            ),
            InstallResult::Failure("write failed".into())
        );
        assert_eq!(
            classify(r#"{"principal_id":"bob","success":true}"#, "alice"),
            InstallResult::Skip
        );
        assert_eq!(classify("not json", "alice"), InstallResult::Skip);
        assert_eq!(
            classify(r#"{"principal_id":"alice"}"#, "alice"),
            InstallResult::Failure("unknown".into())
        );
    }
}
