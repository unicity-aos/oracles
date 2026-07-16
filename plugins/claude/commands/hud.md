---
description: Install or show the Unicity AOS HUD status line.
---

The AOS HUD shows this session's principal and governance health in Claude
Code's status line. A plugin cannot register the main status line itself, so
the user must opt in once.

!`printf '  "statusLine": {\n    "type": "command",\n    "command": "%s/bin/aos-statusline",\n    "padding": 0\n  }\n' "${CLAUDE_PLUGIN_ROOT:-<plugin-root>}"`

Ask before editing user settings. Otherwise show the snippet and explain that
`⬡ aos:<principal> ●` is green only when the runtime and internal broker both
answer, yellow when the runtime is reachable without the broker, and dim when
the runtime is stopped.
