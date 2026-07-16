---
description: List the capsules installed for this Unicity AOS principal.
---

List the installed AOS capsules backing this session's `mcp__aos__*` tools.

!`PRINCIPAL="${AOS_PRINCIPAL_ID:-grok-code}"; aos --principal "$PRINCIPAL" capsule list 2>/dev/null || echo "(unavailable — is Unicity AOS installed?)"`

Summarize the capability domains and their providers. `astrid-mcp` is the
internal runtime broker, not the product name.
