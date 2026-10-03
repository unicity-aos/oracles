#!/usr/bin/env python3
"""Execute the shipped refusal boundary without host accounts or live homes."""

import json
import os
from pathlib import Path
import shutil
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


def reply(event, skip=False, ask=False, context=None):
    return {"schema_version": 1, "event": event,
            "decision": {"skip": skip, "ask": ask, "reason": "policy"}, "context": context}


def emit(value):
    return "printf '%s' " + shlex.quote(json.dumps(value)) + "\n"


class NativeHookDelivery(unittest.TestCase):
    def test_codex_native_hook_does_not_bootstrap_or_probe_capsules(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            home = root / "aos"
            (home / "runtime").mkdir(parents=True)
            fake = root / "aos-fixture"
            log = root / "calls"
            fake.write_text(
                f'#!{sys.executable}\nimport json, os, sys\n'
                'with open(os.environ["HOOK_TEST_LOG"], "a") as log: log.write(" ".join(sys.argv[1:])+"\\n")\n'
                'assert "hook" in sys.argv\nsys.stdin.read()\n'
                'event=sys.argv[sys.argv.index("--event")+1]\n'
                'print(json.dumps({"schema_version":1,"event":event,"decision":{"skip":False},"context":None}))\n'
            )
            fake.chmod(0o700)
            env = {key: value for key, value in os.environ.items()
                   if not key.startswith(("AOS_", "ASTRID_", "CODEX_", "PLUGIN_"))}
            plugin = ROOT / "plugins/unicity-aos"
            env.update(AOS_HOME=str(home), AOS_BIN=str(fake), AOS_PLUGIN_ROOT=str(plugin),
                       ASTRID_HOOK_TOKEN="e" * 64, HOOK_TEST_LOG=str(log), TMPDIR=scratch)
            for event in ("pre_tool_use", "user_prompt_submit", "permission_request"):
                result = subprocess.run(
                    [sys.executable, str(plugin / "bin/aos-native-hook"), "codex", event],
                    input='{"session_id":"native-only"}', env=env, cwd=root,
                    capture_output=True, text=True, timeout=12,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), {})
            calls = log.read_text().splitlines()
            self.assertEqual(len(calls), 3, calls)
            self.assertTrue(all(" hook " in call and "--format json" in call for call in calls))

    def invoke(self, host, event, script):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / "aos-native-hook"
            shutil.copyfile(ROOT / "plugins/common/bin/aos-native-hook", helper)
            launcher = root / "aos-up"
            launcher.write_text("#!/bin/sh\nset -eu\ncat >/dev/null\n" + script)
            launcher.chmod(0o700)
            result = subprocess.run([sys.executable, str(helper), host, event],
                                    input='{"prompt":"test"}', text=True,
                                    capture_output=True, timeout=15)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)

    def test_transport_errors_and_malformed_output_are_explicit_refusals(self):
        for host in ("claude", "codex", "grok"):
            for event in ("pre_tool_use", "user_prompt_submit"):
                for script in ("exit 1\n", "exit 0\n", "printf 'not JSON'\n", "printf '[]'\n",
                               "printf '%s' '{\"unexpected\":true}'\n",
                               "printf '%s' '{\"decision\":\"allow\"}'\n"):
                    with self.subTest(host=host, event=event, script=script):
                        value = self.invoke(host, event, script)
                        if event == "pre_tool_use":
                            self.assertEqual(value["hookSpecificOutput"]["permissionDecision"], "deny")
                        else:
                            self.assertEqual(value["decision"], "block")

    def test_native_decision_is_not_wrapped_as_context(self):
        for host in ("claude", "codex", "grok"):
            value = self.invoke(host, "pre_tool_use", emit(reply("pre_tool_use", skip=True)))
            self.assertEqual(value["hookSpecificOutput"]["permissionDecision"], "deny")
            self.assertNotIn("additionalContext", value["hookSpecificOutput"])
            self.assertEqual(self.invoke(host, "pre_tool_use", emit(reply("pre_tool_use"))), {})

    def test_host_specific_ask_and_context_are_oracle_owned(self):
        for host in ("claude", "codex", "grok"):
            value = self.invoke(host, "pre_tool_use", emit(reply("pre_tool_use", ask=True)))
            self.assertEqual(value["hookSpecificOutput"]["permissionDecision"],
                             "deny" if host == "codex" else "ask")
            prompt = self.invoke(host, "user_prompt_submit", emit(reply("user_prompt_submit", context="context")))
            self.assertEqual(prompt, {} if host == "grok" else {
                "hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"context"}})
        for host in ("claude", "codex"):
            self.assertEqual(self.invoke(host, "permission_request", emit(reply("permission_request", ask=True))), {})

    def test_missing_mismatched_and_malformed_decisions_fail_closed(self):
        for value in [
            reply("wrong_event"),
            {**reply("pre_tool_use"), "decision":None},
            {**reply("pre_tool_use"), "decision":{"skip":"false"}},
            {**reply("pre_tool_use"), "schema_version":True},
            {**reply("pre_tool_use"), "decision":{"skip":False,"reason":"a"*4097}},
        ]:
            self.assertEqual(self.invoke("claude", "pre_tool_use", emit(value))
                             ["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_timeout_is_explicit_refusal(self):
        self.assertEqual(self.invoke("claude", "pre_tool_use", "sleep 30\n")
                         ["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_permission_request_uses_its_own_schema(self):
        for host in ("claude", "codex"):
            value = self.invoke(host, "permission_request", "exit 1\n")
            self.assertEqual(value["hookSpecificOutput"]["decision"]["behavior"], "deny")

    def test_registered_binding_hooks_use_the_refusal_boundary(self):
        for directory in ("claude", "grok", "unicity-aos"):
            hooks = json.loads((ROOT / "plugins" / directory / "hooks/hooks.json").read_text())["hooks"]
            for event in ("PreToolUse", "UserPromptSubmit"):
                commands = [hook["command"] for group in hooks[event] for hook in group["hooks"]]
                self.assertTrue(any("aos-native-hook" in command for command in commands))
            self.assertEqual((ROOT / "plugins" / directory / "bin/aos-native-hook").read_bytes(),
                             (ROOT / "plugins/common/bin/aos-native-hook").read_bytes())


if __name__ == "__main__":
    unittest.main()
