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
        "PATH": "/usr/bin:/bin",
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
            'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
            "#!/bin/sh\n"
            'pwd -P > "$TEST_AOS_CWD"\n'
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
            "TEST_AOS_CWD": str(aos_cwd),
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
