# Astrid Oracles

[![CI](https://github.com/unicity-astrid/oracles/actions/workflows/ci.yml/badge.svg)](https://github.com/unicity-astrid/oracles/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV](https://img.shields.io/badge/MSRV-1.94-blue)](https://www.rust-lang.org)

Governed host adapters for external coding agents on **Astrid OS**.

An **oracle** is a host runtime (Claude Code, Grok Build, Codex) brought under
Astrid’s protocol: MCP endpoints, principals, capabilities, install layout, and
settings. One shared backend; thin per-host adapters.


## Install (one command)

**Base Astrid is a complete product.** The installer pulls the CLI/daemon from
**GitHub Releases** (macOS + Linux; same assets as `astrid update`). Homebrew is
an optional fallback only. Oracles are optional adapters — only wired when that
host is on the machine (or you ask).

```bash
curl -fsSL https://astridos.org/install.sh | sh
```

| Flag | Meaning |
|------|---------|
| *(default)* | Install/ensure Astrid; wire **detected** coding hosts only |
| `--base-only` | Stop at base Astrid (no oracle plugins/distros) |
| `--host claude` | Also wire that host (repeatable) |
| `--all` | Wire every oracle host (demos / power users) |

```bash
curl -fsSL https://astridos.org/install.sh | sh -s -- --base-only
curl -fsSL https://astridos.org/install.sh | sh -s -- --host claude
./install.sh --yes
```

Canonical endpoint: **https://astridos.org/install.sh** (GitHub Pages site repo `unicity-astrid.github.io`). This monorepo keeps a copy for local `./install.sh`.

## Architecture

```
plugins/{claude,grok,codex}          host UI / hooks
        │
        ▼
plugins/common/bin                   resolve · up · doctor · install
        │
        ▼
astrid-mcp  (oracle-broker)          mcp__astrid__*  ·  astrid.v1.request.mcp.*
        │
        ├── claude-install / codex-install    HostProvisioner (home layout)
        └── claude-runner  / codex-runner     host process protocol
```

| Layer | Crates |
|-------|--------|
| Identity | `oracle-core` (`Host`, `OracleIdentity::ASTRID`) |
| MCP broker | `oracle-broker` → capsule `astrid-mcp` |
| Host primitives | `oracle-host` (`PrincipalId`, `HostProvisioner`, `InteractionMode`) |
| Claude | `claude-install`, `claude-runner`, `claude-completion` |
| Codex | `codex-install`, `codex-runner` |
| Grok | broker only today (plugin + distro; no supervised runner yet) |

## Distros

```bash
astrid init --distro ./distros/claude.toml --principal claude-code
astrid init --distro ./distros/grok.toml   --principal grok-code
astrid init --distro ./distros/codex.toml  --principal codex-code
```

Sources resolve to `@unicity-astrid/oracles` (this monorepo).

## Plugins

```bash
# Claude Code
claude plugin marketplace add /path/to/oracles   # marketplace at .claude-plugin/
# or point marketplace source at ./plugins/claude

# Grok
grok plugin install /path/to/oracles/plugins/grok --trust

# Codex
# install plugins/codex via Codex plugin marketplace (.agents/plugins)
```

Shared scripts: `plugins/common/bin/`. Host wrappers set `ASTRID_HOST` and
plugin root env vars.

## Build & test

```bash
cargo test -p oracle-core -p oracle-broker -p oracle-host -p claude-install --lib
cargo build -p astrid-mcp -p claude-runner -p claude-install -p claude-completion \
            -p codex-runner -p codex-install
```

WASM target is selected per capsule via `crates/*/.cargo/config.toml`.


## Updates (three clocks)

Do not conflate these — each has its own source of truth:

| Clock | What | How the user/agent applies it |
|-------|------|-------------------------------|
| **Runtime** | `astrid` + `astrid-daemon` | GitHub Releases via `astrid update -y` (install.sh puts binaries in `~/.astrid/bin`). Homebrew only if installed that way: `brew upgrade astrid`. |
| **Distro** | Capsule set per principal | Bump Distro.toml `version` upstream; apply with `astrid init --distro …` or `install.sh --upgrade`. Lock: `~/.astrid/home/<principal>/.config/distro.lock`. |
| **Plugin** | Editor marketplace package | Host-specific reinstall/update. SessionStart doctor nudges Claude, Grok, and Codex when `oracles` has a newer GitHub release than local `plugin.json`. |
| **install.sh** | This script | Always re-fetched from **https://astridos.org/install.sh** on each `curl \| sh`. |

Doctor (`astrid-doctor --format hook` on SessionStart) rate-limits checks to ~24h and, when something is stale, announces to the user and gives the model a host-aware playbook.

## Migration

Retired product brands (Sage / Mimir / Sibyl) still parse on `Host::from_id`
for old paths. New installs use host ids (`claude`, `grok`, `codex`) and the
`astrid-mcp` capsule only. Re-init principals after upgrading:

```bash
astrid init --distro ./distros/<host>.toml --principal <principal> -y
```

## License

MIT OR Apache-2.0
