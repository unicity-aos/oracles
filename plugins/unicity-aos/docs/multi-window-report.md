# Multiple Codex Windows With Unicity AOS

Each Codex window loads the Unicity AOS plugin independently. The plugin gives
every window the durable `codex-code` principal by default and supplies a
separate session identifier for hook attribution.

## Identity model

- Principal: durable governed identity, normally `codex-code`.
- Session: one Codex conversation or window.
- Workspace: the current working directory, reduced to a stable local token.

The principal owns capabilities, budgets, capsule access, and durable policy.
The session and workspace identifiers provide attribution without multiplying
long-lived principals for ordinary UI windows.

## Runtime path

Codex starts the MCP adapter from `.mcp.json`:

```text
bin/aos-up --principal codex-code
```

The adapter executes:

```text
aos --principal codex-code mcp serve
```

The public `aos` command selects the AOS home, starts the bundled runtime when
needed, authenticates the principal, and exposes the capsule tool surface. The
plugin does not launch the engine daemon or inspect its private home layout.

## Hook attribution

Hook envelopes carry:

- `ASTRID_PRINCIPAL_ID`
- `ASTRID_SESSION_ID`
- `ASTRID_WORKSPACE_ID`
- `ASTRID_HOOK_TOKEN`

Those names are engine protocol identifiers. They remain stable even though
the plugin, MCP server, scripts, and user commands are Unicity AOS branded.
Hooks publish on `codex.v1.hook.<session>.<event>` through the public AOS CLI.

Hook delivery is at-least-once and windows may interleave. Consumers must be
idempotent and must not infer ordering across sessions.

## Failure behavior

Local hook emission is fail-open by default so a temporarily unavailable AOS
runtime does not wedge Codex. Managed deployments can set
`ASTRID_CODEX_HOOK_FAIL_CLOSED=1` when policy requires hook acknowledgement.
MCP startup itself fails if the AOS CLI cannot be resolved or cannot establish
the principal-scoped runtime connection.

## Explicit autonomous agents

Opening another UI window does not create another governed identity. When a
workflow deliberately creates an autonomous agent with independent policy,
quota, or audit ownership, provision a distinct principal and launch that
session with the matching `ASTRID_PRINCIPAL_ID`.
