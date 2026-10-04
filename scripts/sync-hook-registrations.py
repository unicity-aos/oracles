#!/usr/bin/env python3
"""Generate native registrations from the shipped Oracle hook codec inventory."""

import argparse
import json
from pathlib import Path
import runpy

ROOT = Path(__file__).resolve().parent.parent
CODEC = runpy.run_path(str(ROOT / "plugins/common/bin/aos-native-hook"))


def registrations(host):
    root = {"codex": '${PLUGIN_ROOT:-${CODEX_PLUGIN_ROOT:-.}}',
            "claude": '${CLAUDE_PLUGIN_ROOT}', "grok": '${GROK_PLUGIN_ROOT:-${CLAUDE_PLUGIN_ROOT}}'}[host]
    hooks = {}
    for event, (name, _, mode) in sorted(CODEC["hook_inventory"](host).items()):
        if mode == "worktree":
            continue  # Native creator is preserved; registration replaces it.
        command = f'python3 "{root}/bin/aos-native-hook" {host} {event}'
        timeout = 15 if mode not in ("observe", "context") else 3
        hooks[name] = [{"hooks": [{"type": "command", "command": command, "timeout": timeout}]}]
    # Provisioning is separate from bounded event delivery, and runs only at startup.
    starter = "aos-codex-mcp" if host == "codex" else "aos-mcp-start"
    hooks["SessionStart"].insert(0, {"hooks": [{
        "type": "command", "command": f'python3 "{root}/bin/{starter}" --hook',
        "timeout": 5, "statusMessage": "Checking AOS setup status"
    }]})
    # FileChanged is a literal watch list, NOT a wildcard filesystem watch.
    # No matcher means all paths already watched by this client, without adding
    # an accidental literal file named '*'. See the hook contract documentation.
    return {"hooks": hooks}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--export-contract", type=Path,
                        help="Export the codec inventory for an adapter's mapping regression")
    args = parser.parse_args()
    if args.export_contract:
        contract = {host: CODEC["hook_inventory"](host) for host in ("claude", "codex", "grok")}
        args.export_contract.parent.mkdir(parents=True, exist_ok=True)
        args.export_contract.write_text(json.dumps(contract, sort_keys=True, indent=2) + "\n")
    for host, directory in (("codex", "unicity-aos"), ("claude", "claude"), ("grok", "grok")):
        path = ROOT / "plugins" / directory / "hooks/hooks.json"
        expected = json.dumps(registrations(host), indent=2) + "\n"
        if args.check:
            if path.read_text() != expected:
                parser.exit(1, f"Stale hook registrations: {path}\n")
        else:
            path.write_text(expected)


if __name__ == "__main__":
    main()
