#!/usr/bin/env python3
"""Run real Oracle wrappers with a recording AOS fixture, never a live home."""

import json
import hashlib
import os
import shutil
import sys
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(os.environ.get("ORACLE_TEST_ROOT", Path(__file__).resolve().parent.parent))


class HostHookLifecycle(unittest.TestCase):
    def test_session_identity_survives_turn_stop_and_retires_at_session_end(self):
        for host, session_key in (("claude", "session_id"), ("grok", "sessionId"),
                                  ("codex", "session_id")):
            with self.subTest(host=host), tempfile.TemporaryDirectory() as scratch:
                root = Path(scratch)
                home = root / "aos"
                workspace = root / "workspace"
                workspace.mkdir()
                fake = root / "aos-fixture"
                log = root / "calls"
                fake.write_text(
                    "#!/bin/sh\nset -eu\n"
                    'case " $* " in *" hook "*)\n'
                    'printf "%s|%s\\n" "$*" "$ASTRID_HOOK_TOKEN" >> "$HOOK_TEST_LOG"\n'
                    'cat >/dev/null ;; esac\n'
                )
                fake.chmod(0o700)
                env = {key: value for key, value in os.environ.items()
                       if not key.startswith(("AOS_", "ASTRID_", "CLAUDE_", "GROK_", "PLUGIN_"))}
                plugin = ROOT / "plugins" / ("unicity-aos" if host == "codex" else host)
                env.update(AOS_HOME=str(home), AOS_BIN=str(fake), AOS_HOST=host,
                           AOS_PLUGIN_ROOT=str(plugin),
                           PLUGIN_DATA=str(root / "data"),
                           ASTRID_HOST_HOOK_FAIL_CLOSED="1", HOOK_TEST_LOG=str(log),
                           TMPDIR=str(root))
                env[host.upper() + "_PLUGIN_ROOT"] = str(plugin)
                if host == "codex":
                    env["CODEX_PLUGIN_DATA"] = str(root / "data")
                # Do not let an optional jq installation conceal the fallback
                # bug: both sessions have the same parent process without jq.
                tools = root / "tools"
                tools.mkdir()
                for name in ("sh", "dirname", "sed", "awk", "cat", "mktemp", "rm",
                             "mkdir", "chmod", "openssl", "mv", "cksum", "tr",
                             "pwd", "date", "readlink", "basename", "head", "wc",
                             "sleep", "rmdir", "grep"):
                    executable = shutil.which(name)
                    if executable:
                        (tools / name).symlink_to(executable)
                (tools / "python3").symlink_to(sys.executable)
                env["PATH"] = str(tools)
                command = [str(plugin / "bin/aos-up")]
                if host == "codex":
                    command.append("codex")
                command.append("hook")
                sequence = (("session_start", "team/a"), ("session_start", "team.a"),
                            ("stop", "team/a"), ("session_end", "team/a"),
                            ("user_prompt_submit", "team.a"), ("session_end", "team.a"))
                for event, identity in sequence:
                    result = subprocess.run(
                        command + [event], input=json.dumps({session_key: identity}),
                        capture_output=True, text=True, env=env, cwd=workspace, timeout=10,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)
                calls = [line.rsplit("|", 1) for line in log.read_text().splitlines()]
                self.assertEqual(len(calls), len(sequence))
                tokens = {}
                for (event, identity), (args, token) in zip(sequence, calls):
                    route = host + "-" + hashlib.sha256(identity.encode()).hexdigest()
                    self.assertIn("--session " + route + " ", args)
                    self.assertEqual(tokens.setdefault(identity, token), token,
                                     "another session ending must not retire this token")
                    self.assertFalse((root / "data" / host / (route + ".token")).exists())
                self.assertNotEqual(tokens["team/a"], tokens["team.a"])
                for payload in ({}, {session_key: 42}):
                    result = subprocess.run(command + ["pre_tool_use"],
                        input=json.dumps(payload), capture_output=True, text=True,
                        env=env, cwd=workspace, timeout=10)
                    self.assertNotEqual(result.returncode, 0)
                self.assertEqual(len(log.read_text().splitlines()), len(sequence))


if __name__ == "__main__":
    unittest.main()
