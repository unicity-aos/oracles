# Unicity AOS for Grok Build

This plugin connects Grok Build to Unicity AOS as the scoped `grok-code`
principal. It registers the public `aos` MCP server, a short session-start
readiness check, operator commands, and capsule-authoring skills. Commands are
available by their unqualified names; if another plugin defines the same
command, use the host-qualified form such as `/unicity-aos:doctor`.

Install and provision the product and Grok adapter explicitly:

```sh
curl --proto '=https' --tlsv1.2 -fsSL \
  https://raw.githubusercontent.com/unicity-aos/oracles/main/install.sh \
  | sh -s -- --host grok
```

Or install the local plugin during development:

```sh
grok plugin install /path/to/oracles/plugins/grok --trust
grok plugin enable unicity-aos
```

`.mcp.json` attaches the public MCP gateway to the invoking project workspace
with `aos --principal grok-code mcp attach`. Set `AOS_MCP_MODE=per-session` to
use the bounded legacy `mcp serve` process instead. The SessionStart hook only
performs the short `aos mcp ready --format hook` check and registers the host
session; provisioning is an explicit installer action.

Visible tools use `mcp__aos__*`. The internal broker capsule remains
`aos-mcp`, and its stable runtime topics remain `astrid.v1.*`.
