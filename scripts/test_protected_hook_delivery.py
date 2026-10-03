#!/usr/bin/env python3
"""Oracle codec/subprocess proof; fixtures do not establish protected custody."""
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
SOURCE = "cbfc77db-4abc-5b42-99fb-e9c6a6f130dd"
INSTALLATION = "081799f3-98a5-43aa-8669-5af6cd4117fe"
HELPER = ROOT / "plugins/common/bin/aos-protected-hook"


class ProtectedHookDelivery(unittest.TestCase):
    def invoke(self, host, event, body, payload=None, missing_codec=False, local=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            helper = root / "aos-protected-hook"
            shutil.copyfile(HELPER, helper)
            if not missing_codec:
                shutil.copyfile(HELPER.with_name("aos-native-hook"), root / "aos-native-hook")
            evaluator = root / "fixture"
            evaluator.write_text(f"#!{sys.executable}\n" + body)
            evaluator.chmod(0o700)
            command = [sys.executable, "-I", str(helper), "--host", host, "--event", event,
                       "--evaluator", str(evaluator), "--enforcer-source-id", SOURCE,
                       "--service-socket", str(root / "service.sock"),
                       "--service-uid", str(os.getuid() + 1),
                       "--supervised-uid", str(os.getuid()),
                       "--service-installation-id", INSTALLATION]
            if local:
                command = command[:command.index("--service-socket")]
                command += ["--supervised-uid", str(os.getuid()),
                            "--runtime-home", str(root / "runtime"),
                            "--principal", "claude-code",
                            "--audit-journal", str(root / "journal"),
                            "--audit-installation-id", INSTALLATION]
            if payload is None:
                payload = {"hook_event_name": "PreToolUse" if event == "pre_tool_use" else "UserPromptSubmit",
                           "tool_name": "Bash", "tool_input": {"command": "ls"}, "prompt": "hello",
                           "principal": "attacker", "session_id": "not-policy-authority"}
                if host == "grok" and event == "pre_tool_use":
                    payload["toolName"] = payload.pop("tool_name")
                    payload["toolInput"] = payload.pop("tool_input")
                    payload["toolInputTruncated"] = False
            return subprocess.run(command, input=json.dumps(payload), capture_output=True,
                                  text=True, timeout=13)

    def test_normalizes_request_and_preserves_host_decision_semantics(self):
        for host in ("claude", "codex", "grok"):
            for event in ("pre_tool_use", "user_prompt_submit"):
                for mode in ("allow", "deny", "ask"):
                    script = ('import json,sys\nrequest=json.load(sys.stdin)\n'
                              'assert set(request)=={"schema_version","event","action"}\n'
                              'assert "--format" in sys.argv and "oracle-json" in sys.argv\n'
                              'assert "--service-installation-id" in sys.argv\n'
                              f'assert request["event"]=={event!r}\n'
                              f'assert request["action"]["kind"]=={"tool" if event == "pre_tool_use" else "prompt"!r}\n'
                              f'print(json.dumps({{"schema_version":1,"event":{event!r},'
                              f'"decision":{{"skip":{mode == "deny"},"ask":{mode == "ask"},"reason":"rule"}},"context":None}}))\n')
                    result = self.invoke(host, event, script)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    value = json.loads(result.stdout)
                    if mode == "allow":
                        self.assertEqual(value, {})
                    elif event == "user_prompt_submit":
                        self.assertEqual(value["decision"], "block")
                    else:
                        self.assertEqual(value["hookSpecificOutput"]["permissionDecision"],
                                         "ask" if mode == "ask" and host != "codex" else "deny")

    def test_bad_input_never_reaches_evaluator(self):
        parse = runpy.run_path(str(HELPER))["canonical_request"]
        for payload in ([], {}, {"tool_name": "Bash", "tool_input": []},
                        {"hook_event_name": "UserPromptSubmit", "tool_name": "Bash", "tool_input": {}}):
            with self.assertRaises(ValueError):
                parse("claude", "pre_tool_use", json.dumps(payload))

    def test_grok_truncated_or_conflicting_input_cannot_authorize(self):
        parse = runpy.run_path(str(HELPER))["canonical_request"]
        for extra in ({"toolInputTruncated": True}, {"toolInputTruncated": "false"},
                      {"tool_name": "different"}, {"tool_input": {"command": "different"}}):
            payload = {"toolName": "Bash", "toolInput": {"command": "ls"}, **extra}
            with self.assertRaises(ValueError):
                parse("grok", "pre_tool_use", json.dumps(payload))

    def test_failed_empty_wrong_event_and_timed_out_evaluators_deny(self):
        for body in ('import sys;sys.exit(1)', 'print("")', 'print("{}")',
                     'import time;time.sleep(30)',
                     'print(\'{"schema_version":1,"event":"wrong","decision":{"skip":false},"context":null}\')'):
            result = self.invoke("claude", "pre_tool_use", body)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["hookSpecificOutput"]["permissionDecision"], "deny")

    def test_missing_codec_is_blocking_exit_not_silent_allow(self):
        result = self.invoke("claude", "pre_tool_use", 'print("{}")', missing_codec=True)
        self.assertEqual(result.returncode, 2)

    def test_local_mode_pins_runtime_principal_and_journal_without_service_fallback(self):
        script = ('import json,os,sys\nrequest=json.load(sys.stdin)\n'
                  'assert "oracle-local-json" in sys.argv\n'
                  'assert "--service-socket" not in sys.argv\n'
                  'assert sys.argv[sys.argv.index("--principal")+1]=="claude-code"\n'
                  'assert "--audit-journal" in sys.argv and "--audit-installation-id" in sys.argv\n'
                  'assert os.environ["ASTRID_HOME"].endswith("/runtime")\n'
                  'assert "ASTRID_RUN_DIR" not in os.environ\n'
                  'print(json.dumps({"schema_version":1,"event":request["event"],'
                  '"decision":{"skip":True,"ask":False,"reason":"local rule"},"context":None}))\n')
        result = self.invoke("claude", "pre_tool_use", script, local=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        reply = json.loads(result.stdout)["hookSpecificOutput"]
        self.assertEqual(reply["permissionDecision"], "deny")
        self.assertIn("local rule", reply["permissionDecisionReason"])


if __name__ == "__main__":
    unittest.main()
