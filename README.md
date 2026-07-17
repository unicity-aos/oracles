# Unicity AOS Oracles

[![CI](https://github.com/unicity-aos/oracles/actions/workflows/ci.yml/badge.svg)](https://github.com/unicity-aos/oracles/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](#license)

Host integrations that connect Claude Code, Codex, and Grok Build to Unicity
AOS. An oracle gives a coding host an AOS principal, the AOS MCP tool surface,
and host-specific configuration while leaving the runtime boundary intact.

## Install

Install Unicity AOS first:

```sh
curl -fsSL https://aos.unicity.ai/install.sh | sh
```

Then add the detected coding hosts:

```sh
curl -fsSL https://raw.githubusercontent.com/unicity-aos/oracles/main/install.sh | sh
```

Select hosts explicitly when desired:

```sh
curl -fsSL https://raw.githubusercontent.com/unicity-aos/oracles/main/install.sh \
  | sh -s -- --host codex
```

The installer is idempotent. It provisions a least-authority host principal,
installs the exact signed oracle pack, grants that principal only the pack's
capsules, and installs the host marketplace plugin. It writes product state
under `~/.aos`; it never imports or changes a standalone `~/.astrid` tree.

Claude's supervised headless mode runs `claude -p` and requires an Anthropic
API key. Subscription authentication is supported only for Claude Code's own
interactive REPL:

```sh
curl -fsSL https://raw.githubusercontent.com/unicity-aos/oracles/main/install.sh \
  | sh -s -- --host claude --claude-mode repl --claude-auth subscription
```

## Host packs

Oracle packs are additive components, not replacement operating-system
distributions.

| Host | Principal | Additions on top of Community Edition |
|---|---|---|
| Claude Code | `claude-code` | `aos-mcp` |
| Codex | `codex-code` | `aos-mcp` |
| Grok Build | `grok-code` | `aos-mcp` |

Pack manifests live under `packs/`. Host plugins are installed from the signed
release snapshot under `~/.aos/extensions/oracles/plugins/<version>`, never from
a moving repository branch. A successful end-to-end install commits a versioned
receipt under `~/.aos/extensions/oracles/<host>/releases/<version>` and advances
`current`; `Pack.lock` remains as the stable compatibility path. A failed plugin
install never writes a success receipt.

## Architecture

```text
host marketplace plugin
        |
        v
aos --principal <host>-code mcp serve
        |
        v
aos-mcp + host capsules
        |
        v
Unicity AOS Community Edition
```

The customer-facing server, broker capsule, and tool namespace are `aos`,
`aos-mcp`, and `mcp__aos__*`. Neutral runtime identifiers remain unchanged
behind that adapter: `astrid.v1.*`, `astrid-sdk`, the `astrid:*` WIT world, and
the bundled runtime binaries retain their permanent names and provenance.

## Develop

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
scripts/sync-plugins.sh
```

Capsules target `wasm32-unknown-unknown`. Build installable archives with the
`astrid-build` binary from the exact Astrid Runtime release pinned by the AOS
compatibility contract; raw Cargo `.wasm` files are not installable capsules.

## License

MIT OR Apache-2.0
