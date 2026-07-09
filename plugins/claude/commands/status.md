---
description: Show the Astrid daemon status — PID, uptime, connected clients, loaded capsules.
allowed-tools: Bash(astrid status:*)
---

Report the state of the Astrid runtime backing this session.

!`astrid status 2>/dev/null || echo "(unavailable — is the astrid CLI installed?)"`

Summarize: is the daemon up, how many clients are connected, and whether the tool-serving capsules (`astrid-mcp`, `astrid-capsule-cli`) are loaded. If it is not running, note that the plugin boots an ephemeral daemon automatically when the MCP server launches, or you can run `astrid start`.
