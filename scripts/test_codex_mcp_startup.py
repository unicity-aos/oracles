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
PYTHON_DIR = str(Path(shutil.which("python3") or "/usr/bin/python3").resolve().parent)
FAKE_AOS = r"""#!/bin/sh
set -eu
pwd -P > "$TEST_AOS_CWD"
printf "%s\n" "$*" >> "$TEST_AOS_LOG"
printf "%s\n" "$PWD" >> "$TEST_AOS_CWD_LOG"
case " $* " in
  *" capsule show aos-mcp --agent codex-code "*) exit 0 ;;
  *" mcp ready "*) printf "%s\n" '{"version":1,"principal":"codex-code","pid":1,"hook_token":"test"}' ;;
  *" mcp attach "*) printf "%s\n" mcp-ready ;;
  *" mcp serve "*) printf "%s\n" "unexpected mcp serve" >&2; exit 91 ;;
  *) exit 1 ;;
esac
"""


def write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o700)


def python_path() -> str:
    return f"{PYTHON_DIR}:/usr/bin:/bin"


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


def exercise_hook_adapter(root: Path) -> None:
    home = root / "hook-home" / ".aos"
    workspace = root / "hook-workspace"
    plugin_data = root / "hook-plugin-data"
    fake_aos = root / "hook-bin" / "aos"
    args_log = root / "hook-args"
    token_log = root / "hook-tokens"
    payload_log = root / "hook-payload"
    workspace.mkdir()
    write_executable(
        fake_aos,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "$*" >> "$TEST_HOOK_ARGS"\n'
        'printf "%s\\n" "$ASTRID_HOOK_TOKEN" >> "$TEST_HOOK_TOKENS"\n'
        'cat > "$TEST_HOOK_PAYLOAD"\n'
        'printf "%s\\n" "private same-turn context"\n',
    )
    environment = {
        "HOME": str(root / "hook-home"),
        "AOS_HOME": str(home),
        "AOS_BIN": str(fake_aos),
        "AOS_PLUGIN_ROOT": str(PLUGIN),
        "CODEX_PLUGIN_ROOT": str(PLUGIN),
        "PLUGIN_ROOT": str(PLUGIN),
        "CODEX_PLUGIN_DATA": str(plugin_data),
        "ASTRID_PRINCIPAL_ID": "codex-code",
        "ASTRID_CODEX_HOOK_FAIL_CLOSED": "1",
        "PATH": python_path(),
        "TEST_HOOK_ARGS": str(args_log),
        "TEST_HOOK_TOKENS": str(token_log),
        "TEST_HOOK_PAYLOAD": str(payload_log),
        "TMPDIR": str(root),
    }
    payload = json.dumps(
        {"session_id": "hook-session", "turn_id": "turn-one", "prompt": "hello"}
    )
    command = [str(PLUGIN / "bin/aos-up"), "codex", "hook", "user_prompt_submit"]
    for _ in range(2):
        result = subprocess.run(
            command,
            cwd=workspace,
            env=environment,
            input=payload,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
        assert result.stderr == "", result.stderr
        hook_output = json.loads(result.stdout)["hookSpecificOutput"]
        assert hook_output == {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "private same-turn context",
        }

    invocations = args_log.read_text().splitlines()
    assert len(invocations) == 2, invocations
    assert all(
        invocation.startswith(
            "--principal codex-code hook --host codex --session codex-hook-session "
            "--event user_prompt_submit --workspace cwd-"
        )
        for invocation in invocations
    ), invocations
    assert all(" emit " not in f" {invocation} " for invocation in invocations)
    tokens = token_log.read_text().splitlines()
    assert len(tokens) == 2 and tokens[0] == tokens[1], tokens
    assert 32 <= len(tokens[0]) <= 128 and tokens[0].isalnum(), tokens[0]
    assert json.loads(payload_log.read_text()) == json.loads(payload)


def plant_fake_runtime(home: Path, installer: Path, install_log: Path) -> None:
    write_executable(
        installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "$*" >> "$TEST_INSTALL_LOG"\n'
        '[ "$*" = "--host codex --skip-host-plugin --yes --oracle-version 0.2.6" ] '
        '|| { printf "%s\\n" "unexpected installer arguments: $*" >&2; exit 91; }\n'
        'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/runtime/bin" "$AOS_HOME/extensions/oracles/codex"\n'
        'printf "%s\\n" \'version = "0.2.6"\' > "$AOS_HOME/extensions/oracles/codex/Pack.lock"\n'
        'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
        f"{FAKE_AOS}"
        "AOS\n"
        'cp "$AOS_HOME/bin/aos" "$AOS_HOME/runtime/bin/astrid"\n'
        'chmod 700 "$AOS_HOME/bin/aos" "$AOS_HOME/runtime/bin/astrid"\n',
    )
    installer.chmod(0o700)


def main() -> None:
    assert SERVER["command"] == "/bin/sh"
    assert SERVER["args"] == ["./bin/aos-up", "--principal", "codex-code"]
    assert SERVER["cwd"] == "."
    assert SERVER["startup_timeout_sec"] == 20
    assert SERVER["env_vars"] == [
        "AOS_HOME",
        "AOS_BIN",
        "AOS_BIN_ROOT",
        "ASTRID_SESSION_ID",
        "ASTRID_WORKSPACE",
    ]

    with tempfile.TemporaryDirectory(prefix="aos-codex-mcp-") as raw:
        root = Path(raw)
        home = root / "home" / ".aos"
        installer = root / "oracle-installer"
        install_log = root / "installer-args"
        aos_log = root / "aos-args"
        aos_cwd = root / "aos-cwd"
        aos_cwd_log = root / "aos-cwd-log"
        host_workspace = root / "host-project"
        host_workspace.mkdir()
        plant_fake_runtime(home, installer, install_log)

        environment = {
            "HOME": str(root / "home"),
            "AOS_HOME": str(home),
            "AOS_ORACLES_INSTALLER": str(installer),
            "PATH": python_path(),
            "TEST_INSTALL_LOG": str(install_log),
            "TEST_AOS_LOG": str(aos_log),
            "TEST_AOS_CWD": str(aos_cwd),
            "TEST_AOS_CWD_LOG": str(aos_cwd_log),
            "TMPDIR": str(root),
        }

        first = launch(environment)
        assert first.returncode == 0, (first.returncode, first.stdout, first.stderr)
        assert "unexpected mcp serve" not in first.stderr
        assert first.stderr == "", first.stderr
        assert first.stdout.strip() in {"mcp-ready", ""}, first.stdout
        assert (home / "extensions/oracles/codex/Pack.lock").is_file()
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.2.6"
        ]
        invocations = aos_log.read_text().splitlines()
        assert invocations[0] == "capsule show aos-mcp --agent codex-code", invocations
        assert any(" mcp ready " in f" {line} " or line.endswith("mcp ready --format json") or "mcp ready" in line for line in invocations), invocations
        assert any(" mcp attach " in f" {line} " or "mcp attach" in line for line in invocations), invocations
        assert all("mcp serve" not in line for line in invocations), invocations
        runtime = (home / "runtime").resolve()
        attach_lines = [line for line in invocations if "mcp attach" in line]
        assert attach_lines, invocations
        assert f"--workspace {runtime}" in f" {attach_lines[-1]} ", attach_lines[-1]
        assert Path(aos_cwd.read_text().strip()) == runtime

        second = launch(environment)
        assert second.returncode == 0, (second.returncode, second.stdout, second.stderr)
        assert second.stderr == "", second.stderr
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.2.6"
        ], "ready startup unexpectedly re-entered provisioning"

        override_env = dict(environment)
        override_env["ASTRID_WORKSPACE"] = str(host_workspace)
        aos_log.write_text("")
        override = launch(override_env)
        assert override.returncode == 0, (override.returncode, override.stdout, override.stderr)
        override_invocations = aos_log.read_text().splitlines()
        override_attach = [line for line in override_invocations if "mcp attach" in line]
        assert override_attach, override_invocations
        assert f"--workspace {host_workspace.resolve()}" in f" {override_attach[-1]} ", override_attach[-1]
        assert str(PLUGIN.resolve()) not in override_attach[-1]

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
        exercise_hook_adapter(root)


if __name__ == "__main__":
    main()
