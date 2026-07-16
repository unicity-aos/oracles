# Unicity AOS for Grok Build

This plugin connects Grok Build to Unicity AOS as the scoped `grok-code`
principal. It registers the public `aos` MCP server, a read-only session-start
doctor, operator commands, and capsule-authoring skills. Commands are available
by their unqualified names; if another plugin defines the same command, use the
host-qualified form such as `/unicity-aos:doctor`.

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

`.mcp.json` executes `bin/aos-up --principal grok-code`. That wrapper delegates
to `aos --principal grok-code mcp serve`; AOS owns authenticated runtime startup.
The wrapper never provisions principals, installs capsules, or reaches into the
private runtime home.

Visible tools use `mcp__aos__*`. The internal broker capsule remains
`astrid-mcp`, and its stable runtime topics remain `astrid.v1.*`.
