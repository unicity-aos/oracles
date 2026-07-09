# Astrid Oracles

External coding runtimes bound into **Astrid**. One backend, host adapters
only where the host product forces difference.

## Shared vs host-specific

| Crate | Role |
|-------|------|
| `oracle-core` | `Host`, `HostProfile`, singleton `OracleIdentity` (wire: `mcp__astrid__*`) |
| `oracle-broker` | Shared MCP broker (discovery, policy, approval, execute) |
| `oracle-host` | Shared host primitives: `PrincipalId`, atomic fs, `HostProvisioner` install loop, topics |
| `astrid-mcp` | The only broker capsule |
| `codex-install` | Thin `HostProvisioner` for `.codex/` |
| `sage-install` | Claude provisioner — uses shared ids/fs; richer config-aware loop still host-local |
| `sage` | Claude `claude -p` supervisor (protocol-specific) |
| `codex-runner` | Codex bounded-exec runner (protocol-specific) |
| `sage-completion` | Optional Anthropic API completion provider |

Claude stream-json and Codex `exec` are **different host protocols** — those
runners cannot be one codec. What they **must** share is already shared:
identity, install loop, ids, atomic writes, MCP broker.

```
plugins/{claude,grok,codex}  →  astrid-mcp (OracleIdentity::ASTRID)
                             →  host provisioner (HostProvisioner)
                             →  host runner (if supervised)
```

## Distros

```bash
astrid init --distro ./distros/claude.toml --principal claude-code
astrid init --distro ./distros/grok.toml   --principal grok-code
astrid init --distro ./distros/codex.toml  --principal codex-code
```

## Build

```bash
cargo test -p oracle-core -p oracle-broker -p oracle-host --lib
cargo build -p astrid-mcp -p codex-install -p codex-runner -p sage -p sage-install
```

## License

MIT OR Apache-2.0
