#!/usr/bin/env python3
"""Filesystem transaction tests; privilege/readiness are explicitly mocked."""
import copy
import fcntl
import hashlib
import json
from pathlib import Path
import runpy
import stat
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
MODULE = runpy.run_path(str(ROOT / "plugins/common/bin/aos-protected-deploy"))
ACTIVATE = MODULE["activate"]
GLOBALS = ACTIVATE.__globals__
FIXTURE = runpy.run_path(str(ROOT / "scripts/test_protected_settings.py"))["request"]


class SystemInterpreterCustody(unittest.TestCase):
    def test_real_system_interpreter_has_protected_ancestry(self):
        # Actual filesystem metadata, no root activation and no mocks.
        interpreter = Path("/usr/bin/python3").resolve(strict=True)
        MODULE["protected"](str(interpreter), system_executable=True)

    def test_multilink_exception_does_not_apply_to_generated_helpers(self):
        def metadata(path):
            return SimpleNamespace(st_mode=(stat.S_IFREG | 0o755) if path.name == "python3"
                                   else (stat.S_IFDIR | 0o755), st_uid=0, st_nlink=78)
        with patch.object(Path, "lstat", metadata):
            MODULE["protected"]("/usr/bin/python3", system_executable=True)
            with self.assertRaises(ValueError):
                MODULE["protected"]("/usr/bin/python3")

    def test_unprotected_interpreter_is_rejected_even_with_exception(self):
        for mode, uid in ((stat.S_IFREG | 0o777, 0), (stat.S_IFREG | 0o755, 501),
                          (stat.S_IFREG | 0o644, 0), (stat.S_IFLNK | 0o777, 0)):
            def metadata(path):
                return SimpleNamespace(st_mode=mode if path.name == "python3" else stat.S_IFDIR | 0o755,
                                       st_uid=uid if path.name == "python3" else 0, st_nlink=78)
            with patch.object(Path, "lstat", metadata), self.assertRaises(ValueError):
                MODULE["protected"]("/usr/bin/python3", system_executable=True)


class ProtectedDeploy(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.generations = self.root / "generations"
        self.generations.mkdir()
        self.settings = self.root / "managed-settings.json"
        self.original = json.dumps(FIXTURE()["existing_settings"]).encode()
        self.settings.write_bytes(self.original)
        assets = {}
        for name in MODULE["ASSETS"]:
            path = self.root / name
            path.write_bytes((name + " fixture").encode())
            assets[name] = {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        self.request = {"schema_version": 1, "root": str(self.generations),
                        "settings": str(self.settings), "settings_sha256": hashlib.sha256(self.original).hexdigest(),
                        "binding": FIXTURE()["binding"], "python": "/usr/bin/python3", "assets": assets}
        self.guard = patch.dict(GLOBALS, protected=lambda *args, **kwargs: None, verify_ready=lambda *args: None)
        self.guard.start()
        self.uid = patch("os.geteuid", return_value=0)
        self.uid.start()

    def tearDown(self):
        self.uid.stop()
        self.guard.stop()
        # Only this test's TemporaryDirectory, not a real protected generation.
        for path in self.generations.iterdir():
            if path.is_dir():
                path.chmod(0o700)
        self.temporary.cleanup()

    def test_commit_and_retry_preserve_unrelated_settings_and_baseline(self):
        self.settings.chmod(0o600)
        result = ACTIVATE(self.request)
        published = self.settings.read_bytes()
        self.assertNotEqual(published, self.original)
        self.assertEqual((Path(result["generation"]) / "previous-settings.json").read_bytes(), self.original)
        self.assertEqual(ACTIVATE(self.request), result)
        self.assertEqual(self.settings.read_bytes(), published)
        self.assertEqual(self.settings.stat().st_mode & 0o777, 0o600)
        settings = json.loads(published)
        self.assertTrue(settings["companySetting"])
        self.assertEqual(settings["hooks"]["PreToolUse"][0]["hooks"][1]["command"], "other-policy")

    def test_failed_readiness_leaves_old_enforcement_unchanged(self):
        def fail(*args):
            raise ValueError("not ready")
        with patch.dict(GLOBALS, verify_ready=fail):
            with self.assertRaises(ValueError):
                ACTIVATE(self.request)
        self.assertEqual(self.settings.read_bytes(), self.original)
        ACTIVATE(self.request)  # A retry can use the staged verified generation.

    def test_removal_is_exact_retryable_and_does_not_require_a_running_policy(self):
        with self.assertRaises(ValueError):
            ACTIVATE(self.request, remove=True)  # Cannot remove the legacy path.
        self.assertEqual(self.settings.read_bytes(), self.original)
        ACTIVATE(self.request)
        protected_settings = self.settings.read_bytes()
        check = ACTIVATE(self.request, remove=True, check_only=True)
        self.assertEqual(check["registration"], "present")
        self.assertEqual(self.settings.read_bytes(), protected_settings)
        def unavailable(*args):
            raise AssertionError("removal must not restart or probe policy")
        with patch.dict(GLOBALS, verify_ready=unavailable):
            result = ACTIVATE(self.request, remove=True)
            self.assertEqual(ACTIVATE(self.request, remove=True), result)
            self.assertEqual(ACTIVATE(self.request, remove=True, check_only=True)["registration"], "absent")
        settings = json.loads(self.settings.read_bytes())
        self.assertTrue(settings["companySetting"])
        self.assertEqual(settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "other-policy")
        self.assertEqual(settings["hooks"]["UserPromptSubmit"], [])

    def test_removal_preserves_intervening_administrator_changes(self):
        ACTIVATE(self.request)
        changed = self.settings.read_bytes() + b"\n"
        self.settings.write_bytes(changed)
        with self.assertRaises(ValueError):
            ACTIVATE(self.request, remove=True)
        self.assertEqual(self.settings.read_bytes(), changed)

    def test_repair_checks_readiness_before_replacing_and_can_retry_or_remove(self):
        self.assert_successor_transaction(False)

    def test_root_requested_rotation_preserves_old_hooks_until_ready_and_retries(self):
        self.assert_successor_transaction(True)

    def assert_successor_transaction(self, rotate):
        installed = ACTIVATE(self.request)
        old = self.settings.read_bytes()
        successor = copy.deepcopy(self.request)
        successor["settings_sha256"] = hashlib.sha256(old).hexdigest()
        successor["previous_registration"] = {"binding": copy.deepcopy(self.request["binding"]),
            "replacement": {"python": self.request["python"],
                "adapter": str(Path(installed["generation"]) / "aos-protected-hook"),
                "evaluator": str(Path(installed["generation"]) / "codewall-gate")}}
        successor["binding"]["source"] = "11111111-1111-4111-8111-111111111111"
        if rotate:
            successor["binding"]["service_socket"] = "/protected/next.sock"
            successor["binding"]["service_installation_id"] = "22222222-2222-4222-8222-222222222222"
            with self.assertRaises(ValueError):
                ACTIVATE(successor)  # Ordinary repair cannot rotate custody.
            self.assertEqual(self.settings.read_bytes(), old)
            successor["custody_transition"] = "service_rotation"
        def unavailable(*args):
            raise ValueError("new policy unavailable")
        with patch.dict(GLOBALS, verify_ready=unavailable), self.assertRaises(ValueError):
            ACTIVATE(successor)
        self.assertEqual(self.settings.read_bytes(), old)
        result = ACTIVATE(successor)
        self.assertNotEqual(self.settings.read_bytes(), old)
        self.assertEqual(ACTIVATE(successor), result)
        ACTIVATE(successor, remove=True)
        self.assertEqual(json.loads(self.settings.read_bytes())["hooks"]["UserPromptSubmit"], [])

    def test_intervening_edit_is_never_overwritten(self):
        changed = self.original + b"\n"
        def edit(*args):
            self.settings.write_bytes(changed)
        with patch.dict(GLOBALS, verify_ready=edit):
            with self.assertRaises(ValueError):
                ACTIVATE(self.request)
        self.assertEqual(self.settings.read_bytes(), changed)

    def test_asset_mismatch_or_modified_generation_fails_closed(self):
        bad = copy.deepcopy(self.request)
        bad["assets"]["codewall-gate"]["sha256"] = "0" * 64
        with self.assertRaises(ValueError):
            ACTIVATE(bad)
        self.assertEqual(self.settings.read_bytes(), self.original)
        result = ACTIVATE(self.request)
        path = Path(result["generation"]) / "codewall-gate"
        path.chmod(0o755)
        path.write_bytes(b"tampered")
        with self.assertRaises(ValueError):
            ACTIVATE(self.request)

    def test_lock_blocks_another_activation(self):
        lock_path = self.settings.with_name(self.settings.name + ".aos-lock")
        with lock_path.open("wb") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with self.assertRaises(BlockingIOError):
                ACTIVATE(self.request)
        self.assertEqual(self.settings.read_bytes(), self.original)


if __name__ == "__main__":
    unittest.main()
