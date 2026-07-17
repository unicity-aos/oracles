#!/usr/bin/env python3
"""Exercise blank-home Codex MCP bootstrap through the real plugin command."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent.parent
PLUGIN = ROOT / "plugins/unicity-aos"
SERVER = json.loads((PLUGIN / ".mcp.json").read_text())["mcpServers"]["aos"]


def write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o700)


def launch(environment: dict[str, str], plugin: Path = PLUGIN) -> subprocess.CompletedProcess[str]:
    cwd = (plugin / SERVER["cwd"]).resolve()
    return subprocess.run(
        [SERVER["command"], *SERVER["args"]],
        cwd=cwd,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=10,
        check=False,
    )


def main() -> None:
    assert SERVER["command"] == "/bin/sh"
    assert SERVER["args"] == ["./bin/aos-up", "--principal", "codex-code"]
    assert SERVER["cwd"] == "."

    with tempfile.TemporaryDirectory(prefix="aos-codex-mcp-") as raw:
        root = Path(raw)
        home = root / "home" / ".aos"
        installer = root / "oracle-installer"
        install_log = root / "installer-args"
        aos_log = root / "aos-args"

        write_executable(
            installer,
            "#!/bin/sh\n"
            "set -eu\n"
            'printf "%s\\n" "$*" >> "$TEST_INSTALL_LOG"\n'
            '[ "$*" = "--host codex --skip-host-plugin --yes" ] '
            '|| { printf "%s\\n" "unexpected installer arguments: $*" >&2; exit 91; }\n'
            'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/codex"\n'
            'printf "%s\\n" ready > "$AOS_HOME/extensions/oracles/codex/Pack.lock"\n'
            'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
            "#!/bin/sh\n"
            'printf "%s\\n" "$*" >> "$TEST_AOS_LOG"\n'
            'case " $* " in\n'
            '  *" capsule show aos-mcp --agent codex-code "*) exit 0 ;;\n'
            '  *" --principal codex-code mcp serve "*) printf "%s\\n" mcp-ready ;;\n'
            '  *) exit 1 ;;\n'
            "esac\n"
            "AOS\n"
            'chmod 700 "$AOS_HOME/bin/aos"\n',
        )

        environment = {
            "HOME": str(root / "home"),
            "AOS_HOME": str(home),
            "AOS_ORACLES_INSTALLER": str(installer),
            "PATH": "/usr/bin:/bin",
            "TEST_INSTALL_LOG": str(install_log),
            "TEST_AOS_LOG": str(aos_log),
            "TMPDIR": str(root),
        }

        first = launch(environment)
        assert first.returncode == 0, (first.returncode, first.stdout, first.stderr)
        assert first.stdout == "mcp-ready\n", first.stdout
        assert first.stderr == "", first.stderr
        assert (home / "extensions/oracles/codex/Pack.lock").is_file()
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes"
        ]
        assert aos_log.read_text().splitlines() == [
            "capsule show aos-mcp --agent codex-code",
            "--principal codex-code mcp serve",
        ]

        second = launch(environment)
        assert second.returncode == 0, (second.returncode, second.stdout, second.stderr)
        assert second.stdout == "mcp-ready\n", second.stdout
        assert second.stderr == "", second.stderr
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes"
        ], "ready startup unexpectedly re-entered provisioning"

        plugin_copy = root / "plugin-copy"
        shutil.copytree(PLUGIN, plugin_copy)
        configured_environment = dict(environment)
        configured_environment["AOS_BIN"] = str(home / "bin/aos")
        configured_environment["AOS_PLUGIN_ROOT"] = str(plugin_copy)
        subprocess.run(
            [
                "/bin/sh",
                str(plugin_copy / "install.sh"),
                "--bin-root",
                str(home / "bin"),
                "--skip-codex-install",
            ],
            cwd=plugin_copy,
            env=configured_environment,
            check=True,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        generated = json.loads((plugin_copy / ".mcp.json").read_text())["mcpServers"]["aos"]
        assert generated["command"] == SERVER["command"]
        assert generated["args"] == SERVER["args"]
        assert generated["cwd"] == SERVER["cwd"]
        assert generated["startup_timeout_sec"] == SERVER["startup_timeout_sec"]
        assert generated["env"] == {"AOS_BIN": str(home / "bin/aos")}


if __name__ == "__main__":
    main()
