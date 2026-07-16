---
description: Show Unicity AOS runtime status for the grok-code principal.
---

Report the live AOS runtime state.

!`aos status --json 2>/dev/null || echo "(unavailable — is Unicity AOS installed?)"`

Summarize whether the runtime is reachable and whether the internal
`astrid-mcp` broker is loaded.
