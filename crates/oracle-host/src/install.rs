//! Shared install / relink protocol for host provisioners.

use astrid_sdk::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::UNIX_EPOCH;

use crate::ids::PrincipalId;
use crate::topics::HostTopics;

/// Install request payload (host-agnostic wire shape).
///
/// Host-specific fields (e.g. Claude `PrincipalConfig`) ride on the
/// capsule's own request type; this is the shared minimum.
#[derive(Debug, Clone, Deserialize)]
pub struct InstallRequest {
    /// Principal to provision.
    pub principal_id: String,
    /// Reconcile even when already installed.
    #[serde(default)]
    pub force: bool,
}

/// Relink request payload.
#[derive(Debug, Clone, Deserialize)]
pub struct RelinkRequest {
    /// Principal to relink.
    pub principal_id: String,
}

/// KV install marker.
#[derive(Debug, Serialize, Deserialize)]
pub struct InstallMarker {
    /// Provisioner crate version string.
    pub version: String,
    /// Unix millis when written.
    pub installed_at: u64,
    /// Artifact shape version (bump to force reconcile).
    #[serde(default)]
    pub artifact_version: u32,
    /// Optional home path echo (Claude install records this).
    #[serde(default)]
    pub home_path: String,
    /// Provisioner-defined digest of inputs that determine authored files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_digest: Option<String>,
}

/// Progress event on `{host}.v1.install.status`.
#[derive(Debug, Serialize)]
pub struct InstallStatus {
    /// Principal being provisioned.
    pub principal_id: String,
    /// Step name.
    pub step: &'static str,
    /// Optional human message (Claude install uses this).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Terminal event on `{host}.v1.install.complete` — shared fields only.
///
/// Hosts may wrap/extend when publishing (Claude adds `config`, `home_path`).
#[derive(Debug, Serialize)]
pub struct InstallComplete {
    /// Principal.
    pub principal_id: String,
    /// Overall success.
    pub success: bool,
    /// Cache-hit short-circuit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub already_installed: Option<bool>,
    /// Error text on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Artifact version written on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_version: Option<u32>,
    /// Home path when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub home_path: String,
}

/// Host-specific filesystem layout for provisioning.
///
/// The install **loop** (marker, force, status) is shared.
/// Only what gets written under `home://` differs per host.
pub trait HostProvisioner {
    /// Host-specific write context (Claude config, or `()`).
    type Context;

    /// Artifact shape version. Bump when on-disk files change form.
    const ARTIFACT_VERSION: u32;

    /// Host topic/namespace helpers.
    fn topics() -> HostTopics;

    /// Principal home path for status/complete (`home://` or richer).
    fn home_path(_ctx: &Self::Context) -> String {
        "home://".into()
    }

    /// Stable digest of context fields that affect authored files.
    ///
    /// Provisioners without config-bearing files keep the default `None`.
    /// A legacy marker without a digest is stale when a provisioner now
    /// returns `Some`, repairing the projection once after upgrade.
    fn config_digest(_ctx: &Self::Context) -> Option<String> {
        None
    }

    /// Ensure host home dirs exist.
    fn ensure_dirs(ctx: &Self::Context) -> Result<(), SysError>;

    /// Write or rewrite all managed host config files.
    fn write_files(ctx: &Self::Context) -> Result<(), SysError>;

    /// Reconcile when marker is present but artifact is stale.
    fn reconcile_stale(ctx: &Self::Context) -> Result<(), SysError> {
        Self::write_files(ctx)
    }
}

/// Run the shared install loop for provisioner `P`.
///
/// Returns `Ok(true)` when short-circuited as already installed.
pub fn run_install<P: HostProvisioner>(
    principal: &PrincipalId,
    force: bool,
    ctx: &P::Context,
) -> Result<bool, SysError> {
    let topics = P::topics();
    let key = marker_key(&topics, principal);
    let home = P::home_path(ctx);
    let config_digest = P::config_digest(ctx);

    if !force && let Some(marker) = kv::get_json_opt::<InstallMarker>(&key)? {
        if marker_is_current(&marker, P::ARTIFACT_VERSION, &config_digest) {
            return Ok(true);
        }
        let step = if marker.artifact_version < P::ARTIFACT_VERSION {
            "reconcile_stale_artifacts"
        } else {
            "reconcile_changed_config"
        };
        publish_status::<P>(principal, step, None)?;
        P::reconcile_stale(ctx)?;
        write_marker::<P>(&key, &home, config_digest)?;
        return Ok(true);
    }

    publish_status::<P>(principal, "create_dirs", None)?;
    P::ensure_dirs(ctx)?;

    publish_status::<P>(principal, "write_config", None)?;
    P::write_files(ctx)?;

    write_marker::<P>(&key, &home, config_digest)?;
    Ok(false)
}

/// Rewrite host files without touching the install marker.
pub fn run_relink<P: HostProvisioner>(
    _principal: &PrincipalId,
    ctx: &P::Context,
) -> Result<(), SysError> {
    P::write_files(ctx)
}

/// Publish the shared install.complete shape.
pub fn publish_complete<P: HostProvisioner>(
    principal: &PrincipalId,
    success: bool,
    already_installed: Option<bool>,
    error: Option<String>,
    home_path: impl Into<String>,
) -> Result<(), SysError> {
    let topic = P::topics().install_complete();
    ipc::publish_json(
        &topic,
        &InstallComplete {
            principal_id: principal.to_string(),
            success,
            already_installed,
            error,
            artifact_version: success.then_some(P::ARTIFACT_VERSION),
            home_path: home_path.into(),
        },
    )
}

/// Publish install.status.
pub fn publish_status<P: HostProvisioner>(
    principal: &PrincipalId,
    step: &'static str,
    message: Option<String>,
) -> Result<(), SysError> {
    let topic = P::topics().install_status();
    ipc::publish_json(
        &topic,
        &InstallStatus {
            principal_id: principal.to_string(),
            step,
            message,
        },
    )
}

fn marker_key(topics: &HostTopics, principal: &PrincipalId) -> String {
    format!("{}.{}", topics.install_marker_prefix(), principal.as_str())
}

fn marker_is_current(
    marker: &InstallMarker,
    artifact_version: u32,
    config_digest: &Option<String>,
) -> bool {
    marker.artifact_version >= artifact_version && &marker.config_digest == config_digest
}

fn write_marker<P: HostProvisioner>(
    key: &str,
    home_path: &str,
    config_digest: Option<String>,
) -> Result<(), SysError> {
    kv::set_json(
        key,
        &InstallMarker {
            version: env!("CARGO_PKG_VERSION").to_string(),
            installed_at: time::now()?
                .duration_since(UNIX_EPOCH)
                .map_err(|err| SysError::ApiError(err.to_string()))?
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            artifact_version: P::ARTIFACT_VERSION,
            home_path: home_path.to_string(),
            config_digest,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(config_digest: Option<&str>) -> InstallMarker {
        InstallMarker {
            version: "0.1.0".into(),
            installed_at: 1,
            artifact_version: 4,
            home_path: "home://".into(),
            config_digest: config_digest.map(str::to_string),
        }
    }

    #[test]
    fn current_marker_requires_the_exact_config_digest() {
        let expected = Some("blake3:new".to_string());
        assert!(marker_is_current(&marker(Some("blake3:new")), 4, &expected));
        assert!(!marker_is_current(
            &marker(Some("blake3:old")),
            4,
            &expected
        ));
        assert!(!marker_is_current(&marker(None), 4, &expected));
    }

    #[test]
    fn configless_provisioner_keeps_legacy_cache_semantics() {
        assert!(marker_is_current(&marker(None), 4, &None));
        assert!(!marker_is_current(&marker(None), 5, &None));
    }
}
