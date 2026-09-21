#!/usr/bin/env python3
"""Guard the release workflow's draft-to-published transaction."""

from __future__ import annotations

import os
import pathlib
import shutil
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

    def test_release_uses_curated_notes_and_preserves_source_marker(self) -> None:
        self.assertNotIn("--generate-notes", self.workflow)
        self.assertIn('test -s "$notes"', self.workflow)
        self.assertIn('--notes-file "$RUNNER_TEMP/oracle-release-notes.md"', self.workflow)
        self.assertIn('printf \'%s\\n\\n\' "$marker"', self.workflow)
        version = (ROOT / "release/oracle-version").read_text().strip()
        notes = (ROOT / "release/notes" / f"{version}.md").read_text()
        self.assertIn("Changes since the published 2026.9.1", notes)

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
        minimum = self.workflow.index(
            'runtime["version-requirement"] != f">={runtime[\'version\']}"'
        )
        publish = self.workflow.index(
            'gh release edit "$GITHUB_REF_NAME" --draft=false'
        )
        self.assertLess(ready, publish)
        self.assertLess(minimum, publish)
        self.assertIn(
            'runtime["tag"] != f"v{runtime[\'version\']}"',
            self.workflow,
        )
        self.assertIn(
            "runtime compatibility identity does not match its Astrid floor",
            self.workflow,
        )

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


class LatestInstallerTests(unittest.TestCase):
    def test_host_install_entrypoints_default_to_latest(self) -> None:
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            installer = root / "installer.sh"
            installer.write_text('#!/bin/sh\nprintf "%s\\n" "$*"\n')
            for host in ("claude", "grok", "unicity-aos"):
                plugin = ROOT / "plugins" / host
                for override in ((), ("--oracle-version", "2026.9.1")):
                    env = dict(os.environ, AOS_PLUGIN_ROOT=str(plugin),
                               AOS_ORACLES_INSTALLER=str(installer))
                    env.pop("AOS_ORACLES_VERSION", None)
                    result = subprocess.run(
                        [str(plugin / "bin/aos-install"), "--host", "codex", *override],
                        env=env, capture_output=True, text=True, check=True)
                    expected = override[-1] if override else "latest"
                    self.assertIn(f"--oracle-version {expected}", result.stdout)
                source = (plugin / "bin/aos-install").read_text()
                self.assertIn("https://aos.unicity.ai/oracle-install.sh", source)

    def probe(self, url: str, extra: tuple[str, ...] = (), fail: bool = False,
              repository: str = "unicity-aos/oracles") -> tuple[str, str]:
        with tempfile.TemporaryDirectory() as raw:
            root = pathlib.Path(raw)
            curl = root / "curl"
            curl.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$PROBE_LOG"\n'
                            'test "$PROBE_FAIL" = 0 || exit 22\nprintf "%s" "$PROBE_URL"\n')
            curl.chmod(0o700)
            env = dict(os.environ, PATH=str(root) + os.pathsep + os.environ["PATH"],
                       AOS_HOME=str(root / "home"), PROBE_LOG=str(root / "calls"),
                       PROBE_URL=url, PROBE_FAIL=str(int(fail)))
            for name in ("AOS_ORACLE_ASSETS", "AOS_ORACLES_VERSION", "AOS_ORACLES_REPO"):
                env.pop(name, None)
            env["AOS_ORACLES_REPO"] = repository
            # Stop after resolution/argument validation, before any installation.
            result = subprocess.run(["sh", str(ROOT / "install.sh"), *extra,
                                     "--aos-channel", "invalid-probe-channel"],
                                    env=env, capture_output=True, text=True, timeout=5)
            calls = (root / "calls").read_text() if (root / "calls").exists() else ""
            self.assertFalse((root / "home").exists())
            self.assertNotEqual(result.returncode, 0)
            return result.stderr, calls

    def test_default_resolves_new_releases_without_a_source_bump(self) -> None:
        for version in ("2026.9.1", "2026.9.9"):
            error, calls = self.probe("https://github.com/unicity-aos/oracles/releases/tag/v" + version)
            self.assertIn("invalid AOS channel", error)
            self.assertIn("https://github.com/unicity-aos/oracles/releases/latest", calls)
            self.assertIn("%{url_effective}", calls)

    def test_exact_override_does_not_resolve_latest(self) -> None:
        error, calls = self.probe("", ("--oracle-version", "2026.9.1"))
        self.assertIn("invalid AOS channel", error)
        self.assertEqual(calls, "")

    def test_failed_or_unexpected_resolution_never_falls_back_to_a_baked_version(self) -> None:
        for url, fail in (("", True), ("https://evil.example/releases/tag/v2026.9.9", False),
                          ("https://github.com/unicity-aos/oracles/releases/tag/v2026.9.9-rc1", False)):
            error, _ = self.probe(url, fail=fail)
            self.assertNotIn("invalid AOS channel", error)

    def test_local_assets_require_an_explicit_version_without_network(self) -> None:
        error, calls = self.probe("", ("--local-assets", "/nonexistent-fixture"))
        self.assertIn("explicit --oracle-version", error)
        self.assertEqual(calls, "")

    def test_invalid_repository_is_rejected_before_network(self) -> None:
        for repository in ("owner/*", "owner/repo?x", "owner/repo/extra", "../repo"):
            error, calls = self.probe("", repository=repository)
            self.assertIn("invalid Oracle repository", error)
            self.assertEqual(calls, "")

    def test_missing_download_or_python_tool_fails_before_home_mutation(self) -> None:
        commands = "awk basename cat chmod cp curl diff find grep ln mkdir mktemp mv pwd python3 rm sed sort tar tr uniq uname flock lockf".split()
        for missing in ("curl", "python3"):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as raw:
                root = pathlib.Path(raw)
                for command in commands:
                    executable = shutil.which(command)
                    if executable and command != missing:
                        (root / command).symlink_to(executable)
                env = dict(os.environ, PATH=raw, AOS_HOME=str(root / "home"))
                for name in ("AOS_ORACLE_ASSETS", "AOS_ORACLES_VERSION", "AOS_ORACLES_REPO"):
                    env.pop(name, None)
                result = subprocess.run(
                    ["/bin/sh", str(ROOT / "install.sh"), "--oracle-version", "2026.9.2",
                     "--host", "codex", "--yes", "--no-install-aos"],
                    env=env, capture_output=True, text=True, timeout=5)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(missing, result.stderr)
                self.assertFalse((root / "home").exists())


if __name__ == "__main__":
    unittest.main()
