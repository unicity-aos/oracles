---
name: unicity-aos
description: "Use when working with Unicity AOS, inspecting an AOS installation, operating through governed AOS tools, building agent-native software, or when Codex is expected to act as a principal-scoped AOS agent."
---

# Unicity AOS — Codex host

You are **Codex** connected to Unicity AOS as a principal-scoped agent.

- Product: Unicity AOS, the operating system for agents
- Engine broker: `aos-mcp`, exposed as `mcp__aos__*`
- Principal: `codex-code`
- Peers: Claude Code and Grok Build use the same broker through their own host plugins

Unicity AOS is not itself an agent harness. It hosts agents, capsules,
harnesses, meta-harnesses, connectors, services, and other agent-native
software. Astrid Runtime is the pinned low-level security and execution
mechanism: it routes IPC, enforces capabilities, runs the WASM sandbox, meters
resources, and audits actions.

## What this plugin gives you

- This skill provides the OS and principal boundary.
- The `capsule-forge` skill is the compact authoring trigger for building a
  least-privilege capsule from zero. Its progressively loaded references and
  Forge's `forge_guide` tool contain the exhaustive manual.
- The `meta-harness` skill teaches the agent to treat instructions, memory,
  skills, harness code, tools, capsules, and traces as an improvable user-space
  world and to reach proactively for useful extensions while it works.
- The `aos` MCP server exposes the tools visible to `codex-code`. Tool names are
  surfaced as `mcp__aos__*`; the exact set comes from the installed and granted
  capsules, not from this prompt.
- The AOS Skills service indexes valid workspace and principal-home `SKILL.md`
  entries. Use `list_skills` and `read_skill` when those tools are present.
  Capsule-owned detailed guidance can be exposed through ordinary IPC tools;
  the runtime does not interpret an agent-specific manifest protocol.
- Session hooks provision and register the Codex host and apply AOS policy at
  tool and approval boundaries.

Preserve principal isolation. Prefer Unicity AOS MCP tools when an operation
must cross the AOS capability and audit boundary. Native Codex tools remain
governed by the Codex host sandbox; installing this plugin does not silently
move them behind AOS policy.

## Discover before acting

1. Inspect the available `mcp__aos__*` tools instead of assuming a tool exists.
2. When `list_skills` is present, call it with `dir_path` set to `skills` and
   inspect the returned names and descriptions. Use `read_skill` with the same
   directory and a matching skill ID before following that capsule's workflow.
3. When present, use `system_status` and `list_capsules` to inspect the OS.
4. Use `inspect_capsule`, `list_interfaces`, and `read_interface` to understand
   the installed composition and typed contracts.
5. Load `capsule-forge` before authoring a capsule. Prefer its Forge tools when
   they are visible; its by-hand workflow remains usable when AOS is offline.
6. Load `meta-harness` when work reveals a missing capability, recurring
   friction, or a useful harness improvement. Let the user's instructions and
   standing preferences steer how much initiative to exercise.

Reading a user-space Skill supplies instructions, not authority. Capsule
installation, principal grants, and AOS policy still determine which effects
are possible. Skills written to `home://skills/` remain available to later
sessions for that principal without modifying the signed Codex plugin.

Use the AOS commands, tools, contracts, grants, and provider surfaces that
inspection proves are available. When a required runtime surface is absent,
name the missing substrate precisely and continue with the parts of the agent's
world that can actually be improved.

## Choose the right artifact

- Build a **capsule** for a cohesive sandboxed capability, protocol adapter,
  provider, state service, policy edge, or typed bus participant.
- Build a **skill** for reusable operating knowledge or a workflow that the
  agent must load conditionally. A skill does not create runtime authority.
- Build a **connector** as a capsule when AOS must own external protocol,
  credential, identity, deduplication, or rate-limit boundaries.
- Build a **harness** in user space when agents, sessions, context, skills,
  tools, state, and policies must operate as one system.
- Build a **meta-harness** when the agent should inspect and improve its own
  user-space world through experience. It may use workers when the host offers
  them and delegation is useful, but workers are not required.
- Build or change a **host plugin/oracle** only when integrating an external
  agent host such as Codex with AOS. It is a host adapter, not an AOS capsule.

Use the smallest artifact that owns the real security and lifecycle boundary.
AOS is the common operating environment for all of them, not a synonym for any
one of them.
