# Unicity AOS for Codex

This plugin connects Codex to Unicity AOS as the `codex-code` principal and
teaches a fresh session how to operate and build on the OS.

## What loads

On installation, Codex reads `.codex-plugin/plugin.json` and discovers:

- `skills/unicity-aos` for the operating-system and principal boundary;
- `skills/capsule-forge` for a compact authoring workflow plus progressively
  loaded reference chapters;
- `skills/meta-harness` for proactively extending the agent's user-space world
  when work reveals missing capability or reusable leverage;
- `.mcp.json`, which starts `aos --principal codex-code mcp serve`; and
- session and tool hooks under `hooks/hooks.json`.

Skill metadata is available for routing. Codex reads a skill's complete
`SKILL.md` only when the user names it or the request matches its description.
The MCP server independently exposes the tool surface granted to `codex-code` as
`mcp__aos__*`. Installing the plugin therefore supplies both the knowledge of
how to build and the governed tools for inspecting or changing the live OS.

Users and agents can add Skills after the plugin was published by placing valid
entries in the workspace or the principal's `home://skills/`; `aos-skills`
exposes them through `list_skills` and `read_skill`. Capsules remain free to
serve their own detailed manuals over ordinary IPC tools without adding an
AI-specific section to the generic capsule manifest.

This does not rewrite the installed plugin. Codex discovers native plugin
skills from the signed `skills/` snapshot at session start, while AOS-native
skills are fetched on demand through the governed MCP surface. Important
first-party workflows remain vendored into the plugin for automatic routing and
offline use; Forge's bus-served manual is the live, version-matched reference.

## Fresh-session path

For a request such as “while fixing this project, improve your AOS setup when
that would make this and future work better,” a fresh Codex session should:

1. Load `unicity-aos` and recognize AOS as the operating system, not the
   meta-harness.
2. Load `meta-harness` for the agent-world model, proactive initiative, artifact
   choice, evaluation, and continuity.
3. Load `capsule-forge` when a missing connector or capability requires code.
4. Inspect the actual `mcp__aos__*` tools, capsules, and WIT contracts.
5. Decide whether the useful extension belongs inline, after the immediate
   objective, or in durable memory, a skill, or a trace for later.
6. Reuse, compose, configure, remember, or build the smallest useful extension,
   evaluate it, and preserve it for future work.

## Initiative follows the user

The plugin does not impose a self-extension personality or mode enum. Ordinary
instructions such as “think widely,” “decide for yourself,” “only propose,” or
“implement what is useful” tell the agent how much initiative fits the work.
Approved standing preferences can be preserved in principal-scoped memory or
configuration so future sessions inherit the same direction.

Memory preserves intent and continuity; AOS capabilities and operator policy
remain the operational authority boundary. The agent should still reach for a
useful extension proactively. It uses judgment about whether the extension is
needed to complete the present objective, is better made after that work, or
should be retained as a future opportunity.

The static Skills and vendored Forge reference chapters are sufficient to
explain the architecture and author a capsule from zero even if AOS is
temporarily offline. Live discovery, installation, grants, and operation still
require the MCP server and relevant AOS capsules. Workers and subagents are
optional mechanisms: a useful meta-harness can improve memory, skills, context,
harness code, composition, or capsules without them.

The `capsule-forge` trigger and references are vendored from Forge in the AOS
Community Edition source. Host-specific text may explain MCP discovery, but
Forge's bus-served guide remains the version-matched authority for SDK,
manifest, WIT, secret, toolchain, and packaging behavior.

After installing or updating the plugin, start a new Codex thread so its skills,
hooks, and MCP tools are discovered together.
