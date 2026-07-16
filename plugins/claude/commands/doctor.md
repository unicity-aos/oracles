---
description: Diagnose the Unicity AOS runtime backing this session and report any required fix.
allowed-tools: Bash(*/bin/aos-doctor:*)
---

Run the AOS readiness check and present its result.

!`"${CLAUDE_PLUGIN_ROOT}/bin/aos-doctor" --format human 2>/dev/null || echo "(could not run aos-doctor — is the AOS plugin installed correctly?)"`

Relay the report. If setup is missing, preserve the explicit installer command;
do not edit the private runtime home or provision credentials implicitly.
