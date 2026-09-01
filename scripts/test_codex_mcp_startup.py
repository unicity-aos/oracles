#!/usr/bin/env python3
"""Exercise Codex bootstrap and exact active-release MCP attachment."""

from __future__ import annotations

import json
from pathlib import Path
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


def plant_fake_runtime(home: Path, installer: Path) -> None:
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
        'cat > "$AOS_HOME/runtime/bin/astrid" <<\'LEGACY\'\n'
        "#!/bin/sh\n"
        ': > "$TEST_LEGACY_RUNTIME_USED"\n'
        "exit 95\n"
        "LEGACY\n"
        'chmod 700 "$AOS_HOME/bin/aos" "$release/runtime/bin/astrid" "$AOS_HOME/runtime/bin/astrid"\n'
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
        root = Path(raw)
        home = root / "home" / ".aos"
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
            "PATH": python_path(),
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
        assert all(Path(line) == runtime_home for line in astrid_cwd_log.read_text().splitlines())

        second = launch(environment, host_workspace)
        assert second.returncode == 0, (second.returncode, second.stdout, second.stderr)
        assert install_log.read_text().splitlines() == [
            "--host codex --skip-host-plugin --yes --oracle-version 0.3.0"
        ], "ready startup unexpectedly re-entered provisioning"

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

        resolved = resolve_active(environment)
        expected_runtime = home / "releases/2026.9.0/runtime/bin/astrid"
        assert resolved.returncode == 0, (resolved.stdout, resolved.stderr)
        assert resolved.stdout.strip() == str(expected_runtime)

        wrong_runtime = dict(environment)
        wrong_runtime["TEST_RUNTIME_VERSION"] = "0.10.4"
        rejected = resolve_active(wrong_runtime)
        assert rejected.returncode != 0, (rejected.stdout, rejected.stderr)
        assert "expected 0.11.0" in rejected.stderr

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
