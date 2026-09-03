#!/usr/bin/env python3
"""aos-mcp-frame must die with the attach child, without CPython 3.14 abort."""

from __future__ import annotations

import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parent.parent
FRAME = ROOT / "plugins/unicity-aos/bin/aos-mcp-frame"


def runtime_env(executable: str, expected_path: str | None = None) -> dict[str, str]:
    blake3 = subprocess.run(
        ["b3sum", executable], check=True, stdout=subprocess.PIPE, text=True
    ).stdout.split()[0]
    sha256 = subprocess.run(
        ["shasum", "-a", "256", executable], check=True, stdout=subprocess.PIPE, text=True
    ).stdout.split()[0]
    return {
        **os.environ,
        **({"AOS_RUNTIME_PATH": expected_path} if expected_path is not None else {}),
        "AOS_RUNTIME_BLAKE3": blake3,
        "AOS_RUNTIME_SHA256": sha256,
    }


def test_child_exit_with_open_stdin() -> None:
    executable = "/bin/sh"
    proc = subprocess.Popen(
        [sys.executable, "-u", str(FRAME), executable, "-c", "printf '%s\\n' mcp-ready"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=runtime_env(executable, expected_path=executable),
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
    executable = str(Path(sys.executable).resolve())
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
        env=runtime_env(executable, expected_path=executable),
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
        proc = subprocess.Popen(
            [sys.executable, "-u", str(FRAME), executable, "-c", "exit 0"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=runtime_env(executable, expected_path="/definitely/not/release/astrid"),
        )
        if proc.stdin:
            proc.stdin.close()
        stdout, stderr = proc.communicate(timeout=2)
        assert proc.returncode != 0
        assert b"canonical release executable" in stderr
        assert stdout == b""


def main() -> None:
    assert FRAME.is_file()
    test_child_exit_with_open_stdin()
    test_content_length_roundtrip()
    test_matching_bytes_outside_release_path_rejected()


if __name__ == "__main__":
    main()
