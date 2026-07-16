---
description: Show the Unicity AOS principal this session acts as, including capabilities and quota.
---

Report the AOS identity and mandate backing this session. The default is
`grok-code`; an explicit `AOS_PRINCIPAL_ID` overrides it.

!`PRINCIPAL="${AOS_PRINCIPAL_ID:-grok-code}"; aos agent show "$PRINCIPAL" 2>/dev/null || echo "(principal unavailable)"`

!`PRINCIPAL="${AOS_PRINCIPAL_ID:-grok-code}"; aos caps show "$PRINCIPAL" 2>/dev/null || echo "(capabilities unavailable)"`

!`PRINCIPAL="${AOS_PRINCIPAL_ID:-grok-code}"; aos quota show --agent "$PRINCIPAL" 2>/dev/null || echo "(quota unavailable)"`

Summarize the principal, its delegated authority, and budget limits.
