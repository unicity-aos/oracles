//! Per-principal `.claude/` directory layout — paths, sanitization, and
//! KV key derivation.
//!
//! Paths use the `home://` VFS scheme. The kernel resolves it at check
//! time against the invoking principal's home root
//! (`~/.astrid/home/<principal>/`, see core/crates/astrid-kernel/src/lib.rs:75),
//! so per-principal isolation is enforced by the host rather than
//! encoded in the path string. principal_id is still validated as
//! untrusted IPC input — it is used for the KV install-complete key
//! and surfaced in status events — but it never participates in path
//! construction.

use astrid_sdk::prelude::*;

use crate::config::{AuthMode, InteractionMode, PrincipalConfig};

/// Per-principal home root. The kernel binds `home://` to the
/// invoking principal's home directory at check time, so every fs call
/// from this capsule against `home://...` lands inside that principal's
/// scope.
pub(crate) fn principal_home() -> String {
    "home://".to_string()
}

/// `.claude/` config dir under the principal home.
pub(crate) fn claude_dir() -> String {
    "home://.claude".to_string()
}

/// `.claude/projects/` — required by `claude` even when session
/// persistence is disabled.
pub(crate) fn projects_dir() -> String {
    "home://.claude/projects".to_string()
}

/// Path to `.claude/settings.local.json` — the hardened settings file.
pub(crate) fn settings_path() -> String {
    "home://.claude/settings.local.json".to_string()
}

/// Path to `.claude/.mcp.json` — registers the `astrid` MCP server
/// (`astrid mcp serve --principal <id>`) that claude calls `mcp__astrid__*`
/// tools against. See [`mcp_json`] for the body.
pub(crate) fn mcp_path() -> String {
    "home://.claude/.mcp.json".to_string()
}

/// Path to the STAGED managed-settings body — the source the host
/// bind-mounts into Claude's OS-level managed-settings path to make the
/// policy gate + permission posture un-strippable.
///
/// IMPORTANT: Claude does NOT read managed settings from here. Its managed
/// (system) tier is an OS path outside any per-principal home
/// (`/Library/Application Support/ClaudeCode/managed-settings.json` on macOS,
/// `/etc/claude-code/managed-settings.json` on Linux), which this WASM
/// capsule cannot write. So this file is INERT until the host mounts it —
/// the mount is the out-of-process half, filed as core #881. The runner authors only
/// the body (see [`managed_settings_json`]); the host owns placement.
///
/// SECURITY NOTE for the mount (core #881): this staging file lives under the
/// principal's `home://`, which the supervised session CAN write. The mount
/// MUST source from a host-protected copy (and mount read-only) — never
/// bind the live, session-writable file, or a session could edit its own
/// managed tier and defeat the un-strippability this exists to provide.
pub(crate) fn managed_settings_path() -> String {
    "home://.claude/managed-settings.json".to_string()
}

/// Name of the MCP server Astrid registers for the supervised `claude`
/// session — the `astrid mcp serve` stdio shim onto the daemon's astrid-mcp
/// broker. Claude prefixes its tools `mcp__astrid__*` and references it as the
/// `server` of the PreToolUse `mcp_tool` gate hook. Single source of truth
/// for [`mcp_json`] and [`pretooluse_gate_handler`], so the gate's target
/// server can never drift from the registered one.
const MCP_SERVER_NAME: &str = "astrid";

/// Raw tool name of the native-tool PreToolUse policy gate the `mcp_tool`
/// hook calls on the [`MCP_SERVER_NAME`] server. The astrid-mcp broker
/// special-cases this exact name — it evaluates the per-principal policy and
/// returns a binding Claude hook decision instead of dispatching a capsule
/// tool.
///
/// SYNC (load-bearing): must equal `broker::PRETOOLUSE_GATE_TOOL` in the
/// astrid-mcp crate. The crates share no dependency edge, so the value is
/// mirrored, not shared; a drift silently DISABLES the gate (the broker
/// would treat the hook's call as an unknown tool, reply `isError`, and the
/// hook would fail OPEN — the native tool runs ungoverned). A presence test
/// in each crate anchors the value.
const PRETOOLUSE_GATE_TOOL: &str = "astrid_pretooluse_gate";

/// KV key marking a completed install for `principal_id`.
///
/// Namespaced under `claude.` so the prefix can't collide with another
/// capsule's "install.complete.*" markers — every capsule sharing the KV
/// surface keeps its own top-level bucket.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn install_complete_key(sanitized_id: &str) -> String {
    format!("claude.install.complete.{sanitized_id}")
}

/// Validate and normalise a principal id read off the IPC bus.
///
/// Rejects `..`, `/`, `\`, NUL, and any character outside
/// `[A-Za-z0-9._-]`. The accepted alphabet matches every other
/// per-principal VFS resolver in the Astrid stack; this is the only
/// untrusted-input gate in the install path.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn sanitize_principal_id(id: &str) -> Result<String, SysError> {
    Ok(oracle_host::PrincipalId::parse(id)?.to_string())
}

/// Source-of-truth deny list for the headless NATIVE-TOOLS model — the
/// ESCAPE / orchestration surface that stays blocked even though Claude's
/// own dev tools (Bash / Read / Write / Edit / Glob / Grep / Web*) are now
/// ALLOWED and sandboxed. Denied: a second shell (`PowerShell`), sub-agent
/// spawn / teaming, scheduling + control-flow transitions, Astrid's OWN
/// task surface, off-host exfiltration channels, raw MCP resource /
/// tool-loading surfaces, and the indirect-execution skill surface — each
/// a governance axis that does not belong to a single supervised session.
///
/// Defence-in-depth caveat: this list lives in `settings.local.json`,
/// Claude's WEAKEST, fully session-overridable settings tier — a capable
/// session can edit any of these denies away. The BINDING tool gate is
/// the spawn argv (`--permission-mode dontAsk` — anything not in
/// `permissions.allow` is auto-denied — plus `--disallowedTools <this
/// list>`, see `claude_runner::spawn::argv` / `claude_runner::spawn::DENIED_TOOLS`), which
/// sits above every on-disk file tier in Claude's precedence order; and
/// the Astrid HOST sandbox bounds whatever does run. This file copy is a
/// redundant second layer, not the primary enforcement boundary.
///
/// SYNC (load-bearing): every name here MUST also appear in
/// `claude_runner::spawn::DENIED_TOOLS` (claude-runner/src/spawn.rs), the copy hoisted into
/// the binding `--disallowedTools` CLI tier. The two crates have no
/// dependency edge, so the list is mirrored, not shared. A name present
/// HERE but absent THERE is denied only in the overridable tier — a
/// session edits it out of `settings.local.json` and calls it, so the
/// deny is not actually binding. Both sides anchor on the same element
/// count (`assert_headless_shape` here, `denied_tools_count_anchors_…`
/// there) so a one-sided edit trips a test. Any change to one list must
/// mirror to the other.
///
/// Coverage strategy: this list is NOT the boundary and is NOT claimed to
/// be exhaustive. The real boundary is the closed `NATIVE_ALLOW` whitelist
/// under `--permission-mode dontAsk`: any tool not on the allow-list is
/// auto-denied whether or not it appears here (so a near-future Claude tool
/// the list has not caught is denied by default, not allowed). This deny
/// list is a defence-in-depth SNAPSHOT of the known escape / orchestration /
/// exfil / raw-MCP / indirect-exec surface, hoisted into the binding CLI
/// tier so those specific names cannot be re-allowed from an overridable
/// settings file. Entries fall into two groups:
/// * Current surface — known act/spawn/schedule/message/MCP-read/load
///   tools. Inert read-only conveniences (e.g. `AskUserQuestion`) are not
///   listed; the whitelist + dontAsk already deny them.
/// * Legacy / forward-compat aliases — names current builds have renamed or
///   do not ship (e.g. `Task`→`Agent`; `SlashCommand` has no live tool).
///   Denying a name Claude no longer ships is a harmless no-op that keeps
///   the gate intact across the version-pin window.
///
/// Public-in-module so tests can both assert presence in the JSON and use
/// it as a parameterised fixture.
const REQUIRED_DENIES: &[&str] = &[
    // A second shell on its own tool name — the native shell is `Bash`
    // (now allowed, sandboxed); the redundant shell surface stays closed.
    "PowerShell",
    // Sub-agent spawn / teaming — a spawned agent runs its OWN tool calls
    // and is a separate principal-scoping question. `Task` is the legacy
    // `Agent` alias.
    "Agent",
    "Task",
    "Workflow",
    "SendMessage",
    "SendUserMessage",
    "ListAgents",
    "TeamCreate",
    "TeamDelete",
    // Scheduling / control flow — queue a future prompt, reschedule a loop,
    // or drive plan-mode / worktree transitions outside the runner's run loop.
    "CronCreate",
    "CronDelete",
    "CronList",
    "ScheduleWakeup",
    "EnterPlanMode",
    "ExitPlanMode",
    "EnterWorktree",
    "ExitWorktree",
    // Astrid's OWN task surface — these are ASTRID's tools, not Claude's;
    // the supervised session must not drive Astrid's control plane.
    "TaskCreate",
    "TaskGet",
    "TaskList",
    "TaskStop",
    "TaskUpdate",
    "TaskOutput",
    // Network egress — `WebFetch`/`WebSearch` are model-driven HTTP tools
    // that bypass Claude's Bash sandbox (the filesystem sandbox cannot bound
    // them), so read-secret + POST-out is the one exfil path the sandbox
    // misses. Egress OFF by default until a controlled allow-list lands.
    "WebFetch",
    "WebSearch",
    // External / exfiltration surfaces — off-host channels Astrid does not
    // mediate.
    "PushNotification",
    "RemoteTrigger",
    "ShareOnboardingGuide",
    // MCP resource + tool-loading surfaces: reading raw MCP resources or
    // loading deferred tools reaches a tool around the gated surface.
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "ToolSearch",
    "WaitForMcpServers",
    // Indirect-execution surface — a skill can fan out to other tools;
    // deferred until the indirect path is governed. `SlashCommand` is NOT a
    // live tool/alias in current builds (the slash surface is reached via
    // `Skill`) — kept as a harmless forward/back-compat no-op.
    "Skill",
    "SlashCommand",
];

/// Native dev-tool surface auto-approved under the headless native-tools
/// model. With `--permission-mode dontAsk` (binding via argv) only tools
/// matched here — or read-only sandboxed Bash — run without a prompt;
/// everything else, including the [`REQUIRED_DENIES`] escape surface, is
/// auto-denied. `mcp__astrid__*` keeps the registered Astrid MCP server
/// reachable for Astrid-specific operations. Bounds on what these tools can
/// DO come from the sandbox (Astrid host + Claude inner), not this list.
///
/// Deliberately ABSENT: `WebFetch`/`WebSearch` (egress is off by default —
/// the one exfil path the filesystem sandbox does not bound; see
/// [`REQUIRED_DENIES`]); and the background-task tools `Monitor` /
/// `BashOutput` / `KillShell` — in current Claude builds `BashOutput` /
/// `KillShell` fold into the `TaskOutput` / `TaskStop` family that the deny
/// list blocks, so they would be non-functional anyway. v1 is foreground
/// `Bash` only; background tasks ride the later background-task slice.
const NATIVE_ALLOW: &[&str] = &[
    "Bash",
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "Glob",
    "Grep",
    "LSP",
    "TodoWrite",
    "mcp__astrid__*",
];

/// The Claude hook events the runner declares in `settings.local.json`. Each
/// event is wired through the `astrid-emit` native helper, which
/// publishes the Claude-side hook payload on the claude-namespaced
/// `claude.v1.hook.*` topic so the runner's run-loop validator can
/// authenticate the spawn-token and republish on the canonical
/// `hook.v1.event.*` (or claude-namespaced `claude.v1.notification`) topic.
/// See [`HOOK_TOPIC_MAP`] for the per-event topic.
///
/// The COMPLETE Claude hook event surface — every event name here is
/// verified present in the shipped `claude` binary. We wire ALL of them,
/// not just the headless-`-p` subset: the hooks block is authored
/// identically in both interaction modes, and in repl mode (the user's own
/// environment, full tool surface) the interactive-only events
/// (`PermissionRequest`, `MessageDisplay`) and the tool-gated events
/// (`Worktree*`, `Task*`, `TeammateIdle`) all fire. An event that does not
/// fire in a given mode simply never triggers there — authoring it is
/// harmless and keeps the plane complete across modes.
///
/// The set spans: session lifecycle (setup/start/end), prompt + tool-call
/// turns (incl. failures, batches, prompt expansion, permission
/// request/denial), the subagent + background-task + teammate lifecycle,
/// the compaction window, config / instructions / filesystem / worktree
/// observability, MCP elicitation, message display, and notifications. Each
/// canonical target is a `hook.v1.event.<name>` topic (a wildcard publish,
/// so widening needs no cross-capsule contract change). Semantically
/// distinct events stay distinct: `Stop` is a per-turn "assistant message
/// sent" signal, NOT session end — `SessionEnd` is the real terminator
/// (see [`HOOK_TOPIC_MAP`]).
const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "Setup",
    "UserPromptSubmit",
    "UserPromptExpansion",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PostToolBatch",
    "PermissionRequest",
    "PermissionDenied",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "TaskCreated",
    "TaskCompleted",
    "TeammateIdle",
    "PreCompact",
    "PostCompact",
    "ConfigChange",
    "InstructionsLoaded",
    "FileChanged",
    "CwdChanged",
    "WorktreeCreate",
    "WorktreeRemove",
    "Elicitation",
    "ElicitationResult",
    "MessageDisplay",
    "Notification",
];

/// Per-event mapping from Claude's hook name to the claude-namespaced
/// `claude.v1.hook.*` topic that `astrid-emit` publishes on.
///
/// The runner's run-loop validator subscribes to `claude.v1.hook.*`,
/// authenticates the per-(principal, session) spawn token carried in
/// the envelope, and republishes on the canonical `hook.v1.event.<name>`
/// topic (or `claude.v1.notification` for the one event without a
/// canonical equivalent today).
///
/// Semantic care: Claude's `Stop` fires at the end of every response
/// turn (the assistant message was sent), so it maps to `message_sent`,
/// NOT `session_end`. `SessionEnd` — which fires once when the session
/// actually terminates — is what carries `session_end`. Conflating the
/// two would make any session-lifecycle subscriber fire on every turn.
///
/// SYNC: keep aligned with claude_runner::hooks::HOOK_TOPIC_MAP (claude-runner/src/hooks.rs).
/// claude-install cannot import from claude-runner (separate workspace
/// crate, no dependency edge), so the table is mirrored here. Any edit
/// to one side must mirror to the other. Order must match HOOK_EVENTS.
const HOOK_TOPIC_MAP: &[(&str, &str)] = &[
    ("SessionStart", "claude.v1.hook.session_start"),
    ("SessionEnd", "claude.v1.hook.session_end"),
    ("Setup", "claude.v1.hook.session_setup"),
    ("UserPromptSubmit", "claude.v1.hook.message_received"),
    ("UserPromptExpansion", "claude.v1.hook.message_expanded"),
    ("PreToolUse", "claude.v1.hook.before_tool_call"),
    ("PostToolUse", "claude.v1.hook.after_tool_call"),
    ("PostToolUseFailure", "claude.v1.hook.after_tool_call_failed"),
    ("PostToolBatch", "claude.v1.hook.after_tool_batch"),
    ("PermissionRequest", "claude.v1.hook.permission_requested"),
    ("PermissionDenied", "claude.v1.hook.permission_denied"),
    ("Stop", "claude.v1.hook.message_sent"),
    ("StopFailure", "claude.v1.hook.message_failed"),
    ("SubagentStart", "claude.v1.hook.subagent_start"),
    ("SubagentStop", "claude.v1.hook.subagent_stop"),
    ("TaskCreated", "claude.v1.hook.task_created"),
    ("TaskCompleted", "claude.v1.hook.task_completed"),
    ("TeammateIdle", "claude.v1.hook.teammate_idle"),
    ("PreCompact", "claude.v1.hook.on_compaction_started"),
    ("PostCompact", "claude.v1.hook.on_compaction_completed"),
    ("ConfigChange", "claude.v1.hook.config_changed"),
    ("InstructionsLoaded", "claude.v1.hook.instructions_loaded"),
    ("FileChanged", "claude.v1.hook.file_changed"),
    ("CwdChanged", "claude.v1.hook.cwd_changed"),
    ("WorktreeCreate", "claude.v1.hook.worktree_created"),
    ("WorktreeRemove", "claude.v1.hook.worktree_removed"),
    ("Elicitation", "claude.v1.hook.elicitation_requested"),
    ("ElicitationResult", "claude.v1.hook.elicitation_resolved"),
    ("MessageDisplay", "claude.v1.hook.message_displayed"),
    ("Notification", "claude.v1.hook.notification"),
];

/// Lookup the `astrid-emit` topic for a Claude hook event.
fn hook_topic(event: &str) -> &'static str {
    HOOK_TOPIC_MAP
        .iter()
        .find_map(|(k, v)| if *k == event { Some(*v) } else { None })
        .expect("HOOK_EVENTS and HOOK_TOPIC_MAP must stay in sync")
}

/// Build the `hooks` block — identical in both interaction modes.
///
/// Each event invokes the `astrid-emit` native helper with the
/// claude-namespaced `claude.v1.hook.*` topic. `astrid-emit`
/// reads Claude's stdin hook payload, packages it into the envelope
/// shape the runner's validator expects (hook, payload, correlation_id,
/// principal_id, session_id, token), and publishes on the bus.
///
/// Forward-compatible: `astrid-emit` ships separately in the core
/// distribution (filed at astrid#814). Until that binary lands the
/// `command` strings are inert — claude exec-spawns the helper, the
/// shell reports "not found", and Claude treats the hook as a no-op.
/// No change to this file is needed once the helper lands.
///
/// Unix assumption: this assumes a Unix `PATH` lookup for `astrid-emit`;
/// Claude integration is Unix-only today (the `claude` binary, the HOME redirect, and
/// the `/bin/false` `apiKeyHelper` all assume Unix).
fn hooks_block() -> serde_json::Value {
    let mut hooks = serde_json::Map::new();
    for event in HOOK_EVENTS {
        let topic = hook_topic(event);
        let mut handlers = vec![serde_json::json!({
            "type": "command",
            "command": format!("astrid-emit {topic}"),
            "timeout": 10
        })];
        // PreToolUse additionally carries the BINDING-direction policy gate:
        // a synchronous `mcp_tool` handler that asks the astrid-mcp broker for
        // an allow/deny decision on the native tool about to run, and can
        // block it. Every other event stays observe-only (`astrid-emit`). The
        // gate is appended AFTER the observe emit so the audit plane records
        // the attempt regardless of outcome; Claude aggregates multiple
        // PreToolUse handlers with deny > ask > allow precedence, and the
        // gate's allow path returns a no-op (never an explicit allow), so the
        // pair can only ever ADD a denial. See [`pretooluse_gate_handler`].
        if *event == "PreToolUse" {
            handlers.push(pretooluse_gate_handler());
        }
        hooks.insert((*event).to_string(), serde_json::Value::Array(handlers));
    }
    serde_json::Value::Object(hooks)
}

/// The PreToolUse `mcp_tool` hook handler — Astrid's native-tool policy gate.
///
/// Calls the reserved [`PRETOOLUSE_GATE_TOOL`] on the already-connected
/// [`MCP_SERVER_NAME`] server with the name + input of the native tool about
/// to run. The astrid-mcp broker evaluates the per-principal policy and returns
/// a Claude hook decision as the tool's text content; a
/// `permissionDecision:"deny"` BLOCKS the tool. This is the one
/// decision-returning hook — every other event is observe-only.
///
/// `${tool_name}` / `${tool_input}` are substituted by Claude from the hook
/// payload. VERIFIED against the shipped `claude` executor: its `${...}`
/// interpolator `JSON.stringify`s any resolved object, so `${tool_input}`
/// (the whole input object) arrives at the broker as a JSON STRING it parses
/// back for full argument-level rules; a missing path interpolates to `""`.
/// The broker still parses defensively, so any future substitution shape
/// degrades to coarser (tool-name-only) matching, never a broadening.
///
/// REQUIRES Claude Code >= 2.1.118 (which introduced `type:"mcp_tool"`
/// hooks). On an older binary the handler type is unknown and the gate simply
/// does not fire — the sibling observe-only `astrid-emit` handler is
/// unaffected. FAILS OPEN by platform design: a disconnected server, a tool
/// error, or a non-JSON reply lets the tool run, so this is an advisory,
/// best-effort layer. The fail-closed boundary for native tools stays the
/// Astrid host sandbox + the binding `--disallowedTools` deny-list (see
/// [`settings_json`]); the gate adds dynamic, argument-level DENY on top.
fn pretooluse_gate_handler() -> serde_json::Value {
    serde_json::json!({
        "type": "mcp_tool",
        "server": MCP_SERVER_NAME,
        "tool": PRETOOLUSE_GATE_TOOL,
        "input": {
            "tool_name": "${tool_name}",
            "tool_input": "${tool_input}"
        },
        "timeout": 10
    })
}

/// `.claude/settings.local.json` body for the given principal config.
///
/// ENFORCEABILITY: this authors `settings.local.json`, which is Claude's
/// **Local** settings tier — the WEAKEST, fully session-overridable tier
/// in Claude's precedence order (Managed > CLI args > Local > Project >
/// User). A capable session can edit any value here — the deny list, the
/// permission modes, `apiKeyHelper` — and Claude will honour the edit.
/// So nothing in this file is binding on its own.
///
/// The BINDING tool/permission gate is the spawn argv, which sits above
/// every on-disk file tier (only Managed, a fixed SYSTEM-path tier the runner
/// cannot author, outranks it). See `claude_runner::spawn::argv`: `--permission-mode
/// dontAsk` auto-denies any tool not allow-listed (fail-secure, no prompt),
/// `--sandbox` bounds Claude's native tools (under the Astrid host
/// sandbox), `--disallowedTools <REQUIRED_DENIES>` binds the escape-surface
/// deny, `--strict-mcp-config` + `--mcp-config` register the `astrid mcp
/// serve` MCP server for Astrid-specific ops, and `--no-session-persistence`
/// suppresses Claude's own JSONL. Those flags cannot be overridden from
/// within the session.
///
/// This file therefore serves two non-binding purposes:
/// * a redundant, defence-in-depth deny layer ([`REQUIRED_DENIES`]) that
///   only narrows the surface the argv already pins — useful while a
///   value still holds, never relied on as the boundary; and
/// * a carrier for two strings Claude reads only from a settings tier and
///   not from argv: the `apiKeyHelper` command path and the `hooks` block.
///   Both name external commands Claude execs; their integrity rests on
///   the commands themselves (and, for hooks, the per-spawn token the runner's
///   validator checks), not on this file being tamper-proof.
///
/// Branching is driven by the two axes in [`PrincipalConfig`]:
///
/// * [`InteractionMode::Headless`]: the runner drives the loop. The allow list
///   is [`NATIVE_ALLOW`] (Claude's dev tools + the Astrid MCP surface), the
///   escape surface in [`REQUIRED_DENIES`] is denied, `permissions.defaultMode`
///   is `dontAsk` (auto-deny the rest, no prompt), the `sandbox` block bounds
///   Claude's native tools, and `disableSkillShellExecution` blocks the skill
///   shell path.
/// * [`InteractionMode::Repl`]: the user drives `claude` directly. Allow
///   and deny lists are empty (user owns their full Claude environment)
///   and `disableSkillShellExecution` is omitted.
/// * [`AuthMode::ApiKey`]: `apiKeyHelper` is pinned to `/bin/false` so
///   `claude` cannot fall back to ambient creds — the per-principal
///   secret is forwarded as `ANTHROPIC_API_KEY` in the spawn env.
/// * [`AuthMode::Subscription`]: `apiKeyHelper` is omitted entirely so
///   `claude` can use its keychain OAuth path. Caveat (macOS): the
///   keychain entry is keyed by service+account, not by `HOME`, so two
///   principals on the same macOS user share the OAuth token. Use
///   api_key mode (or separate macOS users) for full per-principal
///   isolation. Linux libsecret is namespaced by user session and is
///   unaffected.
///
/// The `hooks` block is **identical in both modes** — declared, not
/// disabled. Each event invokes `astrid-emit <topic>` (the native
/// helper shipping separately in core per astrid#814) so claude's
/// stdin-JSON subprocess hook protocol is bridged onto the
/// `claude.v1.hook.*` IPC topic. The runner's run-loop validator
/// then authenticates the per-(principal, session) spawn token and
/// republishes on canonical `hook.v1.event.*` (or
/// `claude.v1.notification` for the one event without a canonical
/// equivalent today). The `astrid-capsule-hook-bridge` WASM capsule
/// already maps lifecycle events to semantic hooks on the bus side.
pub(crate) fn settings_json(cfg: &PrincipalConfig) -> serde_json::Value {
    let (allow, deny, headless): (Vec<&str>, Vec<&str>, bool) = match cfg.interaction_mode {
        InteractionMode::Headless => (NATIVE_ALLOW.to_vec(), REQUIRED_DENIES.to_vec(), true),
        InteractionMode::Repl => (vec![], vec![], false),
    };

    let mut permissions = serde_json::json!({
        "allow": allow,
        "deny": deny,
    });
    if headless {
        // Fail-secure headless posture: a tool not in `allow` (and not a
        // read-only sandboxed Bash command) is auto-DENIED, never prompted
        // — a prompt would hang the terminal-less `-p` session. Mirrors the
        // binding `--permission-mode dontAsk` argv flag.
        permissions["defaultMode"] = serde_json::json!("dontAsk");
    }

    let mut root = serde_json::json!({
        "permissions": permissions,
        "hooks": hooks_block(),
        "cleanupPeriodDays": 30,
    });
    let obj = root
        .as_object_mut()
        .expect("settings root literal is a JSON object");

    if headless {
        // Claude's own Bash/file sandbox — a best-effort INNER layer under
        // the Astrid host sandbox. Writes are bounded to the working dir
        // (cwd = the principal HOME); `allowUnsandboxedCommands:false` stops
        // a command opting out. Network egress + sensitive-read denials
        // belong in the (un-overridable) managed tier; this overridable copy
        // only ever narrows, and if Claude's sandbox cannot initialise
        // nested inside the host sandbox the host sandbox still binds.
        obj.insert(
            "sandbox".to_string(),
            serde_json::json!({
                "enabled": true,
                // Fail CLOSED if Claude's sandbox cannot initialise (e.g.
                // nested under the host bwrap/seatbelt): refuse to start
                // rather than silently run Bash unsandboxed-by-Claude. Set
                // explicitly, not relying on Claude's default-injection.
                "failIfUnavailable": true,
                "allowUnsandboxedCommands": false,
                "filesystem": { "allowWrite": ["./", "$TMPDIR"] }
            }),
        );
        // The skill subsystem can fan out to shell; `Skill` is denied, but
        // block its shell path defensively too.
        obj.insert(
            "disableSkillShellExecution".to_string(),
            serde_json::json!(true),
        );
    }
    if matches!(cfg.auth_mode, AuthMode::ApiKey) {
        obj.insert("apiKeyHelper".to_string(), serde_json::json!("/bin/false"));
    }
    root
}

/// The staged MANAGED-tier body — the un-strippable enforcement posture.
///
/// Authored at [`managed_settings_path`] for the host to mount into Claude's
/// OS managed-settings path (core #881); inert until then. It reuses the FULL
/// local-tier posture from [`settings_json`] (permissions allow/deny,
/// `defaultMode`, sandbox, `disableSkillShellExecution`, `apiKeyHelper`) but
/// narrows `hooks` to ONLY the binding policy gate
/// ([`pretooluse_gate_handler`]). Rationale:
///
/// * The managed tier carries only what must be UN-STRIPPABLE. The
///   observe-only `astrid-emit` plane is best-effort audit and stays in the
///   (session-overridable) local tier — no need to duplicate 30 fire-and-
///   forget hooks into the managed file.
/// * Claude MERGES hooks across tiers and deduplicates `mcp_tool` handlers by
///   `(server, tool, input)`. The gate here is byte-identical to its local
///   copy, so the two collapse and the gate fires EXACTLY ONCE — even when
///   both tiers are present. Stripping the local copy leaves the managed one
///   standing; that is the whole point.
/// * Permission `allow`/`deny` union across tiers and `deny` is sticky (a
///   higher-tier allow cannot cancel a lower-tier deny), so the managed
///   `deny` surface ([`REQUIRED_DENIES`]) and the `dontAsk` floor become
///   un-strippable, and `apiKeyHelper` (when api-key mode) becomes an
///   un-strippable auth lockdown.
///
/// Authored mode-agnostically like the rest: headless gets the full lockdown
/// posture + gate; repl gets the operator's gate over an otherwise
/// user-owned permission surface.
pub(crate) fn managed_settings_json(cfg: &PrincipalConfig) -> serde_json::Value {
    let mut root = settings_json(cfg);
    // Narrow hooks to the binding gate only; leave every other key (the
    // permission/sandbox/auth posture) exactly as the local tier authors it,
    // so the managed copy is an un-strippable superset-by-precedence.
    if let Some(obj) = root.as_object_mut() {
        obj.insert(
            "hooks".to_string(),
            serde_json::json!({ "PreToolUse": [pretooluse_gate_handler()] }),
        );
    }
    root
}

/// `.claude/.mcp.json` body for the given principal config.
///
/// Registered identically in BOTH modes: the `astrid` MCP server as
/// `astrid mcp serve --principal <principal_id>` — the stdio MCP server
/// that fronts the daemon's astrid-mcp broker (unicity-astrid/astrid#880).
/// `principal_id` is the sanitised invoking principal, baked in so the
/// server stamps the right identity on its broker requests (`astrid mcp
/// serve` does NOT infer it — absent `--principal` it falls back to the
/// active/default principal, mis-scoping tools).
///
/// * [`InteractionMode::Headless`]: claude loads exactly this file via
///   `--strict-mcp-config --mcp-config .claude/.mcp.json` (see
///   `claude_runner::spawn`), does the native MCP handshake, and discovers the
///   `mcp__astrid__*` tools.
/// * [`InteractionMode::Repl`]: the operator drives `claude` interactively
///   in this folder; auto-discovered `.claude/.mcp.json` registers the same
///   Astrid server, so the REPL session is "on Astrid" with the capsule
///   tool surface available. The operator may add more servers; Astrid only
///   guarantees the Astrid one is present.
pub(crate) fn mcp_json(_cfg: &PrincipalConfig, principal_id: &str) -> serde_json::Value {
    // Register the Astrid MCP server in BOTH modes. This is what makes a
    // session "on Astrid": `astrid mcp serve` is the stdio shim onto the
    // daemon's astrid-mcp broker, so Claude — headless OR the interactive
    // REPL the operator drives in this folder — discovers and calls the
    // capsule tool surface (`mcp__astrid__*`). The principal is baked in so
    // the daemon scopes tool execution to this identity. The operator can
    // still add their own MCP servers to this file in repl mode; Astrid only
    // guarantees the Astrid server is present.
    let mut servers = serde_json::Map::new();
    servers.insert(
        MCP_SERVER_NAME.to_string(),
        serde_json::json!({
            "command": "astrid",
            "args": ["mcp", "serve", "--principal", principal_id],
            "env": {}
        }),
    );
    serde_json::json!({ "mcpServers": serde_json::Value::Object(servers) })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // sanitize_principal_id  (security gate — preserved verbatim)
    // ------------------------------------------------------------------

    #[test]
    fn sanitize_accepts_typical_id() {
        assert_eq!(sanitize_principal_id("alice").unwrap(), "alice".to_string());
    }

    #[test]
    fn sanitize_accepts_full_allowed_alphabet() {
        // Every ASCII alnum + the three punctuation characters in the spec
        let id = "AZaz09._-";
        assert_eq!(sanitize_principal_id(id).unwrap(), id.to_string());
    }

    #[test]
    fn sanitize_accepts_pubkey_shaped_id() {
        // 64-char hex — common shape for ed25519 principal ids.
        let id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(sanitize_principal_id(id).unwrap(), id.to_string());
    }

    #[test]
    fn sanitize_rejects_empty_string() {
        let err = sanitize_principal_id("").unwrap_err();
        assert!(matches!(err, SysError::ApiError(_)));
    }

    #[test]
    fn sanitize_rejects_dot() {
        let err = sanitize_principal_id(".").unwrap_err();
        assert!(matches!(err, SysError::ApiError(ref m) if m.contains("reserved")));
    }

    #[test]
    fn sanitize_rejects_dotdot() {
        let err = sanitize_principal_id("..").unwrap_err();
        assert!(matches!(err, SysError::ApiError(ref m) if m.contains("reserved")));
    }

    #[test]
    fn sanitize_rejects_forward_slash() {
        // Path traversal attempt — would escape the principal home.
        let err = sanitize_principal_id("alice/bob").unwrap_err();
        assert!(matches!(err, SysError::ApiError(ref m) if m.contains("disallowed character '/'")));
    }

    #[test]
    fn sanitize_rejects_backslash() {
        let err = sanitize_principal_id("alice\\bob").unwrap_err();
        assert!(matches!(err, SysError::ApiError(ref m) if m.contains("disallowed character")));
    }

    #[test]
    fn sanitize_rejects_nul_byte() {
        // Filesystem syscalls truncate at NUL; never let one through.
        let err = sanitize_principal_id("alice\0bob").unwrap_err();
        assert!(matches!(err, SysError::ApiError(ref m) if m.contains("disallowed character")));
    }

    #[test]
    fn sanitize_rejects_path_traversal_sequence() {
        let err = sanitize_principal_id("../../etc/passwd").unwrap_err();
        // Trips on the '/' before it reaches the literal `..` check.
        assert!(matches!(err, SysError::ApiError(_)));
    }

    #[test]
    fn sanitize_rejects_space() {
        let err = sanitize_principal_id("alice bob").unwrap_err();
        assert!(matches!(err, SysError::ApiError(_)));
    }

    #[test]
    fn sanitize_rejects_unicode() {
        // Non-ASCII letters are outside the allowlist.
        let err = sanitize_principal_id("álice").unwrap_err();
        assert!(matches!(err, SysError::ApiError(_)));
    }

    #[test]
    fn sanitize_rejects_oversized_id() {
        let id: String = "a".repeat(129);
        let err = sanitize_principal_id(&id).unwrap_err();
        assert!(matches!(err, SysError::ApiError(ref m) if m.contains("128")));
    }

    #[test]
    fn sanitize_accepts_id_at_128_limit() {
        let id: String = "a".repeat(128);
        assert_eq!(sanitize_principal_id(&id).unwrap(), id);
    }

    // ------------------------------------------------------------------
    // settings_json — parameterised over the (interaction, auth) matrix.
    //
    // Four combinations × shared per-axis assertions. The security
    // surface (deny list + helper pinning + hook declaration) is the
    // critical invariant — every change must keep it intact for the
    // headless+api_key default, otherwise `claude` could escape the
    // mcp__astrid__* sandbox.
    // ------------------------------------------------------------------

    fn cfg(im: InteractionMode, am: AuthMode) -> PrincipalConfig {
        PrincipalConfig {
            interaction_mode: im,
            auth_mode: am,
            ..PrincipalConfig::default()
        }
    }

    fn assert_headless_shape(v: &serde_json::Value) {
        // Native-tools allow surface: the dev tools + the Astrid MCP server,
        // exactly NATIVE_ALLOW.
        let allow = v
            .pointer("/permissions/allow")
            .and_then(|x| x.as_array())
            .expect("permissions.allow must be a JSON array");
        for required in NATIVE_ALLOW {
            assert!(
                allow.iter().any(|a| a == required),
                "headless: allow list missing native tool '{required}'"
            );
        }
        assert_eq!(
            allow.len(),
            NATIVE_ALLOW.len(),
            "headless: allow list must be exactly NATIVE_ALLOW"
        );

        // The escape surface stays denied, exactly REQUIRED_DENIES — and a
        // native dev tool must never appear on BOTH lists.
        let deny = v
            .pointer("/permissions/deny")
            .and_then(|x| x.as_array())
            .expect("permissions.deny must be a JSON array");
        for required in REQUIRED_DENIES {
            assert!(
                deny.iter().any(|d| d == required),
                "headless: deny list missing escape tool '{required}'"
            );
        }
        assert_eq!(
            deny.len(),
            REQUIRED_DENIES.len(),
            "headless: deny list must contain exactly the escape surface"
        );
        for allowed in NATIVE_ALLOW {
            assert!(
                !deny.iter().any(|d| d == allowed),
                "headless: native tool '{allowed}' must not also be denied"
            );
        }

        // Fail-secure permission mode + Claude's inner sandbox, both pinned.
        assert_eq!(
            v.pointer("/permissions/defaultMode")
                .and_then(|x| x.as_str()),
            Some("dontAsk"),
            "headless: permission defaultMode must be dontAsk (fail-secure)"
        );
        assert_eq!(
            v.pointer("/sandbox/enabled").and_then(|x| x.as_bool()),
            Some(true),
            "headless: Claude's sandbox must be enabled"
        );
        assert_eq!(
            v.pointer("/sandbox/allowUnsandboxedCommands")
                .and_then(|x| x.as_bool()),
            Some(false),
            "headless: a command must not be able to opt out of the sandbox"
        );

        assert_eq!(
            v.pointer("/disableSkillShellExecution")
                .and_then(|x| x.as_bool()),
            Some(true),
            "headless: skill shell execution must be disabled"
        );
    }

    fn assert_repl_shape(v: &serde_json::Value) {
        let allow = v
            .pointer("/permissions/allow")
            .and_then(|x| x.as_array())
            .expect("permissions.allow must be a JSON array");
        assert!(allow.is_empty(), "repl: allow list must be empty");

        let deny = v
            .pointer("/permissions/deny")
            .and_then(|x| x.as_array())
            .expect("permissions.deny must be a JSON array");
        assert!(deny.is_empty(), "repl: deny list must be empty");

        assert!(
            v.pointer("/disableSkillShellExecution").is_none(),
            "repl: disableSkillShellExecution must be omitted"
        );
        // Repl is the user's own environment — sage imposes no native-tools
        // posture: no dontAsk mode, no sandbox object.
        assert!(
            v.pointer("/permissions/defaultMode").is_none(),
            "repl: permission defaultMode must be omitted"
        );
        assert!(
            v.pointer("/sandbox").is_none(),
            "repl: sandbox object must be omitted"
        );
    }

    fn assert_api_key_helper_present(v: &serde_json::Value) {
        assert_eq!(
            v.pointer("/apiKeyHelper").and_then(|x| x.as_str()),
            Some("/bin/false"),
            "api_key: apiKeyHelper must be /bin/false so claude cannot fall back to ambient creds"
        );
    }

    fn assert_api_key_helper_omitted(v: &serde_json::Value) {
        assert!(
            v.pointer("/apiKeyHelper").is_none(),
            "subscription: apiKeyHelper must be omitted entirely so claude can use its keychain OAuth path"
        );
    }

    fn assert_hooks_block_present(v: &serde_json::Value) {
        let hooks = v
            .pointer("/hooks")
            .and_then(|x| x.as_object())
            .expect("hooks block must be a JSON object");
        for event in HOOK_EVENTS {
            let entries = hooks
                .get(*event)
                .and_then(|x| x.as_array())
                .unwrap_or_else(|| panic!("hooks.{event} must be a JSON array"));

            // Every event carries the observe-only `astrid-emit` handler
            // FIRST. PreToolUse additionally carries the policy gate SECOND;
            // no other event has a second handler.
            let expected_len = if *event == "PreToolUse" { 2 } else { 1 };
            assert_eq!(
                entries.len(),
                expected_len,
                "hooks.{event} must have {expected_len} entr(y/ies)"
            );

            let command = entries[0]
                .pointer("/command")
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| panic!("hooks.{event}: command must be a string"));
            assert!(
                command.starts_with("astrid-emit "),
                "hooks.{event}: command must start with 'astrid-emit ' (got {command:?})"
            );
            let expected_topic = hook_topic(event);
            assert!(
                command.ends_with(expected_topic),
                "hooks.{event}: command must end with topic {expected_topic:?} (got {command:?})"
            );
            assert_eq!(
                entries[0].pointer("/type").and_then(|x| x.as_str()),
                Some("command"),
                "hooks.{event}: type must be \"command\""
            );
            assert_eq!(
                entries[0].pointer("/timeout").and_then(|x| x.as_u64()),
                Some(10),
                "hooks.{event}: timeout must be 10s"
            );

            if *event == "PreToolUse" {
                assert_pretooluse_gate(&entries[1]);
            }
        }
    }

    /// The PreToolUse `mcp_tool` gate handler shape: it targets the reserved
    /// gate tool on the registered Astrid server and passes the native tool's
    /// name + input through `${...}` substitution.
    fn assert_pretooluse_gate(entry: &serde_json::Value) {
        assert_eq!(
            entry.pointer("/type").and_then(|x| x.as_str()),
            Some("mcp_tool"),
            "PreToolUse gate handler type must be mcp_tool"
        );
        assert_eq!(
            entry.pointer("/server").and_then(|x| x.as_str()),
            Some(MCP_SERVER_NAME),
            "gate must target the registered Astrid MCP server"
        );
        assert_eq!(
            entry.pointer("/tool").and_then(|x| x.as_str()),
            Some(PRETOOLUSE_GATE_TOOL),
            "gate must call the reserved broker gate tool"
        );
        assert_eq!(
            entry.pointer("/input/tool_name").and_then(|x| x.as_str()),
            Some("${tool_name}"),
            "gate must forward the native tool name"
        );
        assert_eq!(
            entry.pointer("/input/tool_input").and_then(|x| x.as_str()),
            Some("${tool_input}"),
            "gate must forward the native tool input"
        );
    }

    // ----- The four mode-pair tests (full matrix). -----

    #[test]
    fn settings_headless_api_key() {
        let v = settings_json(&cfg(InteractionMode::Headless, AuthMode::ApiKey));
        assert_headless_shape(&v);
        assert_api_key_helper_present(&v);
        assert_hooks_block_present(&v);
    }

    #[test]
    fn settings_headless_subscription() {
        let v = settings_json(&cfg(InteractionMode::Headless, AuthMode::Subscription));
        assert_headless_shape(&v);
        assert_api_key_helper_omitted(&v);
        assert_hooks_block_present(&v);
    }

    #[test]
    fn settings_repl_api_key() {
        let v = settings_json(&cfg(InteractionMode::Repl, AuthMode::ApiKey));
        assert_repl_shape(&v);
        assert_api_key_helper_present(&v);
        assert_hooks_block_present(&v);
    }

    #[test]
    fn settings_repl_subscription() {
        let v = settings_json(&cfg(InteractionMode::Repl, AuthMode::Subscription));
        assert_repl_shape(&v);
        assert_api_key_helper_omitted(&v);
        assert_hooks_block_present(&v);
    }

    // ----- Added behavioural tests beyond the matrix. -----

    #[test]
    fn settings_repl_mode_omits_deny_list() {
        // Defence-in-depth: repl mode must NEVER carry the native-tool
        // deny list, even by accident — the user owns the full Claude
        // environment in repl mode.
        for am in [AuthMode::ApiKey, AuthMode::Subscription] {
            let v = settings_json(&cfg(InteractionMode::Repl, am));
            let deny = v
                .pointer("/permissions/deny")
                .and_then(|x| x.as_array())
                .expect("permissions.deny must be a JSON array");
            assert!(deny.is_empty(), "repl ({am:?}): deny list must be empty");
        }
    }

    #[test]
    fn settings_subscription_mode_omits_helper() {
        // Regression guard: subscription mode must not emit a stray
        // /bin/false helper (that would short-circuit the keychain
        // OAuth fallback that subscription mode relies on).
        for im in [InteractionMode::Headless, InteractionMode::Repl] {
            let v = settings_json(&cfg(im, AuthMode::Subscription));
            assert!(
                v.pointer("/apiKeyHelper").is_none(),
                "subscription ({im:?}): apiKeyHelper must be omitted"
            );
        }
    }

    #[test]
    fn settings_declares_hook_placeholders() {
        // The hooks block must be present, identical across all four
        // (mode, auth) combinations, and wire each event through
        // `astrid-emit <topic>` with timeout=10 so the runner's run-loop
        // validator receives the per-event hook envelope.
        for im in [InteractionMode::Headless, InteractionMode::Repl] {
            for am in [AuthMode::ApiKey, AuthMode::Subscription] {
                let v = settings_json(&cfg(im, am));
                assert_hooks_block_present(&v);
            }
        }
    }

    #[test]
    fn settings_never_emits_bin_true_placeholder() {
        // Regression guard: the pre-#814 no-op placeholder must never
        // reappear once the astrid-emit shim is wired in. Literal
        // assembled at runtime so a source-tree grep for the legacy
        // command reports zero matches; the v1 contract is "hooks emit
        // through astrid-emit, not the legacy placeholder".
        let legacy_command = format!("/bin/{}", "true");
        for im in [InteractionMode::Headless, InteractionMode::Repl] {
            for am in [AuthMode::ApiKey, AuthMode::Subscription] {
                let v = settings_json(&cfg(im, am));
                let hooks = v
                    .pointer("/hooks")
                    .and_then(|x| x.as_object())
                    .expect("hooks block must be a JSON object");
                for event in HOOK_EVENTS {
                    let entries = hooks
                        .get(*event)
                        .and_then(|x| x.as_array())
                        .unwrap_or_else(|| panic!("hooks.{event} must be a JSON array"));
                    for entry in entries {
                        // Non-command handlers (the PreToolUse `mcp_tool`
                        // gate) carry no `command` field — only the
                        // `astrid-emit` command handlers are in scope here.
                        let Some(command) = entry.pointer("/command").and_then(|x| x.as_str())
                        else {
                            continue;
                        };
                        assert_ne!(
                            command, legacy_command,
                            "({im:?}, {am:?}) hooks.{event}: \
                             /bin/true placeholder must not be emitted"
                        );
                        assert!(
                            !command.contains(&legacy_command),
                            "({im:?}, {am:?}) hooks.{event}: \
                             command must not contain legacy /bin/true (got {command:?})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn hook_events_and_topic_map_stay_in_sync() {
        // Defence-in-depth: the two source-of-truth tables for the
        // hook authoring contract must enumerate the same Claude
        // events, in the same order. A drift here would leave one
        // table referencing an event the other doesn't, silently
        // breaking the per-event topic lookup.
        assert_eq!(
            HOOK_EVENTS.len(),
            HOOK_TOPIC_MAP.len(),
            "HOOK_EVENTS and HOOK_TOPIC_MAP must enumerate the same events"
        );
        for (event, (k, _)) in HOOK_EVENTS.iter().zip(HOOK_TOPIC_MAP.iter()) {
            assert_eq!(
                event, k,
                "HOOK_EVENTS and HOOK_TOPIC_MAP must agree on order"
            );
        }
        for (_, topic) in HOOK_TOPIC_MAP {
            assert!(
                topic.starts_with("claude.v1.hook."),
                "topic {topic:?} must live under claude.v1.hook.*"
            );
        }
    }

    #[test]
    fn settings_never_emits_legacy_hook_disable_flag() {
        // Regression guard: the legacy hook-disable flag blocked status
        // lines AND the future hook bridge — must never reappear. Key
        // assembled at runtime to keep the literal name out of source
        // (so a `grep` for the legacy flag in production code reports
        // zero matches; the v1 contract is "hooks declared, not
        // disabled").
        let legacy_key = format!("/{}{}", "disableAll", "Hooks");
        for im in [InteractionMode::Headless, InteractionMode::Repl] {
            for am in [AuthMode::ApiKey, AuthMode::Subscription] {
                let v = settings_json(&cfg(im, am));
                assert!(
                    v.pointer(&legacy_key).is_none(),
                    "({im:?}, {am:?}): legacy hook-disable flag must not be set"
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // mcp_json — mode-gated.
    // ------------------------------------------------------------------

    #[test]
    fn mcp_headless_registers_sage_server() {
        for am in [AuthMode::ApiKey, AuthMode::Subscription] {
            let v = mcp_json(&cfg(InteractionMode::Headless, am), "alice");
            assert_eq!(
                v.pointer("/mcpServers/astrid/command")
                    .and_then(|x| x.as_str()),
                Some("astrid"),
                "headless ({am:?}): the Astrid MCP server command must be `astrid`"
            );
            let args: Vec<&str> = v
                .pointer("/mcpServers/astrid/args")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            assert_eq!(
                args,
                vec!["mcp", "serve", "--principal", "alice"],
                "headless ({am:?}): Astrid server must be `astrid mcp serve --principal <id>` with the baked principal"
            );
        }
    }

    #[test]
    fn mcp_repl_mode_also_registers_sage_server() {
        // The interactive REPL must be "on Astrid" too: it auto-discovers
        // this `.mcp.json` and gets the same Astrid MCP server headless does.
        for am in [AuthMode::ApiKey, AuthMode::Subscription] {
            let v = mcp_json(&cfg(InteractionMode::Repl, am), "alice");
            assert_eq!(
                v.pointer("/mcpServers/astrid/command")
                    .and_then(|x| x.as_str()),
                Some("astrid"),
                "repl ({am:?}): the Astrid MCP server must be registered"
            );
            let args: Vec<&str> = v
                .pointer("/mcpServers/astrid/args")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            assert_eq!(
                args,
                vec!["mcp", "serve", "--principal", "alice"],
                "repl ({am:?}): server must be `astrid mcp serve --principal <id>`"
            );
        }
    }

    // ------------------------------------------------------------------
    // PreToolUse native-tool gate — wiring + cross-function consistency.
    // ------------------------------------------------------------------

    #[test]
    fn pretooluse_is_the_only_event_with_a_second_handler() {
        let v = settings_json(&cfg(InteractionMode::Headless, AuthMode::ApiKey));
        let hooks = v.pointer("/hooks").and_then(|x| x.as_object()).unwrap();
        for event in HOOK_EVENTS {
            let len = hooks.get(*event).and_then(|x| x.as_array()).unwrap().len();
            if *event == "PreToolUse" {
                assert_eq!(len, 2, "PreToolUse must carry the observe emit + the gate");
            } else {
                assert_eq!(len, 1, "hooks.{event} must stay observe-only (one handler)");
            }
        }
    }

    #[test]
    fn gate_targets_the_registered_mcp_server() {
        // Cross-function consistency: the gate hook's `server` must be the
        // SAME server `mcp_json` actually registers, or Claude calls the gate
        // on a server that isn't connected and every gate decision fails open.
        // Both sides resolve from `MCP_SERVER_NAME`, so this pins that the
        // const is what each emits.
        let settings = settings_json(&cfg(InteractionMode::Headless, AuthMode::ApiKey));
        let gate = &settings
            .pointer("/hooks/PreToolUse")
            .and_then(|x| x.as_array())
            .unwrap()[1];
        let gate_server = gate.pointer("/server").and_then(|x| x.as_str()).unwrap();

        let mcp = mcp_json(&cfg(InteractionMode::Headless, AuthMode::ApiKey), "alice");
        let registered = mcp
            .pointer("/mcpServers")
            .and_then(|x| x.as_object())
            .unwrap();
        assert!(
            registered.contains_key(gate_server),
            "gate server {gate_server:?} must be a registered MCP server (have {:?})",
            registered.keys().collect::<Vec<_>>()
        );
        assert_eq!(gate_server, MCP_SERVER_NAME);
    }

    #[test]
    fn pretooluse_gate_tool_name_is_pinned() {
        // Value anchor for the cross-crate SYNC with
        // `sage_mcp::broker::PRETOOLUSE_GATE_TOOL`. No dependency edge between
        // the crates, so the name is mirrored, not shared; the astrid-mcp side
        // pins the same literal in `gate_tool_name_is_pinned`. A rename on one
        // side without the other silently disables the gate, so both anchor
        // the exact string and a deliberate edit must touch both tests.
        assert_eq!(PRETOOLUSE_GATE_TOOL, "astrid_pretooluse_gate");
    }

    // ------------------------------------------------------------------
    // Managed-settings tier — the staged, un-strippable enforcement body.
    // ------------------------------------------------------------------

    fn pretooluse_handlers(v: &serde_json::Value) -> &Vec<serde_json::Value> {
        v.pointer("/hooks/PreToolUse")
            .and_then(|x| x.as_array())
            .expect("hooks.PreToolUse must be an array")
    }

    #[test]
    fn managed_settings_carries_only_the_gate_hook() {
        let v = managed_settings_json(&cfg(InteractionMode::Headless, AuthMode::ApiKey));
        let hooks = v
            .pointer("/hooks")
            .and_then(|x| x.as_object())
            .expect("managed hooks must be an object");
        // Exactly one event (PreToolUse) with exactly one handler (the gate);
        // the observe-only astrid-emit plane stays in the local tier.
        assert_eq!(hooks.len(), 1, "managed hooks must carry only PreToolUse");
        let pre = pretooluse_handlers(&v);
        assert_eq!(pre.len(), 1, "managed PreToolUse must carry only the gate");
        assert_pretooluse_gate(&pre[0]);
        assert!(
            !v.to_string().contains("astrid-emit"),
            "managed tier must not duplicate the observe-only astrid-emit plane"
        );
    }

    #[test]
    fn managed_gate_is_byte_identical_to_local_gate() {
        // Claude merges hooks across tiers and dedups mcp_tool handlers by
        // (server, tool, input). The managed gate MUST equal the local gate
        // so the two collapse and the gate fires ONCE, not twice, once both
        // tiers are live.
        let c = cfg(InteractionMode::Headless, AuthMode::ApiKey);
        let local = settings_json(&c);
        let managed = managed_settings_json(&c);
        // Local PreToolUse is [observe-emit, gate]; managed is [gate].
        assert_eq!(
            &pretooluse_handlers(&local)[1],
            &pretooluse_handlers(&managed)[0]
        );
    }

    #[test]
    fn managed_reuses_the_binding_local_posture() {
        // The managed tier must carry the same un-strippable posture the
        // local tier authors — deny surface, dontAsk floor, sandbox,
        // apiKeyHelper — so stripping the local copy cannot relax them.
        for am in [AuthMode::ApiKey, AuthMode::Subscription] {
            let c = cfg(InteractionMode::Headless, am);
            let local = settings_json(&c);
            let managed = managed_settings_json(&c);
            for key in [
                "/permissions/allow",
                "/permissions/deny",
                "/permissions/defaultMode",
                "/sandbox",
                "/apiKeyHelper",
            ] {
                assert_eq!(
                    local.pointer(key),
                    managed.pointer(key),
                    "managed must mirror local at {key} ({am:?})"
                );
            }
        }
    }

    #[test]
    fn managed_settings_repl_imposes_gate_over_user_surface() {
        // Repl is the user's own environment: the managed tier still imposes
        // the operator's gate, but no permission lockdown (permissions stay
        // empty, as the local repl tier authors them).
        let v = managed_settings_json(&cfg(InteractionMode::Repl, AuthMode::Subscription));
        let pre = pretooluse_handlers(&v);
        assert_eq!(pre.len(), 1);
        assert_pretooluse_gate(&pre[0]);
        assert!(
            v.pointer("/permissions/allow")
                .and_then(|x| x.as_array())
                .unwrap()
                .is_empty(),
            "repl managed: allow must stay empty (user owns the surface)"
        );
    }

    // ------------------------------------------------------------------
    // KV key namespacing — guard against accidental collisions with
    // other capsules sharing the kv surface.
    // ------------------------------------------------------------------

    #[test]
    fn install_complete_key_is_claude_namespaced() {
        assert_eq!(install_complete_key("alice"), "claude.install.complete.alice");
    }

    // ------------------------------------------------------------------
    // Cross-crate deny-list mirror — full-membership drift guard.
    // ------------------------------------------------------------------

    #[test]
    fn required_denies_equal_canonical_binding_mirror() {
        // Drift guard for the mirror with `claude_runner::spawn::DENIED_TOOLS`
        // (claude-runner/src/spawn.rs). No dependency edge exists between the two
        // crates (each is a `cdylib`-only workspace member, so neither can
        // import the other's const as a library), so the lists cannot be
        // `assert_eq!`d directly. Instead this pins `REQUIRED_DENIES`
        // against a FULL, independently-written canonical set — the same
        // canonical surface the spawn-side test
        // (`denied_tools_equal_canonical_required_denies`) pins
        // `DENIED_TOOLS` to. The pairing is load-bearing: `REQUIRED_DENIES`
        // rides only the overridable Local tier (`settings.local.json`), so
        // a name present here but dropped from `DENIED_TOOLS` is denied
        // only in the defeasible tier and never reaches the binding
        // `--disallowedTools` CLI tier. Anchoring BOTH lists to the same
        // canonical set means a one-sided add/drop/substitute fails the
        // build on at least one side, forcing the mirror back into
        // alignment. Stronger than a count anchor: a same-size substitution
        // (swap one tool for another) is caught here.
        //
        // SYNC: this canonical set MUST match the `CANONICAL` set in
        // claude_runner::spawn::tests::denied_tools_equal_canonical_required_denies
        // (claude-runner/src/spawn.rs) exactly.
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

        // Compare as sets so a pure reordering of either list (no semantic
        // effect — deny order is irrelevant) does not spuriously fail,
        // while any membership difference (add, drop, substitute) does.
        let actual: std::collections::BTreeSet<&str> = REQUIRED_DENIES.iter().copied().collect();
        let canonical: std::collections::BTreeSet<&str> = CANONICAL.iter().copied().collect();

        let missing: Vec<&str> = canonical.difference(&actual).copied().collect();
        let unexpected: Vec<&str> = actual.difference(&canonical).copied().collect();
        assert!(
            missing.is_empty() && unexpected.is_empty(),
            "REQUIRED_DENIES drifted from the canonical binding mirror.\n  \
             missing from REQUIRED_DENIES: {missing:?}\n  \
             unexpected in REQUIRED_DENIES: {unexpected:?}\n  \
             re-sync REQUIRED_DENIES with claude_runner::spawn::DENIED_TOOLS.",
        );

        // Belt-and-braces: no duplicate collapsed the set below the list
        // length, which would mask a drift behind an equal-set comparison.
        assert_eq!(
            actual.len(),
            REQUIRED_DENIES.len(),
            "REQUIRED_DENIES contains a duplicate entry",
        );
    }

    // ------------------------------------------------------------------
    // Path layout — sanity that every emitted path lives under the
    // declared fs scope.
    //
    // Paths must all use the `home://` VFS scheme so the kernel binds
    // them to the invoking principal at check time. A literal `~/...`
    // path falls through to the workspace-root branch of the resolver
    // and writes land in the wrong place (silent data loss).
    // ------------------------------------------------------------------

    #[test]
    fn all_paths_live_under_home_scheme() {
        assert!(principal_home().starts_with("home://"));
        assert!(claude_dir().starts_with("home://"));
        assert!(projects_dir().starts_with("home://"));
        assert!(settings_path().starts_with("home://"));
        assert!(mcp_path().starts_with("home://"));
        assert!(managed_settings_path().starts_with("home://"));
    }

    #[test]
    fn no_path_carries_literal_tilde() {
        // Defence-in-depth regression — silent fall-through to workspace
        // root if any of these slip back into the legacy `~/...` form.
        for p in [
            principal_home(),
            claude_dir(),
            projects_dir(),
            settings_path(),
            mcp_path(),
            managed_settings_path(),
        ] {
            assert!(
                !p.starts_with('~'),
                "path '{p}' must use the home:// scheme, not a literal tilde"
            );
        }
    }
}
