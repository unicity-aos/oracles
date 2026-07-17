#!/usr/bin/env python3
"""Keep capsule-owned oracle bus contracts bundled, typed, and in sync."""

from __future__ import annotations

import pathlib
import os
import tomllib
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent

HOSTS = {
    "claude": {
        "install": ROOT / "crates/claude-install",
        "runner": ROOT / "crates/claude-runner",
        "records": {
            "claude.v1.install.run": "claude-install-request",
            "claude.v1.install.relink": "claude-relink-request",
            "claude.v1.install.status": "claude-install-status",
            "claude.v1.install.complete": "claude-install-complete",
        },
    },
    "codex": {
        "install": ROOT / "crates/codex-install",
        "runner": ROOT / "crates/codex-runner",
        "records": {
            "codex.v1.install.run": "codex-install-request",
            "codex.v1.install.relink": "codex-relink-request",
            "codex.v1.install.status": "codex-install-status",
            "codex.v1.install.complete": "codex-install-complete",
        },
    },
}


def manifest(crate: pathlib.Path) -> dict:
    return tomllib.loads((crate / "Capsule.toml").read_text())


def wit_ref(value: object) -> str:
    if isinstance(value, str):
        return value
    if isinstance(value, dict) and isinstance(value.get("wit"), str):
        return value["wit"]
    raise AssertionError(f"invalid topic declaration: {value!r}")


class CapsuleOwnedWitTests(unittest.TestCase):
    def test_runner_and_provisioner_ship_identical_contracts(self) -> None:
        for host, spec in HOSTS.items():
            install_wit = spec["install"] / "wit" / f"{host}-install.wit"
            runner_wit = spec["runner"] / "wit" / f"{host}-install.wit"
            self.assertEqual(
                install_wit.read_bytes(),
                runner_wit.read_bytes(),
                f"{host} runner/provisioner WIT drifted",
            )

    def test_every_install_topic_names_a_local_record(self) -> None:
        for host, spec in HOSTS.items():
            records = spec["records"]
            wit = (spec["install"] / "wit" / f"{host}-install.wit").read_text()
            install_manifest = manifest(spec["install"])
            runner_manifest = manifest(spec["runner"])

            for topic, record in records.items():
                self.assertIn(f"record {record} {{", wit)
                declarations = []
                for value in (install_manifest, runner_manifest):
                    for table in ("publish", "subscribe"):
                        if topic in value.get(table, {}):
                            declarations.append(wit_ref(value[table][topic]))
                self.assertTrue(declarations, f"{topic} is not declared")
                self.assertTrue(
                    all(item == record for item in declarations),
                    f"{topic} does not consistently reference {record}: {declarations}",
                )
                self.assertNotIn("opaque", declarations)

    def test_workload_adapters_are_not_in_the_external_plugin_release(self) -> None:
        artifacts_value = os.environ.get("AOS_WIT_ARTIFACTS")
        if artifacts_value is None:
            self.skipTest("AOS_WIT_ARTIFACTS is not set")
        artifacts = pathlib.Path(artifacts_value)

        for host in HOSTS:
            for role in ("install", "runner"):
                archive = artifacts / f"{host}-{role}.capsule"
                self.assertFalse(archive.exists(), f"workload adapter leaked into plugin release: {archive}")


if __name__ == "__main__":
    unittest.main()
