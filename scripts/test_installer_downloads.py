"""Exercise installer download helpers without touching an installed runtime."""
import hashlib
import http.server
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = b"#!/bin/sh\nexit 0\n"
DIGEST = hashlib.sha256(FIXTURE).hexdigest()


class Downloads(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.work = self.root / "work"
        self.work.mkdir()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.fixture = self.root / "fixture"
        self.fixture.write_bytes(FIXTURE)
        self.log = self.root / "curl.log"
        (self.bin / "curl").write_text(
            '#!/bin/sh\nprintf "%s\\n" "$*" >> "$TEST_LOG"\n'
            'out=\nwhile [ "$#" -gt 0 ]; do\n'
            ' if [ "$1" = -o ]; then shift; out=$1; fi\nshift\ndone\n'
            'if [ "${TEST_FAIL:-0}" = 1 ]; then printf partial > "$out"; exit 28; fi\n'
            'cp "$TEST_FIXTURE" "$out"\n'
        )
        (self.bin / "curl").chmod(0o700)
        source = (ROOT / "install.sh").read_text()
        # Use real checksum validation with a tiny pinned test verifier.
        start = source.find("download_file() {")
        if start < 0:
            start = source.index("ensure_cosign() {")
        helpers = source[start:source.index("verify_release_asset() {")]
        import re
        helpers = re.sub(r"digest=[0-9a-f]{64}", "digest=" + DIGEST, helpers)
        self.script = self.root / "helpers.sh"
        self.script.write_text(
            'set -eu\numask 077\n'
            'die() { echo "$*" >&2; exit 1; }\n'
            'have() { command -v "$1" >/dev/null 2>&1; }\n'
            'platform() { echo darwin-arm64; }\n'
            'sha256_file() { shasum -a 256 "$1" | awk \'{print $1}\'; }\n'
            'COSIGN_VERSION=v3.1.1\n' + helpers + '\n' + 'eval "$TEST_CALL"\n'
        )
        self.env = dict(os.environ, PATH=str(self.bin) + ":/usr/bin:/bin",
                        WORK=str(self.work), AOS_HOME_DIR=str(self.root / "aos"),
                        XDG_CACHE_HOME=str(self.root / "cache"),
                        TEST_LOG=str(self.log), TEST_FIXTURE=str(self.fixture))

    def run_helper(self, call="ensure_cosign", **env):
        return subprocess.run(["sh", str(self.script)],
                              env=dict(self.env, TEST_CALL=call, **env),
                              capture_output=True, text=True)

    @property
    def cache(self):
        return self.root / "cache/aos-oracles/cosign/v3.1.1-darwin-arm64"

    def test_verified_cache_reused_as_private_snapshot(self):
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.cache.read_bytes(), FIXTURE)
        self.assertFalse((self.root / "aos").exists())
        self.log.unlink()
        result = self.run_helper('ensure_cosign; test "$COSIGN" = "$WORK/cosign"')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.log.exists())

    def test_corrupt_cache_redownloaded(self):
        self.cache.parent.mkdir(parents=True)
        self.cache.write_bytes(b"untrusted")
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.cache.read_bytes(), FIXTURE)
        self.assertTrue(self.log.exists())

    def test_partial_download_never_cached(self):
        result = self.run_helper(TEST_FAIL="1")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.cache.exists())
        self.assertFalse((self.work / "cosign").exists())

    def test_wrong_digest_never_cached(self):
        self.fixture.write_bytes(b"tampered")
        result = self.run_helper()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.cache.exists())

    def test_symlink_cache_directory_rejected(self):
        outside = self.root / "outside"
        outside.mkdir()
        self.cache.parent.parent.mkdir(parents=True)
        self.cache.parent.symlink_to(outside, target_is_directory=True)
        result = self.run_helper()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(list(outside.iterdir()))

    def test_symlink_cache_entry_rejected(self):
        self.cache.parent.mkdir(parents=True)
        self.cache.symlink_to(self.fixture)
        result = self.run_helper()
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.fixture.read_bytes(), FIXTURE)

    def test_directory_cache_entry_rejected(self):
        self.cache.mkdir(parents=True)
        result = self.run_helper()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(list(self.cache.iterdir()))

    def test_invalid_timeout_rejected_before_download(self):
        for value in ("0", "-1", "abc", "999999", "1.5"):
            with self.subTest(value=value):
                result = self.run_helper(AOS_ORACLE_DOWNLOAD_TIMEOUT=value)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.log.exists())

    def test_configurable_finite_curl_options(self):
        result = self.run_helper(AOS_ORACLE_DOWNLOAD_TIMEOUT="900")
        self.assertEqual(result.returncode, 0, result.stderr)
        args = self.log.read_text()
        self.assertIn("--max-time 900", args)
        self.assertIn("--connect-timeout 15", args)
        self.assertIn("--retry 2", args)
        self.assertIn("--retry-max-time 1800", args)

    def test_real_curl_retries_transient_http_failure(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            calls = 0

            def do_GET(self):
                Handler.calls += 1
                self.send_response(503 if Handler.calls == 1 else 200)
                self.end_headers()
                self.wfile.write(FIXTURE)

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            self.env["PATH"] = "/usr/bin:/bin"
            call = f'download_file http://127.0.0.1:{server.server_port}/asset "$WORK/asset"'
            result = self.run_helper(call)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((self.work / "asset").read_bytes(), FIXTURE)
            self.assertEqual(Handler.calls, 2)
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_real_curl_timeout_is_bounded_and_discards_partial_file(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            calls = 0

            def do_GET(self):
                Handler.calls += 1
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"partial")
                self.wfile.flush()
                time.sleep(1.5)

            def log_message(self, *args):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            self.env["PATH"] = "/usr/bin:/bin"
            call = f'download_file http://127.0.0.1:{server.server_port}/asset "$WORK/asset"'
            result = self.run_helper(call, AOS_ORACLE_DOWNLOAD_TIMEOUT="1",
                                     AOS_ORACLE_DOWNLOAD_RETRIES="0")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(Handler.calls, 1)
            self.assertFalse((self.work / "asset").exists())
        finally:
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == "__main__":
    unittest.main()
