#!/usr/bin/env python3
"""Execute verifier bootstrap with a modeled slow connection and digest checks."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
DIGEST = "ae1ecd212663f3693ad9edf8b1a183900c9a52d3155ba6e354237f9a0f6463fc"


class VerifierDownloadTest(unittest.TestCase):
    def run_bootstrap(self, mode="slow", initial=None, cache=None):
        source = (ROOT / "install.sh").read_text()
        function = source[source.index("ensure_cosign() {"):source.index("verify_release_asset() {")]
        with tempfile.TemporaryDirectory(prefix="oracle-verifier-") as raw:
            root = Path(raw)
            home = root / "aos"
            cached = home / "cache/verifiers" / DIGEST
            if cache is not None:
                cached.parent.mkdir(parents=True)
                if cache == "symlink":
                    cached.symlink_to(root / "foreign")
                else:
                    cached.write_text(cache)
            if initial is not None:
                (root / "cosign").write_text(initial)
            script = r'''
set -eu
COSIGN_VERSION=v3.1.1
CHECK_ONLY=0
have() { [ "$1" = curl ]; }
say() { printf '%s\n' "$*"; }
die() { printf '%s\n' "$*" >&2; exit 1; }
platform() { printf 'linux-amd64\n'; }
sleep() { /bin/sleep 0.02; }
sha256_file() {
  if [ "$(cat "$1")" = verified ]; then printf '%s\n' "$TEST_DIGEST";
  else printf 'wrong-digest\n'; fi
}
curl() {
  printf 'download\n' >> "$WORK/downloads"
  total=0; connect=0; speed=0; interval=0; output=
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --max-time) shift; total=$1 ;;
      --connect-timeout) shift; connect=$1 ;;
      --speed-limit) shift; speed=$1 ;;
      --speed-time) shift; interval=$1 ;;
      -o) shift; output=$1 ;;
    esac
    shift
  done
  # At the reporter's measured rate this transfer takes roughly 449 seconds.
  [ "$total" -ge 449 ] || return 28
  [ "$connect" -gt 0 ] && [ "$connect" -le 30 ] || return 90
  [ "$speed" -gt 0 ] && [ "$interval" -gt 0 ] || return 91
  [ "$TEST_MODE" != stalled ] || return 28
  if [ "$TEST_MODE" = delayed ]; then /bin/sleep 0.2; fi
  if [ "$TEST_MODE" = corrupt ]; then printf corrupt > "$output";
  else printf verified > "$output"; fi
}
'''
            result = subprocess.run(
                ["sh", "-c", script + function + "\nensure_cosign\nensure_cosign\n"],
                env={**os.environ, "WORK": raw, "AOS_HOME_DIR": str(home),
                     "TEST_MODE": mode, "TEST_DIGEST": DIGEST},
                text=True, capture_output=True, timeout=5, check=False,
            )
            downloads = root / "downloads"
            count = len(downloads.read_text().splitlines()) if downloads.exists() else 0
            return result, count

    def test_slow_healthy_download_and_same_transaction_reuse(self):
        result, count = self.run_bootstrap()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(count, 1)

    def test_progress_before_first_byte_has_no_shell_errors(self):
        result, count = self.run_bootstrap("delayed")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(count, 1)
        self.assertIn("no progress reported", result.stderr)
        self.assertNotIn("No such file", result.stderr)
        self.assertNotIn("integer expression", result.stderr)

    def test_stalled_download_refused(self):
        result, _ = self.run_bootstrap("stalled")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("could not download the Sigstore verifier", result.stderr)

    def test_corrupt_download_refused(self):
        result, _ = self.run_bootstrap("corrupt")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)

    def test_corrupt_existing_verifier_is_not_reused(self):
        result, count = self.run_bootstrap(initial="corrupt")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(count, 1)

    def test_checksum_verified_shared_cache_avoids_download(self):
        result, count = self.run_bootstrap(cache="verified")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(count, 0)

    def test_corrupt_cache_requires_verified_download(self):
        result, count = self.run_bootstrap(cache="corrupt")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(count, 1)

    def test_symlinked_cache_refused_before_download(self):
        result, count = self.run_bootstrap(cache="symlink")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(count, 0)


if __name__ == "__main__":
    unittest.main()
