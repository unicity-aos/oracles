---
description: Show Unicity AOS runtime status for this session.
allowed-tools: Bash(aos status --json:*)
---

Report the AOS runtime state for the configured principal.

!`aos status --json 2>/dev/null || echo "(unavailable — is Unicity AOS installed?)"`

Summarize whether the runtime is reachable, the connected-client count, and
whether the internal `astrid-mcp` broker is loaded. The MCP wrapper starts the
bundled ephemeral runtime when the host connects; do not start a second daemon.
