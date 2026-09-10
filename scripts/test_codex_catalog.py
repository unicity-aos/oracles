#!/usr/bin/env python3
"""Codex compatibility tests with a real subprocess and no live AOS changes."""
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import time
import unittest

from test_mcp_setup_status import Client

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "plugins/unicity-aos/bin/aos-codex-mcp"
loader = importlib.machinery.SourceFileLoader("codex_catalog", str(SOURCE))
spec = importlib.util.spec_from_loader(loader.name, loader)
catalog = importlib.util.module_from_spec(spec)
loader.exec_module(catalog)

FAKE = '''import json, pathlib, sys, time
root = pathlib.Path(sys.argv[1])
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg: continue
    method = msg.get("method")
    if method == "initialize":
        while not (root / "ready").exists(): time.sleep(.01)
        result = {"protocolVersion": msg["params"]["protocolVersion"], "capabilities": {"tools": {}}, "serverInfo": {"name": "fixture", "version": "1"}}
    elif method == "tools/list":
        if (root / "stall").exists(): continue
        result = {"tools": [{"name": name, "description": "Fixture " + name, "inputSchema": {"type": "object"}} for name in json.loads((root / "tools").read_text())]}
    else:
        name = msg.get("params", {}).get("name")
        if name == "install":
            (root / "tools").write_text('["after", "install"]')
            print(json.dumps({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}), flush=True)
        result = {"content": [{"type": "text", "text": str(name)}], "isError": name not in json.loads((root / "tools").read_text())}
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
'''


class CatalogTests(unittest.TestCase):
    def fixture(self, root):
        (root / "tools").write_text('["before", "install"]')
        backend = root / "backend.py"
        backend.write_text(FAKE)
        # Import production class but replace only the verified launcher in this
        # isolated protocol test. Live authentication is not claimed by fixtures.
        runner = root / "runner.py"
        runner.write_text(
            "import importlib.machinery, importlib.util, os, sys\n"
            "loader=importlib.machinery.SourceFileLoader('adapter',sys.argv[1])\n"
            "spec=importlib.util.spec_from_loader(loader.name,loader)\n"
            "mod=importlib.util.module_from_spec(spec); loader.exec_module(mod)\n"
            "os._exit(mod.Connection([sys.executable,sys.argv[2],sys.argv[3]]).run())\n")
        return Client([sys.executable, str(runner), str(SOURCE), str(backend), str(root)], dict(os.environ))

    def test_cold_start_late_install_and_invoke_without_refetch_or_restart(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            client = self.fixture(root)
            try:
                client.initialize()
                initial = client.request("tools/list")["result"]["tools"]
                self.assertEqual([t["name"] for t in initial], ["aos_setup_status", catalog.LIST, catalog.CALL])
                self.assertTrue(client.request("tools/call", {"name": catalog.LIST})["result"]["isError"])
                (root / "ready").touch()
                client.receive()  # native notification remains available
                time.sleep(.1)
                names = client.request("tools/call", {"name": catalog.LIST})["result"]
                self.assertIn("before", names["content"][0]["text"])
                self.assertIn("aos_catalog_notice", names["content"][-1]["text"])
                reply = client.request("tools/call", {"name": catalog.CALL, "arguments": {"name": "install", "arguments": {}}})
                self.assertFalse(reply["result"]["isError"])
                time.sleep(.1)
                reply = client.request("tools/call", {"name": catalog.CALL, "arguments": {"name": "after", "arguments": {}}})
                self.assertEqual(reply["result"]["content"][0]["text"], "after")
                self.assertIn('"name": "after"', reply["result"]["content"][-1]["text"])
                again = client.request("tools/call", {"name": catalog.CALL, "arguments": {"name": "after", "arguments": {}}})
                self.assertEqual(len(again["result"]["content"]), 1)
                removed = client.request("tools/call", {"name": catalog.CALL, "arguments": {"name": "before", "arguments": {}}})
                self.assertTrue(removed["result"]["isError"])
                self.assertEqual(client.request("tools/list")["result"]["tools"], initial)
            finally:
                client.close()

    def test_stalled_catalog_does_not_hold_ordinary_response(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "ready").touch()
            (root / "stall").touch()
            client = self.fixture(root)
            try:
                client.initialize()
                client.receive()
                started = time.monotonic()
                result = client.request("tools/call", {"name": catalog.CALL, "arguments": {"name": "before", "arguments": {}}})["result"]
                self.assertFalse(result["isError"])
                self.assertLess(time.monotonic() - started, .3)
                self.assertEqual(len(result["content"]), 1)
                time.sleep(.4)
                self.assertEqual(client.request("ping")["result"], {})
            finally:
                client.close()

    def test_notices_are_connection_local_and_invalidated_before_refresh(self):
        a, b = catalog.Connection([]), catalog.Connection([])
        a.catalog_valid = True
        a.catalog = {"private": ("digest", "test")}
        result = {"content": [], "isError": False}
        self.assertIn("private", a.with_notice(result)["content"][0]["text"])
        self.assertEqual(b.with_notice(result), result)
        a.changed = catalog.Connection.changed.__get__(a)
        a.changed()
        self.assertFalse(a.catalog_valid)
        self.assertEqual(a.with_notice(result), result)

    def test_pagination_schema_change_and_racing_invalidation(self):
        connection = catalog.Connection([])
        connection.state = "ready"
        sent = []
        connection.to_child = sent.append
        try:
            connection.changed()
            old_id = connection.refresh_id
            connection.changed()  # revoke while the first page is in flight
            tool = {"name": "old", "description": "not allowed", "inputSchema": {"type": "object"}}
            connection.backend({"id": old_id, "result": {"tools": [tool]}})
            self.assertFalse(connection.catalog_valid)
            self.assertNotEqual(connection.refresh_id, old_id)
            tool = dict(tool, name="allowed", description="safe\nmetadata")
            connection.backend({"id": connection.refresh_id, "result": {"tools": [tool], "nextCursor": "second"}})
            self.assertFalse(connection.catalog_valid)
            self.assertEqual(sent[-1]["params"], {"cursor": "second"})
            connection.backend({"id": connection.refresh_id, "result": {"tools": []}})
            notice = connection.with_notice({"content": []})
            self.assertIn("allowed", notice["content"][0]["text"])
            self.assertNotIn('"name": "old"', notice["content"][0]["text"])
            connection.changed()
            tool["inputSchema"] = {"type": "object", "required": ["new_field"]}
            connection.backend({"id": connection.refresh_id, "result": {"tools": [tool]}})
            self.assertTrue(connection.with_notice({"content": []})["content"])
        finally:
            connection.close()

    def test_invalid_catalog_and_bounded_notices(self):
        connection = catalog.Connection([])
        connection.state = "ready"
        connection.to_child = lambda message: None
        try:
            connection.changed()
            connection.backend({"id": connection.refresh_id, "result": {"tools": [{"name": "bad\nname"}]}})
            self.assertFalse(connection.catalog_valid)
            connection.changed()
            tools = [{"name": f"tool_{i}", "description": "x" * 2000} for i in range(10)]
            connection.backend({"id": connection.refresh_id, "result": {"tools": tools}})
            first = connection.with_notice({"content": []})
            data = json.loads(first["content"][0]["text"].split(": ", 1)[1])["aos_catalog_notice"]
            self.assertEqual(len(data), 8)
            self.assertTrue(all(len(item["description"]) == 160 for item in data))
            self.assertTrue(connection.with_notice({"content": []})["content"])
            self.assertEqual(connection.with_notice({"content": []})["content"], [])
        finally:
            connection.close()


if __name__ == "__main__":
    unittest.main()
