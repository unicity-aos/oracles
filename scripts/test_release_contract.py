#!/usr/bin/env python3
"""Guard the release workflow's draft-to-published transaction."""

from __future__ import annotations

import os
import pathlib
import subprocess
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
REHEARSAL_WORKFLOW = ROOT / ".github" / "workflows" / "rehearsal.yml"
ASSET_SCRIPT = ROOT / "scripts" / "oracle-release-assets.sh"

PRIMARY_ASSETS = (
    "claude-pack.toml",
    "codex-pack.toml",
    "grok-pack.toml",
    "aos-oracle-plugins.tar.gz",
    "BLAKE3SUMS.txt",
    "runtime-compatibility.toml",
)


class ReleaseWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text()
        cls.rehearsal = REHEARSAL_WORKFLOW.read_text()
        cls.asset_script = ASSET_SCRIPT.read_text()

    def test_release_stays_draft_until_remote_inventory_matches(self) -> None:
        create = self.workflow.index('gh release create "$GITHUB_REF_NAME"')
        draft = self.workflow.index("--draft", create)
        upload = self.workflow.index('gh release upload "$GITHUB_REF_NAME"', draft)
        remote_inventory = self.workflow.index("'.assets[].name'", upload)
        compare = self.workflow.index(
            "diff -u expected-assets.txt remote-assets.txt", remote_inventory
        )
        publish = self.workflow.index(
            'gh release edit "$GITHUB_REF_NAME" --draft=false', compare
        )
        self.assertLess(create, draft)
        self.assertLess(draft, upload)
        self.assertLess(upload, remote_inventory)
        self.assertLess(remote_inventory, compare)
        self.assertLess(compare, publish)
        self.assertNotIn(
            'gh release create "$GITHUB_REF_NAME" artifacts/*', self.workflow
        )

    def test_draft_reuse_is_bound_to_tag_and_source_commit(self) -> None:
        self.assertIn("--json isDraft --jq .isDraft", self.workflow)
        self.assertIn("--json tagName --jq .tagName", self.workflow)
        self.assertGreaterEqual(
            self.workflow.count("<!-- aos-oracles-source:${GITHUB_SHA} -->"), 2
        )
        self.assertIn("existing draft was created from another source commit", self.workflow)
        self.assertIn("REUSE_RELEASE_DRAFT=true", self.workflow)
        self.assertIn("REUSE_RELEASE_DRAFT=false", self.workflow)

    def test_publication_is_manual_and_release_ready_gated(self) -> None:
        self.assertIn("on:\n  workflow_dispatch:", self.workflow)
        ready = self.workflow.index('runtime["release-ready"] is not True')
        publish = self.workflow.index(
            'gh release edit "$GITHUB_REF_NAME" --draft=false'
        )
        self.assertLess(ready, publish)

    def test_published_release_must_be_platform_immutable(self) -> None:
        publish = self.workflow.index(
            'gh release edit "$GITHUB_REF_NAME" --draft=false'
        )
        immutable = self.workflow.index("--json isImmutable", publish)
        refusal = self.workflow.index(
            "published release is not immutable", immutable
        )
        self.assertLess(publish, immutable)
        self.assertLess(immutable, refusal)

    def test_release_contains_host_adapters_but_no_aos_owned_capsules(self) -> None:
        self.assertNotIn("aos-mcp.wasm", self.workflow)
        self.assertNotIn("aos-mcp.capsule", self.workflow)
        self.assertNotIn("aos-mcp.wasm", self.asset_script)
        self.assertNotIn("aos-mcp.capsule", self.asset_script)
        build = self.workflow.index("Build host adapter pack manifests")
        sign = self.workflow.index("oracle-release-assets.sh sign", build)
        self.assertLess(build, sign)

    def test_release_and_rehearsal_share_the_asset_implementation(self) -> None:
        for workflow in (self.workflow, self.rehearsal):
            self.assertIn("oracle-release-assets.sh build artifacts", workflow)
            self.assertIn("oracle-release-assets.sh sign artifacts", workflow)
            self.assertIn("oracle-release-assets.sh inventory artifacts", workflow)
        self.assertIn("cosign sign-blob", self.asset_script)
        self.assertIn("cosign verify-blob", self.asset_script)
        for asset in PRIMARY_ASSETS:
            self.assertIn(asset, self.asset_script)

    def test_rehearsal_is_main_only_exact_source_and_non_publishing(self) -> None:
        self.assertIn("on:\n  workflow_dispatch:", self.rehearsal)
        self.assertIn("source_commit:", self.rehearsal)
        self.assertIn("github.event.repository.default_branch", self.rehearsal)
        self.assertIn('test "$DEFAULT_BRANCH" = main', self.rehearsal)
        self.assertIn("test \"$GITHUB_REF\" = \"refs/heads/$DEFAULT_BRANCH\"", self.rehearsal)
        self.assertIn("grep -Eq '^[0-9a-f]{40}$'", self.rehearsal)
        self.assertIn('git merge-base --is-ancestor "$SOURCE_COMMIT"', self.rehearsal)
        self.assertIn("ref: ${{ inputs.source_commit }}", self.rehearsal)
        self.assertIn("id-token: write", self.rehearsal)
        self.assertIn(
            "https://github.com/unicity-aos/oracles/.github/workflows/rehearsal.yml@refs/heads/main",
            self.rehearsal,
        )
        self.assertNotIn("contents: write", self.rehearsal)
        self.assertNotIn("environment: release", self.rehearsal)
        self.assertNotIn("gh release", self.rehearsal)
        self.assertNotIn("git tag", self.rehearsal)
        self.assertIn("actions/upload-artifact@043fb46", self.rehearsal)

    def test_inventory_contract_executes_and_rejects_extras(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            artifacts = root / "artifacts"
            artifacts.mkdir()
            for asset in PRIMARY_ASSETS:
                (artifacts / asset).touch()
                (artifacts / f"{asset}.sigstore.json").touch()
            expected = root / "expected-assets.txt"
            subprocess.run(
                [str(ASSET_SCRIPT), "inventory", str(artifacts), str(expected)],
                cwd=ROOT,
                check=True,
            )
            self.assertEqual(len(expected.read_text().splitlines()), 12)

            (artifacts / "unexpected.txt").touch()
            rejected = subprocess.run(
                [str(ASSET_SCRIPT), "inventory", str(artifacts), str(expected)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("unexpected.txt", rejected.stdout)

            (artifacts / "unexpected.txt").unlink()
            (artifacts / "unexpected-directory").mkdir()
            rejected = subprocess.run(
                [str(ASSET_SCRIPT), "inventory", str(artifacts), str(expected)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("unexpected-directory", rejected.stdout)

            (artifacts / "unexpected-directory").rmdir()
            expected_asset = artifacts / PRIMARY_ASSETS[0]
            expected_asset.unlink()
            symlink_target = root / "symlink-target"
            symlink_target.touch()
            expected_asset.symlink_to(symlink_target)
            rejected = subprocess.run(
                [str(ASSET_SCRIPT), "inventory", str(artifacts), str(expected)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("regular non-symlink file", rejected.stderr)

    def test_asset_build_fails_if_blake3_generation_fails(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            fake_bin = root / "bin"
            fake_bin.mkdir()
            fake_b3sum = fake_bin / "b3sum"
            fake_b3sum.write_text('#!/bin/sh\n: > "$B3SUM_CALLED"\nexit 7\n')
            fake_b3sum.chmod(0o755)
            fake_tar = fake_bin / "tar"
            fake_tar.write_text("#!/bin/sh\nexit 0\n")
            fake_tar.chmod(0o755)
            artifacts = root / "artifacts"
            b3sum_called = root / "b3sum-called"
            env = os.environ.copy()
            env["PATH"] = f"{fake_bin}:{env['PATH']}"
            env["B3SUM_CALLED"] = str(b3sum_called)

            rejected = subprocess.run(
                [str(ASSET_SCRIPT), "build", str(artifacts)],
                cwd=ROOT,
                env=env,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertTrue(b3sum_called.is_file())
            self.assertFalse((artifacts / "BLAKE3SUMS.txt").exists())


if __name__ == "__main__":
    unittest.main()
