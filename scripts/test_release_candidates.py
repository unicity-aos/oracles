#!/usr/bin/env python3
"""Exercise RC parsing and minimum ordering at the shipped installer boundary."""

import os
import importlib.util
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent


class ReleaseCandidateTests(unittest.TestCase):
    def test_fresh_home_rollback_keeps_the_lock_inode_until_deletion_ends(self) -> None:
        source = (ROOT / "install.sh").read_text().split("\nensure_b3sum\nhosts=", 1)[0]
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw).resolve()
            home = root / "aos"
            fake_bin = root / "bin"
            fake_bin.mkdir()
            remover = fake_bin / "rm"
            remover.write_text(f"#!{sys.executable}\n" + '''
import fcntl, os, pathlib, subprocess, sys
lock = pathlib.Path(os.environ['TEST_INSTALL_LOCK'])
def assert_locked():
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open('a') as fd:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            with open(os.environ['TEST_LOCK_AUDIT'], 'a') as log: log.write('held\\n')
        else:
            raise SystemExit('concurrent installer acquired during cleanup')
assert_locked()
subprocess.run([os.environ['TEST_REAL_RM'], *sys.argv[1:]], check=True)
if str(lock) not in sys.argv[1:]: assert_locked()
''')
            remover.chmod(0o700)
            script = source + '''
INSTALL_TRANSACTION_ACTIVE=1
ensure_install_destinations claude
acquire_install_lock
mkdir -p "$AOS_HOME_DIR/runtime"
printf owned > "$AOS_HOME_DIR/runtime/payload"
WORK=$(mktemp -d "$AOS_HOME_DIR/capture.XXXXXX")
kill -TERM "$$"
exit 92
'''
            audit = root / "audit"
            result = subprocess.run(["sh", "-c", script, "fresh", "--oracle-version", "2026.10.0-rc.9"],
                env=dict(os.environ, AOS_HOME=str(home), PATH=str(fake_bin) + os.pathsep + os.environ["PATH"],
                    TEST_INSTALL_LOCK=str(home / "extensions/oracles/.install.lock"),
                    TEST_LOCK_AUDIT=str(audit), TEST_REAL_RM=shutil.which("rm")),
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 143, result.stderr)
            self.assertGreaterEqual(len(audit.read_text().splitlines()), 3)
            self.assertFalse((home / "runtime").exists())
            self.assertEqual(sorted(str(path.relative_to(home)) for path in home.rglob('*')),
                ["extensions", "extensions/oracles"])

    def test_duplicate_requested_hosts_are_installed_once(self) -> None:
        source = (ROOT / "install.sh").read_text().split("\nensure_b3sum\nhosts=", 1)[0]
        with tempfile.TemporaryDirectory() as raw:
            result = subprocess.run(["sh", "-c", source + '\nselect_hosts\n', "select",
                "--oracle-version", "2026.10.0-rc.9", "--host", "claude", "--host", "codex",
                "--host", "claude", "--host", "grok", "--host", "codex"],
                env=dict(os.environ, AOS_HOME=str(Path(raw).resolve())),
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.split(), ["claude", "codex", "grok"])

    def test_interrupted_receipt_publication_restores_signature_state(self) -> None:
        source = (ROOT / "install.sh").read_text().split("\nensure_b3sum\nhosts=", 1)[0]
        source = source.replace("restore_receipt_path() {\n", "restore_receipt_path() {\n"
            '  [ "$LOCK_HELD" -eq 1 ] && [ -f "$INSTALL_LOCK" ] || exit 91\n')
        for prior_kind in ("absent", "symlink", "regular"):
            for new_signed in (False, True):
                with self.subTest(prior_kind=prior_kind, new_signed=new_signed), tempfile.TemporaryDirectory() as raw:
                    home = Path(raw).resolve()
                    root = home / "extensions/oracles/claude"
                    old = root / "releases/2026.10.0-rc.8"
                    old.mkdir(parents=True)
                    (old / "Pack.lock").write_text("old pack\n")
                    (old / "Pack.toml.sigstore.json").write_text("old signature\n")
                    (root / "current").symlink_to("releases/2026.10.0-rc.8")
                    (root / "Pack.lock").symlink_to("current/Pack.lock")
                    signature = root / "Pack.lock.sigstore.json"
                    if prior_kind == "symlink":
                        signature.symlink_to("current/Pack.toml.sigstore.json")
                    elif prior_kind == "regular":
                        signature.write_text("old flat signature\n")
                        signature.chmod(0o600)
                    stage = home / "pack-stage"
                    stage.mkdir()
                    (stage / "Pack.toml").write_text("new pack\n")
                    if new_signed:
                        (stage / "Pack.toml.sigstore.json").write_text("new signature\n")
                    release_stage = home / "metadata-stage"
                    release_stage.mkdir()
                    for name in ("BLAKE3SUMS.txt", "runtime-compatibility.toml", "bindings"):
                        (release_stage / name).write_text("")
                    script = source + '''
INSTALL_TRANSACTION_ACTIVE=1
AOS_HOME_EXISTED=1
ensure_install_destinations claude
acquire_install_lock
WORK=$(mktemp -d "$AOS_HOME_DIR/capture.XXXXXX")
RELEASE_STAGE="$AOS_HOME_DIR/metadata-stage"
CURRENT_PACK_BINDINGS="$RELEASE_STAGE/bindings"
PLUGIN_BLAKE3=fixture
ASSET_SOURCE=local
capture_receipt_rollback_state claude
mark_host_committed claude
say() {
  case "$*" in *"oracle pack"*"committed"*) kill -TERM "$$" ;; esac
}
write_receipt claude claude-code "$AOS_HOME_DIR/pack-stage"
exit 92
'''
                    result = subprocess.run(["sh", "-c", script, "receipt", "--oracle-version", "2026.10.0-rc.9"],
                        env=dict(os.environ, AOS_HOME=str(home)), capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode, 143, result.stderr)
                    self.assertEqual(os.readlink(root / "current"), "releases/2026.10.0-rc.8")
                    self.assertEqual(os.readlink(root / "Pack.lock"), "current/Pack.lock")
                    if prior_kind == "absent":
                        self.assertFalse(signature.exists())
                        self.assertFalse(signature.is_symlink())
                    elif prior_kind == "symlink":
                        self.assertEqual(os.readlink(signature), "current/Pack.toml.sigstore.json")
                        self.assertEqual(signature.read_text(), "old signature\n")
                    else:
                        self.assertFalse(signature.is_symlink())
                        self.assertEqual(signature.read_text(), "old flat signature\n")
                        self.assertEqual(signature.stat().st_mode & 0o777, 0o600)
                    self.assertFalse((root / "releases/2026.10.0-rc.9").exists())
                    self.assertFalse((home / "extensions/oracles/.install.lock").exists())

    def test_shipped_capsule_readiness_preserves_absence_and_transport_failure(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fake_aos = root / "aos"
            fake_aos.write_text('#!/bin/sh\nprintf "%s\\n" "$TEST_OUTPUT"\nexit "$TEST_STATUS"\n')
            fake_aos.chmod(0o700)
            absence = "✗ capsule 'aos-mcp' is not installed for agent 'codex-code'"
            for host in ("common", "claude", "grok", "unicity-aos"):
                for name in ("aos-up", "aos-doctor"):
                    source = (ROOT / "plugins" / host / "bin" / name).read_text()
                    function = "capsule_ready() {" + source.split("capsule_ready() {", 1)[1].split("\n}\n", 1)[0] + "\n}\n"
                    for notice in ("! Update available: v2026.9.3 → v2026.9.4. Run `astrid update` to upgrade.",
                                   "! Update available: v2026.10.0-rc.2 → v2026.9.4. Run `astrid update` to upgrade.",
                                   "! Update available: v2026.10.0-rc.1 → v2026.10.0-rc.2. Run `astrid update` to upgrade."):
                        for status, diagnostic, expected in ((1, absence, 1), (2, absence, 93),
                                                            (1, "daemon transport unavailable", 93)):
                            with self.subTest(host=host, name=name, notice=notice, status=status, diagnostic=diagnostic):
                                result = subprocess.run(["sh", "-c", function + "capsule_ready\n"],
                                    text=True, capture_output=True, env=dict(os.environ,
                                    AOS=str(fake_aos), AOS_HOME=str(root / "absent-home"), AOS_HOST="codex",
                                    PRINCIPAL="codex-code", TEST_STATUS=str(status), TEST_OUTPUT=notice + "\n" + diagnostic))
                                self.assertEqual(result.returncode, expected, result.stderr)

    def test_shipped_plugin_identity_guards_accept_only_numbered_candidates(self) -> None:
        # Execute each shipped guard, not a replacement regex in the test.
        for host in ("claude", "grok", "unicity-aos"):
            for name in ("aos-install", "aos-up", "aos-doctor", "lib-aos-resolve.sh"):
                path = ROOT / "plugins" / host / "bin" / name
                guards = [line.split(" ||", 1)[0].rstrip(" \\") for line in path.read_text().splitlines()
                          if "grep -Eq" in line and any(variable in line for variable in
                          ("$ORACLE_VERSION", "$BUNDLED_ORACLE_VERSION", "$_aos_oracle_version"))]
                # The common resolver has no release-identity guard; its
                # callers above own that check. Codex's resolver has one.
                self.assertEqual(len(guards), 0 if name == "lib-aos-resolve.sh" and host != "unicity-aos" else 1, str(path))
                for version, accepted in (("2026.10.0", True), ("2026.10.0-rc.1", True),
                                          ("2026.10.0-rc.12", True), ("2026.10.0-rc.0", False),
                                          ("2026.10.0-rc.01", False), ("2026.10.0-beta.1", False),
                                          ("2026.10.0-rc.1+build", False)):
                    for guard in guards:
                        with self.subTest(path=str(path), version=version):
                            result = subprocess.run(["sh", "-c", guard], capture_output=True,
                                env=dict(os.environ, ORACLE_VERSION=version,
                                         BUNDLED_ORACLE_VERSION=version, _aos_oracle_version=version))
                            self.assertEqual(result.returncode == 0, accepted, result.stderr)

    def test_oracle_channel_does_not_request_a_second_aos_install(self) -> None:
        source = (ROOT / "install.sh").read_text()
        function = "ensure_aos() {" + source.split("ensure_aos() {", 1)[1].split("\ncalendar_version_at_least()", 1)[0]
        # Retain exactly the real function, without subsequent installer helpers.
        function = function[:function.index("\n}\n") + 3]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "bin").mkdir()
            binary = root / "bin/aos"
            binary.write_text("#!/bin/sh\nexit 0\n")
            binary.chmod(0o700)
            program = 'set -eu\nhave() { command -v "$1" >/dev/null 2>&1; }\ndie() { echo "$*" >&2; exit 1; }\n' + function + '\nensure_aos\n'
            result = subprocess.run(["sh", "-c", program], text=True, capture_output=True,
                env=dict(os.environ, AOS_HOME_DIR=str(root), NO_INSTALL_AOS="1",
                         AOS_CHANNEL="", AOS_VERSION="", ORACLE_CHANNEL="dev"))
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_exact_aos_install_reads_product_before_labelled_runtime(self) -> None:
        source = (ROOT / "install.sh").read_text()
        function = "ensure_aos() {" + source.split("ensure_aos() {", 1)[1].split("\ndetect_hosts()", 1)[0]
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw).resolve()
            (home / "bin").mkdir()
            binary = home / "bin/aos"
            binary.write_text("#!/bin/sh\nprintf '%s\\n' 'Unicity AOS 2026.10.0-rc.7' '  runtime: Astrid 2026.10.0-rc.6'\n")
            binary.chmod(0o700)
            installer = home / "source-install.sh"
            installer.write_text("#!/bin/sh\nexit 0\n")
            program = 'set -eu\nhave() { command -v "$1" >/dev/null 2>&1; }\ndie() { echo "$*" >&2; exit 1; }\n' + function + '\nensure_aos\n'
            result = subprocess.run(["sh", "-c", program], capture_output=True, text=True,
                env=dict(os.environ, AOS_HOME_DIR=str(home), AOS_INSTALL_URL=str(installer),
                         NO_INSTALL_AOS="0", AOS_CHANNEL="", AOS_VERSION="2026.10.0-rc.7",
                         ASSUME_YES="0", WORK=str(home)))
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_staging_preserves_numeric_source_and_rejects_wrong_base(self) -> None:
        spec = importlib.util.spec_from_file_location("stage_rc", ROOT / "scripts/stage_release_candidate.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "release").mkdir()
            (root / "packs").mkdir()
            (root / "release/oracle-version").write_text("2026.10.0\n")
            for host in ("claude", "codex", "grok"):
                (root / f"packs/{host}.toml").write_text('[pack]\nversion = "2026.10.0"\naos-version = ">=2026.9.1"\n')
            for host in ("claude", "grok", "unicity-aos"):
                (root / f"plugins/{host}").mkdir(parents=True)
                (root / f"plugins/{host}/.aos-oracle-version").write_text("2026.10.0\n")
            with self.assertRaises(ValueError):
                module.stage(root, "2026.11.0-rc.1")
            self.assertEqual((root / "release/oracle-version").read_text(), "2026.10.0\n")
            module.stage(root, "2026.10.0-rc.1")
            for host in ("claude", "codex", "grok"):
                text = (root / f"packs/{host}.toml").read_text()
                self.assertIn('version = "2026.10.0-rc.1"', text)
                self.assertIn('aos-version = ">=2026.9.1"', text)
            self.assertEqual((root / "plugins/unicity-aos/.aos-oracle-version").read_text(), "2026.10.0-rc.1\n")

    def test_dev_discovery_executes_actual_feed_parser(self) -> None:
        source = (ROOT / "install.sh").read_text()
        parser = source.split("import re, sys, xml.etree.ElementTree as ET", 1)[1].split("' \"$ORACLES_REPO\"", 1)[0]
        program = "import re, sys, xml.etree.ElementTree as ET" + parser
        def feed(*urls):
            return '<feed xmlns="http://www.w3.org/2005/Atom">' + ''.join(
                f'<entry><link href="{url}"/></entry>' for url in urls) + '</feed>'
        prefix = "https://github.com/unicity-aos/oracles/releases/tag/v"
        result = subprocess.run(["python3", "-c", program, "unicity-aos/oracles"],
            input=feed(prefix + "2026.10.0", prefix + "2026.10.0-rc.2", prefix + "2026.10.0-rc.1"),
            capture_output=True, text=True, check=True)
        self.assertEqual(result.stdout.strip(), "2026.10.0-rc.2")
        for url in (prefix + "2026.10.0", prefix + "2026.10.0-rc.01", "https://example.com/releases/tag/v2026.10.0-rc.1"):
            with self.subTest(url=url):
                result = subprocess.run(["python3", "-c", program, "unicity-aos/oracles"],
                    input=feed(url), capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)

    def test_explicit_candidates_reach_the_asset_check(self) -> None:
        result = subprocess.run(
            ["sh", str(ROOT / "install.sh"), "--oracle-version", "2026.10.0-rc.1",
             "--aos-version", "2026.10.0-rc.1", "--local-assets", "/nonexistent-oracle-rc-fixture"],
            capture_output=True, text=True, timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("local asset directory not found", result.stderr)

    def test_noncanonical_candidates_fail_before_assets(self) -> None:
        for version in ("2026.10.0-rc.0", "2026.10.0-rc.01", "2026.10.0-beta.1", "2026.10.0-rc.1+build"):
            with self.subTest(version=version):
                result = subprocess.run(
                    ["sh", str(ROOT / "install.sh"), "--oracle-version", version,
                     "--local-assets", "/nonexistent-oracle-rc-fixture"],
                    capture_output=True, text=True, timeout=10,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("invalid oracle version", result.stderr)

    def test_minimum_ordering_executes_shipped_comparator(self) -> None:
        source = (ROOT / "install.sh").read_text()
        function = "calendar_version_at_least() {" + source.split("calendar_version_at_least() {", 1)[1].split("\nruntime_toml_value()", 1)[0]
        for actual, minimum, accepted in (
            ("2026.10.0-rc.1", "2026.9.4", True),
            ("2026.10.0-rc.1", "2026.10.0", False),
            ("2026.9.4-rc.1", "2026.9.4", False),
            ("2026.10.0", "2026.10.0", True),
            ("2026.10.0-rc.1", "2026.10.1", False),
        ):
            with self.subTest(actual=actual, minimum=minimum):
                result = subprocess.run(
                    ["sh", "-c", function + '\ncalendar_version_at_least "$1" "$2"', "compare", actual, minimum],
                    env=dict(os.environ), capture_output=True, text=True,
                )
                self.assertEqual(result.returncode == 0, accepted, result.stderr)

    def test_rejected_installer_keeps_every_committed_host_pointer(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw).resolve()
            pointers = []
            for host in ("claude", "codex", "grok"):
                root = home / "extensions/oracles" / host
                (root / "releases/2026.10.0-rc.8").mkdir(parents=True)
                (root / "releases/2026.10.0-rc.8/Pack.lock").write_text('[pack]\nversion = "2026.10.0-rc.8"\n')
                for name, target in (("current", "releases/2026.10.0-rc.8"), ("Pack.lock", "current/Pack.lock")):
                    (root / name).symlink_to(target)
                    pointers.append((root / name, target))
            for requested_host in ("claude", "codex", "grok"):
                result = subprocess.run(["sh", str(ROOT / "install.sh"), "--host", requested_host,
                    "--oracle-version", "2026.10.0-rc.7",
                    "--no-install-aos", "--skip-host-plugin", "--yes"],
                    env=dict(os.environ, AOS_HOME=str(home)), capture_output=True, text=True, timeout=10)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("restart the host session", result.stderr)
                for pointer, target in pointers:
                    self.assertTrue(pointer.is_symlink(), (requested_host, pointer, result.stderr))
                    self.assertEqual(os.readlink(pointer), target)
                self.assertFalse((home / "extensions/oracles/.install.lock").exists())

    def test_signal_during_activation_preflight_preserves_committed_receipts(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw).resolve()
            fake_bin = home / "fake-bin"
            fake_bin.mkdir()
            python = fake_bin / "python3"
            python.write_text('#!/bin/sh\nif [ "$1" = - ] && [ "${3:-}" = 2026.10.0-rc.7 ]; then\n'
                              '  printf "activation signal injected\\n" >&2\n'
                              '  kill -TERM "$PPID"\n  exit 143\nfi\nexec "$TEST_REAL_PYTHON" "$@"\n')
            python.chmod(0o700)
            pointers = []
            for host in ("claude", "codex", "grok"):
                root = home / "extensions/oracles" / host
                (root / "releases/2026.10.0-rc.8").mkdir(parents=True)
                (root / "releases/2026.10.0-rc.8/Pack.lock").write_text('[pack]\nversion = "2026.10.0-rc.8"\n')
                for name, target in (("current", "releases/2026.10.0-rc.8"), ("Pack.lock", "current/Pack.lock")):
                    (root / name).symlink_to(target)
                    pointers.append((root / name, target))
            result = subprocess.run(["sh", str(ROOT / "install.sh"), "--host", "claude",
                "--oracle-version", "2026.10.0-rc.7", "--no-install-aos", "--skip-host-plugin", "--yes"],
                env=dict(os.environ, AOS_HOME=str(home), PATH=str(fake_bin) + os.pathsep + os.environ["PATH"],
                         TEST_REAL_PYTHON=sys.executable), capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 143, result.stderr)
            self.assertIn("activation signal injected", result.stderr)
            for pointer, target in pointers:
                self.assertTrue(pointer.is_symlink(), (pointer, result.stderr))
                self.assertEqual(os.readlink(pointer), target)
            self.assertFalse((home / "extensions/oracles/.install.lock").exists())

    def test_signal_during_second_host_capture_preserves_both_hosts(self) -> None:
        source = (ROOT / "install.sh").read_text().split("\nensure_b3sum\nhosts=", 1)[0]
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw).resolve()
            pointers = []
            for host in ("claude", "codex", "grok"):
                root = home / "extensions/oracles" / host
                (root / "releases/2026.10.0-rc.8").mkdir(parents=True)
                (root / "releases/2026.10.0-rc.8/Pack.lock").write_text('[pack]\nversion = "2026.10.0-rc.8"\n')
                (root / "current").symlink_to("releases/2026.10.0-rc.8")
                pointers.append((root / "current", "releases/2026.10.0-rc.8"))
                if host == "codex":
                    (root / "Pack.lock").write_text("prior codex receipt\n")
                else:
                    (root / "Pack.lock").symlink_to("current/Pack.lock")
                    pointers.append((root / "Pack.lock", "current/Pack.lock"))
            script = source + '''
INSTALL_TRANSACTION_ACTIVE=1
AOS_HOME_EXISTED=1
ensure_install_destinations "claude codex"
acquire_install_lock
WORK=$(mktemp -d)
capture_receipt_rollback_state claude
[ "$RECEIPT_ROLLBACK_CAPTURED" -eq 1 ] || exit 91
cp() { say "second host capture signal injected" >&2; kill -TERM "$$"; }
capture_receipt_rollback_state codex
exit 92
'''
            result = subprocess.run(["sh", "-c", script, "capture", "--oracle-version", "2026.10.0-rc.8"],
                env=dict(os.environ, AOS_HOME=str(home)), capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 143, result.stderr)
            self.assertIn("second host capture signal injected", result.stderr)
            for pointer, target in pointers:
                self.assertTrue(pointer.is_symlink(), (pointer, result.stderr))
                self.assertEqual(os.readlink(pointer), target)
            self.assertEqual((home / "extensions/oracles/codex/Pack.lock").read_text(), "prior codex receipt\n")
            self.assertFalse((home / "extensions/oracles/.install.lock").exists())

    def test_between_host_signal_keeps_the_committed_receipt(self) -> None:
        source = (ROOT / "install.sh").read_text().split("\nensure_b3sum\nhosts=", 1)[0]
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw).resolve()
            for host in ("claude", "codex"):
                root = home / "extensions/oracles" / host
                for version in ("2026.10.0-rc.8", "2026.10.0-rc.9"):
                    (root / "releases" / version).mkdir(parents=True)
                    (root / "releases" / version / "Pack.lock").write_text(version)
                (root / "current").symlink_to("releases/2026.10.0-rc.8")
                (root / "Pack.lock").symlink_to("current/Pack.lock")
            script = source + '''
INSTALL_TRANSACTION_ACTIVE=1
AOS_HOME_EXISTED=0
ensure_install_destinations "claude codex"
acquire_install_lock
WORK=$(mktemp -d "$AOS_HOME_DIR/capture.XXXXXX")
capture_receipt_rollback_state claude
atomic_symlink releases/2026.10.0-rc.9 "$AOS_HOME_DIR/extensions/oracles/claude/current"
mark_receipt_committed claude
# A signal between hosts must preserve the completed receipt publication.
kill -TERM "$$"
exit 92
'''
            result = subprocess.run(["sh", "-c", script, "commit", "--oracle-version", "2026.10.0-rc.9"],
                env=dict(os.environ, AOS_HOME=str(home)), capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 143, result.stderr)
            for host, version in (("claude", "2026.10.0-rc.9"), ("codex", "2026.10.0-rc.8")):
                root = home / "extensions/oracles" / host
                self.assertEqual(os.readlink(root / "current"), "releases/" + version)
                self.assertEqual(os.readlink(root / "Pack.lock"), "current/Pack.lock")
                self.assertEqual((root / "Pack.lock").read_text(), version)
            self.assertFalse((home / "extensions/oracles/.install.lock").exists())

    def test_plugin_commit_does_not_suppress_partial_receipt_rollback(self) -> None:
        source = (ROOT / "install.sh").read_text().split("\nensure_b3sum\nhosts=", 1)[0]
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw).resolve()
            root = home / "extensions/oracles/claude"
            for version in ("2026.10.0-rc.8", "2026.10.0-rc.9"):
                (root / "releases" / version).mkdir(parents=True)
                (root / "releases" / version / "Pack.lock").write_text(version)
            (root / "current").symlink_to("releases/2026.10.0-rc.8")
            (root / "Pack.lock").write_text("prior flat receipt\n")
            (root / "Pack.lock").chmod(0o600)
            script = source + '''
INSTALL_TRANSACTION_ACTIVE=1
AOS_HOME_EXISTED=1
ensure_install_destinations "claude"
acquire_install_lock
WORK=$(mktemp -d "$AOS_HOME_DIR/capture.XXXXXX")
capture_receipt_rollback_state claude
mark_host_committed claude
NEW_RECEIPT="$AOS_HOME_DIR/extensions/oracles/claude/releases/2026.10.0-rc.9"
atomic_symlink releases/2026.10.0-rc.9 "$AOS_HOME_DIR/extensions/oracles/claude/current"
# Interrupt before the legacy flat Pack.lock is replaced with its new pointer.
kill -TERM "$$"
exit 92
'''
            result = subprocess.run(["sh", "-c", script, "partial", "--oracle-version", "2026.10.0-rc.9"],
                env=dict(os.environ, AOS_HOME=str(home)), capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 143, result.stderr)
            self.assertEqual(os.readlink(root / "current"), "releases/2026.10.0-rc.8")
            self.assertFalse((root / "Pack.lock").is_symlink())
            self.assertEqual((root / "Pack.lock").read_text(), "prior flat receipt\n")
            self.assertEqual((root / "Pack.lock").stat().st_mode & 0o777, 0o600)
            self.assertFalse((root / "releases/2026.10.0-rc.9").exists())
            self.assertFalse((home / "extensions/oracles/.install.lock").exists())

    def test_committed_host_pack_cannot_be_implicitly_downgraded(self) -> None:
        source = (ROOT / "install.sh").read_text()
        function = "validate_oracle_activation() {" + source.split("validate_oracle_activation() {", 1)[1].split("\nruntime_toml_value()", 1)[0]
        with tempfile.TemporaryDirectory(prefix="oracle-activation-") as raw:
            home = Path(raw).resolve()
            host = home / "extensions/oracles/claude"
            host.mkdir(parents=True)
            current = host / "current"
            for installed, requested, allow, accepted in (
                ("2026.10.0-rc.8", "2026.10.0-rc.7", 0, False),
                ("2026.10.0-rc.8", "2026.10.0-rc.8", 0, True),
                ("2026.10.0-rc.9", "2026.10.0-rc.10", 0, True),
                ("2026.10.0-rc.10", "2026.10.0-rc.9", 0, False),
                ("2026.10.0", "2026.10.0-rc.8", 0, False),
                ("2026.10.0-rc.8", "2026.10.0", 0, True),
                ("2026.9.4", "2026.10.0-rc.1", 0, True),
                ("2026.10.0-rc.8", "2026.10.0-rc.7", 1, True),
            ):
                with self.subTest(installed=installed, requested=requested, allow=allow):
                    if current.is_symlink():
                        current.unlink()
                    current.symlink_to("releases/" + installed)
                    result = subprocess.run(
                        ["sh", "-c", function + '\nvalidate_oracle_activation "claude codex grok"'],
                        env=dict(os.environ, AOS_HOME_DIR=str(home), ORACLES_VERSION=requested,
                                 ALLOW_ORACLE_DOWNGRADE=str(allow)),
                        capture_output=True, text=True, timeout=5,
                    )
                    self.assertEqual(result.returncode == 0, accepted, result.stderr)
                    self.assertEqual(os.readlink(current), "releases/" + installed)
                    if not accepted:
                        self.assertIn("restart the host session", result.stderr)
            current.unlink()
            result = subprocess.run(
                ["sh", "-c", function + '\nvalidate_oracle_activation "claude"'],
                env=dict(os.environ, AOS_HOME_DIR=str(home), ORACLES_VERSION="2026.10.0-rc.8",
                         ALLOW_ORACLE_DOWNGRADE="0"),
                capture_output=True, text=True, timeout=5,
            )
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
