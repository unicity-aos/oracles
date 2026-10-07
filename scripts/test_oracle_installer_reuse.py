#!/usr/bin/env python3
"""Host-pack repair must not implicitly replace an installed AOS channel."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
PLUGINS = ("claude", "grok", "unicity-aos")


class InstallerReuseTest(unittest.TestCase):
    def test_channel_selection(self) -> None:
        with tempfile.TemporaryDirectory(prefix="oracle-installer-reuse-") as raw:
            root = Path(raw).resolve()
            home = root / "aos"
            installer = root / "installer.sh"
            installer.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
            executable = home / "bin/aos"
            executable.parent.mkdir(parents=True)
            executable.write_text('#!/bin/sh\nexit 0\n')
            executable.chmod(0o700)
            environment = {
                **os.environ,
                "AOS_HOME": str(home),
                "AOS_ORACLES_INSTALLER": str(installer),
                "TMPDIR": str(root),
            }
            for key in ("AOS_BIN_DIR", "AOS_PLUGIN_ROOT", "CODEX_PLUGIN_ROOT",
                        "CLAUDE_PLUGIN_ROOT", "GROK_PLUGIN_ROOT"):
                environment.pop(key, None)
            cases = (
                (True, (), True),
                (False, (), False),
                (True, ("--aos-channel", "dev"), False),
                (True, ("--aos-channel", "stable"), False),
                (True, ("--aos-version", "2026.10.0-rc.2"), False),
            )
            for plugin in PLUGINS:
                for installed, options, reuse in cases:
                    with self.subTest(plugin=plugin, installed=installed, options=options):
                        executable.chmod(0o700 if installed else 0o600)
                        result = subprocess.run(
                            ["sh", str(ROOT / "plugins" / plugin / "bin/aos-install"),
                             "--host", "codex", "--yes", *options],
                            env=environment, text=True, capture_output=True,
                            timeout=5, check=False,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr)
                        arguments = result.stdout.splitlines()
                        self.assertEqual("--no-install-aos" in arguments, reuse)
                        self.assertEqual(arguments[:3], ["--host", "codex", "--skip-host-plugin"])
                        self.assertIn("--yes", arguments)
                        for option in options:
                            self.assertIn(option, arguments)


if __name__ == "__main__":
    unittest.main()
