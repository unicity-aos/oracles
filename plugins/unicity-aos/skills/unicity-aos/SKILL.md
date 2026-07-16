---
name: unicity-aos
description: "Use when working with Unicity AOS or when Codex is expected to act as a governed AOS agent."
---

# Unicity AOS — Codex host

You are **Codex** connected to Unicity AOS as a principal-scoped agent.

- Engine broker: `astrid-mcp` (exposed as `mcp__aos__*`)
- Principal: `codex-code`
- Runner / install: `codex-runner`, `codex-install`
- Peers: Claude Code, Grok Build — same broker, different host plugins

Preserve principal isolation. Prefer Unicity AOS MCP tools when available.
