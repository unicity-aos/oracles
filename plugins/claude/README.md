# Unicity AOS for Claude Code

This plugin connects a normal Claude Code session to Unicity AOS as an
explicitly provisioned principal. AOS remains the product boundary; the plugin
is a thin host adapter for MCP configuration, readiness reporting, operator
commands, and the optional HUD.

## What it installs

- `.mcp.json` attaches the public `aos` MCP server to the invoking project
  workspace.
- `bin/aos-up` remains the host hook adapter and legacy serve entrypoint.
- SessionStart performs the short `aos mcp ready --format hook` check and
  registers the host session; provisioning is an explicit installer action.
- `/unicity-aos:doctor`, `/unicity-aos:status`, `/unicity-aos:whoami`,
  `/unicity-aos:capsules`, and `/unicity-aos:hud`
  expose operator views.
- `skills/forge` documents end-to-end AOS capsule authoring.
- `output-styles/aos.md` grounds Claude as a principal-scoped AOS agent.

The adapter does not start a private daemon itself. The `aos` command owns the
bundled runtime, authenticated principal selection, and quiet ephemeral startup.
The default MCP path attaches to the shared project gateway; set
`AOS_MCP_MODE=per-session` to use the bounded legacy `mcp serve` process.

## Install and provision

The repository installer ensures signed Unicity AOS, installs the marketplace
plugin, and provisions the selected host:

```sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/unicity-aos/oracles/main/install.sh \
  | sh -s -- --host claude
```

The base product installer remains:

```sh
curl --proto '=https' --tlsv1.2 -fsSL https://aos.unicity.ai/install.sh | sh
```

Marketplace commands, when installing by hand:

```sh
claude plugin marketplace add unicity-aos/oracles
claude plugin install unicity-aos@unicity-aos-oracles
```

## Principal model

`userConfig.principal` defaults to `claude-code`. It is a scoped host principal,
not the administrative `default` principal. The value is passed before the
runtime command:

```sh
aos --principal claude-code mcp attach --workspace "$PWD"
```

On a blank slate, the host-scoped installer creates `claude-code` and installs
only the Claude oracle pack. Later startup failures remain visible with recovery
guidance; the wrapper never broadens the principal's authority.

## Tool boundary

Claude exposes the registered server's tools as `mcp__aos__*`. Native Claude
Code tools continue to use Claude Code's sandbox. Calls through `mcp__aos__*`
also pass AOS policy, capability, and audit enforcement.

The internal runtime broker capsule remains `aos-mcp`, and its stable bus
contracts remain under `astrid.v1.*`. Those are engine identifiers, not the
customer-facing product or MCP namespace.

## HUD

Run `/unicity-aos:hud` for the opt-in status-line snippet. It renders:

```text
⬡ aos:<principal> ●  │ <model> │ <directory> ⎇ <branch> │ <context> │ <cost>
```

Green means the runtime and internal broker both answer; yellow means the
runtime is reachable without the broker; dim means the runtime is stopped.
Health is cached briefly under `$AOS_HOME/cache/oracles`.

## Development overrides

- `AOS_BIN=/absolute/path/to/aos` selects an explicit product command.
- `AOS_HOME=/absolute/product/home` selects `<home>/bin/aos` and remains
  authoritative if set.
- An optional `AOS_BIN` entry in `.mcp.json` supports a checked-out product
  build.

The resolver never falls back to `~/.astrid` and never invokes the private
runtime command directly.
