# codex-runner

Codex host runner capsule for Astrid oracles.

Accepts `codex.v1.request.*` IPC, runs bounded `codex exec` turns (or refuses
spawn in REPL mode), optionally mirrors JSONL onto `codex.v1.event.*`, and
publishes lifecycle events.

## Identity

- Principal family: `codex-code`
- Config KV: `codex.principal.config`
- Sessions: `codex.session.<id>`
- Hooks: `codex.v1.hook.<session_id>.<event>`

Same identity split as Claude: principal for capability scope, session for
concurrent windows.

## Shared backend

MCP tools go through **`astrid-mcp`**, not a Codex-branded broker. This crate
is host protocol only.
