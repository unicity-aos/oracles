# sibyl

Codex headless agent runner for Astrid OS.

This capsule accepts `sibyl.v1.request.*` IPC messages, runs bounded
`codex exec --json` turns under a per-principal Codex policy, mirrors Codex
JSONL records onto `sibyl.v1.event.<session_id>.codex.*`, and publishes
`sibyl.v1.event.*` lifecycle events.

## Session Model

Sibyl uses the same identity split as Sage:

- `principal_id` is the durable Astrid identity that owns policy, config,
  secrets, memory, budgets, and audit scope.
- `session_id` is one Codex runtime/thread/window. It is interpolated into IPC
  topics and must be a single topic segment, so dots are rejected.
- `workspace_id` is optional execution context for a mounted CWD, worktree, or
  future remote workspace. It is not an identity boundary.

The runner records minimal `sibyl.session.<session_id>` metadata in KV before
running a headless turn. Hook shims publish to
`sibyl.v1.hook.<session_id>.<event>` with the original Codex hook payload
base64-encoded inside an attribution envelope.

Headless `codex exec` runs receive `ASTRID_PRINCIPAL_ID`, `ASTRID_SESSION_ID`,
`ASTRID_WORKSPACE_ID` when known, and a per-session `ASTRID_HOOK_TOKEN`.
Incoming hook events are audited as verified only when that token matches the
registered session token.

## Codex-Specific Controls

`sibyl.v1.request.settings.set` supports the Sage-like governance fields
`interaction_mode`, `approval_policy`, `sandbox_mode`, `model`, and `profile`,
plus Codex-native execution knobs:

- `ephemeral` maps to `codex exec --ephemeral`.
- `ignore_user_config` maps to `codex exec --ignore-user-config`.
- `ignore_rules` maps to `codex exec --ignore-rules`.
- `skip_git_repo_check` maps to `codex exec --skip-git-repo-check`.
- `mirror_json_events` controls Astrid bus mirroring for `codex exec --json`.

On first run, before a principal has persisted settings in KV, these values are
seeded from the capsule `[env]` overlay written by install/init. After
`settings.set` writes KV, the persisted settings are authoritative.
