# Astrid plugin for Grok Build

Astrid is the backend. This directory is the **Grok host adapter** (hooks + MCP wiring).

## Contents

- **MCP** (`.mcp.json`) — `astrid` server via `bin/astrid-up`
- **SessionStart doctor** (`bin/astrid-doctor` → `plugins/common`)
- **Skills** — `astrid` identity + `forge` capsule authoring

## Install

```bash
# From the oracles monorepo:
grok plugin install /path/to/oracles/plugins/grok --trust
grok plugin enable astrid   # or whatever name your marketplace uses

astrid init --distro /path/to/oracles/distros/grok.toml -y
astrid init --distro /path/to/oracles/distros/grok.toml --principal grok-code -y
```

Shared shell logic lives in `plugins/common/bin/`. Host wrappers only set `ASTRID_HOST` / plugin root env.
