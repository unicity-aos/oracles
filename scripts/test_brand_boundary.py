#!/usr/bin/env python3
"""Keep product-facing AOS names separate from neutral Astrid identifiers."""

from __future__ import annotations

import json
import pathlib
import re
import tomllib
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
        self.assertEqual(
            codex_mcp["env_vars"], ["AOS_HOME", "AOS_BIN", "AOS_BIN_ROOT"]
        )

    def test_codex_plugin_teaches_building_on_the_os(self) -> None:
        plugin = ROOT / "plugins/unicity-aos"
        manifest = load_json("plugins/unicity-aos/.codex-plugin/plugin.json")
        self.assertIn("operating system for agents", manifest["description"])
        self.assertIn(
            "Notice and build useful extensions to this agent's AOS world while working.",
            manifest["interface"]["defaultPrompt"],
        )

        required = {
            "skills/unicity-aos/SKILL.md": (
                "Unicity AOS is not itself an agent harness",
                "Load `capsule-forge` before authoring a capsule",
                "workspace and principal-home",
                "`list_skills`",
                "`read_skill`",
                "supplies instructions, not authority",
                "Choose the right artifact",
                "improvable user-space",
                "user's instructions",
                "AOS is the common operating environment",
            ),
            "skills/capsule-forge/SKILL.md": (
                "Build naturally on Unicity AOS",
                "Call `forge_guide`",
                "`references/<topic>.md`",
                "Keep Skills out of Capsule.toml",
                "Generated code does not self-promote",
                "where it should live",
                "Priority is not just sort order",
            ),
            "skills/meta-harness/SKILL.md": (
                "Treat the AOS user-space environment",
                "Exercise initiative",
                "Reach for the ability proactively",
                "The user's instruction sets the degree of freedom",
                "Worker or subagent",
                "optional pattern, not a prerequisite",
                "Improve harness code from experience",
                "Definition of done",
            ),
        }
        for relative, needles in required.items():
            body = (plugin / relative).read_text()
            for needle in needles:
                self.assertIn(needle, body, relative)

        references = plugin / "skills/capsule-forge/references"
        for topic in (
            "foundations",
            "workspace",
            "capsule",
            "manifest",
            "capabilities",
            "ipc",
            "wit",
            "skills",
            "authority",
            "build",
            "security",
            "meta-harness",
        ):
            reference = references / f"{topic}.md"
            self.assertTrue(reference.is_file(), reference)
            self.assertGreaterEqual(len(reference.read_text().splitlines()), 35, reference)

    def test_aos_hosts_describe_user_space_skills(self) -> None:
        for path in (
            "plugins/common/bin/aos-doctor",
            "plugins/claude/bin/aos-doctor",
            "plugins/grok/bin/aos-doctor",
            "plugins/unicity-aos/bin/aos-doctor",
        ):
            body = (ROOT / path).read_text()
            self.assertIn("Workspace and principal-home Skills", body, path)
            self.assertIn("list_skills", body, path)
            self.assertIn("read_skill", body, path)
            self.assertIn("ordinary IPC tools", body, path)

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

    def test_aos_broker_is_a_product_dependency_not_an_oracle_asset(self) -> None:
        self.assertFalse((ROOT / "crates/oracle-broker").exists())
        self.assertFalse((ROOT / "crates/aos-mcp").exists())
        for pack in (ROOT / "packs").glob("*.toml"):
            value = tomllib.loads(pack.read_text())
            self.assertNotIn("capsule", value)
            self.assertIn(
                {"name": "aos-mcp", "availability": "required"},
                value["aos-capsule"],
            )


if __name__ == "__main__":
    unittest.main()
