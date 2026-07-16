---
description: Show the Unicity AOS principal this session acts as, including capabilities and quota.
allowed-tools: Bash(aos agent show:*), Bash(aos caps show:*), Bash(aos quota show:*)
---

Report the AOS identity and mandate backing this session.

!`PRINCIPAL="${CLAUDE_PLUGIN_OPTION_PRINCIPAL:-${CLAUDE_PLUGIN_OPTION_principal:-claude-code}}"; aos agent show "$PRINCIPAL" 2>/dev/null || echo "(principal unavailable)"`

!`PRINCIPAL="${CLAUDE_PLUGIN_OPTION_PRINCIPAL:-${CLAUDE_PLUGIN_OPTION_principal:-claude-code}}"; aos caps show "$PRINCIPAL" 2>/dev/null || echo "(capabilities unavailable)"`

!`PRINCIPAL="${CLAUDE_PLUGIN_OPTION_PRINCIPAL:-${CLAUDE_PLUGIN_OPTION_principal:-claude-code}}"; aos quota show --agent "$PRINCIPAL" 2>/dev/null || echo "(quota unavailable)"`

Summarize the principal, its delegated authority, and any budget limits. State
plainly that `claude-code` is least-authority and is not the `default` admin.
