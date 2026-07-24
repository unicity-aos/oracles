#!/usr/bin/env python3
"""Exercise Codex MCP launch from the inherited project workspace."""

from __future__ import annotations

import json
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


def launch(
    environment: dict[str, str], workspace: Path, plugin: Path = PLUGIN
) -> subprocess.CompletedProcess[str]:
    server = json.loads((plugin / ".mcp.json").read_text())["mcpServers"]["aos"]
    return subprocess.run(
        [server["command"], *server["args"]],
        cwd=workspace,
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
    assert SERVER["args"][0] == "-c"
    assert (
        "exec \"$aos\" --principal codex-code mcp serve --request-timeout 1d5m"
        in SERVER["args"][1]
    )
    assert SERVER["tool_timeout_sec"] == 86_100
    assert "cwd" not in SERVER
    assert SERVER["env_vars"] == ["AOS_HOME", "AOS_BIN", "AOS_BIN_ROOT"]

    with tempfile.TemporaryDirectory(prefix="aos-codex-mcp-") as raw:
        root = Path(raw)
        home = root / "home" / ".aos"
        workspace = root / "user-project"
        workspace.mkdir()
        fake_aos = root / "bin" / "aos"
        aos_log = root / "aos-args"
        aos_cwd = root / "aos-cwd"

        write_executable(
            fake_aos,
            "#!/bin/sh\n"
            "set -eu\n"
            'pwd -P > "$TEST_AOS_CWD"\n'
            'printf "%s\\n" "$*" >> "$TEST_AOS_LOG"\n'
            '[ "$*" = "--principal codex-code mcp serve --request-timeout 1d5m" ] || exit 91\n'
            'printf "%s\\n" mcp-ready\n',
        )

        environment = {
            "HOME": str(root / "home"),
            "AOS_HOME": str(home),
            "AOS_BIN": str(fake_aos),
            "PATH": "/usr/bin:/bin",
            "TEST_AOS_LOG": str(aos_log),
            "TEST_AOS_CWD": str(aos_cwd),
            "TMPDIR": str(root),
        }

        first = launch(environment, workspace)
        assert first.returncode == 0, (first.returncode, first.stdout, first.stderr)
        assert first.stdout == "mcp-ready\n", first.stdout
        assert first.stderr == "", first.stderr
        assert aos_log.read_text().splitlines() == [
            "--principal codex-code mcp serve --request-timeout 1d5m",
        ]
        assert Path(aos_cwd.read_text().strip()) == workspace.resolve()

        plugin_copy = root / "plugin-copy"
        shutil.copytree(PLUGIN, plugin_copy)
        configured_environment = dict(environment)
        configured_environment["AOS_PLUGIN_ROOT"] = str(plugin_copy)
        subprocess.run(
            [
                "/bin/sh",
                str(plugin_copy / "install.sh"),
                "--bin-root",
                str(fake_aos.parent),
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
        assert "cwd" not in generated
        assert generated["startup_timeout_sec"] == SERVER["startup_timeout_sec"]
        assert generated["tool_timeout_sec"] == SERVER["tool_timeout_sec"]
        assert generated["env_vars"] == SERVER["env_vars"]
        assert generated["env"] == {"AOS_BIN": str(fake_aos)}

        generated_result = launch(environment, workspace, plugin_copy)
        assert generated_result.returncode == 0, (
            generated_result.returncode,
            generated_result.stdout,
            generated_result.stderr,
        )
        assert Path(aos_cwd.read_text().strip()) == workspace.resolve()
        exercise_hook_adapter(root)


if __name__ == "__main__":
    main()
