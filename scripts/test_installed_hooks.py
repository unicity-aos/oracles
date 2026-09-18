#!/usr/bin/env python3
"""Probe real installed hooks in an explicitly selected, running QA home.

No provisioning, daemon startup, or trust substitution is performed here. The
caller owns the disposable home and its lifecycle. Missing replies are failures,
including when a plugin wrapper suppresses its underlying delivery error.
"""

import argparse
import json
import os
from pathlib import Path
import secrets
import subprocess
import time
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--aos-home", required=True, type=Path)
    parser.add_argument("--oracle-version", required=True)
    args = parser.parse_args()
    home = args.aos_home.resolve(strict=True)
    aos = home / "bin/aos"
    for host, plugin in (("codex", "unicity-aos"), ("claude", "claude"), ("grok", "grok")):
        root = home / "extensions/oracles/plugins" / args.oracle_version / "plugins" / plugin
        env = os.environ.copy()
        for key in ("AOS_BIN", "AOS_PLUGIN_ROOT", "CODEX_PLUGIN_ROOT", "PLUGIN_ROOT",
                    "CLAUDE_PLUGIN_ROOT", "GROK_PLUGIN_ROOT", "AOS_HOST",
                    "ASTRID_PRINCIPAL_ID", "AOS_PRINCIPAL_ID"):
            env.pop(key, None)
        env.update(AOS_HOME=str(home), ASTRID_RUN_DIR=str(home / "run"),
                   ASTRID_SESSION_ID="hook-proof-" + uuid.uuid4().hex,
                   ASTRID_HOOK_TOKEN=secrets.token_hex(32),
                   ASTRID_CODEX_HOOK_FAIL_CLOSED="1", ASTRID_HOST_HOOK_FAIL_CLOSED="1")
        env[{"codex": "CODEX_PLUGIN_ROOT", "claude": "CLAUDE_PLUGIN_ROOT",
             "grok": "GROK_PLUGIN_ROOT"}[host]] = str(root)
        # Probe the strict public AOS ingress first: wrapper exit 0 alone is
        # not evidence of a response. Use the same authenticated session route.
        for event in ("session_start", "pre_tool_use", "post_tool_use"):
            commands = [("direct", [str(aos), "--principal", host + "-code", "hook",
                         "--host", host, "--session", env["ASTRID_SESSION_ID"], "--event", event])]
            commands.append(("plugin", [str(root / "bin/aos-up")]
                             + (["codex"] if host == "codex" else []) + ["hook", event]))
            for path, command in commands:
                start = time.monotonic()
                result = subprocess.run(command, input="{}", text=True, capture_output=True,
                                        cwd=home / "runtime", env=env, timeout=10)
                elapsed = time.monotonic() - start
                print(json.dumps(dict(host=host, path=path, event=event,
                                      seconds=round(elapsed, 4), exit=result.returncode)), flush=True)
                if result.returncode:
                    raise RuntimeError(result.stderr)
                if elapsed >= 2:
                    raise RuntimeError(f"{host} {path} {event}: {elapsed:.3f}s exceeds warm hook budget")


if __name__ == "__main__":
    main()
