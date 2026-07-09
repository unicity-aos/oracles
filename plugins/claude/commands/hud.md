---
description: Install (or show) the Astrid HUD status line — principal + daemon state in your bottom bar.
allowed-tools: Bash(echo:*), Bash(printf:*)
---

The Astrid HUD overlays your principal and daemon state onto Claude Code's status line. A plugin cannot register the main status line itself, so it has to be wired into the user's settings once.

Here is the exact `statusLine` block to add to `~/.claude/settings.json` (resolved to this plugin's installed path):

!`printf '  "statusLine": {\n    "type": "command",\n    "command": "%s/bin/astrid-statusline",\n    "padding": 0\n  }\n' "${CLAUDE_PLUGIN_ROOT:-<plugin-root>}"`

Do this:

1. Show the user the block above.
2. Offer to add it to `~/.claude/settings.json` for them (read the file, merge the `statusLine` key without clobbering other settings, write it back). Ask first — this edits their personal settings file.
3. Tell them the HUD appears after the status line refreshes (or on next launch), rendering: `⬡ astrid:<principal> ●  │ <model> │ <dir> ⎇ <branch> │ <context-bar> │ <cost>` — the dot reflects governance health: green ● when governed (daemon up **and** the astrid-mcp broker loaded), yellow ◐ when the daemon is up but the broker is missing (native tools run **ungoverned**), dim ○ when the daemon is down.
