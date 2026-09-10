#!/usr/bin/env python3
"""Optional installed-Codex integration probe; local fake Responses, no inference.

Usage: python3 -B scripts/probe_codex_catalog_app_server.py /path/to/codex
Uses disposable CODEX_HOME, fixture backend and real functions.exec dispatch.
Does not install a plugin or certify the real AOS runtime's authentication.
"""
import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from test_codex_catalog import FAKE, SOURCE


def probe(cli):
    scripts = [
        "text(ALL_TOOLS.filter(t => t.name.includes('catalog_probe')).map(t => t.name));",
        "text(await tools.mcp__catalog_probe__aos_list_tools({}));",
        "text(await tools.mcp__catalog_probe__aos_call_tool({name:'install',arguments:{}}));",
        "text(await tools.mcp__catalog_probe__aos_call_tool({name:'after',arguments:{}}));",
    ]
    requests = []
    class Model(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            requests.append(json.loads(self.rfile.read(int(self.headers['Content-Length']))))
            n = len(requests) - 1
            if n % 2 == 0:
                output = {"id": f"tool_{n}", "type": "custom_tool_call", "call_id": f"call_{n}", "name": "exec", "namespace": "functions", "input": scripts[n // 2]}
            else:
                output = {"id": f"msg_{n}", "type": "message", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "complete", "annotations": []}]}
            response = {"id": f"resp_{n}", "object": "response", "status": "completed", "output": [output], "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}
            data = "event: response.output_item.done\ndata: " + json.dumps({"type": "response.output_item.done", "output_index": 0, "item": output}) + "\n\n"
            data += "event: response.completed\ndata: " + json.dumps({"type": "response.completed", "response": response}) + "\n\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(data.encode())))
            self.end_headers()
            self.wfile.write(data.encode())

    with tempfile.TemporaryDirectory(prefix="aos-catalog-probe-") as raw:
        root = Path(raw)
        (root / "tools").write_text('["before", "install"]')
        (root / "backend.py").write_text(FAKE)
        http = ThreadingHTTPServer(("127.0.0.1", 0), Model)
        threading.Thread(target=http.serve_forever, daemon=True).start()
        config = f'model_provider = "fixture"\n[model_providers.fixture]\nname = "Local fixture"\nwire_api = "responses"\nbase_url = "http://127.0.0.1:{http.server_port}/v1"\n'
        config += '[mcp_servers.catalog_probe]\ncommand = ' + json.dumps(sys.executable) + '\nargs = ' + json.dumps([str(Path(__file__).resolve()), "fixture", raw]) + '\n'
        (root / "config.toml").write_text(config)
        with (root / "stderr").open("w") as stderr:
            proc = subprocess.Popen([cli, "app-server"], env=dict(os.environ, CODEX_HOME=raw), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr, text=True)
            events = queue.Queue()
            def read():
                for line in proc.stdout:
                    events.put(json.loads(line))
            threading.Thread(target=read, daemon=True).start()
            serial = 0
            def call(method, params):
                nonlocal serial
                serial += 1
                proc.stdin.write(json.dumps({"id": serial, "method": method, "params": params}) + "\n")
                proc.stdin.flush()
                while True:
                    msg = events.get(timeout=30)
                    if msg.get("id") == serial:
                        if "error" in msg:
                            raise RuntimeError(msg["error"])
                        return msg["result"]
            try:
                call("initialize", {"clientInfo": {"name": "catalog-probe", "version": "1"}, "capabilities": {"experimentalApi": True}})
                proc.stdin.write('{"method":"initialized"}\n')
                proc.stdin.flush()
                tid = call("thread/start", {"cwd": raw, "ephemeral": True, "model": "gpt-6-astra", "approvalPolicy": "on-request"})["thread"]["id"]
                outputs = []
                for index in range(len(scripts)):
                    if index == 1:
                        (root / "ready").touch()
                        time.sleep(.4)
                    call("turn/start", {"threadId": tid, "input": [{"type": "text", "text": "Run fixture", "text_elements": []}]})
                    while True:
                        msg = events.get(timeout=30)
                        if msg.get("method") == "mcpServer/elicitation/request":
                            assert msg["params"]["serverName"] == "catalog_probe", msg
                            assert msg["params"]["_meta"]["tool_params"] in ({"name": "install", "arguments": {}}, {"name": "after", "arguments": {}}), msg
                            print(json.dumps({"fixture_approval": msg["params"]}), flush=True)
                            proc.stdin.write(json.dumps({"id": msg["id"], "result": {"action": "accept", "content": {}, "_meta": None}}) + "\n")
                            proc.stdin.flush()
                            continue
                        if msg.get("method") == "turn/completed":
                            assert msg["params"]["turn"]["status"] == "completed", msg
                            break
                    result = [item for item in requests[-1].get("input", []) if item.get("type") == "custom_tool_call_output"][-1]
                    outputs.append(json.dumps(result))
                    if index:
                        tool_result = json.loads(result["output"][-1]["text"])
                        assert not tool_result.get("isError"), tool_result
                        if index == 3:
                            assert tool_result["content"][0]["text"] == "after", tool_result
                    print(json.dumps({"step": index, "result": result}), flush=True)
                assert "aos_call_tool" in outputs[0]
                assert "before" in outputs[1]
                assert "install" in outputs[2]
                assert "after" in outputs[3] and "aos_catalog_notice" in outputs[3]
                print("PASS: actual Codex tool execution reached a late-installed capability without reload/restart.")
            finally:
                proc.stdin.close()
                try:
                    proc.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    proc.terminate()
                    proc.wait(timeout=5)
                proc.stdout.close()
                http.shutdown()
                http.server_close()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "fixture":
        loader = importlib.machinery.SourceFileLoader("adapter", str(SOURCE))
        spec = importlib.util.spec_from_loader(loader.name, loader)
        module = importlib.util.module_from_spec(spec)
        loader.exec_module(module)
        os._exit(module.Connection([sys.executable, str(Path(sys.argv[2]) / "backend.py"), sys.argv[2]]).run())
    else:
        probe(sys.argv[1])
