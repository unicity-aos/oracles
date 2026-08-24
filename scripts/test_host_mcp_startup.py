#!/usr/bin/env python3
"""Exercise Claude and Grok MCP readiness without timing assumptions."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parent.parent
HOSTS = {
    "claude": {
        "root_var": "CLAUDE_PLUGIN_ROOT",
        "principal": "claude-code",
        "timeout_key": None,
        "timeout": None,
    },
    "grok": {
        "root_var": "GROK_PLUGIN_ROOT",
        "principal": "grok-code",
        "timeout_key": None,
        "timeout": None,
    },
}


def wait_for(path: Path, process: subprocess.Popen[str], timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while not path.exists():
        if process.poll() is not None:
            stdout, stderr = process.communicate()
            raise AssertionError(
                f"MCP launcher exited before readiness marker: {stdout=} {stderr=}"
            )
        if time.monotonic() >= deadline:
            process.kill()
            process.wait()
            raise AssertionError(f"timed out waiting for {path}")
        time.sleep(0.01)


def write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o700)


def wait_for_log_text(
    directory: Path,
    needle: str,
    processes: tuple[subprocess.Popen[str], ...],
    timeout: float = 5.0,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if any(
            needle in path.read_text(errors="replace")
            for path in directory.glob("*")
            if path.is_file()
        ):
            return
        for process in processes:
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                raise AssertionError(
                    f"launcher exited before log marker {needle!r}: {stdout=} {stderr=}"
                )
        time.sleep(0.01)
    raise AssertionError(f"timed out waiting for log marker {needle!r}")


def wait_for_file_count(
    directory: Path,
    count: int,
    process: subprocess.Popen[str],
    timeout: float = 5.0,
) -> None:
    deadline = time.monotonic() + timeout
    while len(list(directory.glob("*"))) < count:
        if process.poll() is not None:
            stdout, stderr = process.communicate()
            raise AssertionError(
                f"launcher exited before {count} arrivals: {stdout=} {stderr=}"
            )
        if time.monotonic() >= deadline:
            process.kill()
            process.wait()
            raise AssertionError(f"timed out waiting for {count} arrivals")
        time.sleep(0.01)


def wait_for_path(path: Path, timeout: float = 5.0) -> None:
    deadline = time.monotonic() + timeout
    while not path.exists():
        if time.monotonic() >= deadline:
            raise AssertionError(f"timed out waiting for {path}")
        time.sleep(0.01)


def launch(
    host: str,
    workspace: Path,
    environment: dict[str, str],
    server: dict | None = None,
) -> subprocess.Popen[str]:
    if server is None:
        server = json.loads((ROOT / f"plugins/{host}/.mcp.json").read_text())[
            "mcpServers"
        ]["aos"]
    command = server["command"].replace(
        f"${{{HOSTS[host]['root_var']}}}", str(ROOT / f"plugins/{host}")
    )
    return subprocess.Popen(
        [command, *server["args"]],
        cwd=workspace,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def assert_success(process: subprocess.Popen[str], expected_stdout: str = "") -> None:
    try:
        stdout, stderr = process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        stdout, stderr = process.communicate()
        raise AssertionError(f"MCP launcher did not exit: {stdout=} {stderr=}") from None
    assert process.returncode == 0, (process.returncode, stdout, stderr)
    assert stdout == expected_stdout, stdout
    assert stderr == "", stderr


def exercise_hook_adapter(host: str, root: Path) -> None:
    spec = HOSTS[host]
    test_root = root / f"{host}-hook"
    home = test_root / "home" / ".aos"
    workspace = test_root / "workspace"
    plugin_data = test_root / "plugin-data"
    fake_aos = test_root / "bin" / "aos"
    args_log = test_root / "hook-args"
    token_log = test_root / "hook-tokens"
    payload_log = test_root / "hook-payload"
    workspace.mkdir(parents=True)
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
        "HOME": str(test_root / "home"),
        "AOS_HOME": str(home),
        "AOS_BIN": str(fake_aos),
        "AOS_HOST": host,
        "AOS_PLUGIN_ROOT": str(ROOT / f"plugins/{host}"),
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "PLUGIN_DATA": str(plugin_data),
        "ASTRID_PRINCIPAL_ID": str(spec["principal"]),
        "ASTRID_HOST_HOOK_FAIL_CLOSED": "1",
        "PATH": "/usr/bin:/bin",
        "TEST_HOOK_ARGS": str(args_log),
        "TEST_HOOK_TOKENS": str(token_log),
        "TEST_HOOK_PAYLOAD": str(payload_log),
        "TMPDIR": str(test_root),
    }
    payload = json.dumps(
        {"session_id": "hook-session", "turn_id": "turn-one", "prompt": "hello"}
    )
    command = [str(ROOT / f"plugins/{host}/bin/aos-up"), "hook", "user_prompt_submit"]
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
    expected = (
        f"--principal {spec['principal']} hook --host {host} "
        f"--session {host}-hook-session --event user_prompt_submit --workspace cwd-"
    )
    assert all(invocation.startswith(expected) for invocation in invocations), invocations
    assert all(" emit " not in f" {invocation} " for invocation in invocations)
    tokens = token_log.read_text().splitlines()
    assert len(tokens) == 2 and tokens[0] == tokens[1], tokens
    assert 32 <= len(tokens[0]) <= 128 and tokens[0].isalnum(), tokens[0]
    assert json.loads(payload_log.read_text()) == json.loads(payload)


def exercise_session_hooks(host: str, root: Path) -> None:
    spec = HOSTS[host]
    hooks = json.loads((ROOT / f"plugins/{host}/hooks/hooks.json").read_text())["hooks"]
    session_start = hooks["SessionStart"]
    ready_hook = session_start[0]["hooks"][0]
    session_hook = session_start[1]["hooks"][0]
    assert "aos-doctor" not in ready_hook["command"]
    assert f"aos-up {host} ready --format hook" in ready_hook["command"]
    assert ready_hook["timeout"] == 15
    assert f"aos-up {host} session-start" in session_hook["command"]
    assert session_hook["timeout"] == 15

    test_root = root / f"{host}-session-hooks"
    home = test_root / "home" / ".aos"
    workspace = test_root / "workspace"
    fake_aos = test_root / "fake-aos"
    args_log = test_root / "args"
    workspace.mkdir(parents=True)
    write_executable(
        fake_aos,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "$*" >> "$TEST_AOS_ARGS"\n'
        'case " $* " in\n'
        f'  *" --principal {spec["principal"]} mcp ready --format hook "*) printf "%s\\n" ready-context ;;\n'
        f'  *" --principal {spec["principal"]} hook --host {host} "*) printf "%s\\n" session-context ;;\n'
        '  *) exit 91 ;;\n'
        "esac\n",
    )
    environment = {
        "HOME": str(test_root / "home"),
        "AOS_HOME": str(home),
        "AOS_BIN": str(fake_aos),
        "AOS_HOST": host,
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "ASTRID_HOST_HOOK_FAIL_CLOSED": "1",
        "PATH": "/usr/bin:/bin",
        "TEST_AOS_ARGS": str(args_log),
        "TMPDIR": str(test_root),
    }
    ready = subprocess.run(
        [str(ROOT / f"plugins/{host}/bin/aos-up"), host, "ready", "--format", "hook"],
        cwd=workspace,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=5,
        check=False,
    )
    assert ready.returncode == 0, (ready.returncode, ready.stdout, ready.stderr)
    assert ready.stdout == "ready-context\n"
    assert ready.stderr == ""

    session = subprocess.run(
        [str(ROOT / f"plugins/{host}/bin/aos-up"), host, "session-start"],
        cwd=workspace,
        env=environment,
        input=json.dumps({"session_id": "session-hook"}),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=5,
        check=False,
    )
    assert session.returncode == 0, (session.returncode, session.stdout, session.stderr)
    assert session.stderr == ""
    assert json.loads(session.stdout)["hookSpecificOutput"] == {
        "hookEventName": "SessionStart",
        "additionalContext": "session-context",
    }
    invocations = args_log.read_text().splitlines()
    assert invocations[0] == (
        f"--principal {spec['principal']} mcp ready --format hook"
    )
    assert invocations[1].startswith(
        f"--principal {spec['principal']} hook --host {host} "
    )
    assert "--event session_start" in invocations[1]


def exercise_host(host: str, root: Path) -> None:
    spec = HOSTS[host]
    server = json.loads((ROOT / f"plugins/{host}/.mcp.json").read_text())[
        "mcpServers"
    ]["aos"]
    assert server["command"] == "/bin/sh"
    assert server["args"][0] == "-c"
    command = server["args"][1]
    assert 'mcp attach --workspace "$workspace"' in command
    assert 'mcp serve --workspace "$workspace" --request-timeout 1d5m' in command
    assert server["startup_timeout_sec"] == 5
    assert server["env_vars"] == [
        "AOS_HOME",
        "AOS_BIN",
        "AOS_BIN_ROOT",
        "AOS_MCP_MODE",
    ]
    assert "cwd" not in server

    home = root / host / "home" / ".aos"
    workspace = root / host / "user-project"
    cwd_log = root / host / "aos-cwd"
    args_log = root / host / "aos-args"
    workspace.mkdir(parents=True)
    (home / "runtime").mkdir(parents=True)
    principal = spec["principal"]
    expected_workspace = workspace.resolve()
    write_executable(
        root / host / "fake-aos",
        "#!/bin/sh\n"
        "set -eu\n"
        'pwd -P > "$TEST_AOS_CWD"\n'
        'printf "%s\\n" "$*" >> "$TEST_AOS_ARGS"\n'
        'case " $* " in\n'
        f'  *" --principal {principal} mcp attach --workspace "*) printf "%s\\n" mcp-ready ;;\n'
        f'  *" --principal {principal} mcp serve --workspace "*) printf "%s\\n" mcp-ready ;;\n'
        '  *" --principal operator-code mcp attach --workspace "*) printf "%s\\n" mcp-ready ;;\n'
        '  *) exit 91 ;;\n'
        "esac\n",
    )
    environment = {
        "HOME": str(root / host / "home"),
        "AOS_HOME": str(home),
        "AOS_BIN": str(root / host / "fake-aos"),
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "PATH": "/usr/bin:/bin",
        "TEST_AOS_CWD": str(cwd_log),
        "TEST_AOS_ARGS": str(args_log),
    }

    first = launch(host, workspace, environment)
    assert_success(first, "mcp-ready\n")
    assert Path(cwd_log.read_text().strip()) == (home / "runtime").resolve()
    assert args_log.read_text().splitlines() == [
        f"--principal {principal} mcp attach --workspace {expected_workspace}"
    ]

    fallback_environment = dict(environment)
    fallback_environment["AOS_MCP_MODE"] = "per-session"
    second = launch(host, workspace, fallback_environment)
    assert_success(second, "mcp-ready\n")
    assert args_log.read_text().splitlines() == [
        f"--principal {principal} mcp attach --workspace {expected_workspace}",
        f"--principal {principal} mcp serve --workspace {expected_workspace} --request-timeout 1d5m",
    ]

    if host == "claude":
        configured = dict(server)
        configured["args"] = [*server["args"][:-1], "operator-code"]
        configured_environment = dict(environment)
        configured_process = launch(
            host, workspace, configured_environment, configured
        )
        assert_success(configured_process, "mcp-ready\n")
        assert args_log.read_text().splitlines()[-1] == (
            f"--principal operator-code mcp attach --workspace {expected_workspace}"
        )


def exercise_blank_slate_bootstrap(host: str, root: Path) -> None:
    spec = HOSTS[host]
    home = root / f"{host}-bootstrap" / "home" / ".aos"
    workspace = root / f"{host}-bootstrap" / "user-project"
    installer_log = root / f"{host}-bootstrap" / "installer-args"
    cwd_log = root / f"{host}-bootstrap" / "aos-cwd"
    args_log = root / f"{host}-bootstrap" / "aos-args"
    fake_installer = root / f"{host}-bootstrap" / "fake-installer"
    workspace.mkdir(parents=True)

    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "$*" > "$TEST_INSTALL_LOG"\n'
        'host=""\n'
        'while [ "$#" -gt 0 ]; do\n'
        '  case "$1" in --host) shift; host=${1:-} ;; esac\n'
        '  shift\n'
        'done\n'
        '[ "$host" = "$TEST_EXPECTED_HOST" ]\n'
        'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/$host"\n'
        'printf "%s\\n" \'version = "0.2.6"\' > "$AOS_HOME/extensions/oracles/$host/Pack.lock"\n'
        'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
        "#!/bin/sh\n"
        'pwd -P > "$TEST_AOS_CWD"\n'
        'printf "%s\\n" "$*" > "$TEST_AOS_ARGS"\n'
        "AOS\n"
        'chmod 700 "$AOS_HOME/bin/aos"\n',
    )

    environment = {
        "HOME": str(root / f"{host}-bootstrap" / "home"),
        "AOS_HOME": str(home),
        "AOS_HOST": host,
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "PATH": "/usr/bin:/bin",
        "TEST_EXPECTED_HOST": host,
        "TEST_INSTALL_LOG": str(installer_log),
        "TEST_AOS_CWD": str(cwd_log),
        "TEST_AOS_ARGS": str(args_log),
    }

    process = launch(host, workspace, environment)
    assert_success(process)
    invocation = installer_log.read_text().strip()
    assert f"--host {host}" in invocation
    assert "--skip-host-plugin" in invocation
    assert "--yes" in invocation
    assert "--oracle-version 0.2.6" in invocation
    assert "--claude-auth" not in invocation
    assert "--claude-mode" not in invocation

    for other_host in HOSTS:
        receipt = home / f"extensions/oracles/{other_host}/Pack.lock"
        assert receipt.exists() == (other_host == host), receipt
    assert Path(cwd_log.read_text().strip()) == (home / "runtime").resolve()
    assert args_log.read_text().strip() == (
        f"--principal {spec['principal']} mcp serve"
    )


def exercise_doctor_waits_for_concurrent_bootstrap(host: str, root: Path) -> None:
    spec = HOSTS[host]
    test_root = root / f"{host}-doctor-race"
    home = test_root / "home" / ".aos"
    fake_bin = test_root / "fake-bin"
    wait_marker = test_root / "wait-observed"
    wait_gate = test_root / "release-waiter"
    fake_installer = test_root / "active-installer"
    attempts = test_root / "installer-attempts"
    fake_bin.mkdir(parents=True)

    write_executable(
        fake_bin / "sleep",
        "#!/bin/sh\n"
        ': > "$TEST_WAIT_MARKER"\n'
        'while [ ! -e "$TEST_WAIT_GATE" ]; do /bin/sleep 0.01; done\n',
    )
    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" attempt >> "$TEST_INSTALL_ATTEMPTS"\n'
        'lock="$AOS_HOME/extensions/oracles/.install.lock"\n'
        'if [ -f "$lock" ]; then\n'
        '  printf "%s\\n" "aos-oracles: another oracle installation is active for $AOS_HOME" >&2\n'
        "  exit 1\n"
        "fi\n"
        'host="$TEST_EXPECTED_HOST"\n'
        'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/$host"\n'
        'printf "%s\\n" \'version = "0.2.6"\' > "$AOS_HOME/extensions/oracles/$host/Pack.lock"\n'
        'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
        "#!/bin/sh\n"
        'case " ${*:-} " in\n'
        '  *" --version "*) printf "%s\\n" "Unicity AOS 2026.1.0" ;;\n'
        '  *" capsule show aos-mcp "*) exit 0 ;;\n'
        '  *" status --json "*) printf "%s\\n" \'{"state":"running"}\' ;;\n'
        '  *" update --check "*) exit 0 ;;\n'
        '  *) exit 0 ;;\n'
        "esac\n"
        "AOS\n"
        'chmod 700 "$AOS_HOME/bin/aos"\n',
    )

    # Another host owns the shared installer lock. Completing that install does
    # not create this host's receipt, so the doctor must retry its own installer.
    active_lock = home / "extensions/oracles/.install.lock"
    active_lock.parent.mkdir(parents=True)
    active_lock.write_text(f"{os.getpid()}\n")

    environment = {
        "HOME": str(test_root / "home"),
        "AOS_HOME": str(home),
        "AOS_HOST": host,
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "PATH": f"{fake_bin}:/usr/bin:/bin",
        "TMPDIR": str(test_root),
        "TEST_WAIT_MARKER": str(wait_marker),
        "TEST_WAIT_GATE": str(wait_gate),
        "TEST_INSTALL_ATTEMPTS": str(attempts),
        "TEST_EXPECTED_HOST": host,
    }
    doctor = subprocess.Popen(
        [str(ROOT / f"plugins/{host}/bin/aos-doctor"), "--format", "hook"],
        cwd=test_root,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    wait_for(wait_marker, doctor)
    active_lock.unlink()
    wait_gate.touch()
    stdout, stderr = doctor.communicate(timeout=5)
    assert doctor.returncode == 0, (doctor.returncode, stdout, stderr)
    assert stderr == "", stderr
    context = json.loads(stdout)["hookSpecificOutput"]["additionalContext"]
    assert "governed oracle session ready" in context
    assert str(spec["principal"]) in context
    assert "Workspace and principal-home Skills can extend this host" in context
    assert "list_skills" in context
    assert "read_skill" in context
    attempt_count = len(attempts.read_text().splitlines())
    assert attempt_count in (1, 2), attempt_count


def exercise_abandoned_lock_recovery(
    host: str, root: Path, actor: str
) -> None:
    spec = HOSTS[host]
    test_root = root / f"{host}-{actor}-abandoned-lock"
    home = test_root / "home" / ".aos"
    workspace = test_root / "user-project"
    fake_bin = test_root / "fake-bin"
    fake_installer = test_root / "contended-installer"
    wait_marker = test_root / "wait-observed"
    wait_gate = test_root / "release-waiter"
    attempts = test_root / "installer-attempts"
    args_log = test_root / "aos-args"
    workspace.mkdir(parents=True)
    fake_bin.mkdir()
    stale_lock = home / "extensions/oracles/.install.lock"
    stale_lock.parent.mkdir(parents=True)

    lock_owner = subprocess.Popen(["/bin/sleep", "60"])
    stale_lock.write_text(f"{lock_owner.pid}\n")

    write_executable(
        fake_bin / "sleep",
        "#!/bin/sh\n"
        ': > "$TEST_WAIT_MARKER"\n'
        'while [ ! -e "$TEST_WAIT_GATE" ]; do /bin/sleep 0.01; done\n',
    )

    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" attempt >> "$TEST_INSTALL_ATTEMPTS"\n'
        'lock="$AOS_HOME/extensions/oracles/.install.lock"\n'
        'if [ -f "$lock" ]; then\n'
        '  owner=$(cat "$lock")\n'
        '  if kill -0 "$owner" 2>/dev/null; then\n'
        '    printf "%s\\n" "aos-oracles: another oracle installation is active for $AOS_HOME" >&2\n'
        "    exit 1\n"
        "  fi\n"
        '  rm -f "$lock"\n'
        "fi\n"
        'host="$TEST_EXPECTED_HOST"\n'
        'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/$host"\n'
        'printf "%s\\n" \'version = "0.2.6"\' > "$AOS_HOME/extensions/oracles/$host/Pack.lock"\n'
        'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
        "#!/bin/sh\n"
        'printf "%s\\n" "$*" >> "$TEST_AOS_ARGS"\n'
        'case " ${*:-} " in\n'
        '  *" --version "*) printf "%s\\n" "Unicity AOS 2026.1.0" ;;\n'
        '  *" capsule show aos-mcp "*) exit 0 ;;\n'
        '  *" status --json "*) printf "%s\\n" \'{"state":"running"}\' ;;\n'
        '  *" update --check "*) exit 0 ;;\n'
        '  *) exit 0 ;;\n'
        "esac\n"
        "AOS\n"
        'chmod 700 "$AOS_HOME/bin/aos"\n',
    )
    environment = {
        "HOME": str(test_root / "home"),
        "AOS_HOME": str(home),
        "AOS_HOST": host,
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "PATH": f"{fake_bin}:/usr/bin:/bin",
        "TMPDIR": str(test_root),
        "TEST_EXPECTED_HOST": host,
        "TEST_INSTALL_ATTEMPTS": str(attempts),
        "TEST_WAIT_MARKER": str(wait_marker),
        "TEST_WAIT_GATE": str(wait_gate),
        "TEST_AOS_ARGS": str(args_log),
    }

    if actor == "launcher":
        process = launch(host, workspace, environment)
    else:
        process = subprocess.Popen(
            [str(ROOT / f"plugins/{host}/bin/aos-doctor"), "--format", "hook"],
            cwd=workspace,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    try:
        wait_for(wait_marker, process)
        lock_owner.terminate()
        lock_owner.wait(timeout=5)
        wait_gate.touch()
        if actor == "launcher":
            assert_success(process)
            assert [
                line
                for line in args_log.read_text().splitlines()
                if line.endswith(" mcp serve")
            ] == [
                f"--principal {spec['principal']} mcp serve"
            ]
        else:
            stdout, stderr = process.communicate(timeout=5)
            assert process.returncode == 0, (process.returncode, stdout, stderr)
            assert stderr == "", stderr
            context = json.loads(stdout)["hookSpecificOutput"]["additionalContext"]
            assert "governed oracle session ready" in context
            assert "Workspace and principal-home Skills can extend this host" in context
    finally:
        if lock_owner.poll() is None:
            lock_owner.terminate()
            lock_owner.wait(timeout=5)
    recorded_attempts = attempts.read_text().splitlines()
    assert recorded_attempts
    assert set(recorded_attempts) == {"attempt"}
    assert not stale_lock.exists()
    assert (home / f"extensions/oracles/{host}/Pack.lock").is_file()


def exercise_concurrent_launchers_use_private_logs(host: str, root: Path) -> None:
    spec = HOSTS[host]
    test_root = root / f"{host}-concurrent-launchers"
    home = test_root / "home" / ".aos"
    workspace = test_root / "user-project"
    fake_installer = test_root / "contended-installer"
    arrivals = test_root / "arrivals"
    loser_marker = test_root / "loser-observed"
    release_gate = test_root / "release-winner"
    args_log = test_root / "aos-args"
    workspace.mkdir(parents=True)

    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'mkdir -p "$TEST_ARRIVALS"\n'
        'touch "$TEST_ARRIVALS/$$"\n'
        'while [ "$(find "$TEST_ARRIVALS" -type f | wc -l | tr -d " ")" -lt 2 ]; do /bin/sleep 0.01; done\n'
        'lock="$AOS_HOME/extensions/oracles/.install.lock"\n'
        'guard="${lock}.guard"\n'
        'mkdir -p "${lock%/*}"\n'
        'if mkdir "$guard" 2>/dev/null; then\n'
        '  printf "%s\\n" "$$" > "$lock"\n'
        '  while [ ! -e "$TEST_LOSER_MARKER" ]; do /bin/sleep 0.01; done\n'
        '  printf "%s\\n" "winner still provisioning"\n'
        '  while [ ! -e "$TEST_RELEASE_GATE" ]; do /bin/sleep 0.01; done\n'
        '  host="$TEST_EXPECTED_HOST"\n'
        '  mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/$host"\n'
        '  printf "%s\\n" \'version = "0.2.6"\' > "$AOS_HOME/extensions/oracles/$host/Pack.lock"\n'
        '  cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
        "#!/bin/sh\n"
        'printf "%s\\n" "$*" >> "$TEST_AOS_ARGS"\n'
        "AOS\n"
        '  chmod 700 "$AOS_HOME/bin/aos"\n'
        '  rm -f "$lock"\n'
        '  rmdir "$guard"\n'
        'else\n'
        '  printf "%s\\n" "aos-oracles: another oracle installation is active for $AOS_HOME" >&2\n'
        '  touch "$TEST_LOSER_MARKER"\n'
        '  /bin/sleep 0.1\n'
        '  exit 1\n'
        'fi\n',
    )
    environment = {
        "HOME": str(test_root / "home"),
        "AOS_HOME": str(home),
        "AOS_HOST": host,
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "PATH": "/usr/bin:/bin",
        "TEST_EXPECTED_HOST": host,
        "TEST_ARRIVALS": str(arrivals),
        "TEST_LOSER_MARKER": str(loser_marker),
        "TEST_RELEASE_GATE": str(release_gate),
        "TEST_AOS_ARGS": str(args_log),
    }

    first = launch(host, workspace, environment)
    wait_for_file_count(arrivals, 1, first)
    second = launch(host, workspace, environment)
    wait_for(loser_marker, second)
    wait_for_log_text(
        home / "cache/oracles", "winner still provisioning", (first, second)
    )
    logs = list((home / "cache/oracles").glob(f"{host}-bootstrap.log.*"))
    assert len(logs) == 2, logs
    assert any("winner still provisioning" in path.read_text() for path in logs)
    assert any("another oracle installation is active" in path.read_text() for path in logs)
    release_gate.touch()
    assert_success(first)
    assert_success(second)
    assert [
        line
        for line in args_log.read_text().splitlines()
        if line.endswith(" mcp serve")
    ] == [
        f"--principal {spec['principal']} mcp serve",
        f"--principal {spec['principal']} mcp serve",
    ]
    assert not list((home / "cache/oracles").glob(f"{host}-bootstrap.log.*"))


def exercise_bootstrap_survives_wrapper_timeout(host: str, root: Path) -> None:
    spec = HOSTS[host]
    test_root = root / f"{host}-wrapper-timeout"
    home = test_root / "home" / ".aos"
    workspace = test_root / "user-project"
    fake_installer = test_root / "delayed-installer"
    started_marker = test_root / "installer-started"
    release_gate = test_root / "release-installer"
    args_log = test_root / "aos-args"
    workspace.mkdir(parents=True)

    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'host=""\n'
        'while [ "$#" -gt 0 ]; do\n'
        '  case "$1" in --host) shift; host=${1:-} ;; esac\n'
        '  shift\n'
        'done\n'
        '[ "$host" = "$TEST_EXPECTED_HOST" ]\n'
        'touch "$TEST_STARTED_MARKER"\n'
        'while [ ! -e "$TEST_RELEASE_GATE" ]; do /bin/sleep 0.01; done\n'
        'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/extensions/oracles/$host"\n'
        'printf "%s\\n" \'version = "0.2.6"\' > "$AOS_HOME/extensions/oracles/$host/Pack.lock"\n'
        'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
        "#!/bin/sh\n"
        'printf "%s\\n" "$*" > "$TEST_AOS_ARGS"\n'
        "AOS\n"
        'chmod 700 "$AOS_HOME/bin/aos"\n',
    )
    environment = {
        "HOME": str(test_root / "home"),
        "AOS_HOME": str(home),
        "AOS_HOST": host,
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "AOS_MCP_STARTUP_TIMEOUT_SECS": "1",
        "PATH": "/usr/bin:/bin",
        "TEST_EXPECTED_HOST": host,
        "TEST_STARTED_MARKER": str(started_marker),
        "TEST_RELEASE_GATE": str(release_gate),
        "TEST_AOS_ARGS": str(args_log),
    }

    first = launch(host, workspace, environment)
    wait_for(started_marker, first)
    stdout, stderr = first.communicate(timeout=3)
    assert first.returncode == 1, (first.returncode, stdout, stderr)
    assert stdout == "", stdout
    assert "startup timed out" in stderr
    receipt = home / f"extensions/oracles/{host}/Pack.lock"
    assert not receipt.exists()

    release_gate.touch()
    wait_for_path(receipt)
    environment["AOS_MCP_STARTUP_TIMEOUT_SECS"] = "300"
    retry = launch(host, workspace, environment)
    assert_success(retry)
    assert args_log.read_text().strip() == (
        f"--principal {spec['principal']} mcp serve"
    )


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="aos-host-mcp-") as raw:
        root = Path(raw)
        for host in HOSTS:
            exercise_hook_adapter(host, root)
            exercise_session_hooks(host, root)
            exercise_host(host, root)


if __name__ == "__main__":
    main()
