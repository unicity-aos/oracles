---
description: List the capsules installed for this Unicity AOS principal.
allowed-tools: Bash(aos --principal * capsule list:*)
---

List the installed AOS capsules. These provide the `mcp__aos__*` tools visible
to this session.

!`PRINCIPAL="${CLAUDE_PLUGIN_OPTION_PRINCIPAL:-${CLAUDE_PLUGIN_OPTION_principal:-claude-code}}"; aos --principal "$PRINCIPAL" capsule list 2>/dev/null || echo "(unavailable — is Unicity AOS installed?)"`

Summarize which capability domains are available and which capsule provides
each one. `astrid-mcp` is the internal runtime broker and should be described as
an implementation detail, not as the product.
