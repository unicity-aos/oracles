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
RUNTIME_GENERATION = (PLUGIN / ".aos-runtime-generation").read_text().strip()


def write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o700)


def launch(
    environment: dict[str, str],
    plugin: Path = PLUGIN,
    *,
    write_generation: bool = True,
) -> subprocess.CompletedProcess[str]:
    if write_generation:
        marker = Path(environment["AOS_HOME"]) / "update/active-generation.toml"
        marker.parent.mkdir(parents=True, exist_ok=True)
        marker.write_text(
            'schema-version = 1\n'
            f'runtime-generation = "{RUNTIME_GENERATION}"\n'
        )
    receipt = Path(environment["AOS_HOME"]) / "extensions/oracles/codex/Pack.lock"
    if receipt.is_file():
        versions = [
            line.removeprefix('version = "').removesuffix('"')
            for line in receipt.read_text().splitlines()
            if line.startswith('version = "') and line.endswith('"')
        ]
        if len(versions) == 1:
            oracle_marker = receipt.parent / "Generation.lock"
            oracle_marker.write_text(
                f"oracle:codex:{versions[0]}:{RUNTIME_GENERATION}\n"
            )
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
    assert SERVER["env_vars"] == ["AOS_HOME", "AOS_BIN", "AOS_BIN_ROOT"]

    with tempfile.TemporaryDirectory(prefix="aos-codex-mcp-") as raw:
        root = Path(raw)
        home = root / "home" / ".aos"
        installer = root / "oracle-installer"
        install_log = root / "installer-args"
        aos_log = root / "aos-args"
        aos_cwd = root / "aos-cwd"

        write_executable(
            installer,
            "#!/bin/sh\n"
            "set -eu\n"
            'printf "%s\\n" "$*" >> "$TEST_INSTALL_LOG"\n'
            '[ "$*" = "--host codex --skip-host-plugin --yes --oracle-version 0.2.6" ] '
            '|| { printf "%s\\n" "unexpected installer arguments: $*" >&2; exit 91; }\n'
            'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/codex"\n'
            'printf "%s\\n" \'version = "0.2.6"\' > "$AOS_HOME/extensions/oracles/codex/Pack.lock"\n'
            'printf "oracle:codex:0.2.6:%s\\n" "$TEST_RUNTIME_GENERATION" > "$AOS_HOME/extensions/oracles/codex/Generation.lock"\n'
            'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
            "#!/bin/sh\n"
            'pwd -P > "$TEST_AOS_CWD"\n'
            'printf "%s\\n" "$*" >> "$TEST_AOS_LOG"\n'
            'case " $* " in\n'
            '  *" capsule show aos-mcp --agent codex-code "*) exit 0 ;;\n'
            '  *" --principal codex-code mcp serve "*)\n'
            '    [ "$ASTRID_HOST_GENERATION" = "oracle:codex:0.2.6:$TEST_RUNTIME_GENERATION" ]\n'
            '    [ "$ASTRID_HOST_GENERATION_FILE" = "$AOS_HOME/extensions/oracles/codex/Generation.lock" ]\n'
            '    printf "%s\\n" mcp-ready\n'
            '    ;;\n'
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
            "TEST_AOS_CWD": str(aos_cwd),
            "TEST_RUNTIME_GENERATION": RUNTIME_GENERATION,
            "TMPDIR": str(root),
        }

        first = launch(environment)
        assert first.returncode == 0, (first.returncode, first.stdout, first.stderr)
        assert first.stdout == "mcp-ready\n", first.stdout
        assert first.stderr == "", first.stderr
        assert (home / "extensions/oracles/codex/Pack.lock").is_file()
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.2.6"
        ]
        assert aos_log.read_text().splitlines() == [
            "capsule show aos-mcp --agent codex-code",
            "--principal codex-code mcp serve",
        ]
        assert Path(aos_cwd.read_text().strip()) == (home / "runtime").resolve()

        second = launch(environment)
        assert second.returncode == 0, (second.returncode, second.stdout, second.stderr)
        assert second.stdout == "mcp-ready\n", second.stdout
        assert second.stderr == "", second.stderr
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.2.6"
        ], "ready startup unexpectedly re-entered provisioning"

        aos_calls_before_fencing = aos_log.read_text()
        receipt = home / "extensions/oracles/codex/Pack.lock"
        receipt.write_text('version = "0.2.7"\n')
        stale_pack = launch(environment, write_generation=False)
        assert stale_pack.returncode == 78, stale_pack
        assert "newer or invalid for plugin release" in stale_pack.stderr
        assert receipt.read_text() == 'version = "0.2.7"\n'
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.2.6"
        ]
        assert aos_log.read_text() == aos_calls_before_fencing

        receipt.write_text('version = "0.2.6"\n')
        (receipt.parent / "Generation.lock").write_text(
            f"oracle:codex:0.2.6:{RUNTIME_GENERATION}\n"
        )
        active = home / "update/active-generation.toml"
        active.write_text(
            'schema-version = 1\n'
            'runtime-generation = "astrid:0.10.3:0000000000000000000000000000000000000000"\n'
        )
        mixed_runtime = launch(environment, write_generation=False)
        assert mixed_runtime.returncode == 78, mixed_runtime
        assert "does not match installed AOS runtime" in mixed_runtime.stderr
        assert aos_log.read_text() == aos_calls_before_fencing

        hook_command = [
            "/bin/sh",
            "./bin/aos-up",
            "codex",
            "hook",
            "user_prompt_submit",
        ]
        first_hook = subprocess.run(
            hook_command,
            cwd=PLUGIN,
            env=environment,
            text=True,
            input="{}",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        second_hook = subprocess.run(
            hook_command,
            cwd=PLUGIN,
            env=environment,
            text=True,
            input="{}",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert first_hook.returncode == second_hook.returncode == 0
        assert "does not match installed AOS runtime" in first_hook.stderr
        assert second_hook.stderr == ""
        assert aos_log.read_text() == aos_calls_before_fencing

        active.write_text(
            'schema-version = 1\n'
            f'runtime-generation = "{RUNTIME_GENERATION}"\n'
        )

        legacy_home = root / "legacy-home/.aos"
        legacy_aos_called = root / "legacy-aos-called"
        legacy_installer_called = root / "legacy-installer-called"
        write_executable(
            legacy_home / "bin/aos",
            "#!/bin/sh\n" f': > "{legacy_aos_called}"\n',
        )
        legacy_installer = root / "legacy-installer"
        write_executable(
            legacy_installer,
            "#!/bin/sh\n" f': > "{legacy_installer_called}"\n',
        )
        legacy_environment = dict(environment)
        legacy_environment.update(
            {
                "HOME": str(root / "legacy-home"),
                "AOS_HOME": str(legacy_home),
                "AOS_ORACLES_INSTALLER": str(legacy_installer),
            }
        )
        legacy = launch(legacy_environment, write_generation=False)
        assert legacy.returncode == 78, legacy
        assert "no committed runtime generation" in legacy.stderr
        assert not legacy_aos_called.exists()
        assert not legacy_installer_called.exists()

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
        assert generated["env_vars"] == SERVER["env_vars"]
        assert generated["env"] == {"AOS_BIN": str(home / "bin/aos")}


if __name__ == "__main__":
    main()
