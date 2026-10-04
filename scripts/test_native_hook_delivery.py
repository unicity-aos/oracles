#!/usr/bin/env python3
"""Execute the shipped refusal boundary without host accounts or live homes."""

import json
import os
import runpy
from pathlib import Path
import shutil
import shlex
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
CODEC = runpy.run_path(str(ROOT / "plugins/common/bin/aos-native-hook"))


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
            expected = 0
            for host, directory in (("codex", "unicity-aos"), ("claude", "claude"), ("grok", "grok")):
                plugin = ROOT / "plugins" / directory
                env.update(AOS_HOME=str(home), AOS_BIN=str(fake), AOS_PLUGIN_ROOT=str(plugin),
                           ASTRID_HOOK_TOKEN="e" * 64, HOOK_TEST_LOG=str(log), TMPDIR=scratch)
                env.pop("AOS_HOST", None)
                env.pop("GROK_PLUGIN_ROOT", None)
                env[host.upper() + "_PLUGIN_ROOT"] = str(plugin)
                for event, (_, _, mode) in CODEC["hook_inventory"](host).items():
                    if mode == "worktree":
                        continue
                    with self.subTest(host=host, event=event):
                        result = subprocess.run(
                            [sys.executable, str(plugin / "bin/aos-native-hook"), host, event],
                            input='{"session_id":"native-only"}', env=env, cwd=root,
                            capture_output=True, text=True, timeout=12,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(json.loads(result.stdout), {})
                        expected += 1
            calls = log.read_text().splitlines()
            self.assertEqual(len(calls), expected, calls)
            self.assertEqual(expected, 58)
            self.assertTrue(all(" hook " in call and "--format json" in call for call in calls))

    def test_inventory_and_native_semantics_for_every_event(self):
        for host, count in (("claude", 33), ("codex", 12), ("grok", 15)):
            inventory = CODEC["hook_inventory"](host)
            self.assertEqual(len(inventory), count)
            for event, (name, canonical, mode) in inventory.items():
                with self.subTest(host=host, event=event):
                    self.assertTrue(canonical.replace("_", "").isalnum())
                    allowed = CODEC["native_output"](host, event, reply(event))
                    self.assertEqual(allowed, {})
                    denied = CODEC["native_output"](host, event, reply(event, skip=True))
                    self.assertTrue(CODEC["valid_output"](host, event, denied), denied)
                    if mode in ("observe", "context"):
                        self.assertEqual(denied, {})
                    elif mode in ("block", "exit2", "worktree"):
                        self.assertEqual(denied["decision"], "block")
                    elif mode == "continue":
                        self.assertIs(denied["continue"], False)
                    elif mode == "elicitation":
                        self.assertEqual(denied["hookSpecificOutput"]["action"], "decline")
                    else:
                        self.assertEqual(denied["hookSpecificOutput"]["hookEventName"], name)

    def test_registration_inventory_preserves_native_worktree_ownership(self):
        for host, directory, count in (("claude", "claude", 31), ("codex", "unicity-aos", 12), ("grok", "grok", 15)):
            hooks = json.loads((ROOT / "plugins" / directory / "hooks/hooks.json").read_text())["hooks"]
            inventory = CODEC["hook_inventory"](host)
            self.assertEqual(len(hooks), count)
            self.assertEqual(set(hooks), {name for name, _, mode in inventory.values() if mode != "worktree"})
            self.assertNotIn("WorktreeCreate", hooks)
            self.assertNotIn("WorktreeRemove", hooks)
            for event, (name, _, mode) in inventory.items():
                if mode == "worktree":
                    continue
                commands = [hook for group in hooks[name] for hook in group["hooks"]
                            if "aos-native-hook" in hook["command"]]
                self.assertEqual(len(commands), 1)
                self.assertIn(f' {host} {event}', commands[0]["command"])
                self.assertEqual(commands[0]["timeout"], 3 if mode in ("observe", "context") else 15)
            if host == "claude":
                self.assertNotIn("matcher", hooks["FileChanged"][0])

    def test_short_deadline_observation_does_not_block_the_host(self):
        started = time.monotonic()
        self.assertEqual(self.invoke("codex", "interrupt", "sleep 30\n"), {})
        self.assertLess(time.monotonic() - started, 3)

    def test_elicited_content_is_redacted_before_delivery(self):
        script = ('input=$(cat); case "$input" in *private-secret*) exit 1;; esac\n'
                  + emit(reply("elicitation_result")))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / "aos-native-hook"
            shutil.copyfile(ROOT / "plugins/common/bin/aos-native-hook", helper)
            launcher = root / "aos-up"
            launcher.write_text("#!/bin/sh\nset -eu\n" + script)
            launcher.chmod(0o700)
            result = subprocess.run([sys.executable, str(helper), "claude", "elicitation_result"],
                input='{"session_id":"s","content":{"password":"private-secret"}}',
                text=True, capture_output=True, timeout=12)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout), {})

    def test_task_denial_uses_exit_two_and_stderr(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / "aos-native-hook"
            shutil.copyfile(ROOT / "plugins/common/bin/aos-native-hook", helper)
            launcher = root / "aos-up"
            launcher.write_text("#!/bin/sh\ncat >/dev/null\n" + emit(reply("task_completed", skip=True)))
            launcher.chmod(0o700)
            result = subprocess.run([sys.executable, str(helper), "claude", "task_completed"],
                input='{}', text=True, capture_output=True, timeout=12)
            self.assertEqual(result.returncode, 2)
            self.assertEqual(result.stdout, "")
            self.assertIn("policy", result.stderr)

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
            contextual = CODEC["native_output"](host, "pre_tool_use",
                reply("pre_tool_use", skip=True, context="policy context"))
            self.assertEqual(contextual["hookSpecificOutput"]["additionalContext"], "policy context")
            self.assertTrue(CODEC["valid_output"](host, "pre_tool_use", contextual))
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
