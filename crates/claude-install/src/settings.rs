//! Concrete writers for `.claude/settings.local.json` and
//! `.claude/.mcp.json`. Both go through [`oracle_host::fs::write_atomic`]
//! so a crashed install never leaves a half-written config file.

use astrid_sdk::prelude::*;
use oracle_host::fs as atomic;

use crate::{claude_md, config::PrincipalConfig, layout};

/// Write the `.claude/settings.local.json` for the invoking principal,
/// shaped by `cfg` (interaction × auth axes — see
/// [`layout::settings_json`]). Assumes `.claude/` already exists (the
/// install handler creates it before calling here). The path resolves
/// through `home://`, which the kernel scopes to the per-invocation
/// principal — no principal_id appears in the path.
pub(crate) fn write_settings(cfg: &PrincipalConfig) -> Result<(), SysError> {
    let path = layout::settings_path();
    let body = serde_json::to_vec_pretty(&layout::settings_json(cfg))?;
    atomic::write_atomic(&path, &body)
}

/// Write the `.claude/.mcp.json` for the invoking principal, shaped by
/// `cfg`. It registers the `aos` MCP server (`aos --principal
/// <principal_id> mcp serve`) that Claude loads and discovers
/// `mcp__aos__*` from. `principal_id` is baked into the server argv so
/// broker requests are always scoped to the intended identity.
pub(crate) fn write_mcp(cfg: &PrincipalConfig, principal_id: &str) -> Result<(), SysError> {
    let path = layout::mcp_path();
    let value = match cfg.interaction_mode {
        crate::config::InteractionMode::Headless => layout::mcp_json(cfg, principal_id),
        crate::config::InteractionMode::Repl => {
            let existing = if fs::exists(&path)? {
                serde_json::from_str(&fs::read_to_string(&path)?).map_err(|error| {
                    SysError::ApiError(format!("invalid existing .claude/.mcp.json: {error}"))
                })?
            } else {
                serde_json::json!({})
            };
            layout::merge_repl_mcp(existing, principal_id)?
        }
    };
    let body = serde_json::to_vec_pretty(&value)?;
    atomic::write_atomic(&path, &body)
}

/// Write the staged `.claude/managed-settings.json` for the invoking
/// principal — the MANAGED-tier body ([`layout::managed_settings_json`]).
///
/// Claude does NOT read managed settings from this path; it reads them from
/// the OS system path this WASM capsule cannot write. This file is the source
/// the host bind-mounts into that system path (core #881), inert until then.
/// It carries the un-strippable enforcement posture — the policy gate hook
/// plus the permission / sandbox / auth lockdown. Atomic like the others, so
/// a crashed install never leaves a half-written managed body.
pub(crate) fn write_managed_settings(cfg: &PrincipalConfig) -> Result<(), SysError> {
    let path = layout::managed_settings_path();
    let body = serde_json::to_vec_pretty(&layout::managed_settings_json(cfg))?;
    atomic::write_atomic(&path, &body)
}

/// Write the `.claude/CLAUDE.md` for the invoking principal — the
/// standalone AOS grounding (what Unicity AOS is and the role it runs
/// the agent in), branched on interaction mode. User-tier memory Claude
/// loads every session; authored as plain UTF-8 markdown through
/// [`oracle_host::fs::write_atomic`].
pub(crate) fn write_claude_md(cfg: &PrincipalConfig) -> Result<(), SysError> {
    let path = claude_md::claude_md_path();
    atomic::write_atomic(&path, claude_md::claude_md(cfg).as_bytes())
}
