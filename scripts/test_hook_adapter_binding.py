#!/usr/bin/env python3
"""Execute installer binding logic, including refusal and warm-up paths."""
from pathlib import Path
import subprocess
import unittest

ROOT = Path(__file__).resolve().parents[1]
INSTALLER = (ROOT / "install.sh").read_text()
FUNCTION = INSTALLER.split("bind_native_hook_adapter() {", 1)[1].split("\ninstall_pack() {", 1)[0]
SOURCE = "4f77eeff-73fe-5ded-9cf1-175d822e6661"


class HookBindingTests(unittest.TestCase):
    def execute(self, mode):
        fixture = r'''
set -eu
die() { printf '%s\n' "$*" >&2; exit 1; }
binding_hash() { printf expected; }
RESOLVED_AOS_IDENTITIES=unused
grep() {
  case "$*" in
    *'aos-hook-adapter-oracle '*)
      [ "$MODE" != unreadable ] || return 2
      [ "$MODE" != legacy ] ;;
    *) command grep "$@" ;;
  esac
}
calls=0
load_capsule_record() {
  calls=$((calls + 1))
  CAPSULE_HASH=expected
  CAPSULE_SOURCE="$SOURCE"
  case "$MODE" in
    warm) [ "$calls" -gt 1 ] || CAPSULE_SOURCE=unloaded ;;
    unloaded) CAPSULE_SOURCE=unloaded ;;
    mismatch) CAPSULE_HASH=foreign ;;
    invalid) CAPSULE_SOURCE=/some/artifact/path ;;
    absent) return 1 ;;
  esac
}
sleep() { :; }
aos() { printf '%s\n' "$*" >&2; }
'''
        program = fixture + "\nbind_native_hook_adapter() {" + FUNCTION
        program += "\nbind_native_hook_adapter claude-code\n"
        return subprocess.run(["sh", "-c", program], text=True, capture_output=True,
                              env={"MODE": mode, "SOURCE": SOURCE}, timeout=5)

    def test_loaded_and_warming_bind_the_registered_source(self):
        for mode in ("loaded", "warm"):
            with self.subTest(mode=mode):
                result = self.execute(mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn("--principal default capsule config aos-mcp --agent claude-code "
                              "--set AOS_ORACLE_ADAPTER_SOURCE_ID=" + SOURCE, result.stderr)

    def test_untrusted_or_unavailable_adapter_never_writes_binding(self):
        for mode in ("unloaded", "mismatch", "invalid", "absent", "unreadable"):
            with self.subTest(mode=mode):
                result = self.execute(mode)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("capsule config", result.stderr)

    def test_binding_precedes_install_readiness(self):
        pack = INSTALLER.split("install_pack() {", 1)[1].split("\nreconcile_obsolete_bindings()", 1)[0]
        self.assertLess(pack.index('done < "$RESOLVED_AOS_IDENTITIES"'),
                        pack.index('bind_native_hook_adapter "$principal"'))
        self.assertLess(pack.index('bind_native_hook_adapter "$principal"'),
                        pack.index('oracle integration ready'))

    def test_legacy_pack_without_native_adapter_is_unchanged(self):
        result = self.execute("legacy")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("capsule config", result.stderr)


if __name__ == "__main__":
    unittest.main()
