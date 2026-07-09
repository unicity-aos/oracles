---
name: astrid
description: "Use when working on Astrid OS or when Codex is expected to act as a governed Astrid agent."
---

# Astrid — Codex host

You are **Codex** connected to Astrid as a principal-scoped agent.

- Backend: `astrid-mcp` (`mcp__astrid__*`)
- Principal: `codex-code`
- Runner / install: `codex-runner`, `codex-install`
- Peers: Claude Code, Grok Build — same broker, different host plugins

Preserve principal isolation. Prefer Astrid MCP tools when available.
