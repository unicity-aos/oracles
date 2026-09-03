#!/usr/bin/env python3
"""aos-mcp-frame canonical launch checks and attach-process lifetime."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parent.parent
FRAME = ROOT / "plugins/unicity-aos/bin/aos-mcp-frame"


def runtime_env(executable: str) -> dict[str, str]:
    blake3 = subprocess.run(
        ["b3sum", executable], check=True, stdout=subprocess.PIPE, text=True
    ).stdout.split()[0]
    sha256 = subprocess.run(
        ["shasum", "-a", "256", executable], check=True, stdout=subprocess.PIPE, text=True
    ).stdout.split()[0]
    return {
        **os.environ,
        "AOS_HOME": str(Path(executable).parents[4]),
        # A caller-provided path is not launch authority, even when it names
        # the same bytes that pass both digest checks.
        "AOS_RUNTIME_PATH": "/definitely/not/release/astrid",
        "AOS_RUNTIME_BLAKE3": blake3,
        "AOS_RUNTIME_SHA256": sha256,
    }


def canonical_release(root: Path, source: str = "/bin/sh") -> str:
    root = root.resolve()
    executable = root / "releases/2026.9.0/runtime/bin/astrid"
    executable.parent.mkdir(parents=True)
    shutil.copyfile(source, executable)
    executable.chmod(0o700)
    return str(executable)


def assert_rejected(
    executable: str,
    environment: dict[str, str],
    expected_error: bytes,
) -> None:
    proc = subprocess.Popen(
        [sys.executable, "-u", str(FRAME), executable, "-c", "exit 0"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
    )
    if proc.stdin:
        proc.stdin.close()
    stdout, stderr = proc.communicate(timeout=2)
    assert proc.returncode != 0
    assert expected_error in stderr
    assert stdout == b""


def test_child_exit_with_open_stdin() -> None:
    with tempfile.TemporaryDirectory() as raw:
        executable = canonical_release(Path(raw))
        proc = subprocess.Popen(
            [sys.executable, "-u", str(FRAME), executable, "-c", "printf '%s\\n' ready"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=runtime_env(executable),
        )
        try:
            deadline = time.time() + 2
            while proc.poll() is None and time.time() < deadline:
                time.sleep(0.02)
            assert proc.poll() == 0, (
                proc.poll(),
                proc.stdout.read() if proc.stdout else b"",
                proc.stderr.read() if proc.stderr else b"",
            )
        finally:
            if proc.poll() is None:
                proc.send_signal(signal.SIGTERM)
                proc.wait(timeout=1)


def test_content_length_roundtrip() -> None:
    with tempfile.TemporaryDirectory() as raw:
        executable = canonical_release(Path(raw), str(Path(sys.executable).resolve()))
        child = (
            "import sys\n"
            "line = sys.stdin.readline()\n"
            "sys.stdout.write(line if line.endswith('\\n') else line + '\\n')\n"
            "sys.stdout.flush()\n"
        )
        proc = subprocess.Popen(
            [sys.executable, "-u", str(FRAME), executable, "-u", "-c", child],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=runtime_env(executable),
        )
        payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "ping"}).encode()
        assert proc.stdin is not None and proc.stdout is not None
        proc.stdin.write(f"Content-Length: {len(payload)}\r\n\r\n".encode() + payload)
        proc.stdin.flush()
        header = b""
        while b"\r\n\r\n" not in header:
            chunk = proc.stdout.read(1)
            assert chunk, header
            header += chunk
        head, extra = header.split(b"\r\n\r\n", 1)
        length = None
        for line in head.split(b"\n"):
            line = line.strip().strip(b"\r")
            if line.lower().startswith(b"content-length:"):
                length = int(line.split(b":", 1)[1].strip())
        assert length is not None, head
        body = extra
        if len(body) < length:
            more = proc.stdout.read(length - len(body))
            assert more is not None
            body += more
        assert json.loads(body[:length])["method"] == "ping"
        proc.stdin.close()
        assert proc.wait(timeout=2) == 0


def test_matching_bytes_outside_release_path_rejected() -> None:
    with tempfile.TemporaryDirectory() as temp_dir:
        executable = str(Path(temp_dir) / "astrid")
        Path(executable).write_text("#!/bin/sh\nexit 0\n")
        Path(executable).chmod(0o700)
        (Path(temp_dir) / "empty-aos-home/releases").mkdir(parents=True)
        environment = runtime_env(executable)
        environment["AOS_HOME"] = str(Path(temp_dir) / "empty-aos-home")
        environment["AOS_RUNTIME_PATH"] = executable
        assert_rejected(executable, environment, b"runtime is not the canonical release executable")


def test_symlinked_release_ancestor_rejected() -> None:
    with tempfile.TemporaryDirectory() as temp_dir:
        root = Path(temp_dir)
        external_bin = root / "external-bin"
        external_bin.mkdir()
        external_executable = external_bin / "astrid"
        external_executable.write_text("#!/bin/sh\nexit 0\n")
        external_executable.chmod(0o700)
        release_root = root / "aos/releases/2026.9.0/runtime"
        release_root.mkdir(parents=True)
        (release_root / "bin").symlink_to(external_bin, target_is_directory=True)
        executable = str(release_root / "bin/astrid")
        environment = runtime_env(executable)
        environment["AOS_HOME"] = str(root / "aos")
        environment["AOS_RUNTIME_PATH"] = executable
        assert_rejected(executable, environment, b"release path is not canonical")


def main() -> None:
    assert FRAME.is_file()
    test_child_exit_with_open_stdin()
    test_content_length_roundtrip()
    test_matching_bytes_outside_release_path_rejected()
    test_symlinked_release_ancestor_rejected()


if __name__ == "__main__":
    main()
