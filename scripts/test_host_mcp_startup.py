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
        # Claude expands CLAUDE_PLUGIN_ROOT in the MCP command string.
        "launch": "plugin-root-var",
    },
    "grok": {
        "root_var": "GROK_PLUGIN_ROOT",
        "principal": "grok-code",
        "timeout_key": "startup_timeout_sec",
        "timeout": 305,
        # Grok Build is more reliable with Codex-style relative launch + cwd
        # than with ${GROK_PLUGIN_ROOT} expansion in the command field.
        "launch": "relative-cwd",
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
    host: str, workspace: Path, environment: dict[str, str]
) -> subprocess.Popen[str]:
    spec = HOSTS[host]
    plugin = ROOT / f"plugins/{host}"
    server = json.loads((plugin / ".mcp.json").read_text())["mcpServers"]["aos"]
    env = dict(environment)
    for key, value in server.get("env", {}).items():
        env.setdefault(key, value)
    if spec["launch"] == "relative-cwd":
        cwd = (plugin / server.get("cwd", ".")).resolve()
        argv = [server["command"], *server["args"]]
    else:
        cwd = workspace
        command = server["command"].replace(
            f"${{{spec['root_var']}}}", str(plugin)
        )
        argv = [command, *server["args"]]
    return subprocess.Popen(
        argv,
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def assert_success(process: subprocess.Popen[str]) -> None:
    try:
        stdout, stderr = process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        stdout, stderr = process.communicate()
        raise AssertionError(f"MCP launcher did not exit: {stdout=} {stderr=}") from None
    assert process.returncode == 0, (process.returncode, stdout, stderr)
    assert stdout == "", stdout
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


def exercise_host(host: str, root: Path) -> None:
    spec = HOSTS[host]
    server = json.loads((ROOT / f"plugins/{host}/.mcp.json").read_text())[
        "mcpServers"
    ]["aos"]
    if spec["timeout_key"] is not None:
        assert server[spec["timeout_key"]] == spec["timeout"]
    else:
        assert "timeout" not in server
        assert "startup_timeout_sec" not in server
    if spec["launch"] == "relative-cwd":
        assert server["command"] == "/bin/sh"
        assert server["cwd"] == "."
        assert server["args"] == ["./bin/aos-up", "--principal", spec["principal"]]
        assert server.get("env", {}).get("AOS_HOST") == host
        assert "AOS_HOME" in server.get("env_vars", [])
        assert "GROK_PLUGIN_ROOT" in server.get("env_vars", [])
    else:
        assert "cwd" not in server
        assert server["args"][-1] == spec["principal"] or server["args"] == [
            "--principal",
            "${user_config.principal}",
        ]

    home = root / host / "home" / ".aos"
    workspace = root / host / "user-project"
    fake_bin = root / host / "fake-bin"
    wait_marker = root / host / "wait-observed"
    wait_gate = root / host / "release-waiter"
    cwd_log = root / host / "aos-cwd"
    args_log = root / host / "aos-args"
    workspace.mkdir(parents=True)
    fake_bin.mkdir()

    write_executable(
        fake_bin / "sleep",
        "#!/bin/sh\n"
        ': > "$TEST_WAIT_MARKER"\n'
        'while [ ! -e "$TEST_WAIT_GATE" ]; do /bin/sleep 0.01; done\n',
    )
    # Keep bootstrap attempts off the network. exercise_host only needs the
    # readiness wait loop; blank-slate coverage owns the real installer path.
    fake_installer = root / host / "fast-installer"
    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "aos-oracles: another oracle installation is active for $AOS_HOME" >&2\n'
        "exit 1\n",
    )
    environment = {
        "HOME": str(root / host / "home"),
        "AOS_HOME": str(home),
        "AOS_HOST": host,
        str(spec["root_var"]): str(ROOT / f"plugins/{host}"),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "PATH": f"{fake_bin}:/usr/bin:/bin",
        "TEST_WAIT_MARKER": str(wait_marker),
        "TEST_WAIT_GATE": str(wait_gate),
        "TEST_AOS_CWD": str(cwd_log),
        "TEST_AOS_ARGS": str(args_log),
    }

    # Model a SessionStart installer already holding the shared lock. These
    # cases control each half of readiness independently; launcher-owned
    # bootstrap is exercised separately below.
    active_lock = home / "extensions/oracles/.install.lock"
    active_lock.parent.mkdir(parents=True)
    active_lock.write_text(f"{os.getpid()}\n")

    write_executable(
        home / "bin/aos",
        "#!/bin/sh\n"
        'pwd -P > "$TEST_AOS_CWD"\n'
        'printf "%s\\n" "$*" > "$TEST_AOS_ARGS"\n',
    )

    # The executable alone is not a committed host installation.
    process = launch(host, workspace, environment)
    wait_for(wait_marker, process)
    receipt = home / f"extensions/oracles/{host}/Pack.lock"
    receipt.parent.mkdir(parents=True)
    receipt.write_text('version = "0.2.6"\n')
    wait_gate.touch()
    assert_success(process)

    assert Path(cwd_log.read_text().strip()) == (home / "runtime").resolve()
    assert args_log.read_text().strip() == (
        f"--principal {spec['principal']} mcp serve"
    )

    # A receipt alone is not ready until the product command is executable.
    (home / "bin/aos").unlink()
    wait_marker.unlink()
    wait_gate.unlink()
    process = launch(host, workspace, environment)
    wait_for(wait_marker, process)
    write_executable(
        home / "bin/aos",
        "#!/bin/sh\n"
        'pwd -P > "$TEST_AOS_CWD"\n'
        'printf "%s\\n" "$*" > "$TEST_AOS_ARGS"\n',
    )
    wait_gate.touch()
    assert_success(process)

    # A ready host never enters the polling path.
    wait_marker.unlink()
    wait_gate.unlink()
    process = launch(host, workspace, environment)
    assert_success(process)
    assert not wait_marker.exists()


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
            exercise_host(host, root)
            exercise_blank_slate_bootstrap(host, root)
            exercise_doctor_waits_for_concurrent_bootstrap(host, root)
            exercise_abandoned_lock_recovery(host, root, "launcher")
            exercise_abandoned_lock_recovery(host, root, "doctor")
            exercise_concurrent_launchers_use_private_logs(host, root)
            exercise_bootstrap_survives_wrapper_timeout(host, root)


if __name__ == "__main__":
    main()
