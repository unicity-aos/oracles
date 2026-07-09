//! `.claude/CLAUDE.md` authoring — standalone Astrid grounding the runner
//! writes for a managed Claude agent.
//!
//! The distribution does not have to ship the `spark` identity capsule,
//! so this file is the baseline grounding on its own: what Astrid OS is
//! and the role it runs the agent in. It is background, not behavioural
//! rules (the reader is a capable model) and not a tool catalogue (the
//! `mcp__astrid__*` tools describe themselves). It claims no filesystem
//! isolation or sandbox — only the capability mediation the kernel
//! actually enforces — so it cannot over-promise a guarantee the runtime
//! does not yet make.
//!
//! Content is principal-agnostic and depends only on the interaction
//! mode, so the same fleet grounding lands for every principal of a
//! given mode. The companion managed-tier `claudeMd` (un-overridable,
//! fleet-wide) rides with the managed-settings system-path work and is
//! not authored here yet.

use crate::config::{InteractionMode, PrincipalConfig};

/// Path to the per-principal user-tier `.claude/CLAUDE.md` Claude loads
/// every session.
pub(crate) fn claude_md_path() -> String {
    "home://.claude/CLAUDE.md".to_string()
}

/// Grounding body for a headless managed agent.
const HEADLESS: &str = "# Astrid OS

You are Claude, running as a managed agent inside Astrid OS: a secure, capability-based agent runtime. Astrid started you to do work for one principal, and mediates everything you reach on the system through capabilities granted to that principal.

Those capabilities are exposed to you as the `mcp__astrid__*` tools, which describe themselves. They are your interface to the system, and the Astrid kernel decides what each one may do.

You are running headless: Astrid drives this session rather than a person typing to you.

Astrid regenerates this file on install, so edits here are not durable.
";

/// Grounding body for an interactive (repl) managed agent. Adds the
/// honest note that the agent's own built-in tools run with the
/// operator's ordinary authority and are not Astrid-mediated.
const REPL: &str = "# Astrid OS

You are Claude, running as a managed agent inside Astrid OS: a secure, capability-based agent runtime, working with the principal at the keyboard. Astrid mediates what you reach through capabilities granted to that principal, exposed to you as the `mcp__astrid__*` tools, which describe themselves and are gated by the Astrid kernel. Your own built-in tools also work here, with the operator's ordinary authority, and are not mediated by Astrid.

Astrid regenerates this file on install, so edits here are not durable.
";

/// Select the `CLAUDE.md` body for the principal's interaction mode.
pub(crate) fn claude_md(cfg: &PrincipalConfig) -> &'static str {
    match cfg.interaction_mode {
        InteractionMode::Headless => HEADLESS,
        InteractionMode::Repl => REPL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(mode: InteractionMode) -> PrincipalConfig {
        PrincipalConfig {
            interaction_mode: mode,
            ..PrincipalConfig::default()
        }
    }

    #[test]
    fn path_lives_under_home_scheme() {
        assert!(claude_md_path().starts_with("home://"));
        assert!(claude_md_path().ends_with("/CLAUDE.md"));
    }

    #[test]
    fn both_modes_ground_the_agent() {
        let headless = claude_md(&cfg(InteractionMode::Headless));
        let repl = claude_md(&cfg(InteractionMode::Repl));
        for body in [headless, repl] {
            assert!(body.starts_with("# Astrid OS"));
            assert!(body.contains("mcp__astrid__"));
            assert!(body.contains("capability"));
            assert!(body.contains("not durable"));
        }
    }

    #[test]
    fn headless_states_programmatic_drive() {
        assert!(claude_md(&cfg(InteractionMode::Headless)).contains("headless"));
    }

    #[test]
    fn repl_flags_unmediated_builtins() {
        // The repl body must be explicit that built-in tools are not
        // Astrid-mediated — the one safety-relevant delta from headless.
        let body = claude_md(&cfg(InteractionMode::Repl));
        assert!(body.contains("built-in"));
        assert!(body.contains("not mediated by Astrid"));
    }

    #[test]
    fn claims_no_isolation_guarantee() {
        // Guard against a future edit over-claiming a sandbox or
        // cross-principal isolation the runtime does not enforce.
        for body in [HEADLESS, REPL] {
            let lower = body.to_lowercase();
            assert!(!lower.contains("sandbox"));
            assert!(!lower.contains("isolat"));
        }
    }
}
