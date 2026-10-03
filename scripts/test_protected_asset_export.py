#!/usr/bin/env python3
"""Exercise the installer's exact export function; signature verification is separate."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
INSTALLER = ROOT / "install.sh"
TEXT = INSTALLER.read_text()
FUNCTION = TEXT[TEXT.index("export_protected_assets() {"):TEXT.index("prepare_plugin_snapshot() {")]
NAMES = ("aos-protected-hook", "aos-native-hook", "aos-protected-settings", "aos-protected-deploy")


class ProtectedAssetExport(unittest.TestCase):
    def run_export(self, root, bad=None):
        with tarfile.open(root / "aos-oracle-plugins.tar.gz", "w:gz") as archive:
            for name in NAMES:
                if bad == "missing" and name == NAMES[0]:
                    continue
                member = tarfile.TarInfo("plugins/claude/bin/" + name)
                data = (ROOT / "plugins/common/bin" / name).read_bytes()
                member.size = len(data)
                if bad == "symlink" and name == NAMES[0]:
                    member.type = tarfile.SYMTYPE
                    member.linkname = "/not-an-asset"
                archive.addfile(member, io.BytesIO(data))
                if bad == "duplicate" and name == NAMES[0]:
                    archive.addfile(member, io.BytesIO(data))
        script = ('set -eu\nRELEASE_STAGE=$1\nPROTECTED_ASSETS=$2\n'
                  'ORACLES_REPO=unicity-aos/oracles\nORACLES_VERSION=2026.9.3\n'
                  + FUNCTION + '\nexport_protected_assets\n')
        return subprocess.run(["sh", "-c", script, "export-test", str(root), str(root / "export")],
                              capture_output=True, text=True)

    def test_exports_exact_helpers_and_binds_every_byte(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            result = self.run_export(root)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads((root / "export/manifest.json").read_text())
            self.assertEqual(manifest["source"], "signed-release")
            self.assertEqual(set(manifest["assets"]), set(NAMES))
            for name in NAMES:
                data = (root / "export" / name).read_bytes()
                self.assertEqual(data, (ROOT / "plugins/common/bin" / name).read_bytes())
                self.assertEqual(manifest["assets"][name], hashlib.sha256(data).hexdigest())
            self.assertNotEqual(self.run_export(root).returncode, 0)
            self.assertEqual(json.loads((root / "export/manifest.json").read_text()), manifest)

    def test_missing_duplicate_and_linked_members_do_not_export(self):
        for bad in ("missing", "duplicate", "symlink"):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                self.assertNotEqual(self.run_export(root, bad).returncode, 0)
                self.assertFalse((root / "export").exists())

    def test_local_assets_cannot_claim_signed_protected_export(self):
        with tempfile.TemporaryDirectory() as directory:
            result = subprocess.run(["sh", str(INSTALLER), "--local-assets", directory,
                                     "--protected-assets", str(Path(directory) / "export")],
                                    capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("protected deployment assets require a signed release", result.stderr)
            self.assertFalse((Path(directory) / "export").exists())


if __name__ == "__main__":
    unittest.main()
