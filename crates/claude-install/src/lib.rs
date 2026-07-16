//! claude-install — Claude host provisioner on the shared [`HostProvisioner`] loop.
//!
//! Layout bodies (settings / mcp / CLAUDE.md) stay in this crate.
//! Marker, force, status steps live in `oracle-host`.

#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

mod claude_md;
mod config;
mod layout;
mod settings;

use astrid_sdk::prelude::*;
use oracle_core::Host;
use oracle_host::fs as atomic;
use oracle_host::ids::stamped_principal;
use oracle_host::{
    HostProvisioner, HostTopics, PrincipalId, publish_status as host_publish_status, run_install,
    run_relink,
};
use serde::{Deserialize, Serialize};

use crate::config::PrincipalConfig;
use crate::layout::{claude_dir, principal_home, projects_dir};

/// Install-time IPC payload (`claude.v1.install.run`).
#[derive(Debug, Clone, Deserialize)]
pub struct InstallRequest {
    /// Untrusted principal id.
    pub principal_id: String,
    /// Re-run even when the completion marker is set.
    #[serde(default)]
    pub force: bool,
    /// Per-principal interaction × auth shape; absent → default.
    #[serde(default)]
    pub config: Option<PrincipalConfig>,
}

/// Relink-time IPC payload (`claude.v1.install.relink`).
#[derive(Debug, Clone, Deserialize)]
pub struct RelinkRequest {
    /// Untrusted principal id.
    pub principal_id: String,
    /// Per-principal interaction × auth shape; absent → default.
    #[serde(default)]
    pub config: Option<PrincipalConfig>,
}

/// Terminal event on `claude.v1.install.complete` (includes config echo).
#[derive(Debug, Clone, Serialize)]
struct InstallComplete {
    principal_id: String,
    success: bool,
    home_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    already_installed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    config: Option<PrincipalConfig>,
}

/// Write context for [`ClaudeLayout`].
#[derive(Clone)]
struct ClaudeCtx {
    principal_id: String,
    config: PrincipalConfig,
}

/// Artifact shape of authored `.claude/` files.
const ARTIFACT_VERSION: u32 = 4;

struct ClaudeLayout;

impl HostProvisioner for ClaudeLayout {
    type Context = ClaudeCtx;
    const ARTIFACT_VERSION: u32 = ARTIFACT_VERSION;

    fn topics() -> HostTopics {
        HostTopics::for_host(Host::Claude)
    }

    fn home_path(_ctx: &Self::Context) -> String {
        principal_home()
    }

    fn config_digest(ctx: &Self::Context) -> Option<String> {
        let canonical = serde_json::to_vec(&ctx.config).ok()?;
        Some(format!("blake3:{}", blake3::hash(&canonical).to_hex()))
    }

    fn ensure_dirs(ctx: &Self::Context) -> Result<(), SysError> {
        let _ = host_publish_status::<Self>(
            &PrincipalId::parse(&ctx.principal_id)?,
            "mkdir",
            Some("creating .claude/ and projects/".into()),
        );
        fs::create_dir_all(&claude_dir())?;
        fs::create_dir_all(&projects_dir())?;
        // Scrub stale temp siblings from a prior crashed write.
        atomic::cleanup_temp(&layout::settings_path());
        atomic::cleanup_temp(&layout::managed_settings_path());
        atomic::cleanup_temp(&layout::mcp_path());
        atomic::cleanup_temp(&claude_md::claude_md_path());
        Ok(())
    }

    fn write_files(ctx: &Self::Context) -> Result<(), SysError> {
        let principal = PrincipalId::parse(&ctx.principal_id)?;
        let _ = host_publish_status::<Self>(
            &principal,
            "write_settings",
            Some("writing settings.local.json".into()),
        );
        settings::write_settings(&ctx.config)?;
        let _ = host_publish_status::<Self>(
            &principal,
            "write_managed",
            Some("writing managed-settings.json".into()),
        );
        settings::write_managed_settings(&ctx.config)?;
        let _ = host_publish_status::<Self>(
            &principal,
            "write_mcp",
            Some("writing .mcp.json (AOS MCP server)".into()),
        );
        settings::write_mcp(&ctx.config, &ctx.principal_id)?;
        let _ = host_publish_status::<Self>(
            &principal,
            "write_claude_md",
            Some("writing CLAUDE.md".into()),
        );
        settings::write_claude_md(&ctx.config)?;
        Ok(())
    }

    fn reconcile_stale(ctx: &Self::Context) -> Result<(), SysError> {
        Self::ensure_dirs(ctx)?;
        Self::write_files(ctx)
    }
}

/// Claude install capsule.
#[derive(Default)]
pub struct ClaudeInstall;

#[capsule]
impl ClaudeInstall {
    /// `claude.v1.install.run`
    #[astrid::interceptor("handle_install")]
    pub fn handle_install(&self, req: InstallRequest) -> Result<(), SysError> {
        let principal = match stamped_principal(&req.principal_id) {
            Ok(principal) => principal,
            Err(error) => {
                log::warn(format!(
                    "claude-install: rejected install with mismatched principal: {error}"
                ));
                return Ok(());
            }
        };
        let cfg = req.config.unwrap_or_else(|| {
            log::info(
                "claude-install: no config on InstallRequest, defaulting to {headless, api_key}",
            );
            PrincipalConfig::default()
        });
        if let Err(error) = cfg.validate() {
            publish_complete_local(&InstallComplete {
                principal_id: principal.to_string(),
                success: false,
                home_path: String::new(),
                already_installed: None,
                error: Some(error),
                config: None,
            });
            return Ok(());
        }
        let ctx = ClaudeCtx {
            principal_id: principal.to_string(),
            config: cfg,
        };
        match run_install::<ClaudeLayout>(&principal, req.force, &ctx) {
            Ok(already) => {
                publish_install_choices(principal.as_str(), &cfg, already);
                publish_complete_local(&InstallComplete {
                    principal_id: principal.to_string(),
                    success: true,
                    home_path: ClaudeLayout::home_path(&ctx),
                    already_installed: Some(already),
                    error: None,
                    config: Some(cfg),
                });
            }
            Err(e) => {
                log::error(format!("claude-install failed for {principal}: {e}"));
                atomic::cleanup_temp(&layout::settings_path());
                atomic::cleanup_temp(&layout::managed_settings_path());
                atomic::cleanup_temp(&layout::mcp_path());
                publish_complete_local(&InstallComplete {
                    principal_id: principal.to_string(),
                    success: false,
                    home_path: String::new(),
                    already_installed: None,
                    error: Some(e.to_string()),
                    config: None,
                });
            }
        }
        Ok(())
    }

    /// `claude.v1.install.relink`
    #[astrid::interceptor("handle_relink")]
    pub fn handle_relink(&self, req: RelinkRequest) -> Result<(), SysError> {
        let principal = match stamped_principal(&req.principal_id) {
            Ok(principal) => principal,
            Err(error) => {
                log::warn(format!(
                    "claude-install: rejected relink with mismatched principal: {error}"
                ));
                return Ok(());
            }
        };
        let cfg = req.config.unwrap_or_else(|| {
            log::info(
                "claude-install: no config on RelinkRequest, defaulting to {headless, api_key}",
            );
            PrincipalConfig::default()
        });
        if let Err(error) = cfg.validate() {
            publish_complete_local(&InstallComplete {
                principal_id: principal.to_string(),
                success: false,
                home_path: String::new(),
                already_installed: None,
                error: Some(error),
                config: None,
            });
            return Ok(());
        }
        let ctx = ClaudeCtx {
            principal_id: principal.to_string(),
            config: cfg,
        };
        // Relink always rewrites files; also ensure dirs exist.
        match ClaudeLayout::ensure_dirs(&ctx)
            .and_then(|_| run_relink::<ClaudeLayout>(&principal, &ctx))
        {
            Ok(()) => {
                let _ = ipc::publish_json(
                    "claude.v1.audit.settings_changed",
                    &serde_json::json!({
                        "principal_id": principal.as_str(),
                        "new_config": cfg,
                    }),
                );
                publish_complete_local(&InstallComplete {
                    principal_id: principal.to_string(),
                    success: true,
                    home_path: ClaudeLayout::home_path(&ctx),
                    already_installed: None,
                    error: None,
                    config: Some(cfg),
                });
            }
            Err(e) => {
                log::error(format!("claude-install relink failed for {principal}: {e}"));
                publish_complete_local(&InstallComplete {
                    principal_id: principal.to_string(),
                    success: false,
                    home_path: String::new(),
                    already_installed: None,
                    error: Some(e.to_string()),
                    config: None,
                });
            }
        }
        Ok(())
    }
}

fn publish_complete_local(event: &InstallComplete) {
    let topic = HostTopics::for_host(Host::Claude).install_complete();
    let _ = ipc::publish_json(&topic, event);
}

fn publish_install_choices(principal_id: &str, cfg: &PrincipalConfig, cache_hit: bool) {
    let _ = ipc::publish_json(
        "claude.v1.audit.install_choices",
        &serde_json::json!({
            "principal_id": principal_id,
            "config": cfg,
            "cache_hit": cache_hit,
        }),
    );
}

#[cfg(test)]
fn artifact_is_stale(marker_version: u32) -> bool {
    marker_version < ARTIFACT_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, InteractionMode};
    use oracle_host::InstallMarker;

    #[test]
    fn install_request_without_config_defaults_to_headless_api_key_v1() {
        let payload = r#"{"principal_id":"alice"}"#;
        let req: InstallRequest = serde_json::from_str(payload).expect("payload must deserialize");
        assert_eq!(req.principal_id, "alice");
        assert!(!req.force);
        assert!(req.config.is_none());
        let resolved = req.config.unwrap_or_default();
        assert_eq!(resolved.interaction_mode, InteractionMode::Headless);
        assert_eq!(resolved.auth_mode, AuthMode::ApiKey);
        assert_eq!(resolved.schema_version, PrincipalConfig::SCHEMA_VERSION);
    }

    #[test]
    fn relink_request_without_config_defaults_to_headless_api_key_v1() {
        let payload = r#"{"principal_id":"alice"}"#;
        let req: RelinkRequest = serde_json::from_str(payload).expect("payload must deserialize");
        assert!(req.config.is_none());
        let resolved = req.config.unwrap_or_default();
        assert_eq!(resolved.interaction_mode, InteractionMode::Headless);
        assert_eq!(resolved.auth_mode, AuthMode::ApiKey);
    }

    #[test]
    fn install_request_with_config_round_trips() {
        let payload = r#"{
            "principal_id":"alice",
            "force":true,
            "config":{
                "interaction_mode":"repl",
                "auth_mode":"subscription",
                "schema_version":1
            }
        }"#;
        let req: InstallRequest = serde_json::from_str(payload).expect("payload must deserialize");
        assert!(req.force);
        let cfg = req.config.expect("config must be Some");
        assert_eq!(cfg.interaction_mode, InteractionMode::Repl);
        assert_eq!(cfg.auth_mode, AuthMode::Subscription);
    }

    #[test]
    fn install_complete_failure_omits_config_field() {
        let event = InstallComplete {
            principal_id: "alice".into(),
            success: false,
            home_path: String::new(),
            already_installed: None,
            error: Some("boom".into()),
            config: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("\"config\""));
    }

    #[test]
    fn install_complete_success_carries_config_echo() {
        let event = InstallComplete {
            principal_id: "alice".into(),
            success: true,
            home_path: "/home/alice".into(),
            already_installed: None,
            error: None,
            config: Some(PrincipalConfig::default()),
        };
        let v: serde_json::Value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            v.pointer("/config/interaction_mode")
                .and_then(|x| x.as_str()),
            Some("headless")
        );
    }

    #[test]
    fn legacy_marker_defaults_artifact_version_to_zero_and_is_stale() {
        let payload = r#"{"installed_at":1,"version":"0.1.0","home_path":"/home/alice"}"#;
        let marker: InstallMarker =
            serde_json::from_str(payload).expect("legacy marker must deserialize");
        assert_eq!(marker.artifact_version, 0);
        assert!(artifact_is_stale(marker.artifact_version));
    }

    #[test]
    fn current_and_future_artifact_versions_are_not_stale() {
        assert!(!artifact_is_stale(ARTIFACT_VERSION));
        assert!(!artifact_is_stale(ARTIFACT_VERSION + 1));
    }

    #[test]
    fn fresh_marker_round_trips_current_artifact_version() {
        let marker = InstallMarker {
            installed_at: 1,
            version: env!("CARGO_PKG_VERSION").to_string(),
            home_path: "/home/alice".into(),
            artifact_version: ARTIFACT_VERSION,
            config_digest: ClaudeLayout::config_digest(&ClaudeCtx {
                principal_id: "alice".into(),
                config: PrincipalConfig::default(),
            }),
        };
        let v: serde_json::Value = serde_json::to_value(&marker).unwrap();
        assert_eq!(
            v.get("artifact_version")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(ARTIFACT_VERSION))
        );
        assert!(
            v.get("config_digest")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|digest| digest.starts_with("blake3:"))
        );
    }

    #[test]
    fn config_digest_is_stable_and_changes_with_auth_projection() {
        let default_ctx = ClaudeCtx {
            principal_id: "alice".into(),
            config: PrincipalConfig::default(),
        };
        let mut changed = default_ctx.clone();
        changed.config.interaction_mode = InteractionMode::Repl;
        changed.config.auth_mode = AuthMode::Subscription;

        assert_eq!(
            ClaudeLayout::config_digest(&default_ctx),
            ClaudeLayout::config_digest(&default_ctx)
        );
        assert_ne!(
            ClaudeLayout::config_digest(&default_ctx),
            ClaudeLayout::config_digest(&changed)
        );
    }
}
