#!/usr/bin/env python3
"""Exercise Codex bootstrap and exact active-release MCP attachment."""

from __future__ import annotations

import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parent.parent
PLUGIN = ROOT / "plugins/unicity-aos"
SERVER = json.loads((PLUGIN / ".mcp.json").read_text())["mcpServers"]["aos"]
PYTHON_DIR = str(Path(shutil.which("python3") or "/usr/bin/python3").resolve().parent)


def write_executable(path: Path, body: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body)
    path.chmod(0o700)


def python_path() -> str:
    return f"{PYTHON_DIR}:/usr/bin:/bin"


def launch(
    environment: dict[str, str], cwd: Path, plugin: Path = PLUGIN
) -> subprocess.CompletedProcess[str]:
    env = dict(environment)
    env["CODEX_PLUGIN_ROOT"] = str(plugin)
    env["PLUGIN_ROOT"] = str(plugin)
    return subprocess.run(
        [SERVER["command"], *SERVER["args"]],
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=10,
        check=False,
    )


def resolve_active(
    environment: dict[str, str], plugin: Path = PLUGIN
) -> subprocess.CompletedProcess[str]:
    env = dict(environment)
    env["AOS_PLUGIN_ROOT"] = str(plugin)
    env["CODEX_PLUGIN_ROOT"] = str(plugin)
    env["PLUGIN_ROOT"] = str(plugin)
    command = (
        f'. "{plugin}/bin/lib-aos-resolve.sh"; '
        'aos_resolve_active_runtime && printf "%s\\n" "$ASTRID"'
    )
    return subprocess.run(
        ["/bin/sh", "-c", command],
        cwd=plugin,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=5,
        check=False,
    )


def runtime_manifest(
    product_version: str = "2026.9.0", runtime_version: str = "0.11.0"
) -> str:
    target = "aarch64-apple-darwin"
    return json.dumps(
        {
            "schema_version": 2,
            "product": {
                "name": "Unicity AOS Community Edition",
                "version": product_version,
            },
            "target": target,
            "layout": {
                "release_directory": f"releases/{product_version}",
                "runtime_executables": "runtime/bin",
                "capsule_assets": "capsules",
            },
            "runtime": {
                "repository": "astrid-runtime/astrid",
                "version": runtime_version,
                "tag": f"v{runtime_version}",
                "asset": f"astrid-{runtime_version}-{target}.tar.gz",
                "digest": "blake3:" + "0" * 64,
                "release_workflow_identity": (
                    "https://github.com/astrid-runtime/astrid/.github/workflows/"
                    f"release.yml@refs/tags/v{runtime_version}"
                ),
            },
        },
        indent=2,
    ) + "\n"


def ready_compatibility(release_ready: bool = True) -> str:
    return (
        'schema-version = 1\n\n[runtime]\n'
        'repository = "astrid-runtime/astrid"\n'
        'version = "0.11.0"\n'
        'tag = "v0.11.0"\n'
        'version-requirement = "=0.11.0"\n'
        'release-workflow-identity = "https://github.com/astrid-runtime/astrid/'
        '.github/workflows/release.yml@refs/tags/v0.11.0"\n'
        f"release-ready = {'true' if release_ready else 'false'}\n"
    )


def executable_statement(
    *,
    schema_version: int = 2,
    target: str = "aarch64-apple-darwin",
    path: str = "runtime/bin/astrid",
    blake3: str = "0" * 64,
    sha256: str = "0" * 64,
) -> str:
    return (
        f"schema-version = {schema_version}\n"
        'product = "unicity-aos-ce"\n'
        'version = "2026.9.0"\n\n'
        "[[executables]]\n"
        f'target = "{target}"\n'
        f'path = "{path}"\n'
        f'blake3 = "{blake3}"\n'
        f'sha256 = "{sha256}"\n'
    )


def full_executable_statement(
    *,
    blake3: str,
    sha256: str,
    daemon_blake3: str,
    daemon_sha256: str,
) -> str:
    records = []
    for target in (
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
    ):
        for path in ("runtime/bin/astrid", "runtime/bin/astrid-daemon"):
            if path == "runtime/bin/astrid":
                blake = blake3
                sha = sha256
            else:
                blake = daemon_blake3
                sha = daemon_sha256
            records.append(
                "[[executables]]\n"
                f'target = "{target}"\n'
                f'path = "{path}"\n'
                f'blake3 = "{blake}"\n'
                f'sha256 = "{sha}"\n'
            )
    return (
        "schema-version = 2\n"
        'product = "unicity-aos-ce"\n'
        'version = "2026.9.0"\n\n' + "\n".join(records)
    )


def plant_fake_runtime(home: Path, installer: Path) -> None:
    statement_script = (
        'cat > "$release/unicity-aos-2026.9.0-release.toml" <<STATEMENT\n'
        "schema-version = 2\n"
        'product = "unicity-aos-ce"\n'
        'version = "2026.9.0"\n'
        "\n"
    )
    for target in (
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-unknown-linux-gnu",
    ):
        for path in ("runtime/bin/astrid", "runtime/bin/astrid-daemon"):
            digest_prefix = "runtime" if path.endswith("/astrid") else "daemon"
            statement_script += (
                "[[executables]]\n"
                f'target = "{target}"\n'
                f'path = "{path}"\n'
                f'blake3 = "${digest_prefix}_blake3"\n'
                f'sha256 = "${digest_prefix}_sha256"\n'
            )
    statement_script += "STATEMENT\n"
    write_executable(
        installer,
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "$*" >> "$TEST_INSTALL_LOG"\n'
        '[ "$*" = "--host codex --skip-host-plugin --yes --oracle-version 0.3.0" ] '
        '|| { printf "%s\\n" "unexpected installer arguments: $*" >&2; exit 91; }\n'
        'release="$AOS_HOME/releases/2026.9.0"\n'
        'receipt_root="$AOS_HOME/extensions/oracles/codex"\n'
        'receipt="$receipt_root/releases/0.3.0"\n'
        'mkdir -p "$AOS_HOME/bin" "$AOS_HOME/runtime/bin" "$release/runtime/bin" "$receipt"\n'
        'printf "%s\\n" \'version = "0.3.0"\' > "$receipt/Pack.lock"\n'
        'cat > "$receipt/Receipt.toml" <<\'RECEIPT\'\n'
        'schema-version = 1\n'
        'oracle-version = "0.3.0"\n'
        'host = "codex"\n'
        'principal = "codex-code"\n'
        'source = "release"\n'
        'plugin-snapshot = "../../../plugins/0.3.0"\n'
        'plugin-blake3 = "0000000000000000000000000000000000000000000000000000000000000000"\n'
        'RECEIPT\n'
        'cat > "$receipt/runtime-compatibility.toml" <<\'COMPAT\'\n'
        f"{ready_compatibility()}"
        "COMPAT\n"
        'cat > "$release/Distro.toml" <<\'DISTRO\'\n'
        'schema-version = 1\n\n[distro]\nid = "unicity-ce"\nversion = "2026.9.0"\n'
        "DISTRO\n"
        'cat > "$release/release-manifest.json" <<\'MANIFEST\'\n'
        f"{runtime_manifest()}"
        "MANIFEST\n"
        'cat > "$AOS_HOME/bin/aos" <<\'AOS\'\n'
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "$*" >> "$TEST_AOS_LOG"\n'
        'pwd -P >> "$TEST_AOS_CWD_LOG"\n'
        'if [ "${1:-}" = --version ]; then printf "%s\\n" "Unicity AOS 2026.9.0"; exit 0; fi\n'
        'case " $* " in\n'
        '  *" capsule show aos-mcp --agent codex-code "*) exit 0 ;;\n'
        '  *) exit 92 ;;\n'
        "esac\n"
        "AOS\n"
        'cat > "$release/runtime/bin/astrid" <<\'ASTRID\'\n'
        "#!/bin/sh\n"
        "set -eu\n"
        'printf "%s\\n" "$*" >> "$TEST_ASTRID_LOG"\n'
        'pwd -P >> "$TEST_ASTRID_CWD_LOG"\n'
        'if [ "${1:-}" = --version ]; then printf "astrid %s\\n" "${TEST_RUNTIME_VERSION:-0.11.0}"; exit 0; fi\n'
        'case " $* " in\n'
        '  *" mcp ready "*) printf "%s\\n" \'{"version":1,"principal":"codex-code"}\' ;;\n'
        '  *" mcp attach "*) printf "%s\\n" mcp-ready ;;\n'
        '  *" mcp serve "*) printf "%s\\n" "unexpected mcp serve" >&2; exit 93 ;;\n'
        '  *) exit 94 ;;\n'
        "esac\n"
        "ASTRID\n"
        'cat > "$release/runtime/bin/astrid-daemon" <<\'DAEMON\'\n'
        "#!/bin/sh\n"
        "exit 0\n"
        "DAEMON\n"
        'cat > "$AOS_HOME/runtime/bin/astrid" <<\'LEGACY\'\n'
        "#!/bin/sh\n"
        ': > "$TEST_LEGACY_RUNTIME_USED"\n'
        "exit 95\n"
        "LEGACY\n"
        'chmod 700 "$AOS_HOME/bin/aos" "$release/runtime/bin/astrid" '
        '"$release/runtime/bin/astrid-daemon" "$AOS_HOME/runtime/bin/astrid"\n'
        'runtime_blake3=$(b3sum "$release/runtime/bin/astrid" | awk \'{print $1}\')\n'
        'runtime_sha256=$(shasum -a 256 "$release/runtime/bin/astrid" | awk \'{print $1}\')\n'
        'daemon_blake3=$(b3sum "$release/runtime/bin/astrid-daemon" | awk \'{print $1}\')\n'
        'daemon_sha256=$(shasum -a 256 "$release/runtime/bin/astrid-daemon" | awk \'{print $1}\')\n'
        f"{statement_script}"
        'ln -s "releases/0.3.0" "$receipt_root/current"\n'
        'ln -s "current/Pack.lock" "$receipt_root/Pack.lock"\n',
    )


def exercise_hook_adapter(root: Path) -> None:
    home = root / "hook-home" / ".aos"
    workspace = root / "hook-workspace"
    plugin_data = root / "hook-plugin-data"
    fake_aos = root / "hook-bin" / "aos"
    args_log = root / "hook-args"
    token_log = root / "hook-tokens"
    payload_log = root / "hook-payload"
    workspace.mkdir(parents=True)
    write_executable(
        fake_aos,
        "#!/bin/sh\n"
        "set -eu\n"
        'case " $* " in\n'
        '  *" capsule show aos-mcp --agent codex-code "*)\n'
        '    printf "%s\\n" '
        '"capsule \'aos-mcp\' is not installed for agent \'codex-code\'" >&2\n'
        "    exit 1\n"
        "    ;;\n"
        "esac\n"
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
        hook_output = json.loads(result.stdout)["hookSpecificOutput"]
        assert hook_output == {
            "hookEventName": "UserPromptSubmit",
            "additionalContext": "private same-turn context",
        }
    invocations = args_log.read_text().splitlines()
    assert len(invocations) == 2, invocations
    assert all("--workspace cwd-" in invocation for invocation in invocations), invocations
    tokens = token_log.read_text().splitlines()
    assert len(tokens) == 2 and tokens[0] == tokens[1], tokens
    assert json.loads(payload_log.read_text()) == json.loads(payload)


def exercise_prebootstrap_principal_gate(root: Path) -> None:
    test_root = root / "codex-prebootstrap-principal"
    home = test_root / "home" / ".aos"
    workspace = test_root / "workspace"
    fake_aos = test_root / "bin" / "aos"
    fake_installer = test_root / "installer"
    aos_log = test_root / "aos-args"
    installer_log = test_root / "installer-args"
    workspace.mkdir(parents=True)
    write_executable(
        fake_aos,
        "#!/bin/sh\n"
        'printf "%s\\n" "$*" >> "$TEST_AOS_LOG"\n'
        "exit 0\n",
    )
    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        'printf "%s\\n" "$*" >> "$TEST_INSTALL_LOG"\n'
        "exit 1\n",
    )
    environment = {
        "HOME": str(test_root / "home"),
        "AOS_BIN": str(fake_aos),
        "AOS_HOME": str(home),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "CODEX_PLUGIN_ROOT": str(PLUGIN),
        "PLUGIN_ROOT": str(PLUGIN),
        "PATH": python_path(),
        "TEST_AOS_LOG": str(aos_log),
        "TEST_INSTALL_LOG": str(installer_log),
        "TMPDIR": str(test_root),
    }
    for foreign_environment in (
        {**environment, "ASTRID_PRINCIPAL_ID": "foreign-principal"},
        {**environment, "AOS_PRINCIPAL_ID": "foreign-principal"},
    ):
        launcher = subprocess.run(
            [str(PLUGIN / "bin/aos-up"), "--help"],
            cwd=workspace,
            env=foreign_environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert launcher.returncode != 0, (launcher.stdout, launcher.stderr)
        assert "refusing non-codex-code principal" in launcher.stderr

        doctor = subprocess.run(
            [str(PLUGIN / "bin/aos-doctor"), "--format", "hook"],
            cwd=workspace,
            env=foreign_environment,
            input="",
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert doctor.returncode != 0, (doctor.stdout, doctor.stderr)
        assert "refusing non-codex-code principal" in doctor.stderr

    assert not aos_log.exists()
    assert not installer_log.exists()
    assert not home.exists()

    for allowed_environment in (
        {**environment, "AOS_PRINCIPAL_ID": ""},
        {**environment, "AOS_PRINCIPAL_ID": "codex-code"},
    ):
        launcher = subprocess.run(
            [str(PLUGIN / "bin/aos-up"), "--help"],
            cwd=workspace,
            env=allowed_environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert launcher.returncode == 0, (launcher.stdout, launcher.stderr)
        assert "--principal codex-code --help" in aos_log.read_text().splitlines(), (
            aos_log.read_text(),
            launcher.stdout,
            launcher.stderr,
        )

        doctor = subprocess.run(
            [str(PLUGIN / "bin/aos-doctor"), "--format", "human"],
            cwd=workspace,
            env=allowed_environment,
            input="",
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert doctor.returncode == 0, (doctor.stdout, doctor.stderr)
        assert "is not provisioned for the oracle pack" in doctor.stdout
    assert not installer_log.exists()
    assert not home.exists()


def exercise_transport_failure(root: Path) -> None:
    test_root = root / "codex-capsule-transport"
    home = test_root / "home" / ".aos"
    workspace = test_root / "workspace"
    fake_aos = test_root / "bin" / "aos"
    fake_installer = test_root / "installer"
    aos_log = test_root / "aos-args"
    installer_log = test_root / "installer-args"
    workspace.mkdir(parents=True)
    write_executable(
        fake_aos,
        "#!/bin/sh\n"
        'printf "%s\\n" "$*" >> "$TEST_AOS_LOG"\n'
        'printf "%s\\n" "daemon transport failed while reading capsule metadata" >&2\n'
        "exit 93\n",
    )
    write_executable(
        fake_installer,
        "#!/bin/sh\n"
        'printf "%s\\n" "$*" >> "$TEST_INSTALL_LOG"\n'
        "exit 1\n",
    )
    environment = {
        "HOME": str(test_root / "home"),
        "AOS_BIN": str(fake_aos),
        "AOS_HOME": str(home),
        "AOS_ORACLES_INSTALLER": str(fake_installer),
        "CODEX_PLUGIN_ROOT": str(PLUGIN),
        "PLUGIN_ROOT": str(PLUGIN),
        "PATH": python_path(),
        "TEST_AOS_LOG": str(aos_log),
        "TEST_INSTALL_LOG": str(installer_log),
        "TMPDIR": str(test_root),
    }
    for arguments in ([], ["codex", "ensure-principal"]):
        result = subprocess.run(
            [str(PLUGIN / "bin/aos-up"), *arguments],
            cwd=workspace,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert result.returncode == 93, (arguments, result.stdout, result.stderr)
        assert "daemon transport failed while reading capsule metadata" in result.stderr
        doctor = subprocess.run(
            [str(PLUGIN / "bin/aos-doctor"), "--format", "hook"],
            cwd=workspace,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert doctor.returncode == 93, (doctor.stdout, doctor.stderr)
        assert "daemon transport failed while reading capsule metadata" in doctor.stderr
    assert aos_log.exists()
    assert not installer_log.exists()
    assert not (home / "runtime").exists()
    assert not (home / "cache").exists()
    assert not (home / "extensions/oracles/codex/Pack.lock").exists()


def main() -> None:
    assert SERVER["command"] == "/bin/sh"
    assert SERVER["args"][0] == "-c"
    assert "CODEX_PLUGIN_ROOT" in SERVER["args"][1]
    assert "cwd" not in SERVER
    assert SERVER["startup_timeout_sec"] == 20
    assert SERVER["env_vars"] == [
        "AOS_HOME",
        "AOS_BIN",
        "AOS_BIN_ROOT",
        "ASTRID_SESSION_ID",
        "ASTRID_WORKSPACE",
        "AOS_HOST_WORKSPACE",
        "CODEX_WORKSPACE",
    ]

    with tempfile.TemporaryDirectory(prefix="aos-codex-mcp-") as raw:
        root = Path(raw).resolve()
        exercise_prebootstrap_principal_gate(root)
        exercise_transport_failure(root)
        home = root / "home" / ".aos"
        fake_bin = root / "bin"
        write_executable(
            fake_bin / "b3sum",
            "#!/bin/sh\nshasum -a 256 \"$1\" | awk '{print $1}'\n",
        )
        installer = root / "oracle-installer"
        install_log = root / "installer-args"
        aos_log = root / "aos-args"
        aos_cwd_log = root / "aos-cwd-log"
        astrid_log = root / "astrid-args"
        astrid_cwd_log = root / "astrid-cwd-log"
        legacy_used = root / "legacy-runtime-used"
        host_workspace = root / "host-project"
        other_workspace = root / "other-project"
        host_workspace.mkdir()
        other_workspace.mkdir()
        plant_fake_runtime(home, installer)

        environment = {
            "HOME": str(root / "home"),
            "AOS_HOME": str(home),
            "AOS_ORACLES_INSTALLER": str(installer),
            "PATH": f"{fake_bin}:{python_path()}",
            "TEST_INSTALL_LOG": str(install_log),
            "TEST_AOS_LOG": str(aos_log),
            "TEST_AOS_CWD_LOG": str(aos_cwd_log),
            "TEST_ASTRID_LOG": str(astrid_log),
            "TEST_ASTRID_CWD_LOG": str(astrid_cwd_log),
            "TEST_LEGACY_RUNTIME_USED": str(legacy_used),
            "TMPDIR": str(root),
        }

        first = launch(environment, host_workspace)
        assert first.returncode == 0, (first.returncode, first.stdout, first.stderr)
        assert first.stderr == "", first.stderr
        assert first.stdout.strip() in {"mcp-ready", ""}, first.stdout
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.3.0"
        ]
        attach = [line for line in astrid_log.read_text().splitlines() if "mcp attach" in line]
        assert attach == [
            f"--principal codex-code mcp attach --workspace {host_workspace.resolve()}"
        ], attach
        assert not legacy_used.exists(), "mutable runtime/bin Astrid was executed"
        runtime_home = (home / "runtime").resolve()
        cwd_lines = astrid_cwd_log.read_text().splitlines()
        assert all(Path(line) == runtime_home for line in cwd_lines), (
            runtime_home,
            cwd_lines,
            home,
        )

        second = launch(environment, host_workspace)
        assert second.returncode == 0, (second.returncode, second.stdout, second.stderr)
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.3.0"
        ], "ready startup unexpectedly re-entered provisioning"

        # A provisioned default home must survive a relaunch whose environment
        # has no AOS_HOME; the adapter exports the already-resolved home.
        default_home_environment = {
            key: value for key, value in environment.items() if key != "AOS_HOME"
        }
        default_home = launch(default_home_environment, host_workspace)
        assert default_home.returncode == 0, (
            default_home.returncode,
            default_home.stdout,
            default_home.stderr,
        )
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.3.0"
        ], "default-home relaunch unexpectedly re-entered provisioning"

        # The equals form is dispatch, not a raw-AOS escape hatch. It must use
        # the same byte-gated frame path as the separated value form.
        astrid_log.write_text("")
        equals_form = subprocess.run(
            [str(PLUGIN / "bin/aos-up"), "--principal=codex-code"],
            cwd=host_workspace,
            env=default_home_environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=10,
            check=False,
        )
        assert equals_form.returncode == 0, (
            equals_form.returncode,
            equals_form.stdout,
            equals_form.stderr,
        )
        equals_attach = [
            line for line in astrid_log.read_text().splitlines() if "mcp attach" in line
        ]
        assert equals_attach == [
            f"--principal codex-code mcp attach --workspace {host_workspace.resolve()}"
        ], equals_attach
        assert not any("mcp serve" in line for line in astrid_log.read_text().splitlines())

        # Every accepted spelling and dispatch position takes the same byte
        # gated attach path. A principal in a command tail is rejected rather
        # than silently promoted to a raw product-CLI invocation.
        astrid_log.write_text("")
        for principal_arguments in (
            ["--principal=codex-code"],
            ["--principal", "codex-code"],
            ["--", "--principal=codex-code"],
            ["--", "--principal", "codex-code"],
        ):
            astrid_log.write_text("")
            dispatch = subprocess.run(
                [str(PLUGIN / "bin/aos-up"), *principal_arguments],
                cwd=host_workspace,
                env=default_home_environment,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=10,
                check=False,
            )
            assert dispatch.returncode == 0, (
                principal_arguments,
                dispatch.stdout,
                dispatch.stderr,
            )
            expected = (
                f"--principal codex-code mcp attach --workspace {host_workspace.resolve()}"
            )
            dispatch_entries = [
                line for line in astrid_log.read_text().splitlines() if "mcp attach" in line
            ]
            assert dispatch_entries == [expected], (
                principal_arguments,
                astrid_log.read_text(),
            )
            assert not any(
                "mcp serve" in line for line in astrid_log.read_text().splitlines()
            )

        astrid_log_before = astrid_log.read_text()
        for misplaced_arguments in (
            ["foo", "--principal=codex-code"],
            ["foo", "--principal", "codex-code"],
            ["--help", "--", "--foo"],
            ["--", "--", "--help"],
        ):
            rejected_position = subprocess.run(
                [str(PLUGIN / "bin/aos-up"), *misplaced_arguments],
                cwd=host_workspace,
                env=default_home_environment,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=5,
                check=False,
            )
            assert rejected_position.returncode != 0, (
                misplaced_arguments,
                rejected_position.stdout,
                rejected_position.stderr,
            )
        assert astrid_log.read_text() == astrid_log_before

        override_env = dict(environment)
        override_env["ASTRID_WORKSPACE"] = str(other_workspace.resolve())
        astrid_log.write_text("")
        override = launch(override_env, host_workspace)
        assert override.returncode == 0, (override.returncode, override.stdout, override.stderr)
        override_attach = [line for line in astrid_log.read_text().splitlines() if "mcp attach" in line]
        assert override_attach == [
            f"--principal codex-code mcp attach --workspace {other_workspace.resolve()}"
        ], override_attach

        conflict_env = dict(environment)
        conflict_env["ASTRID_WORKSPACE"] = str(host_workspace.resolve())
        conflict_env["CODEX_WORKSPACE"] = str(other_workspace.resolve())
        conflict = launch(conflict_env, host_workspace)
        assert conflict.returncode != 0, (conflict.stdout, conflict.stderr)
        assert "conflicting project workspace identities" in conflict.stderr

        plugin_cwd = launch(environment, PLUGIN)
        assert plugin_cwd.returncode != 0, (plugin_cwd.stdout, plugin_cwd.stderr)
        assert "did not supply an exact project workspace" in plugin_cwd.stderr

        # ASTRID_HOME is Astrid state configuration, never an executable-root
        # override. A malicious or stale value cannot redirect active release
        # resolution away from the AOS-owned product runtime.
        astrid_state_home = root / "astrid-state-home"
        astrid_state_home.mkdir()
        state_env = dict(environment)
        state_env["ASTRID_HOME"] = str(astrid_state_home)
        astrid_log.write_text("")
        state_launch = launch(state_env, host_workspace)
        assert state_launch.returncode == 0, (
            state_launch.returncode,
            state_launch.stdout,
            state_launch.stderr,
        )
        state_attach = [
            line for line in astrid_log.read_text().splitlines() if "mcp attach" in line
        ]
        assert state_attach == [
            f"--principal codex-code mcp attach --workspace {host_workspace.resolve()}"
        ], state_attach

        # Both the explicit argv boundary and ambient principal configuration
        # fail closed before selecting or executing the runtime.
        astrid_log_before = astrid_log.read_text()
        for foreign_environment, foreign_args in (
            ({**environment, "ASTRID_PRINCIPAL_ID": "foreign-principal"}, []),
            (environment, ["--principal", "foreign-principal"]),
            (environment, ["--principal=foreign-principal"]),
        ):
            foreign = subprocess.run(
                [str(PLUGIN / "bin/aos-up"), *foreign_args],
                cwd=host_workspace,
                env=foreign_environment,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=5,
                check=False,
            )
            assert foreign.returncode != 0, (foreign.stdout, foreign.stderr)
            assert "refusing non-codex-code principal" in foreign.stderr
        assert astrid_log.read_text() == astrid_log_before, (
            astrid_log_before,
            astrid_log.read_text(),
        )

        resolved = resolve_active(environment)
        expected_runtime = home / "releases/2026.9.0/runtime/bin/astrid"
        assert resolved.returncode == 0, (resolved.stdout, resolved.stderr)
        assert resolved.stdout.strip() == str(expected_runtime)

        # Selection alone is not launch authority. The byte gate is callable
        # immediately before every adapter exec and rejects post-selection
        # substitution before the runtime can start.
        executable_bytes = expected_runtime.read_bytes()
        expected_digests = subprocess.run(
            [
                "/bin/sh",
                "-c",
                f'printf "%s %s" "$(b3sum {expected_runtime} | awk \'{{print $1}}\')" '
                f'"$(shasum -a 256 {expected_runtime} | awk \'{{print $1}}\')"',
            ],
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            check=True,
        ).stdout.split()
        expected_runtime.write_bytes(executable_bytes + b"# substituted\n")
        byte_gate = subprocess.run(
            [
                "/bin/sh",
                "-c",
                f'. "{PLUGIN}/bin/lib-aos-resolve.sh"; '
                f'ASTRID="{expected_runtime}"; export ASTRID; '
                f'_aos_active_runtime_path="{expected_runtime}"; '
                f'_aos_home="{home}"; '
                f'_aos_release="{home / "releases/2026.9.0"}"; '
                f'_aos_expected_blake3="{expected_digests[0]}"; '
                f'_aos_expected_sha256="{expected_digests[1]}"; '
                "export _aos_expected_blake3 _aos_expected_sha256; "
                "_aos_verify_active_runtime_bytes",
            ],
            cwd=PLUGIN,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert byte_gate.returncode != 0, (byte_gate.stdout, byte_gate.stderr)
        assert "BLAKE3 digest does not match" in byte_gate.stderr
        expected_runtime.write_bytes(executable_bytes)
        expected_runtime.chmod(0o700)

        noncanonical_gate = subprocess.run(
            [
                "/bin/sh",
                "-c",
                f'. "{PLUGIN}/bin/lib-aos-resolve.sh"; '
                'ASTRID="/bin/sh"; export ASTRID; '
                f'_aos_active_runtime_path="{expected_runtime}"; '
                f'_aos_home="{home}"; '
                f'_aos_release="{home / "releases/2026.9.0"}"; '
                f'_aos_expected_blake3="{expected_digests[0]}"; '
                f'_aos_expected_sha256="{expected_digests[1]}"; '
                "export _aos_active_runtime_path _aos_home _aos_release "
                "_aos_expected_blake3 _aos_expected_sha256; "
                "_aos_verify_active_runtime_bytes",
            ],
            cwd=PLUGIN,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=5,
            check=False,
        )
        assert noncanonical_gate.returncode != 0
        assert "not the canonical active release executable" in noncanonical_gate.stderr

        # The schema-v2 executable table is the only accepted byte authority.
        # Its absence, v1 predecessor form, and archive-only manifest digest
        # all fail closed.
        statement = home / "releases/2026.9.0/unicity-aos-2026.9.0-release.toml"
        statement_text = statement.read_text()
        statement.unlink()
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "no regular signed executable statement" in rejected.stderr
        statement.write_text(executable_statement(schema_version=1))
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "must use schema-version 2" in rejected.stderr
        statement.write_text(
            'schema-version = 2\nproduct = "unicity-aos-ce"\nversion = "2026.9.0"\n'
        )
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "exactly eight executable records" in rejected.stderr
        wrong_path_statement = statement_text.replace(
            'path = "runtime/bin/astrid"\n',
            'path = "runtime/bin/astrid-other"\n',
            1,
        )
        statement.write_text(wrong_path_statement)
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "unsupported executable path" in rejected.stderr
        statement.write_text(statement_text)

        daemon_runtime = home / "releases/2026.9.0/runtime/bin/astrid-daemon"
        daemon_bytes = daemon_runtime.read_bytes()
        daemon_runtime.unlink()
        accepted = resolve_active(environment)
        assert accepted.returncode == 0, (accepted.stdout, accepted.stderr)
        daemon_runtime.write_text("#!/bin/sh\nexit 3\n")
        daemon_runtime.chmod(0o700)
        accepted = resolve_active(environment)
        assert accepted.returncode == 0, (accepted.stdout, accepted.stderr)
        daemon_runtime.write_bytes(daemon_bytes)
        daemon_runtime.chmod(0o700)

        substituted = expected_runtime.read_text() + "# substituted bytes\n"
        expected_runtime.write_text(substituted)
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "BLAKE3 digest does not match" in rejected.stderr
        expected_runtime.write_text(expected_runtime.read_text().replace(substituted, ""))

        preserved_bytes = expected_runtime.read_bytes()
        expected_runtime.unlink()
        expected_runtime.symlink_to(root / "outside-runtime")
        (root / "outside-runtime").write_text("#!/bin/sh\nexit 0\n")
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "missing its bundled Astrid CLI" in rejected.stderr
        expected_runtime.unlink()
        expected_runtime.write_bytes(preserved_bytes)
        expected_runtime.chmod(0o700)

        # The same matching bytes reached through a symlinked home ancestor are
        # not a canonical release path, even when every named component looks
        # regular under the alias.
        home_alias = root / "alias-home-parent"
        home_alias.symlink_to(home.parent, target_is_directory=True)
        alias_environment = dict(environment)
        alias_environment["AOS_HOME"] = str(home_alias / ".aos")
        rejected = resolve_active(alias_environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "symlinked home ancestor" in rejected.stderr
        canceled_alias_environment = dict(environment)
        canceled_alias_environment["AOS_HOME"] = str(
            home_alias / ".." / "home" / ".aos"
        )
        rejected = resolve_active(canceled_alias_environment)
        assert rejected.returncode != 0, (
            canceled_alias_environment["AOS_HOME"],
            rejected.stdout,
            rejected.stderr,
        )
        assert "symlinked home ancestor" in rejected.stderr, (
            canceled_alias_environment["AOS_HOME"],
            rejected.stdout,
            rejected.stderr,
        )
        home_alias.unlink()

        wrong_runtime = dict(environment)
        wrong_runtime["TEST_RUNTIME_VERSION"] = "0.10.4"
        rejected = resolve_active(wrong_runtime)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "BLAKE3 digest does not match" in rejected.stderr
        expected_runtime.write_text(
            "#!/bin/sh\n"
            "set -eu\n"
            '[ "${1:-}" = --version ] && printf "astrid 0.10.4\\n"\n'
        )
        expected_runtime.chmod(0o700)
        wrong_report_digests = subprocess.run(
            ["/bin/sh", "-c", (
                f'printf "%s %s" "$(b3sum {expected_runtime} | awk \'{{print $1}}\')" '
                f'"$(shasum -a 256 {expected_runtime} | awk \'{{print $1}}\')"'
            )],
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=True,
        ).stdout.split()
        current_digests = re.search(
            r'^blake3 = "([0-9a-f]+)"\nsha256 = "([0-9a-f]+)"$',
            statement_text,
            re.MULTILINE,
        )
        assert current_digests
        wrong_statement = statement_text.replace(
            current_digests.group(1), wrong_report_digests[0]
        ).replace(current_digests.group(2), wrong_report_digests[1])
        statement.write_text(wrong_statement)
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "expected 0.11.0" in rejected.stderr
        statement.write_text(statement_text)

        manifest = home / "releases/2026.9.0/release-manifest.json"
        manifest.write_text(runtime_manifest(runtime_version="0.10.4"))
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "runtime identity mismatch" in rejected.stderr
        manifest.write_text(runtime_manifest())

        malformed_manifest = json.loads(runtime_manifest())
        malformed_manifest["layout"]["runtime_executables"] = "../runtime/bin"
        manifest.write_text(json.dumps(malformed_manifest) + "\n")
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "runtime layout mismatch" in rejected.stderr

        malformed_manifest = json.loads(runtime_manifest())
        malformed_manifest["runtime"]["digest"] = "blake3:not-a-digest"
        manifest.write_text(json.dumps(malformed_manifest) + "\n")
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "runtime digest is invalid" in rejected.stderr
        manifest.write_text(runtime_manifest())

        compatibility = home / "extensions/oracles/codex/current/runtime-compatibility.toml"
        compatibility.write_text(ready_compatibility(release_ready=False))
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "does not authorize a released Astrid 0.11.0" in rejected.stderr
        compatibility.write_text(ready_compatibility())

        receipt = home / "extensions/oracles/codex/current/Receipt.toml"
        receipt_text = receipt.read_text()
        receipt.write_text(receipt_text.replace('oracle-version = "0.3.0"', 'oracle-version = "0.2.6"'))
        rejected = resolve_active(environment)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "receipt identity does not match this plugin" in rejected.stderr
        receipt.write_text(receipt_text)

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
        assert "cwd" not in generated
        assert generated["startup_timeout_sec"] == SERVER["startup_timeout_sec"]
        assert generated["env_vars"] == SERVER["env_vars"]
        assert generated["env"] == {"AOS_BIN": str(home / "bin/aos")}
        exercise_hook_adapter(root)


if __name__ == "__main__":
    main()
