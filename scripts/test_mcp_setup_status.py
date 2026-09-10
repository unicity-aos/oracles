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

    def initialize(self):
        result = self.request("initialize", {"protocolVersion": "2025-11-25", "capabilities": {},
                                             "clientInfo": {"name": "test", "version": "1"}})
        assert result["result"]["capabilities"]["tools"]["listChanged"]
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def status(self):
        reply = self.request("tools/call", {"name": adapter.STATUS_NAME, "arguments": {}})
        return json.loads(reply["result"]["content"][0]["text"]), reply

    def close(self):
        self.proc.stdin.close()
        self.proc.wait(timeout=6)
        self.proc.stdout.close()
        self.proc.stderr.close()


class SetupTests(unittest.TestCase):
    def fixture(self, root, host, framed=False):
        plugin = root / host
        (plugin / "bin").mkdir(parents=True)
        shutil.copyfile(SOURCE, plugin / "bin/aos-mcp-start")
        launcher = plugin / "bin/aos-up"
        launcher.write_text(FAKE)
        launcher.chmod(0o700)
        server = json.loads((ROOT / "plugins" / host / ".mcp.json").read_text())["mcpServers"]["aos"]
        env = dict(os.environ, FIXTURE_ROOT=str(root), CODEX_PLUGIN_ROOT=str(plugin),
                   PLUGIN_ROOT=str(plugin), CLAUDE_PLUGIN_ROOT=str(plugin), GROK_PLUGIN_ROOT=str(plugin))
        def expand(value):
            for name in ("CODEX_PLUGIN_ROOT", "CLAUDE_PLUGIN_ROOT", "GROK_PLUGIN_ROOT"):
                value = value.replace("${" + name + "}", str(plugin))
            return value
        command = [expand(server["command"]), *[expand(arg) for arg in server["args"]]]
        return Client(command, env, framed)

    def test_all_packaged_commands_answer_before_install_then_call_same_connection(self):
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
                        tools = client.request("tools/list")["result"]["tools"]
                        self.assertEqual([tool["name"] for tool in tools], [adapter.STATUS_NAME])
                        self.assertIn("error", client.request("tools/call", {"name": "fixture_echo"}))
                        self.assertEqual(client.request("ping")["result"], {})
                        (root / "release").touch()
                        self.assertEqual(client.receive()["method"], "notifications/tools/list_changed")
                        self.assertEqual(client.status()[0]["state"], "ready")
                        tools = client.request("tools/list")["result"]["tools"]
                        self.assertEqual([tool["name"] for tool in tools], [adapter.STATUS_NAME, "fixture_echo"])
                        # Host IDs may equal the adapter's private initialization ID.
                        reply = client.request("tools/call", {"name": "fixture_echo"}, "aos-initialize")
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
                self.assertEqual(len(client.request("tools/list")["result"]["tools"]), 1)
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
                self.assertEqual([tool["name"] for tool in tools], [adapter.STATUS_NAME])
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
