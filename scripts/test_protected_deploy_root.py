#!/usr/bin/env python3
"""Opt-in container test: real ownership/setuid/commands; synthetic policy only.

Run in a disposable Linux container as root, with this repository read-only.
This does not prove Codewall policy, signed downloads, or a live Claude host.
"""
import copy
import hashlib
import json
import os
from pathlib import Path
import pwd
import runpy
import shutil
import subprocess
import sys
import tempfile


def main():
    if os.geteuid() != 0 or not Path("/.dockerenv").exists():
        raise SystemExit("requires a disposable root Docker container")
    repo = Path(__file__).resolve().parent.parent
    uid = pwd.getpwnam("nobody").pw_uid
    interpreter = str(Path(sys.executable).resolve())
    with tempfile.TemporaryDirectory(prefix="oracle-root-", dir="/opt") as directory:
        root = Path(directory)
        root.chmod(0o755)
        helpers = root / "helpers"
        helpers.mkdir(mode=0o755)
        for name in ("aos-protected-deploy", "aos-protected-settings", "aos-protected-hook", "aos-native-hook"):
            shutil.copyfile(repo / "plugins/common/bin" / name, helpers / name)
            (helpers / name).chmod(0o555)
        planner = runpy.run_path(str(helpers / "aos-protected-settings"))
        gate = root / "fixture-gate"
        body = (f"#!{interpreter}\nimport json,os,sys\n"
                f"assert os.getuid() == {uid}, 'readiness did not drop privilege'\n"
                "if '--check' in sys.argv: sys.exit(0)\n"
                "request=json.load(sys.stdin)\n"
                "print(json.dumps({'schema_version':1,'event':request['event'],"
                "'decision':{'skip':False},'context':None}))\n")
        gate.write_text(body)
        gate.chmod(0o555)
        binding = {"gate": str(gate), "principal": "claude-code",
                   "source": "11111111-1111-4111-8111-111111111111", "supervised_uid": uid,
                   "service_socket": "/fixture/old.sock", "service_uid": uid - 1,
                   "service_installation_id": "22222222-2222-4222-8222-222222222222"}
        settings = root / "settings.json"
        document = {"otherSetting": True, "hooks": {}}
        for event in ("PreToolUse", "UserPromptSubmit"):
            group = {"hooks": [{"type": "command", "timeout": 10,
                                "command": planner["legacy_command"](binding, event)}]}
            if event == "PreToolUse":
                group["matcher"] = "*"
            document["hooks"][event] = [group]
        settings.write_text(json.dumps(document))
        settings.chmod(0o644)
        generations = root / "generations"
        generations.mkdir(mode=0o755)
        def asset(path):
            return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        request = {"schema_version": 1, "root": str(generations), "settings": str(settings),
                   "settings_sha256": hashlib.sha256(settings.read_bytes()).hexdigest(),
                   "binding": binding, "python": interpreter,
                   "assets": {"aos-protected-hook": asset(helpers / "aos-protected-hook"),
                              "aos-native-hook": asset(helpers / "aos-native-hook"), "codewall-gate": asset(gate)}}
        def invoke(data, *args, user=None, success=True):
            options = {} if user is None else {"user": user, "group": pwd.getpwuid(user).pw_gid, "extra_groups": []}
            result = subprocess.run([interpreter, "-I", str(helpers / "aos-protected-deploy"), *args],
                input=json.dumps(data), text=True, capture_output=True, timeout=30, **options)
            if not success:
                assert result.returncode != 0, result.stdout
                return None
            assert result.returncode == 0, result.stderr
            return json.loads(result.stdout)
        original = settings.read_bytes()
        invoke(request, user=uid, success=False)
        assert settings.read_bytes() == original
        first = invoke(request)
        assert invoke(request) == first
        old = settings.read_bytes()
        successor = copy.deepcopy(request)
        successor["settings_sha256"] = hashlib.sha256(old).hexdigest()
        successor["previous_registration"] = {"binding": binding,
            "replacement": {"python": interpreter,
                "adapter": str(Path(first["generation"]) / "aos-protected-hook"),
                "evaluator": str(Path(first["generation"]) / "codewall-gate")}}
        successor["binding"].update(service_socket="/fixture/new.sock",
            service_installation_id="33333333-3333-4333-8333-333333333333",
            source="44444444-4444-4444-8444-444444444444")
        invoke(successor, success=False)
        assert settings.read_bytes() == old
        successor["custody_transition"] = "service_rotation"
        failed_gate = root / "unavailable-gate"
        failed_gate.write_text(f"#!{interpreter}\nraise SystemExit(7)\n")
        failed_gate.chmod(0o555)
        failed = copy.deepcopy(successor)
        failed["assets"]["codewall-gate"] = asset(failed_gate)
        invoke(failed, success=False)
        assert settings.read_bytes() == old
        second = invoke(successor)
        assert settings.read_bytes() != old
        assert invoke(successor) == second
        assert settings.stat().st_uid == 0 and settings.stat().st_mode & 0o777 == 0o644
        invoke(successor, "--remove")
        invoke(successor, "--remove")
        assert json.loads(settings.read_bytes()) == {"otherSetting": True,
            "hooks": {"PreToolUse": [], "UserPromptSubmit": []}}
        # TemporaryDirectory owns only this container fixture; staged dirs are immutable.
        for path in generations.iterdir():
            if path.is_dir():
                path.chmod(0o700)
    print("PASS: real root custody, non-root refusal, dropped-UID readiness, migration, rotation, retry, removal; synthetic policy")


if __name__ == "__main__":
    main()
