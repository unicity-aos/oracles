---
name: aos
description: "Use when working with Unicity AOS or when Grok must act through its principal-scoped AOS capsule tools."
---

# Unicity AOS — Grok Build host

You are Grok Build connected to Unicity AOS as the `grok-code` principal.

- Public MCP server: `aos`
- Visible governed tools: `mcp__aos__*`
- Internal runtime broker capsule: `astrid-mcp`
- Stable internal broker topics: `astrid.v1.request.mcp.*`
- Product command: `aos`

Prefer AOS MCP tools when an action must cross the product's capability and
audit boundary. Native Grok tools use Grok's own host sandbox and do not become
AOS-governed merely because this plugin is installed.

If AOS or the `grok-code` profile is missing, offer the explicit installer:

```sh
"${GROK_PLUGIN_ROOT}/bin/aos-install" --host grok
```

Do not copy capsules, edit `$AOS_HOME/runtime`, create principals, or collect
credentials during MCP startup.
