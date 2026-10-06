#!/usr/bin/env python3
"""Exercise RC parsing and minimum ordering at the shipped installer boundary."""

import os
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent


class ReleaseCandidateTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
