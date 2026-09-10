"""Run real preflight functions against advisory and transport diagnostics."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parent.parent
NOTICE = "! Update available: v2026.9.1 → v2026.9.0. Run `astrid update` to upgrade."


class PreflightNotices(unittest.TestCase):
    def test_launcher_and_doctor(self):
        for host in ("unicity-aos", "claude", "grok"):
            principal = "codex-code" if host == "unicity-aos" else host + "-code"
            absent = f"✗ capsule 'aos-mcp' is not installed for agent '{principal}'"
            for name in ("aos-up", "aos-doctor"):
                source = (ROOT / "plugins" / host / "bin" / name).read_text()
                start = source.index("capsule_ready() {")
                function = source[start:source.index("\n}", start) + 2]
                for diagnostic, status, expected in (
                    (absent, 1, 1),
                    (NOTICE + "\n" + absent, 1, 1),
                    (NOTICE + "\n" + absent + "\ntransport failure", 1, 93),
                    (NOTICE, 1, 93),
                    (NOTICE + "\n" + absent, 2, 93),
                ):
                    with self.subTest(host=host, name=name, diagnostic=diagnostic, status=status):
                        script = function + """
AOS=fake_aos
AOS_HOME=/nonexistent-preflight-notice-test
AOS_HOST=$1
PRINCIPAL=$2
fake_aos() { printf '%s\\n' "$3" >&2; return "$4"; }
"""  # Bind fixture values independently of the function's command arguments.
                        script = script.replace(
                            'fake_aos() { printf \'%s\\n\' "$3" >&2; return "$4"; }',
                            'diagnostic=$3; status=$4\nfake_aos() { printf \'%s\\n\' "$diagnostic" >&2; return "$status"; }',
                        )
                        result = subprocess.run(
                            ["sh", "-c", script + "\ncapsule_ready", "test", host,
                             principal, diagnostic, str(status)],
                            capture_output=True, text=True,
                        )
                        self.assertEqual(result.returncode, expected, result.stderr)


if __name__ == "__main__":
    unittest.main()
