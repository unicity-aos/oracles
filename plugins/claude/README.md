# Astrid — Claude Code plugin

Turn a vanilla `claude` REPL into a **governed Astrid agent**. Installing this
plugin registers the Astrid MCP server (`astrid mcp serve`) so Claude Code
discovers and calls the live Astrid capsule tool surface (filesystem, http,
shell, system, skills, …), scoped to an Astrid principal — with the daemon
brought up automatically and torn down when you're done.

The integration lives entirely in **Claude Code's** plugin system. Astrid stays
agent-agnostic — it's just an MCP server on the bus; this plugin is the
Claude-specific adapter. A Codex/Gemini adapter would be its own plugin against
the same `astrid mcp serve` surface.

## What it bundles

- **MCP server** (`.mcp.json` → `bin/astrid-up`) — ensures a daemon is running,
  then becomes `astrid mcp serve --principal <you>`. The daemon is booted in
  **ephemeral** mode: it self-cleans ~30s after the last client disconnects, so
  it lives exactly as long as the editor. An already-running daemon (persistent
  or shared with another session) is reused untouched.
- **Runtime bootstrapper** (`bin/astrid-install`) — installs the Astrid runtime
  (`astrid` + `astrid-daemon`) via Homebrew, or a prebuilt release binary into
  `~/.astrid/bin`, so the runtime is not a manual prerequisite. Invoked
  *explicitly* — by you, or by Claude when the SessionStart doctor flags Astrid
  missing — never silently at boot (the MCP server has no TTY and owns stdout for
  JSON-RPC, so it can't prompt or install behind your back).
- **SessionStart doctor** (`hooks/hooks.json` → `bin/astrid-doctor`) — at
  session start, injects who you are (principal), what governs you, and the
  daemon/broker readiness state into context, with the exact fix for any gap
  (including offering to run the bootstrapper when Astrid isn't installed).
- **Astrid HUD status line** (`bin/astrid-statusline`) — overlays your principal
  and daemon state onto Claude Code's bottom bar (see *HUD* below).
- **`/astrid:*` commands** — `/astrid:whoami`, `/astrid:status`,
  `/astrid:capsules`, `/astrid:doctor`, `/astrid:hud`. Read-only operator
  views, backed by the `astrid` CLI.
- **Capsule-authoring skill** (`skills/forge/SKILL.md` → `/astrid:forge`) — the
  complete, self-contained guide to writing an Astrid capsule: the minimal file
  set, the `#[capsule]`/`#[astrid::tool]` macros, `Capsule.toml` + the bus ACL,
  the build→install→call loop, and every footgun. Auto-invokes when you ask to
  create or build a capsule, and pairs with the daemon's forge tools
  (`scaffold_capsule`, `validate_manifest`, `capsule_doctor`, …) when the forge
  capsule is loaded.
- **Astrid identity output-style** — *Astrid agent*: frames Claude as a
  capability-scoped, audited agent acting for a principal (coding instructions
  preserved). Enable via `/output-style`.
- **`userConfig.principal`** — the Astrid principal this session acts as.
  Defaults to `claude-code`, a least-authority agent in the custom `claude`
  family group (self-scoped baseline `self:*,delegate:self:*`, **not** the admin
  `default`), auto-provisioned on first launch and decoupled from your operator
  CLI context (which stays `default`). Set to `default` to run with admin
  authority, or any name to run as that scoped agent; clearing it falls back to
  the least-authority `claude-code` — never the admin `default`.

## Prerequisites

- The **Astrid runtime** (`astrid` + `astrid-daemon`) as a matching pair,
  **from a revision that has `astrid mcp serve`**. Resolution order (first
  hit wins):
  1. `ASTRID_BIN` + `ASTRID_DAEMON` (both executable)
  2. `ASTRID_BIN_ROOT` env (directory containing both binaries)
  3. `ASTRID_BIN_ROOT` in this plugin's `.mcp.json` `env` block
  4. Nearby worktree build: walk up from cwd / plugin root for
     `core/target/{debug,release}` or `target/{debug,release}`
  5. Install locations: `~/.cargo/bin`, `~/.astrid/bin`, Homebrew, `/usr/local/bin`, PATH

  Dev checkouts therefore prefer a local `core/target/debug` over a stale
  Homebrew install. If a daemon is already running from a *different* binary
  than the one resolved, `bin/astrid-up` refuses to attach (fail-closed) and
  tells you to stop it first — mixed CLI/daemon pairs are a common cause of
  MCP handshake failures.

  Don't have a runtime? The plugin ships `bin/astrid-install` (Homebrew, or a
  prebuilt release binary) and the SessionStart doctor offers to run it when
  it's missing — so the runtime is not a hard prerequisite.
  (Or: `brew install unicity-astrid/tap/astrid`.)

  **Pin a worktree build explicitly:**

  ```jsonc
  // astrid-plugin/.mcp.json — env block
  "env": {
    "ASTRID_BIN_ROOT": "/path/to/astrid/core/target/debug"
  }
  ```

- The **astrid-mcp broker** capsule loaded in the daemon (it answers
  `astrid.v1.request.mcp.*`). Without it, the server connects but lists no
  tools. `/astrid:doctor` tells you if it's missing.

## Install (the wow path)

**Dev / today — load it raw, no install:**

```sh
claude --plugin-dir /path/to/oracles/plugins/claude
```

Leave the principal at its `claude-code` default (a scoped agent in the `claude`
family group, auto-provisioned on first launch) — or set your own. The daemon
starts on first launch;
`mcp__astrid__*` tools appear.

**Distributed — your own marketplace (no Anthropic gatekeeping):**

```sh
# the marketplace manifest lives at this repo's root
claude plugin marketplace add unicity-astrid/oracles       # or a local path
claude plugin install astrid@astrid
```

`--scope user` makes every `claude` session in every project an Astrid agent.

## HUD (status line)

A plugin can't register the *main* status line — Claude Code only lets a plugin
default `agent`/`subagentStatusLine`. Wire it once yourself, or run
`/astrid:hud` to get the exact snippet (resolved to this plugin's path) and have
Claude offer to add it:

```jsonc
// ~/.claude/settings.json
"statusLine": {
  "type": "command",
  "command": "<plugin-root>/bin/astrid-statusline",
  "padding": 0
}
```

Renders: `⬡ astrid:<principal> ●  │ <model> │ <dir> ⎇ <branch> │ <context-bar> │ <cost>`
— the dot reflects governance health: green ● when governed (daemon up **and** the
astrid-mcp broker loaded), yellow ◐ when the daemon is up but the broker is missing
(native tools run **ungoverned**), dim ○ when the daemon is down. It is a cached
round-trip check (`astrid status`), not a socket-exists test — cooperative plugin
mode has no sandbox floor under a dead gate, so the dot must not claim governance
it can't back.

## Caveat: tool *calls* need a trusted ingress (one-time)

`tools/list` is ungated — Claude Code will **see** the Astrid tools immediately.
`tools/call` is confused-deputy gated by the astrid-mcp broker: the **first** tool
call from a new client session opens a one-time interactive consent prompt. Approving
it records trust for that session's `source_id` (under the per-principal KV key
`mcp.ingress.trust.<source_id>`), and every later call in the session dispatches
without re-prompting. There is deliberately **no** operator allow-list or config key:
trust is granted only by accepting the prompt, so a non-ingress capsule can never
puppet the broker into acting on your behalf. If your client can't display the prompt
(no MCP elicitation support), the call fails closed — the denial message names the
`source_id` and the next step (approve it from a client that renders elicitation, such
as the Astrid CLI uplink). (Single-tenant: you the operator
are admin via `default`, so authorizing your own session's ingress is a
self-stamp — fine on your own machine. The session itself runs as the scoped
`claude-code` principal, and per-connection auth has since landed: `agent create`
mints a per-principal ed25519 keypair and `astrid mcp serve` signs the socket
handshake with it, so the daemon cryptographically binds the connection to that
principal — the scoping is now an enforced floor, not just an honest default. A
connection that can't sign is stamped the no-capability `anonymous` (fail-closed).)
`/astrid:doctor` flags this if your calls come back denied.

## Not yet: native-action governance on the bus

Today the plugin governs everything that flows **through** Astrid (every
`mcp__astrid__*` call passes the broker policy gate + capability checks + audit)
and observes session start. It does **not** yet push Claude's *native* tool use
(Bash/Read/Write/Edit) onto the Astrid bus — `astrid-emit` requires a sage spawn
token that an interactive session doesn't have, and the CLI has no token-free
publish path. Surfacing native actions for capsule-side governance needs a small
core addition (a principal-authenticated emit path); it's the next milestone for
the hook plane, not something the plugin can close on its own.
