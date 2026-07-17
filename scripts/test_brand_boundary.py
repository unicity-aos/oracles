#!/usr/bin/env python3
"""Keep product-facing AOS names separate from neutral Astrid identifiers."""

from __future__ import annotations

import json
import pathlib
import re
import unittest


ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_json(path: str) -> dict:
    return json.loads((ROOT / path).read_text())


class BrandBoundaryTests(unittest.TestCase):
    def test_marketplaces_publish_the_aos_plugin(self) -> None:
        for path in (
            ".claude-plugin/marketplace.json",
            ".grok-plugin/marketplace.json",
        ):
            value = load_json(path)
            self.assertEqual(value["name"], "unicity-aos-oracles")
            self.assertEqual([plugin["name"] for plugin in value["plugins"]], ["unicity-aos"])

        codex = load_json(".agents/plugins/marketplace.json")
        self.assertEqual(codex["name"], "unicity-aos-oracles")
        self.assertEqual([plugin["name"] for plugin in codex["plugins"]], ["unicity-aos"])

    def test_host_manifests_and_mcp_names_are_product_facing(self) -> None:
        manifests = (
            "plugins/claude/.claude-plugin/plugin.json",
            "plugins/grok/.grok-plugin/plugin.json",
            "plugins/unicity-aos/.codex-plugin/plugin.json",
        )
        for path in manifests:
            self.assertEqual(load_json(path)["name"], "unicity-aos")

        for path in (
            "plugins/claude/.mcp.json",
            "plugins/grok/.mcp.json",
            "plugins/unicity-aos/.mcp.json",
        ):
            self.assertEqual(set(load_json(path)["mcpServers"]), {"aos"})

        codex_mcp = load_json("plugins/unicity-aos/.mcp.json")["mcpServers"]["aos"]
        self.assertEqual(codex_mcp["cwd"], ".")
        self.assertEqual(codex_mcp["command"], "/bin/sh")
        self.assertGreaterEqual(codex_mcp["startup_timeout_sec"], 300)

    def test_retired_public_names_do_not_return(self) -> None:
        roots = [ROOT / "README.md", ROOT / "install.sh", ROOT / "plugins"]
        forbidden = {
            "mcp__astrid__": "retired public MCP namespace",
            "/astrid:": "retired public command namespace",
            "astrid mcp serve": "private engine command exposed as product CLI",
            "astrid agent modify": "private engine command exposed as product CLI",
            "--no-migrate-prompt": "retired cross-product state migration flag",
            "plugin remove astrid@astrid-oracles": "fresh AOS install mutates the standalone Astrid plugin",
            "plugin uninstall astrid@astrid-oracles": "fresh AOS install mutates the standalone Astrid plugin",
            "plugin uninstall astrid": "fresh AOS install mutates the standalone Astrid plugin",
        }
        for root in roots:
            paths = [root] if root.is_file() else [path for path in root.rglob("*") if path.is_file()]
            for path in paths:
                try:
                    body = path.read_text()
                except UnicodeDecodeError:
                    continue
                for needle, reason in forbidden.items():
                    self.assertNotIn(needle, body, f"{reason}: {path.relative_to(ROOT)}")

    def test_aos_wrappers_do_not_use_the_legacy_home(self) -> None:
        for path in (
            ROOT / "plugins/common/bin/aos-up",
            ROOT / "plugins/common/bin/lib-aos-resolve.sh",
            ROOT / "plugins/unicity-aos/bin/aos-up",
            ROOT / "plugins/unicity-aos/bin/lib-aos-resolve.sh",
        ):
            body = path.read_text()
            active_lines = [
                line
                for line in body.splitlines()
                if not re.match(r"^\s*#", line)
            ]
            self.assertNotIn(".astrid", "\n".join(active_lines), str(path.relative_to(ROOT)))

    def test_aos_broker_preserves_the_neutral_engine_wire(self) -> None:
        identity = (ROOT / "crates/oracle-core/src/identity.rs").read_text()
        self.assertIn('CapsuleName("aos-mcp")', identity)
        self.assertNotIn('CapsuleName("astrid-mcp")', identity)
        self.assertIn('Topic("astrid.v1.tools.list")', identity)
        self.assertIn('McpNamespace("aos")', identity)
        self.assertIn('McpToolPrefix("mcp__aos__")', identity)
        manifest = (ROOT / "crates/aos-mcp/Capsule.toml").read_text()
        self.assertIn("@unicity-astrid/wit/", manifest)
        self.assertNotIn("@astrid-runtime/wit/", manifest)


if __name__ == "__main__":
    unittest.main()
