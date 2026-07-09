//! Building the `claude -p` subprocess.
//!
//! Centralises the hardened argv + env we feed [`process::Command`].
//! Every flag here matters: dropping `--strict-mcp-config` smuggles in
//! `.mcp.json` from the redirected HOME; dropping `--permission-mode
//! dontAsk` lets a tool prompt (and hang the terminal-less `-p` session)
//! instead of fail-secure auto-denying; dropping `--sandbox` removes
//! Claude's inner bound on its native tools (the Astrid host sandbox still
//! binds, but defense-in-depth is lost); etc.
//!
//! Native-tools model: Claude uses its OWN tools (Bash/Read/Write/Edit/…),
//! sandboxed — NOT Astrid capsule-tool replacements. The escape /
//! orchestration surface stays denied; the dev tools are allow-listed.
//!
//! Enforcement lives in the argv on purpose. Claude's tier precedence is
//! `Managed > CLI args > Local (settings.local.json) > Project > User`.
//! CLI args are the only session-un-overridable tier reachable from
//! capsule-space — sage owns the full argv and execs `claude` directly,
//! so anything expressed as a flag here sits above every on-disk file
//! tier and a running session cannot edit it away. Two flags carry that
//! weight: `--disallowedTools` hoists the deny list out of the Local
//! tier (`settings.local.json`, fully session-overridable) into the
//! binding CLI tier, and `--setting-sources local` narrows on-disk tier
//! loading to just the runner's own authored file so a stray `user`/`project`
//! settings file cannot dilute the posture. Managed and CLI flags always
//! load regardless of `--setting-sources`, so the narrowing only shrinks
//! the overridable surface; it never weakens the managed tier.

use astrid_sdk::prelude::*;
use sha2::{Digest, Sha256};
use std::time::Duration;

const CLAUDE_BIN: &str = "claude";

// ---- Persistent-tier spawn knobs --------------------------------------
//
// `claude -p` runs on the PERSISTENT process tier so it outlives a sage
// capsule reload; the supervisor re-`attach`es by ProcessId rather than
// abandoning the conversation. These tune the host-side child lifecycle
// (see `astrid_sdk::process::Command`).

/// Idle-reap backstop: a persistent child with NO read/write/wait/signal
/// for this long is reaped by the host. A SUPERVISED session never idles —
/// the supervisor reads its stdout every ~50 ms (see
/// [`crate::supervisor::TICK_INTERVAL`]) — so this only catches a child
/// sage permanently ABANDONED (e.g. the capsule fails to reload and never
/// re-attaches). Set far above any plausible reload gap (seconds) so a
/// healthy child is never reaped mid-reload. NO `max_lifetime` is set: a
/// legitimate agent task can run arbitrarily long and a wall-clock ceiling
/// would SIGKILL a healthy session.
const IDLE_REAP: Duration = Duration::from_secs(30 * 60);

/// Post-exit retention: after `claude` exits the host keeps the id + drained
/// log tail this long before auto-reaping. Sized to outlast a reload so a
/// child that exits DURING the reload gap is still observable (exit code +
/// tail) when the reconcile attaches — letting sage publish a truthful
/// `exited` instead of a vague `lost`.
const EXIT_RETENTION: Duration = Duration::from_secs(5 * 60);

/// Per-stream stdout/stderr ring capacity. Sized to comfortably hold the
/// stdout a streaming `claude -p` emits across a reload gap so the reconcile
/// loses no buffered output (host-clamped to the profile ceiling).
const LOG_RING_BYTES: u32 = 4 * 1024 * 1024;

/// Built-in Claude tools denied at the CLI-args tier.
///
/// Claude's tier precedence is `Managed > CLI args > Local
/// (settings.local.json) > Project (settings.json) > User
/// (~/.claude/settings.json)`. CLI args are the only
/// session-un-overridable tier reachable from capsule-space: sage owns
/// the full argv and execs `claude` directly, so a deny passed here as
/// `--disallowedTools` sits above every on-disk file tier and a session
/// cannot edit it away mid-run. The same list also rides in
/// `settings.local.json` (claude-install's `REQUIRED_DENIES`) as
/// defense-in-depth, but that file is the *Local* tier — Claude's
/// weakest, fully session-overridable scope — so the file copy alone is
/// not binding. Hoisting it into argv is the genuine enforceable upgrade.
///
/// A bare tool name (no scope qualifier) removes the tool from the
/// model's context entirely. The rule is exhaustive by design: any
/// built-in that can act (file/shell/web/task), surface other tools, or
/// drive control flow outside the `mcp__astrid__*` surface is denied so a
/// session cannot reach around the sandbox via a tool the list forgot.
///
/// SYNC: this list MUST be identical (same set) to
/// `sage_install::layout::REQUIRED_DENIES` (claude-install/src/layout.rs).
/// The two crates have no dependency edge (each claude-runner crate is a
/// `cdylib`-only workspace member, so one cannot import the other's const
/// as a library without adding `rlib` to the producer's crate-type — a
/// build-profile change out of scope), so the list is mirrored here by
/// hand — same arrangement as the `HOOK_TOPIC_MAP` mirror. The mirror is
/// load-bearing, not cosmetic: `REQUIRED_DENIES` rides only the fully
/// session-overridable Local tier (`settings.local.json`), so a tool
/// denied there but ABSENT here is not actually blocked — a capable
/// session edits it out of the Local file and calls it. Only the names
/// that appear in THIS list reach the binding `--disallowedTools` CLI
/// tier. Any edit to either list must mirror to the other; the
/// [`denied_tools_equal_canonical_required_denies`] test pins this copy
/// against an independently-written canonical set (full membership, not
/// just count) so a name added to or dropped from either side fails the
/// build, and the per-name presence on the Local-tier copy is asserted in
/// claude-install's `assert_headless_shape`.
const DENIED_TOOLS: &[&str] = &[
    // ESCAPE / ORCHESTRATION surface — denied even under the native-tools
    // model. These are NOT the dev tools (Bash/Read/Write/Edit/Glob/Grep/
    // Web* are now ALLOWED, sandboxed); they are the surfaces that either
    // spawn other agents, drive Astrid's own control plane, reach off-host
    // channels, or load tools around the gated surface — each a distinct
    // governance axis that does not belong to a single supervised session.
    //
    // PowerShell: a second shell on its own tool name; the native shell is
    // `Bash`, so the redundant shell surface stays closed.
    "PowerShell",
    // Sub-agent spawn / teaming — a spawned agent runs its OWN tool calls
    // and is a separate principal-scoping question (cap-scoped child
    // principals are tracked separately). `Task` is the legacy `Agent`
    // alias.
    "Agent",
    "Task",
    "Workflow",
    "SendMessage",
    // Agent-roster + cross-agent messaging (current tool names + the
    // `Brief`/`ListPeers` aliases fold into these).
    "SendUserMessage",
    "ListAgents",
    "TeamCreate",
    "TeamDelete",
    // Scheduling / control flow — queue a future prompt, reschedule a
    // loop, or drive plan-mode / worktree transitions outside the runner's run
    // loop. sage owns session lifecycle.
    "CronCreate",
    "CronDelete",
    "CronList",
    "ScheduleWakeup",
    "EnterPlanMode",
    "ExitPlanMode",
    "EnterWorktree",
    "ExitWorktree",
    // Astrid's OWN task surface — these are ASTRID's tools, not Claude's;
    // the supervised session must not drive Astrid's control plane (mapping
    // Claude background tasks onto Astrid Tasks is a separate design).
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskStop",
    "TaskUpdate",
    "TaskOutput",
    // Network egress — `WebFetch`/`WebSearch` are model-driven HTTP tools
    // that do NOT traverse Claude's Bash sandbox, so the filesystem sandbox
    // does not bound them: read-a-secret + POST-it-out is the one exfil
    // path the sandbox misses. Egress is OFF by default (binding) until a
    // controlled allow-list lands (host net policy / managed-tier domain
    // allowlist). Re-enable deliberately, not by default.
    "WebFetch",
    "WebSearch",
    // External / exfiltration surfaces — off-host channels sage does not
    // mediate.
    "PushNotification",
    "RemoteTrigger",
    "ShareOnboardingGuide",
    // MCP resource + tool-loading surfaces: reading raw MCP resources or
    // loading deferred tools is a way to reach a tool around the gated
    // surface.
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "ToolSearch",
    "WaitForMcpServers",
    // Indirect-execution surface: a skill can fan out to other tools.
    // Deferred to a later slice (skills-from-capsules) so the indirect path
    // is governed before it opens. `SlashCommand` is NOT a live tool/alias
    // in current builds (the slash surface is reached via `Skill`) — kept as
    // a harmless forward/back-compat no-op.
    "Skill",
    "SlashCommand",
];

/// Inputs for [`spawn_claude`].
pub(crate) struct SpawnInputs<'a> {
    /// Principal that owns this session. Threaded into the child env as
    /// `ASTRID_PRINCIPAL_ID` so the `astrid-emit` hook helper can stamp
    /// it on outgoing `claude.v1.hook.*` events. Sage's run
    /// loop re-verifies the claim against the per-(principal, session)
    /// hook token before republishing on the canonical `hook.v1.event.*`
    /// topic; the env value itself is untrusted (the child process may
    /// have leaked or rewritten it).
    pub principal_id: &'a str,
    pub session_id: &'a str,
    pub home_path: &'a str,
    pub identity_path: &'a str,
    /// Anthropic API key — when `Some`, threaded into the child env as
    /// `ANTHROPIC_API_KEY`. When `None`, the env var is OMITTED entirely
    /// so Claude falls back to its keychain OAuth credential path
    /// (subscription / `claude /login` mode). Note: argv is identical in
    /// both auth modes, so [`argv_hash`] is stable across modes for the
    /// same `session_id` — only the spawn env differs. That preserves
    /// the audit-stability invariant: a downstream consumer correlating
    /// by `flags_hash` will see the same fingerprint regardless of how
    /// the subprocess authenticated.
    pub api_key: Option<&'a str>,
    /// Per-(principal, session) random token, minted by sage at spawn
    /// time and persisted in KV under
    /// `claude.hook_token.<principal>.<session>`. Threaded into the child
    /// env as `ASTRID_HOOK_TOKEN`; `astrid-emit` echoes it back in the
    /// hook envelope so sage can distinguish authentic hook fires from
    /// forged `claude.v1.hook.*` publishes. Always present
    /// (no Option) — every spawn gets a token. Must NEVER appear in
    /// argv: it would land in process listings / audit logs and the
    /// `flags_hash` audit-stability invariant
    /// ([`argv_hash_unchanged_across_auth_modes`]) requires argv stay
    /// independent of per-spawn secrets.
    pub hook_token: &'a str,
    /// Model tier to run claude under (Astrid per-principal governance).
    /// `Default` omits `--model`; the rest pass the CLI alias. Argv-
    /// visible (and so in the `flags_hash`) by design — governance should
    /// be auditable, unlike the per-spawn secret in `hook_token`.
    pub model: crate::config::ModelPreference,
    /// Optional per-session agentic-turn cap (Astrid governance).
    /// `Some(n)` adds `--max-turns n`; `None` omits it. Argv-visible.
    pub max_turns: Option<u32>,
}

/// Outcome of a successful spawn — the live persistent process plus the
/// audit fingerprint of the argv we used so the spawn event can record it.
pub(crate) struct Spawned {
    pub process: process::PersistentProcess,
    /// The host-owned persistent id (`== process.id()`), captured here so
    /// the caller can persist it into the [`SessionRecord`] before the
    /// handle is moved into the runtime map. This is what a later capsule
    /// incarnation re-`attach`es to across a reload.
    pub process_id: String,
    pub os_pid: u32,
    /// Deterministic SHA-256 digest of the full argv, formatted as
    /// `"sha256:<hex>"`. Computed by [`argv_hash`] with a NUL byte
    /// separator between arguments so distinct argv lists (e.g.
    /// `["a", "bc"]` vs `["ab", "c"]`) cannot collide. Reproducible
    /// across capsule reloads and across the fleet — audit consumers
    /// can use it as a stable fingerprint to cross-reference spawns.
    pub flags_hash: String,
}

/// Build the hardened argv for `claude -p`.
fn argv(
    session_id: &str,
    identity_path: &str,
    model: crate::config::ModelPreference,
    max_turns: Option<u32>,
) -> Vec<String> {
    let mut args = vec![
        "-p".to_string(),
        "--input-format".to_string(),
        "stream-json".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        // Register EXACTLY the runner's own MCP server and nothing else.
        // `--strict-mcp-config` makes claude ignore every auto-discovered
        // `.mcp.json`; `--mcp-config` then points it at the single file
        // claude-install authored under the principal's HOME, whose `sage`
        // server is `astrid mcp serve` (the rmcp stdio shim onto the
        // astrid-mcp broker — unicity-astrid/astrid#880). claude does the
        // native MCP handshake against it and discovers the `mcp__astrid__*`
        // tools from `tools/list`, then executes them DIRECTLY against that
        // server over MCP — sage never sees or dispatches the calls. The
        // path is cwd-relative (cwd = HOME, set below) so the argv stays
        // byte-identical across principals — the per-principal identity
        // rides inside the file's argv, not here, to keep the `flags_hash`
        // fingerprint stable.
        "--strict-mcp-config".to_string(),
        "--mcp-config".to_string(),
        ".claude/.mcp.json".to_string(),
        // Constrain which on-disk setting tiers load to `local` only —
        // the scope of the `settings.local.json` claude-install authors
        // (`local` = `.claude/settings.local.json`, which under the HOME
        // redirect resolves to the runner's authored file). This excludes the
        // `user` (`~/.claude/settings.json`) and `project`
        // (`.claude/settings.json`) tiers so a stray file in either —
        // dropped into the redirected HOME or the project dir — cannot
        // dilute the runner's posture by injecting allow rules or flipping a
        // permission mode. Managed (system/MDM) and CLI flags always
        // load regardless of this list, so this only NARROWS the
        // overridable file surface; it cannot weaken the managed tier.
        // the runner's hooks block has no CLI-flag form, so `local` must stay
        // in the list for the `astrid-emit` hook wiring to load.
        "--setting-sources".to_string(),
        "local".to_string(),
        // Native-tools model: Claude uses its OWN tools (Bash / Read /
        // Write / Edit / Glob / Grep / Web*), NOT Astrid capsule-tool
        // replacements. They run under TWO sandboxes:
        //   * Astrid's HOST sandbox (bwrap/seatbelt) — applied by the host
        //     to this persistent-tier spawn unconditionally. The BINDING
        //     floor: bounds writes to the principal home + the host net
        //     policy, regardless of anything below.
        //   * Claude's own `--sandbox` — a best-effort INNER layer. If it
        //     cannot initialise nested inside the host sandbox the host
        //     sandbox still binds, so this never weakens the floor.
        // `--permission-mode dontAsk` is fail-secure in headless `-p`: a
        // tool not matched by a `permissions.allow` rule is auto-DENIED,
        // never prompted (a prompt would hang — no terminal). The
        // `permissions.allow` list + sandbox config are authored in
        // settings; the ESCAPE / orchestration surface (sub-agent spawn,
        // scheduling, Astrid's own task/cron tools, raw MCP resource reads)
        // stays denied HERE in the binding CLI-args tier — above every
        // on-disk file tier in Claude's precedence
        // (`Managed > CLI args > Local > Project > User`), and a session
        // cannot edit its own process argv. See [`DENIED_TOOLS`].
        "--sandbox".to_string(),
        "--permission-mode".to_string(),
        "dontAsk".to_string(),
        "--disallowedTools".to_string(),
        DENIED_TOOLS.join(" "),
        // The registered `astrid mcp serve` MCP server (`--mcp-config`
        // above) stays available for Astrid-specific operations
        // (`mcp__astrid__*`), but it is no longer the EXCLUSIVE surface —
        // Claude's native tools are primary. The astrid-mcp broker still
        // enforces capability checks + the argument-level policy gate
        // (`policy::evaluate`) on any `mcp__astrid__*` call.
        // -p only: skip writing claude's own session JSONL. Source of
        // truth for the conversation is the bus + the runner's KV records.
        "--no-session-persistence".to_string(),
        "--append-system-prompt-file".to_string(),
        identity_path.to_string(),
        "--session-id".to_string(),
        session_id.to_string(),
    ];
    // Astrid per-principal governance, appended at a fixed position so
    // the argv stays deterministic for the `flags_hash` fingerprint.
    // Argv-visible by design — governance should be auditable, unlike the
    // per-spawn secret in `hook_token` which must never reach argv. When
    // both are at their defaults (`Default` model, `None` turns) NO flags
    // are added, so the argv is byte-identical to the pre-governance form.
    if let Some(alias) = model.cli_alias() {
        args.push("--model".to_string());
        args.push(alias.to_string());
    }
    if let Some(turns) = max_turns {
        args.push("--max-turns".to_string());
        args.push(turns.to_string());
    }
    args
}

/// Construct + spawn the `claude -p` background process. Returns a live
/// [`process::Process`] plus the audit fingerprint.
///
/// Defense-in-depth re-validates `principal_id` and `session_id` so the
/// hardened argv + spawn env can never carry an unsanitised id, even if
/// a future caller path forgets the gate at the IPC boundary. The
/// session_id flows into `--session-id` (claude treats it as opaque,
/// but pathological values would still pollute the spawn log line);
/// principal_id is captured in the spawn audit event.
pub(crate) fn spawn_claude(inputs: &SpawnInputs<'_>) -> Result<Spawned, SysError> {
    crate::validate_id("principal_id", inputs.principal_id)?;
    crate::validate_id("session_id", inputs.session_id)?;

    let args = argv(
        inputs.session_id,
        inputs.identity_path,
        inputs.model,
        inputs.max_turns,
    );

    // Build the command. ANTHROPIC_API_KEY is conditional on auth_mode:
    // when the caller supplies `Some(key)` (api_key mode) we thread it
    // through, when `None` (subscription mode) we OMIT the call entirely
    // so Claude falls back to the keychain OAuth credential written by
    // `claude /login`. The argv itself is identical in both modes — see
    // [`SpawnInputs::api_key`] for the audit-stability rationale.
    let mut cmd = process::Command::new(CLAUDE_BIN)
        .args(args.iter().cloned())
        .env("HOME", inputs.home_path)
        // Belt-and-braces: also disable session persistence via env.
        // Some claude versions honour the flag; older builds may only
        // honour the env. Either path is fine.
        .env("CLAUDE_CODE_SKIP_PROMPT_HISTORY", "1")
        // Hook-bridge envelope: the `astrid-emit` helper (shipping in
        // core via astrid#814) reads these three vars and stamps them
        // onto every `claude.v1.hook.*` publish. Sage's run
        // loop verifies the token against KV before republishing on
        // the canonical `hook.v1.event.*` topic — env values are
        // untrusted on the way in, but the KV lookup acts as the CA.
        .env("ASTRID_PRINCIPAL_ID", inputs.principal_id)
        .env("ASTRID_SESSION_ID", inputs.session_id)
        .env("ASTRID_HOOK_TOKEN", inputs.hook_token)
        .cwd(inputs.home_path)
        // Persistent tier: keep the stdin pipe open so the supervisor can
        // write user turns / tool results across pooled-instance resets AND
        // re-attach after a sage capsule reload. `label` surfaces the
        // session in `process::list`; the lifecycle knobs are documented on
        // their consts above.
        .keep_stdin_open(true)
        .label(format!("sage:{}", inputs.session_id))
        .idle_timeout(IDLE_REAP)
        .exit_retention(EXIT_RETENTION)
        .log_ring_bytes(LOG_RING_BYTES);
    if let Some(key) = inputs.api_key {
        cmd = cmd.env("ANTHROPIC_API_KEY", key);
    }

    let proc = cmd.spawn_persistent()?;
    let process_id = proc.id().as_str().to_string();
    // os_pid is observability-only (audit correlation); a status miss right
    // after spawn is non-fatal, so fall back to 0 rather than failing spawn.
    let os_pid = proc.status().ok().and_then(|s| s.os_pid).unwrap_or(0);

    let flags_hash = argv_hash(&args);

    Ok(Spawned {
        process: proc,
        process_id,
        os_pid,
        flags_hash,
    })
}

/// Compute a deterministic SHA-256 fingerprint over an argv list.
///
/// Each argument is fed in followed by a `0x00` separator byte; that
/// way `["a", "bc"]` and `["ab", "c"]` hash to different values even
/// though their concatenations are identical. The output is formatted
/// as `"sha256:<lowercase-hex>"` so audit consumers can tell the
/// algorithm at a glance.
fn argv_hash(args: &[String]) -> String {
    let mut hasher = Sha256::new();
    for a in args {
        hasher.update(a.as_bytes());
        hasher.update([0x00]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// argv_hash must be deterministic: the same argv produces the
    /// same digest across calls (and, by extension, across process
    /// invocations and capsule reloads). This is the property the
    /// previous DefaultHasher implementation violated because the
    /// SipHash seed is randomised per process.
    #[test]
    fn argv_hash_is_deterministic() {
        let args = vec![
            "-p".to_string(),
            "--session-id".to_string(),
            "abc123".to_string(),
        ];
        let a = argv_hash(&args);
        let b = argv_hash(&args);
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
        // SHA-256 hex is 64 chars; plus the "sha256:" prefix = 71.
        assert_eq!(a.len(), "sha256:".len() + 64);
    }

    /// The 0x00 separator must make `["a", "bc"]` distinct from
    /// `["ab", "c"]`. Without it the two would collide.
    #[test]
    fn argv_hash_distinguishes_boundaries() {
        let lhs = argv_hash(&["a".to_string(), "bc".to_string()]);
        let rhs = argv_hash(&["ab".to_string(), "c".to_string()]);
        assert_ne!(lhs, rhs);
    }

    /// Known-answer test: pin the digest for the empty argv so a
    /// regression that silently changes the framing (e.g. drops the
    /// separator, switches algorithm) is caught.
    ///
    /// `Sha256::new().finalize()` for the empty input is the
    /// well-known constant
    /// `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`.
    #[test]
    fn argv_hash_empty_is_known_value() {
        let h = argv_hash(&[]);
        assert_eq!(
            h,
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    /// Audit-stability invariant: argv is constructed from `session_id`
    /// and `identity_path` only — the auth mode flows through env, not
    /// argv. So [`argv_hash`] must produce the same fingerprint for a
    /// given `session_id` regardless of whether the caller is in
    /// api_key mode (`Some(key)`) or subscription mode (`None`). A
    /// downstream audit consumer correlating spawns by `flags_hash`
    /// will not be able to distinguish auth modes from this digest
    /// alone — that's by design; `auth_mode` is published alongside
    /// `flags_hash` on `claude.v1.audit.spawn`.
    ///
    /// The enforcement flags added to the argv (`--disallowedTools`,
    /// `--setting-sources`) are likewise built from neither the auth
    /// mode nor any per-spawn secret, so adding them does not perturb
    /// this invariant: the digest stays stable across auth modes, only
    /// its value changed (a deliberate fingerprint update — see
    /// [`argv_carries_tier_narrowing_enforcement`]).
    #[test]
    fn argv_hash_unchanged_across_auth_modes() {
        let sid = "sid-stable";
        let path = "home://.claude/.claude-identity-sid-stable";
        let args = argv(sid, path, crate::config::ModelPreference::Default, None);
        let hash_api_key = argv_hash(&args);
        let hash_subscription = argv_hash(&args);
        assert_eq!(hash_api_key, hash_subscription);

        // Belt-and-braces: also confirm that the argv builder itself is
        // not silently branching on something out-of-band — calling it
        // twice with the same inputs must produce byte-identical output.
        let args_again = argv(sid, path, crate::config::ModelPreference::Default, None);
        assert_eq!(args, args_again);
    }

    /// Per-principal governance: a non-default model tier and a turn cap
    /// append `--model <alias>` and `--max-turns <n>`; the defaults
    /// (`Default` model, `None` turns) append neither, so the governed
    /// argv is a strict superset (appended suffix) of the ungoverned one
    /// and the `flags_hash` fingerprint differs — governance is auditable.
    #[test]
    fn argv_carries_governance_when_set() {
        use crate::config::ModelPreference;
        let base = argv("sid", "id", ModelPreference::Default, None);
        assert!(!base.iter().any(|a| a == "--model"));
        assert!(!base.iter().any(|a| a == "--max-turns"));

        let governed = argv("sid", "id", ModelPreference::Opus, Some(20));
        let mi = governed
            .iter()
            .position(|a| a == "--model")
            .expect("--model present");
        assert_eq!(governed[mi + 1], "opus");
        let ti = governed
            .iter()
            .position(|a| a == "--max-turns")
            .expect("--max-turns present");
        assert_eq!(governed[ti + 1], "20");

        assert_ne!(argv_hash(&base), argv_hash(&governed));
        // Governance flags are appended, so base is a prefix of governed.
        assert_eq!(&governed[..base.len()], &base[..]);
    }

    /// The argv must carry the two session-un-overridable enforcement
    /// levers reachable from capsule-space: `--disallowedTools` (the deny
    /// list hoisted into the binding CLI tier) and `--setting-sources
    /// local` (on-disk tier narrowing). Both ride above every on-disk
    /// file tier in Claude's precedence, so a running session cannot edit
    /// them away. Dropping either silently regresses enforcement back to
    /// the fully session-overridable `settings.local.json` posture, so
    /// pin their presence.
    #[test]
    fn argv_carries_tier_narrowing_enforcement() {
        let args = argv(
            "sid",
            "home://.claude/.claude-identity-sid",
            crate::config::ModelPreference::Default,
            None,
        );

        // `--setting-sources local`: the value must be exactly `local`
        // (the scope of the runner's authored settings.local.json). `user` or
        // `project` would either drop the runner's own hook wiring or readmit a
        // stray settings file — both regressions.
        let ss = args
            .iter()
            .position(|a| a == "--setting-sources")
            .expect("argv must pass --setting-sources to narrow on-disk tiers");
        assert_eq!(
            args.get(ss + 1).map(String::as_str),
            Some("local"),
            "--setting-sources must be `local` — the scope of the runner's settings.local.json",
        );

        // Native-tools model: the binding floor is `--permission-mode
        // dontAsk` (anything not allow-listed is auto-denied) + Claude's
        // `--sandbox`. Pin both — dropping either silently widens the
        // surface or reintroduces a hang-on-prompt.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "--permission-mode" && w[1] == "dontAsk"),
            "argv must pass --permission-mode dontAsk (fail-secure headless)",
        );
        assert!(
            args.iter().any(|a| a == "--sandbox"),
            "argv must pass --sandbox (Claude's inner sandbox layer)",
        );

        // `--disallowedTools`: the escape-surface deny rides the binding
        // CLI tier. The value is the space-joined DENIED_TOOLS list, so a
        // session cannot override it from any on-disk file.
        let dt = args
            .iter()
            .position(|a| a == "--disallowedTools")
            .expect("argv must pass --disallowedTools to bind the deny list");
        let joined = args
            .get(dt + 1)
            .expect("--disallowedTools must carry a value");
        assert_eq!(joined, &DENIED_TOOLS.join(" "));
        // Spot-check the load-bearing ESCAPE tools are individually present
        // in the rendered BINDING value: `Agent`/`Task`/`Workflow`
        // (sub-agent spawn), `PowerShell` (the redundant shell), `Cron*`/
        // `ScheduleWakeup` (scheduling), `TaskCreate`/`TaskList` (Astrid's
        // own task surface), `ListMcpResourcesTool`/`ReadMcpResourceTool`
        // (raw MCP around the gated surface). A deny only in the overridable
        // `settings.local.json` is NOT binding, so pin each in argv.
        for tool in [
            "Agent",
            "Task",
            "Workflow",
            "PowerShell",
            "ScheduleWakeup",
            "CronCreate",
            "TaskCreate",
            "TaskList",
            "ListMcpResourcesTool",
            "ReadMcpResourceTool",
            // Egress is denied by default (exfil path the fs sandbox misses).
            "WebFetch",
            "WebSearch",
        ] {
            assert!(
                joined.split(' ').any(|t| t == tool),
                "deny list must include {tool}",
            );
        }
        // The native dev tools must NOT be denied — they are the whole
        // point of the native-tools model (sandbox-bounded, not removed).
        for tool in ["Bash", "Read", "Write", "Edit", "Glob", "Grep"] {
            assert!(
                !joined.split(' ').any(|t| t == tool),
                "native dev tool {tool} must NOT be in the deny list",
            );
        }
    }

    /// Escape vectors that inject agents / plugins / extra dir-scope are
    /// argv flags, NOT tool names — the deny list cannot touch them. sage
    /// owns the full argv and must NEVER emit them (it never forwards
    /// user-controlled argv). Pin their absence so a future edit that adds
    /// one trips the build.
    #[test]
    fn argv_never_emits_untool_gated_escape_flags() {
        let args = argv(
            "sid",
            "home://.claude/.claude-identity-sid",
            crate::config::ModelPreference::Opus,
            Some(10),
        );
        for forbidden in [
            "--agents",
            "--plugin-dir",
            "--plugin-url",
            "--add-dir",
            "--dangerously-skip-permissions",
            "--permission-prompt-tool",
        ] {
            assert!(
                !args.iter().any(|a| a == forbidden),
                "argv must never emit {forbidden} — it is not tool-name-gated",
            );
        }
    }

    /// `DENIED_TOOLS` mirrors `sage_install::layout::REQUIRED_DENIES`
    /// across a crate boundary with no dependency edge. Guard the local
    /// copy's shape so an accidental trim is caught here: every entry is
    /// a bare tool name (no scope qualifier — a bare name removes the
    /// tool from context, a scoped rule like `Bash(rm *)` would only
    /// deny matching calls and leave the tool reachable), and there are
    /// no duplicates. The exhaustiveness contract itself is asserted in
    /// claude-install's tests against the settings.local.json deny array.
    #[test]
    fn denied_tools_are_bare_unscoped_names() {
        let mut seen = std::collections::HashSet::new();
        for t in DENIED_TOOLS {
            assert!(
                !t.contains('(') && !t.contains(' ') && !t.is_empty(),
                "{t} must be a bare unscoped tool name",
            );
            assert!(seen.insert(*t), "{t} is duplicated in DENIED_TOOLS");
        }
    }

    /// Drift guard for the cross-crate mirror with
    /// `sage_install::layout::REQUIRED_DENIES`.
    ///
    /// No dependency edge exists between the two crates (each is a
    /// `cdylib`-only workspace member, so neither can import the other's
    /// const as a library), so the lists cannot be `assert_eq!`d
    /// directly. Instead this test pins `DENIED_TOOLS` against a FULL,
    /// independently-written canonical set — the exhaustive headless deny
    /// surface. This is deliberately stronger than a count anchor: a count
    /// check passes whenever the two lists merely have the same length,
    /// so swapping one tool for another (same count, different members)
    /// would slip through. Comparing the whole set catches additions,
    /// removals, AND substitutions.
    ///
    /// The matching guard on the other side of the mirror lives in
    /// claude-install's `assert_headless_shape`, which asserts every
    /// `REQUIRED_DENIES` member appears in the authored `settings.local.json`
    /// deny array and that the array length equals `REQUIRED_DENIES.len()`.
    /// Together the two guards anchor both copies to the same canonical
    /// surface: a name added to or dropped from either list, without the
    /// mirror edit, fails the build on at least one side.
    ///
    /// SYNC: when you add/remove a denied tool, update this canonical set
    /// AND `REQUIRED_DENIES` (claude-install/src/layout.rs) in lockstep.
    #[test]
    fn denied_tools_equal_canonical_required_denies() {
        // The canonical headless ESCAPE deny surface under the native-tools
        // model — the orchestration / control-plane / exfil / raw-MCP /
        // indirect-execution tools blocked even though Claude's dev tools
        // are allowed. Independently written (not derived from DENIED_TOOLS)
        // so it is a genuine cross-check, not a tautology. MUST match
        // `sage_install::layout::REQUIRED_DENIES` exactly. The native dev
        // tools (Bash/Read/Write/Edit/MultiEdit/NotebookEdit/Glob/Grep/Web*/
        // LSP/Monitor/BashOutput/KillShell/TodoWrite) are deliberately
        // ABSENT — they are allow-listed, sandbox-bounded, not removed.
        const CANONICAL: &[&str] = &[
            "PowerShell",
            "Agent",
            "Task",
            "Workflow",
            "SendMessage",
            "SendUserMessage",
            "ListAgents",
            "TeamCreate",
            "TeamDelete",
            "CronCreate",
            "CronDelete",
            "CronList",
            "ScheduleWakeup",
            "EnterPlanMode",
            "ExitPlanMode",
            "EnterWorktree",
            "ExitWorktree",
            "TaskCreate",
            "TaskGet",
            "TaskList",
            "TaskStop",
            "TaskUpdate",
            "TaskOutput",
            "WebFetch",
            "WebSearch",
            "PushNotification",
            "RemoteTrigger",
            "ShareOnboardingGuide",
            "ListMcpResourcesTool",
            "ReadMcpResourceTool",
            "ToolSearch",
            "WaitForMcpServers",
            "Skill",
            "SlashCommand",
        ];

        // Compare as sets so a pure reordering of either list (which has no
        // semantic effect — `--disallowedTools` is order-insensitive) does
        // not spuriously fail, while any membership difference (add, drop,
        // or substitute) does.
        let actual: std::collections::BTreeSet<&str> = DENIED_TOOLS.iter().copied().collect();
        let canonical: std::collections::BTreeSet<&str> = CANONICAL.iter().copied().collect();

        // Surface the exact drift on failure so the fix is obvious.
        let missing: Vec<&str> = canonical.difference(&actual).copied().collect();
        let unexpected: Vec<&str> = actual.difference(&canonical).copied().collect();
        assert!(
            missing.is_empty() && unexpected.is_empty(),
            "DENIED_TOOLS drifted from the canonical REQUIRED_DENIES mirror.\n  \
             missing from DENIED_TOOLS (denied only in the overridable Local tier, \
             NOT in the binding CLI tier): {missing:?}\n  \
             unexpected in DENIED_TOOLS (not in canonical set): {unexpected:?}\n  \
             re-sync DENIED_TOOLS with sage_install::layout::REQUIRED_DENIES.",
        );

        // Belt-and-braces: no duplicate collapsed the set below the list
        // length, which would mask a drift behind an equal-set comparison.
        assert_eq!(
            actual.len(),
            DENIED_TOOLS.len(),
            "DENIED_TOOLS contains a duplicate entry",
        );
    }
}
