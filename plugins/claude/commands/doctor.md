---
description: Diagnose the Astrid runtime backing this session and report readiness + any fix.
allowed-tools: Bash(*/bin/astrid-doctor:*)
---

Run the Astrid readiness check and present the result.

!`"${CLAUDE_PLUGIN_ROOT}/bin/astrid-doctor" --format human 2>/dev/null || echo "(could not run astrid-doctor — is the Astrid plugin installed correctly?)"`

Relay the report. If it flags a gap (no daemon, missing `astrid-mcp` broker, or denied tool calls needing a trusted ingress), state the gap and the exact fix it gives. Otherwise confirm the session is a healthy, governed Astrid agent.
