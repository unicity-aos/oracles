# Astrid Oracles

Governed host adapters for external coding agents on **Astrid OS**.

An **oracle** is a host runtime (Claude Code, Grok Build, Codex) brought under
Astrid’s protocol: MCP endpoints, principals, capabilities, install layout, and
settings. One shared backend; thin per-host adapters.

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

## Migration

Retired product brands (Sage / Mimir / Sibyl) still parse on `Host::from_id`
for old paths. New installs use host ids (`claude`, `grok`, `codex`) and the
`astrid-mcp` capsule only. Re-init principals after upgrading:

```bash
astrid init --distro ./distros/<host>.toml --principal <principal> -y
```

## License

MIT OR Apache-2.0
