---
description: List the Astrid capsules installed in this runtime (the tool surface available over MCP).
---

List the installed Astrid capsules — these back the `astrid__*` MCP tools this session can call.

!`astrid capsule list 2>/dev/null || echo "(unavailable — is the astrid CLI installed?)"`

Summarize which capability domains are available (filesystem, http, shell, system, skills, …) and which capsule provides each. Note that only capsules with tool handlers contribute callable tools, and only after the `astrid-mcp` broker is provisioned for this principal.
