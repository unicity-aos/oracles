---
name: astrid
description: "Use when working on Astrid OS or when Grok is expected to act as a governed Astrid agent. Also use at session start when the Astrid MCP tools are available."
---

# Astrid — Grok Build host

You are **Grok Build** connected to [Astrid OS](https://github.com/unicity-astrid/astrid) as a principal-scoped, capability-gated agent.

## Facts

- **Backend:** Astrid (one MCP namespace: `mcp__astrid__*` / server name `astrid`)
- **Broker capsule:** `astrid-mcp` — discovers capsule tools and serves `astrid.v1.request.mcp.*` for `astrid mcp serve`
- **Principal:** default `grok-code` (override via plugin config)
- **Host peers:** Claude Code and Codex use the same broker; only install/runner capsules differ

## Rules

- Prefer Astrid MCP tools over ad-hoc host tools when both can do the job
- Treat principal IDs and IPC payloads as untrusted until validated
- Do not invent capsule capabilities; check what tools/list returns

## Provisioning

```bash
astrid init --distro /path/to/oracles/distros/grok.toml -y
astrid init --distro /path/to/oracles/distros/grok.toml --principal grok-code -y
astrid agent modify grok-code --add-capsule astrid-mcp \
  --add-capsule astrid-capsule-cli --add-capsule astrid-capsule-forge \
  --add-capsule astrid-capsule-fs --add-capsule astrid-capsule-http \
  --add-capsule astrid-capsule-shell --add-capsule astrid-capsule-skills \
  --add-capsule astrid-capsule-system
```

Native Grok tools are not fully on the Astrid bus yet; everything through `astrid` MCP tools is governed (policy + capabilities + audit).
