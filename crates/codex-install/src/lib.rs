//! codex-install — thin [`oracle_host::HostProvisioner`] for Codex.
//!
//! Install loop, markers, ids, and atomic writes live in `oracle-host`.
//! This crate only owns the `.codex/` file bodies.

#![deny(unsafe_code)]
#![deny(clippy::all)]
#![deny(unreachable_pub)]
#![warn(missing_docs)]

use astrid_sdk::prelude::*;
use oracle_core::Host;
use oracle_host::fs::write_atomic;
use oracle_host::ids::stamped_principal;
use oracle_host::{
    HostProvisioner, HostTopics, InstallRequest, PrincipalId, RelinkRequest, publish_complete,
    run_install, run_relink,
};

/// Artifact shape for managed `.codex/` files.
///
/// v4: SessionStart doctor prefers the Codex plugin `bin/aos-doctor` (update
/// clocks). The neutral runtime doctor has no `--format hook` equivalent.
const ARTIFACT_VERSION: u32 = 4;

struct CodexLayout;

fn base_config(principal: &PrincipalId) -> String {
    format!(
        r#"# Unicity AOS-managed Codex base config.
# Principal policy: `codex --profile aos` uses aos.config.toml

[features]
hooks = true

[mcp_servers.aos]
command = "aos"
args = ["--principal", "{}", "mcp", "serve"]
enabled = true
required = true
default_tools_approval_mode = "prompt"
startup_timeout_sec = 20
tool_timeout_sec = 600
"#,
        principal.as_str()
    )
}

impl HostProvisioner for CodexLayout {
    type Context = PrincipalId;
    const ARTIFACT_VERSION: u32 = ARTIFACT_VERSION;

    fn topics() -> HostTopics {
        HostTopics::for_host(Host::Codex)
    }

    fn ensure_dirs(_ctx: &Self::Context) -> Result<(), SysError> {
        fs::create_dir_all("home://.codex")?;
        fs::create_dir_all("home://.codex/hooks")?;
        Ok(())
    }

    fn write_files(principal: &Self::Context) -> Result<(), SysError> {
        write_atomic(
            "home://.codex/config.toml",
            base_config(principal).as_bytes(),
        )?;
        write_atomic(
            "home://.codex/aos.config.toml",
            br#"approval_policy = "on-request"
sandbox_mode = "workspace-write"
default_permissions = ":workspace"
model_reasoning_summary = "auto"
model_verbosity = "medium"

[features]
hooks = true
"#,
        )?;
        // SessionStart doctor: plugin path first (has runtime/plugin/distro
        // update clocks + host playbook). Fall back to no-op. Full policy hooks
        // live in the marketplace plugin (`plugins/unicity-aos/hooks/hooks.json`).
        write_atomic(
            "home://.codex/hooks.json",
            br#"{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "startup|resume|clear|compact",
        "hooks": [
          {
            "type": "command",
            "command": "root=\"${PLUGIN_ROOT:-${CODEX_PLUGIN_ROOT:-}}\"; if [ -x \"$root/bin/aos-doctor\" ]; then \"$root/bin/aos-doctor\" --format hook; else exit 0; fi",
            "timeout": 15,
            "statusMessage": "Checking Unicity AOS updates"
          }
        ]
      }
    ]
  }
}
"#,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_mcp_config_bakes_the_validated_principal() {
        let principal = PrincipalId::parse("codex-code").expect("valid principal");
        let config: toml::Value = toml::from_str(&base_config(&principal)).expect("valid TOML");
        let server = &config["mcp_servers"]["aos"];
        assert_eq!(server["command"].as_str(), Some("aos"));
        assert_eq!(
            server["args"]
                .as_array()
                .expect("args")
                .iter()
                .map(toml::Value::as_str)
                .collect::<Vec<_>>(),
            [
                Some("--principal"),
                Some("codex-code"),
                Some("mcp"),
                Some("serve"),
            ]
        );
    }
}

/// Codex install capsule.
#[derive(Default)]
pub struct CodexInstall;

#[capsule]
impl CodexInstall {
    /// `codex.v1.install.run`
    #[astrid::interceptor("handle_install")]
    pub fn handle_install(&self, req: InstallRequest) -> Result<(), SysError> {
        let principal = stamped_principal(&req.principal_id)?;
        match run_install::<CodexLayout>(&principal, req.force, &principal) {
            Ok(already) => publish_complete::<CodexLayout>(
                &principal,
                true,
                Some(already),
                None,
                "home://.codex",
            ),
            Err(err) => {
                publish_complete::<CodexLayout>(&principal, false, None, Some(err.to_string()), "")
            }
        }
    }

    /// `codex.v1.install.relink`
    #[astrid::interceptor("handle_relink")]
    pub fn handle_relink(&self, req: RelinkRequest) -> Result<(), SysError> {
        let principal = stamped_principal(&req.principal_id)?;
        match run_relink::<CodexLayout>(&principal, &principal) {
            Ok(()) => publish_complete::<CodexLayout>(
                &principal,
                true,
                Some(false),
                None,
                "home://.codex",
            ),
            Err(err) => {
                publish_complete::<CodexLayout>(&principal, false, None, Some(err.to_string()), "")
            }
        }
    }
}
