# Astrid Oracles

Monorepo for every Astrid-governed oracle — external coding runtimes under Astrid: **Sage** (Claude Code), **Mimir** (Grok Build), **Sibyl** (Codex).

Oracles are not Astrid's own agents (those live elsewhere). These are Claude Code, Grok Build, and Codex bound into Astrid: plugin + distro + broker + optional supervisor.

One shared MCP broker. Product identity is typed. Host plugins keep different hooks and install paths.

## Layout

```
crates/
  oracle-core/       # Product, ProductProfile, NewTypes (DistroId, PrincipalFamily, …)
  oracle-broker/     # Shared MCP broker (discovery, policy, approval, execute)
  sage-mcp/         # Thin capsule → ProductProfile::SAGE
  mimir-mcp/        # Thin capsule → ProductProfile::MIMIR
  sibyl-mcp/        # Thin capsule → ProductProfile::SIBYL
  sage/             # Claude supervisor (headless/repl)
  sage-install/     # Claude home provisioner
  sage-completion/  # Anthropic API completion provider (optional)
  sibyl/            # Codex supervisor
  sibyl-install/    # Codex provisioner
plugins/
  sage/             # Claude Code plugin (.claude-plugin, SessionStart doctor, …)
  mimir/            # Grok Build plugin (.grok-plugin, …)
  sibyl/            # Codex plugin (.codex-plugin, PreToolUse hooks, …)
distros/
  sage.toml
  mimir.toml
  sibyl.toml
```

## Design

- **Kernel stays dumb.** Broker is a capsule; policy/approval are PDP at the edge.
- **One broker implementation.** `oracle-broker` is the only place tool discovery, confused-deputy gating, grants, and PreToolUse logic live.
- **Product identity is data.** `oracle_core::ProductProfile` holds every wire string (`mcp__sage__`, `sage.v1.audit.*`, principal family, …). Thin capsules call `oracle_broker::install(&ProductProfile::…)` then forward interceptors.
- **NewTypes, not string soup.** `DistroId`, `PrincipalFamily`, `McpToolPrefix`, `BusNamespace`, `LogTag`, …
- **Host plugins differ on purpose.** Claude, Grok, and Codex have different hook surfaces; plugins stay separate under `plugins/`.

## Build

```bash
# Shared libraries + unit tests (native)
cargo test -p oracle-core -p oracle-broker

# Product MCP capsules (wasm32-unknown-unknown via per-crate .cargo/config.toml)
cargo build -p sage-mcp -p mimir-mcp -p sibyl-mcp

# Supervisors / installers
cargo build -p sage -p sage-install -p sibyl -p sibyl-install
```

## Distros

```bash
astrid init --distro ./distros/sage.toml  --principal claude-code
astrid init --distro ./distros/mimir.toml --principal grok-code
astrid init --distro ./distros/sibyl.toml --principal sibyl-code
```

Capsule sources in the distros point at `@unicity-astrid/oracles`; the installer picks the release asset by capsule name.

## Principals

| Product | Principal family | MCP namespace |
|---------|------------------|--------------|
| Sage    | `claude-code`    | `mcp__sage__*` |
| Mimir   | `grok-code`      | `mcp__mimir__*` |
| Sibyl   | `sibyl-code`     | `mcp__sibyl__*` |

## License

MIT OR Apache-2.0
