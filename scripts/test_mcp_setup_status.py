#!/usr/bin/env python3
"""Real pipe tests of the packaged MCP commands; no network or runtime needed."""
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parent.parent
sys.dont_write_bytecode = True
SOURCE = ROOT / "plugins/common/bin/aos-mcp-start"
loader = importlib.machinery.SourceFileLoader("setup_adapter", str(SOURCE))
spec = importlib.util.spec_from_loader(loader.name, loader)
adapter = importlib.util.module_from_spec(spec)
loader.exec_module(adapter)

FAKE = '''#!/usr/bin/env python3
import json, os, pathlib, sys, time
root = pathlib.Path(os.environ["FIXTURE_ROOT"])
(root / "pid").write_text(str(os.getpid()))
with (root / "starts").open("a") as out: out.write("start\\n")
while not (root / "release").exists(): time.sleep(.01)
if (root / "fail").exists(): sys.exit(7)
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if "id" not in request: continue
    if method in ("tools/list", "tools/call"):
        with (root / "forwarded-requests").open("a") as out:
            out.write(json.dumps({"method": method, "params": request.get("params", {})}) + "\\n")
    if method == "initialize":
        result = {"protocolVersion": request["params"]["protocolVersion"], "capabilities": {"tools": {"listChanged": True}}, "serverInfo": {"name": "fixture", "version": "1"}}
        if (root / "wrong-protocol").exists(): result["protocolVersion"] = "1999-01-01"
    elif method == "tools/list":
        result = {"tools": [{"name": "fixture_echo", "inputSchema": {"type": "object"}}]}
    else:
        if request.get("params", {}).get("name") == "crash": sys.exit(9)
        result = {"content": [{"type": "text", "text": "real backend result"}], "isError": False}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
'''


class Client:
    def __init__(self, command, env, framed=False):
        self.proc = subprocess.Popen(command, env=env, stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.events = queue.Queue()
        self.framed = framed
        threading.Thread(target=adapter.read_events,
                         args=(self.proc.stdout, "reply", self.events), daemon=True).start()

    def send(self, message):
        raw = json.dumps(message).encode()
        if self.framed:
            raw = f"Content-Length: {len(raw)}\r\n\r\n".encode() + raw
        else:
            raw += b"\n"
        self.proc.stdin.write(raw)
        self.proc.stdin.flush()

    def receive(self):
        _, mode, message = self.events.get(timeout=3)
        assert message is not None, self.proc.stderr.read().decode()
        assert mode == ("content-length" if self.framed else "json-line")
        return message

    def request(self, method, params=None, identity=1):
        self.send({"jsonrpc": "2.0", "id": identity, "method": method, "params": params or {}})
        while True:
            message = self.receive()
            if message.get("id") == identity:
                return message

    def initialize(self, version="2025-11-25"):
        result = self.request("initialize", {"protocolVersion": version, "capabilities": {},
                                             "clientInfo": {"name": "test", "version": "1"}})
        assert result["result"]["capabilities"]["tools"]["listChanged"]
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return result

    def status(self):
        reply = self.request("tools/call", {"name": adapter.STATUS_NAME, "arguments": {}})
        return json.loads(reply["result"]["content"][0]["text"]), reply

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=6)
        self.proc.stdout.close()
        self.proc.stderr.close()


class SetupTests(unittest.TestCase):
    def test_update_notice_refresh_is_detached_and_reaches_hook_and_status(self):
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw)
            bindir = home / "bin"
            bindir.mkdir()
            aos = bindir / "aos"
            aos.write_text("#!/bin/sh\n[ \"$*\" = 'update --check' ] || exit 91\nsleep 2\necho 'Update available: AOS 2026.9.2 -> 2026.9.3'\n")
            aos.chmod(0o700)
            env = dict(os.environ, AOS_HOME=raw, AOS_BIN=str(aos))
            command = [sys.executable, str(ROOT / "plugins/unicity-aos/bin/aos-codex-mcp"), "--hook"]
            started = time.monotonic()
            result = subprocess.run(command, env=env, capture_output=True, timeout=1.5, check=True)
            self.assertLess(time.monotonic() - started, 1.5)
            self.assertNotIn("Update available", result.stdout.decode())
            stamp = home / "update/host-update-check/stamp"
            deadline = time.monotonic() + 6
            while not stamp.exists() and time.monotonic() < deadline:
                time.sleep(.05)
            self.assertTrue(stamp.exists(), "detached check did not complete")
            for script in (SOURCE, ROOT / "plugins/unicity-aos/bin/aos-codex-mcp"):
                result = subprocess.run([sys.executable, str(script), "--hook"], env=env,
                                        capture_output=True, timeout=1.5, check=True)
                context = json.loads(result.stdout)["hookSpecificOutput"]["additionalContext"]
                self.assertIn("Tell the user: Update available:", context)
            from unittest.mock import patch
            with patch.dict(os.environ, env):
                codex_loader = importlib.machinery.SourceFileLoader("update_codex", command[1])
                codex_spec = importlib.util.spec_from_loader(codex_loader.name, codex_loader)
                codex = importlib.util.module_from_spec(codex_spec)
                codex_loader.exec_module(codex)
                for implementation in (adapter, codex):
                    connection = implementation.Connection([])
                    connection.initialized = True
                    replies = []
                    connection.result = lambda request, result: replies.append(result)
                    connection.host({"id": 1, "method": "tools/call", "params": {"name": adapter.STATUS_NAME}})
                    status = json.loads(replies[0]["content"][0]["text"])
                    self.assertIn("Update available:", status["update_notice"])

    def test_failed_update_check_is_not_cached_as_success(self):
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw)
            aos = home / "aos"
            aos.write_text("#!/bin/sh\nexit 69\n")
            aos.chmod(0o700)
            result = subprocess.run([str(ROOT / "plugins/common/bin/aos-update-check"), str(aos)],
                                    env=dict(os.environ, AOS_HOME=raw), capture_output=True, timeout=3)
            self.assertEqual(result.returncode, 0)
            state = home / "update/host-update-check"
            self.assertFalse((state / "stamp").exists())
            self.assertTrue((state / "retry").exists())
            self.assertFalse((state / "lock").exists())

    def test_fresh_command_center_inventory_does_not_start_another_checker(self):
        with tempfile.TemporaryDirectory() as raw:
            home = Path(raw)
            state = home / "update/command-center"
            state.mkdir(parents=True)
            (state / "inventory.json").write_text(json.dumps({
                "schema_version": 1, "checked_at": int(time.time()), "items": [
                    {"id": "oracle:codex", "candidate_version": "2026.10.0", "availability": "available"},
                    {"id": "capsule:private:secret-name", "candidate_version": "1.0.0", "availability": "available"},
                ]}))
            aos = home / "aos"
            aos.write_text("#!/bin/sh\ntouch \"$AOS_HOME/unexpected-check\"\nexit 1\n")
            aos.chmod(0o700)
            result = subprocess.run([str(ROOT / "plugins/common/bin/aos-update-check"), str(aos)],
                                    env=dict(os.environ, AOS_HOME=raw), capture_output=True, timeout=2)
            self.assertEqual(result.returncode, 0)
            self.assertIn(b"Oracle for Codex 2026.10.0", result.stdout)
            self.assertNotIn(b"secret-name", result.stdout)
            self.assertFalse((home / "unexpected-check").exists())

    def test_malformed_command_center_inventory_falls_back_without_traceback(self):
        for items in ([None], [{"id": []}], "not-an-array"):
            with self.subTest(items=items), tempfile.TemporaryDirectory() as raw:
                home = Path(raw)
                state = home / "update/command-center"
                state.mkdir(parents=True)
                (state / "inventory.json").write_text(json.dumps({
                    "schema_version": 1, "checked_at": int(time.time()), "items": items,
                }))
                aos = home / "aos"
                aos.write_text("#!/bin/sh\nprintf 'Update available: AOS 2026.10.0\\n'\n")
                aos.chmod(0o700)
                result = subprocess.run([str(ROOT / "plugins/common/bin/aos-update-check"), str(aos)],
                                        env=dict(os.environ, AOS_HOME=raw), capture_output=True, timeout=3)
                self.assertEqual(result.returncode, 0)
                self.assertIn(b"AOS 2026.10.0", result.stdout)
                self.assertEqual(result.stderr, b"")

    def test_legacy_proposals_negotiate_a_backend_supported_version(self):
        for version in ("2024-11-05", "2025-03-26", "2025-06-18"):
            with self.subTest(version=version), tempfile.TemporaryDirectory() as raw:
                client = self.fixture(Path(raw), "unicity-aos")
                try:
                    reply = client.initialize(version)
                    self.assertEqual(reply["result"]["protocolVersion"], "2025-11-25")
                finally:
                    client.close()

    def fixture(self, root, host, framed=False):
        plugin = root / host
        (plugin / "bin").mkdir(parents=True)
        adapter_path = plugin / "bin/aos-mcp-start"
        shutil.copyfile(SOURCE, adapter_path)
        if host == "unicity-aos":
            shutil.copyfile(ROOT / "plugins" / host / "bin/aos-codex-mcp", plugin / "bin/aos-codex-mcp")
            shutil.copyfile(ROOT / "plugins" / host / "bin/aos-configure-mcp",
                            plugin / "bin/aos-configure-mcp")
            shutil.copyfile(ROOT / "plugins" / host / ".mcp.json", plugin / ".mcp.json")
            subprocess.run([sys.executable, str(plugin / "bin/aos-configure-mcp")], check=True)
        launcher = plugin / "bin/aos-up"
        launcher.write_text(FAKE)
        launcher.chmod(0o700)
        server = json.loads((ROOT / "plugins" / host / ".mcp.json").read_text())["mcpServers"]["aos"]
        env = dict(os.environ, FIXTURE_ROOT=str(root), CODEX_PLUGIN_ROOT=str(plugin),
                   PLUGIN_ROOT=str(plugin), CLAUDE_PLUGIN_ROOT=str(plugin), GROK_PLUGIN_ROOT=str(plugin))
        if host == "unicity-aos":
            server = json.loads((plugin / ".mcp.json").read_text())["mcpServers"]["aos"]
            for name in ("CODEX_PLUGIN_ROOT", "PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT", "GROK_PLUGIN_ROOT", "AOS_PLUGIN_ROOT"):
                env.pop(name, None)
        def expand(value):
            for name in ("CODEX_PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT", "GROK_PLUGIN_ROOT"):
                value = value.replace("${" + name + "}", str(plugin))
            return value
        command = [expand(server["command"]), *[expand(arg) for arg in server["args"]]]
        return Client(command, env, framed)

    def test_initial_tools_list_returns_immediately_with_stable_tools(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            client = self.fixture(root, "claude")
            try:
                client.initialize()
                self.assertEqual(client.status()[0]["state"], "starting")
                started = time.monotonic()
                tools = client.request("tools/list")["result"]["tools"]
                elapsed = time.monotonic() - started
                self.assertLess(elapsed, .5)
                self.assertEqual([tool["name"] for tool in tools],
                                 [adapter.STATUS_NAME, adapter.LIST_TOOLS_NAME, adapter.CALL_TOOL_NAME])
                self.assertEqual(client.status()[0]["state"], "starting")
                (root / "release").touch()
                self.assertEqual(client.receive()["method"], "notifications/tools/list_changed")
                self.assertEqual(client.status()[0]["state"], "ready")
                refreshed = client.request("tools/list")["result"]["tools"]
                self.assertEqual([tool["name"] for tool in refreshed],
                                 [adapter.STATUS_NAME, adapter.LIST_TOOLS_NAME,
                                  adapter.CALL_TOOL_NAME, "fixture_echo"])
            finally:
                client.close()

    def test_cache_first_client_can_use_runtime_tools_after_late_readiness(self):
        """A host may cache the first list and ignore list_changed for this session."""
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            client = self.fixture(root, "claude")
            try:
                client.initialize()
                initial_tools = client.request("tools/list")["result"]["tools"]
                names = {tool["name"] for tool in initial_tools}

                # Deliberately do not act on notifications/tools/list_changed.
                self.assertIn(adapter.STATUS_NAME, names)
                self.assertIn("aos_list_tools", names)
                self.assertIn("aos_call_tool", names)

                (root / "release").touch()
                deadline = time.monotonic() + 2
                while client.status()[0]["state"] == "starting":
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.01)

                catalogue = client.request("tools/call", {
                    "name": "aos_list_tools", "arguments": {}, "principal": "foreign",
                    "transport": "attacker-selected", "_meta": {"progressToken": 9},
                })["result"]
                listed = json.loads(catalogue["content"][0]["text"])
                self.assertEqual([tool["name"] for tool in listed["tools"]], ["fixture_echo"])

                invoked = client.request("tools/call", {
                    "name": "aos_call_tool",
                    "arguments": {"name": "fixture_echo", "arguments": {}},
                    "principal": "foreign", "transport": "attacker-selected",
                    "_meta": {"progressToken": 10},
                })["result"]
                self.assertEqual(invoked["content"][0]["text"], "real backend result")
                forwarded = [json.loads(line) for line in (root / "forwarded-requests").read_text().splitlines()]
                self.assertEqual(forwarded, [
                    {"method": "tools/list", "params": {"_meta": {"progressToken": 9}}},
                    {"method": "tools/call", "params": {
                        "name": "fixture_echo", "arguments": {}, "_meta": {"progressToken": 10},
                    }},
                ])
            finally:
                client.close()

    def test_all_packaged_commands_keep_initialize_and_status_responsive(self):
        for host in ("unicity-aos", "claude", "grok"):
            for framed in (False, True):
                with self.subTest(host=host, framed=framed), tempfile.TemporaryDirectory() as raw:
                    root = Path(raw)
                    client = self.fixture(root, host, framed)
                    try:
                        start = time.monotonic()
                        client.initialize()
                        self.assertLess(time.monotonic() - start, 2)
                        self.assertFalse((root / "release").exists())
                        self.assertEqual(client.status()[0]["state"], "starting")
                        expected = [adapter.STATUS_NAME, adapter.LIST_TOOLS_NAME, adapter.CALL_TOOL_NAME]
                        self.assertIn("error", client.request("tools/call", {"name": "fixture_echo"}))
                        self.assertEqual(client.request("ping")["result"], {})
                        tools = client.request("tools/list")["result"]["tools"]
                        self.assertEqual([tool["name"] for tool in tools], expected)
                        (root / "release").touch()
                        ready_deadline = time.monotonic() + 2
                        while client.status()[0]["state"] == "starting":
                            self.assertLess(time.monotonic(), ready_deadline)
                            time.sleep(.01)
                        self.assertEqual(client.status()[0]["state"], "ready")
                        if host != "unicity-aos":
                            tools = client.request("tools/list")["result"]["tools"]
                        self.assertEqual([tool["name"] for tool in tools], expected + (["fixture_echo"] if host != "unicity-aos" else []))
                        # Host IDs may equal the adapter's private initialization ID.
                        params = {"name": "aos_call_tool", "arguments": {"name": "fixture_echo", "arguments": {}}} if host == "unicity-aos" else {"name": "fixture_echo"}
                        reply = client.request("tools/call", params, "aos-initialize")
                        self.assertEqual(reply["result"]["content"][0]["text"], "real backend result")
                        self.assertEqual((root / "starts").read_text(), "start\n")
                    finally:
                        client.close()

    def test_failed_install_stays_queryable_without_claiming_tools(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "fail").touch()
            client = self.fixture(root, "unicity-aos")
            try:
                client.initialize()
                (root / "release").touch()
                client.receive()
                status, reply = client.status()
                self.assertEqual(status["state"], "failed")
                self.assertTrue(reply["result"]["isError"])
                self.assertEqual(len(client.request("tools/list")["result"]["tools"]), 3)
            finally:
                client.close()

    def test_failed_backend_keeps_stable_tools_available(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "fail").touch()
            client = self.fixture(root, "claude")
            try:
                client.initialize()
                tools = client.request("tools/list")["result"]["tools"]
                self.assertEqual([tool["name"] for tool in tools],
                                 [adapter.STATUS_NAME, adapter.LIST_TOOLS_NAME, adapter.CALL_TOOL_NAME])
                (root / "release").touch()
                deadline = time.monotonic() + 2
                while client.status()[0]["state"] == "starting":
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(.01)
                self.assertEqual(client.status()[0]["state"], "failed")
            finally:
                client.close()

    def test_disconnect_reaps_own_slow_launcher(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            client = self.fixture(root, "grok")
            client.initialize()
            deadline = time.monotonic() + 3
            while not (root / "pid").exists():
                self.assertLess(time.monotonic(), deadline)
                time.sleep(.01)
            pid = int((root / "pid").read_text())
            client.close()
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    def test_protocol_mismatch_is_not_ready(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            (root / "wrong-protocol").touch()
            client = self.fixture(root, "claude")
            try:
                client.initialize()
                (root / "release").touch()
                client.receive()
                self.assertEqual(client.status()[0]["state"], "failed")
            finally:
                client.close()

    def test_backend_exit_fails_pending_call_and_removes_runtime_tools(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            client = self.fixture(root, "grok")
            try:
                client.initialize()
                (root / "release").touch()
                client.receive()
                self.assertIn("error", client.request("tools/call", {"name": "crash"}))
                self.assertEqual(client.status()[0]["state"], "failed")
                tools = client.request("tools/list")["result"]["tools"]
                self.assertEqual([tool["name"] for tool in tools],
                                 [adapter.STATUS_NAME, adapter.LIST_TOOLS_NAME, adapter.CALL_TOOL_NAME])
            finally:
                client.close()

    def test_session_start_status_does_not_install_or_wait_for_stdin(self):
        for host in ("unicity-aos", "claude", "grok"):
            with self.subTest(host=host), tempfile.TemporaryDirectory() as raw:
                plugin = ROOT / "plugins" / host
                hooks = json.loads((plugin / "hooks/hooks.json").read_text())
                hook = hooks["hooks"]["SessionStart"][0]["hooks"][0]
                self.assertLessEqual(hook["timeout"], 5)
                env = dict(os.environ, AOS_HOME=str(Path(raw) / "untouched"),
                           CODEX_PLUGIN_ROOT=str(plugin), PLUGIN_ROOT=str(plugin),
                           CLAUDE_PLUGIN_ROOT=str(plugin), GROK_PLUGIN_ROOT=str(plugin))
                proc = subprocess.Popen(["/bin/sh", "-c", hook["command"]], env=env,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE)
                proc.wait(timeout=2)  # stdin deliberately left open
                output = json.loads(proc.stdout.read())
                self.assertIn("aos_setup_status", output["hookSpecificOutput"]["additionalContext"])
                self.assertFalse(Path(env["AOS_HOME"]).exists())
                proc.stdin.close()
                proc.stdout.close()

    def test_vendored_adapter_is_identical(self):
        for host in ("claude", "grok", "unicity-aos"):
            self.assertEqual((ROOT / "plugins" / host / "bin/aos-mcp-start").read_bytes(), SOURCE.read_bytes())


if __name__ == "__main__":
    unittest.main()
