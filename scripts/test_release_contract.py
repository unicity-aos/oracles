#!/usr/bin/env python3
"""Guard the release workflow's draft-to-published transaction."""

from __future__ import annotations

import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"


class ReleaseWorkflowContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.workflow = WORKFLOW.read_text()

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

    def test_signed_pack_records_the_built_wasm_identity(self) -> None:
        build = self.workflow.index("built_hash=$(b3sum artifacts/aos-mcp.wasm")
        inject = self.workflow.index("wasm-blake3", build)
        sign = self.workflow.index("cosign sign-blob", inject)
        self.assertLess(build, inject)
        self.assertLess(inject, sign)


if __name__ == "__main__":
    unittest.main()
