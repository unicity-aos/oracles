"""Exercise the installer identity helper, without an installed runtime."""
from pathlib import Path
import subprocess
import unittest

SOURCE = (Path(__file__).resolve().parents[1] / "install.sh").read_text()
HELPERS = SOURCE[SOURCE.index("blake3_file() {"):SOURCE.index("acquire_install_lock() {")]
DIGEST = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"


class BundledChecksums(unittest.TestCase):
    def run_helper(self, output=DIGEST, code=0):
        script = (
            'set -eu\nB3SUM=\n'
            'die() { printf "%s\\n" "$*" >&2; exit 1; }\n'
            f'aos() {{ test "$1" = checksum; test "$2" = "input with spaces"; '
            f'printf "%s\\n" "{output}"; return {code}; }}\n'
            + HELPERS + '\nblake3_file "input with spaces"\n'
        )
        return subprocess.run(["sh", "-c", script], capture_output=True, text=True)

    def test_missing_b3sum_uses_bundled_checksum(self):
        result = self.run_helper()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), DIGEST)

    def test_bundled_failure_is_not_success(self):
        result = self.run_helper(code=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(result.stdout, "")

    def test_malformed_digest_is_rejected(self):
        for value in ("", "oops", "0" * 63, "0" * 65, DIGEST + " extra", DIGEST + "\n" + DIGEST):
            with self.subTest(value=value):
                result = self.run_helper(output=value)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")


if __name__ == "__main__":
    unittest.main()
