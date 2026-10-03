#!/usr/bin/env python3
"""Pure migration planning, with exact legacy commands and no live settings."""
import copy
import json
from pathlib import Path
import runpy
import shlex
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "plugins/common/bin/aos-protected-settings"
PLAN = runpy.run_path(str(SCRIPT))["plan"]
REMOVE = runpy.run_path(str(SCRIPT))["remove_registration"]
REPAIR = runpy.run_path(str(SCRIPT))["repair_registration"]
ROTATE = runpy.run_path(str(SCRIPT))["rotate_registration"]
SOURCE = "cbfc77db-4abc-5b42-99fb-e9c6a6f130dd"
IDENTITY = "081799f3-98a5-43aa-8669-5af6cd4117fe"


def request():
    command = (f"'/protected/gate' --codewall-managed-hook --principal 'claude-code' "
               f"--enforcer-source-id '{SOURCE}' --supervised-uid 501 "
               f"--service-socket '/protected/service.sock' --service-uid 502 "
               f"--service-installation-id '{IDENTITY}'")
    return {"schema_version": 1, "host": "claude", "binding": {
        "gate": "/protected/gate", "principal": "claude-code", "source": SOURCE,
        "supervised_uid": 501, "service_socket": "/protected/service.sock",
        "service_uid": 502, "service_installation_id": IDENTITY},
        "replacement": {"python": "/usr/bin/python3", "adapter": "/protected/oracle/aos-protected-hook",
                        "evaluator": "/protected/new-gate"},
        "existing_settings": {"companySetting": True, "hooks": {
            "PreToolUse": [{"matcher": "*", "hooks": [
                {"type": "command", "command": command, "timeout": 10},
                {"type": "command", "command": "other-policy", "timeout": 7}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command",
                "command": command + " --event user-prompt-submit", "timeout": 10}]}],
            "FutureEvent": [{"hooks": [{"type": "command", "command": "future-policy"}]}]}}}


class ProtectedSettings(unittest.TestCase):
    def test_rotation_is_explicit_and_preserves_stable_identity(self):
        for kind in ("service_rotation", "local_reenrolment"):
            data = request()
            if kind == "local_reenrolment":
                data["binding"] = dict(gate="/protected/gate", principal="claude-code",
                    source=SOURCE, supervised_uid=501, runtime_home="/runtime",
                    runtime_run_dir="/run", audit_journal="/old/journal",
                    audit_installation_id=IDENTITY)
                module = runpy.run_path(str(SCRIPT))
                for event in ("PreToolUse", "UserPromptSubmit"):
                    data["existing_settings"]["hooks"][event][0]["hooks"][0]["command"] = module["legacy_command"](data["binding"], event)
                changes = dict(audit_journal="/new/journal", audit_installation_id=SOURCE)
                stable = (("runtime_home", "/other"), ("runtime_run_dir", None))
            else:
                changes = dict(service_socket="/new/service.sock", service_installation_id=SOURCE)
                stable = (("service_uid", 503),)
            data["existing_settings"] = PLAN(data)
            binding = dict(data["binding"], **changes)
            with self.assertRaises(ValueError):
                REPAIR(data, binding, data["replacement"])
            rotated = ROTATE(data, binding, data["replacement"], kind)
            self.assertEqual(rotated["hooks"]["FutureEvent"], data["existing_settings"]["hooks"]["FutureEvent"])
            for key, value in stable + (("principal", "other"), ("supervised_uid", 504)):
                with self.assertRaises(ValueError, msg=key):
                    ROTATE(data, dict(binding, **{key: value}), data["replacement"], kind)
            for key in changes:
                with self.assertRaises(ValueError, msg=key):
                    ROTATE(data, dict(binding, **{key: data["binding"][key]}), data["replacement"], kind)
            with self.assertRaises(ValueError):
                ROTATE(data, binding, data["replacement"], "unknown")

    def test_repair_refreshes_source_without_moving_authority_or_other_hooks(self):
        data = request()
        data["existing_settings"] = PLAN(data)
        original = copy.deepcopy(data)
        binding = dict(data["binding"], source=IDENTITY, gate="/protected/new-gate")
        replacement = dict(data["replacement"], adapter="/protected/new/aos-protected-hook",
                           evaluator="/protected/new/codewall-gate")
        updated = REPAIR(data, binding, replacement)
        self.assertEqual(data, original)
        self.assertEqual(updated["hooks"]["FutureEvent"], data["existing_settings"]["hooks"]["FutureEvent"])
        self.assertEqual(updated["hooks"]["PreToolUse"][0]["hooks"][1],
                         data["existing_settings"]["hooks"]["PreToolUse"][0]["hooks"][1])
        for event in ("PreToolUse", "UserPromptSubmit"):
            command = shlex.split(updated["hooks"][event][0]["hooks"][0]["command"])
            self.assertEqual(command[command.index("--enforcer-source-id") + 1], IDENTITY)
            self.assertEqual(command[command.index("--service-installation-id") + 1], IDENTITY)
        for field, value in (("principal", "other"), ("supervised_uid", 504),
                             ("service_uid", 505), ("service_socket", "/different/socket"),
                             ("service_installation_id", SOURCE)):
            with self.assertRaises(ValueError, msg=field):
                REPAIR(data, dict(binding, **{field: value}), replacement)
        with self.assertRaises(ValueError):
            REPAIR(request(), binding, replacement)  # No legacy rewrite through repair.

    def test_mixed_ownership_cannot_leave_an_unaccounted_enforcement_path(self):
        data = request()
        migrated = PLAN(data)
        data["existing_settings"]["hooks"]["FutureEvent"].append({"hooks": [
            migrated["hooks"]["PreToolUse"][0]["hooks"][0]]})
        with self.assertRaises(ValueError):
            PLAN(data)
        removal = request()
        legacy = copy.deepcopy(removal["existing_settings"]["hooks"]["PreToolUse"][0]["hooks"][0])
        removal["existing_settings"] = PLAN(removal)
        removal["existing_settings"]["hooks"]["FutureEvent"].append({"hooks": [legacy]})
        with self.assertRaises(ValueError):
            REMOVE(removal)

    def test_remove_only_oracle_owned_pair_preserves_other_policy(self):
        data = request()
        with self.assertRaises(ValueError):
            REMOVE(data)  # Legacy protection is not an Oracle removal target.
        data["existing_settings"] = PLAN(data)
        original = copy.deepcopy(data)
        result = REMOVE(data)
        self.assertEqual(data, original)
        self.assertTrue(result["companySetting"])
        self.assertEqual(result["hooks"]["FutureEvent"], data["existing_settings"]["hooks"]["FutureEvent"])
        self.assertEqual(result["hooks"]["PreToolUse"][0]["hooks"],
                         [data["existing_settings"]["hooks"]["PreToolUse"][0]["hooks"][1]])
        self.assertEqual(result["hooks"]["UserPromptSubmit"], [])

    def test_removal_refuses_changed_identity_or_incomplete_registration(self):
        data = request()
        data["existing_settings"] = PLAN(data)
        # Protected-service identity is the socket/UID/installation tuple;
        # caller-selected principals are deliberately absent from that command.
        for field, value in (("service_socket", "/other/service.sock"), ("service_uid", 503),
                             ("service_installation_id", SOURCE), ("source", IDENTITY)):
            changed = copy.deepcopy(data)
            changed["binding"][field] = value
            with self.assertRaises(ValueError, msg=field):
                REMOVE(changed)
        data["existing_settings"]["hooks"].pop("UserPromptSubmit")
        with self.assertRaises(ValueError):
            REMOVE(data)

    def test_local_migration_preserves_explicit_runtime_and_audit_identity(self):
        for run_dir in (None, "/local/run"):
            data = request()
            data["binding"] = {
                "gate": "/protected/gate", "principal": "claude-code", "source": SOURCE,
                "supervised_uid": 501, "runtime_home": "/local/runtime",
                "runtime_run_dir": run_dir, "audit_journal": "/local/journal",
                "audit_installation_id": IDENTITY}
            run_env = " ASTRID_RUN_DIR='/local/run'" if run_dir else ""
            unset = "" if run_dir else " -u ASTRID_RUN_DIR"
            command = (f"/usr/bin/env{unset} ASTRID_HOME='/local/runtime'{run_env} "
                       f"'/protected/gate' --codewall-managed-hook --principal 'claude-code' "
                       f"--enforcer-source-id '{SOURCE}' --supervised-uid 501 "
                       f"--audit-journal '/local/journal' --audit-installation-id '{IDENTITY}'")
            for event, suffix in (("PreToolUse", ""), ("UserPromptSubmit", " --event user-prompt-submit")):
                data["existing_settings"]["hooks"][event][0]["hooks"][0]["command"] = command + suffix
            result = PLAN(data)
            words = shlex.split(result["hooks"]["PreToolUse"][0]["hooks"][0]["command"])
            for flag, value in (("--runtime-home", "/local/runtime"), ("--principal", "claude-code"),
                                ("--audit-journal", "/local/journal"), ("--audit-installation-id", IDENTITY)):
                self.assertEqual(words[words.index(flag) + 1], value)
            self.assertNotIn("--service-socket", words)
            self.assertEqual("--runtime-run-dir" in words, run_dir is not None)
            data["binding"]["runtime_home"] = "/different/runtime"
            with self.assertRaises(ValueError):
                PLAN(data)

    def test_replaces_only_exact_bindings_without_mutating_source(self):
        data = request()
        original = copy.deepcopy(data)
        result = PLAN(data)
        self.assertEqual(data, original)
        self.assertTrue(result["companySetting"])
        self.assertEqual(result["hooks"]["FutureEvent"], data["existing_settings"]["hooks"]["FutureEvent"])
        self.assertEqual(result["hooks"]["PreToolUse"][0]["hooks"][1],
                         data["existing_settings"]["hooks"]["PreToolUse"][0]["hooks"][1])
        for event in ("PreToolUse", "UserPromptSubmit"):
            hook = result["hooks"][event][0]["hooks"][0]
            words = shlex.split(hook["command"])
            self.assertEqual(words[:3], ["/usr/bin/python3", "-I", "/protected/oracle/aos-protected-hook"])
            self.assertEqual(words[words.index("--service-installation-id") + 1], IDENTITY)
            self.assertEqual(words[words.index("--service-uid") + 1], "502")
            self.assertEqual(words[words.index("--supervised-uid") + 1], "501")
            self.assertEqual(hook["timeout"], 15)

    def test_changed_authority_and_non_service_bindings_are_rejected(self):
        for field, value in (("principal", "other"), ("source", IDENTITY),
                             ("service_installation_id", SOURCE), ("service_uid", 503),
                             ("supervised_uid", 503), ("service_socket", "/other/socket")):
            data = request()
            data["binding"][field] = value
            with self.assertRaises(ValueError, msg=field):
                PLAN(data)
        data = request()
        data["binding"]["runtime_home"] = "/user/runtime"
        with self.assertRaises(ValueError):
            PLAN(data)

    def test_incomplete_duplicate_wrong_event_and_malformed_registrations_reject(self):
        mutations = [
            lambda hooks: hooks.pop("UserPromptSubmit"),
            lambda hooks: hooks["PreToolUse"].append(copy.deepcopy(hooks["PreToolUse"][0])),
            lambda hooks: hooks.update(FutureEvent=hooks.pop("UserPromptSubmit")),
            lambda hooks: hooks["PreToolUse"][0].update(matcher="Read"),
            lambda hooks: hooks["PreToolUse"][0]["hooks"][0].update(command="--codewall-managed-hook garbage"),
            lambda hooks: hooks["PreToolUse"][0]["hooks"][0].update(async_=True),
        ]
        for mutate in mutations:
            data = request()
            mutate(data["existing_settings"]["hooks"])
            snapshot = copy.deepcopy(data)
            with self.assertRaises(ValueError):
                PLAN(data)
            self.assertEqual(data, snapshot)

    def test_cli_emits_no_plan_on_failure(self):
        data = request()
        data["host"] = "codex"
        result = subprocess.run([sys.executable, "-I", str(SCRIPT)],
                                input=json.dumps(data), text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
