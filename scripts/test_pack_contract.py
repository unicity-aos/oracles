#!/usr/bin/env python3
"""Validate the additive oracle-pack contract."""

from __future__ import annotations

import pathlib
import tomllib
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent
EXPECTED = {
    "claude": ["aos-mcp"],
    "codex": ["aos-mcp"],
    "grok": ["aos-mcp"],
}


class PackContractTests(unittest.TestCase):
    def test_packs_are_additive_and_exact(self) -> None:
        for host, expected in EXPECTED.items():
            value = tomllib.loads((ROOT / "packs" / f"{host}.toml").read_text())
            self.assertEqual(value["schema-version"], 1)
            pack = value["pack"]
            self.assertEqual(pack["host"], host)
            self.assertEqual(pack["principal"], f"{host}-code")
            self.assertEqual(pack["version"], "0.2.0")
            capsules = value["capsule"]
            self.assertEqual([item["name"] for item in capsules], expected)
            self.assertEqual(
                [item["asset"] for item in capsules],
                [f"{name}.capsule" for name in expected],
            )

    def test_packs_do_not_redeclare_the_ce_distribution(self) -> None:
        for path in sorted((ROOT / "packs").glob("*.toml")):
            value = tomllib.loads(path.read_text())
            self.assertNotIn("distro", value)
            names = {item["name"] for item in value["capsule"]}
            self.assertNotIn("astrid-capsule-cli", names)
            self.assertNotIn("astrid-capsule-system", names)
            self.assertNotIn("astrid-capsule-forge", names)
            self.assertFalse(any(name.endswith(("-install", "-runner")) for name in names))


if __name__ == "__main__":
    unittest.main()
