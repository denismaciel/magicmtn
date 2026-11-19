"""Exercise the installed OpenCode plugin and managed runtime with a loopback model."""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

RUNTIME = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(RUNTIME))
from agent import atomic_json
from harnesses.opencode import OpenCodeHarness


class ModelHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.server.broker_calls.append((self.path, self.headers.get("authorization")))
        payload = b'[{"id":"example","name":"Example","tools":[]}]'
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.calls.append((self.path, self.headers.get("authorization"), body))
        chunk = {"id": "smoke", "object": "chat.completion.chunk", "created": 0, "model": body["model"]}

        def event(delta, finish=None):
            value = chunk | {"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            return f"data: {json.dumps(value)}\n\n"

        tools = [tool["function"]["name"] for tool in body.get("tools", []) if "function" in tool]
        last_role = body["messages"][-1]["role"]
        if "mcp_catalog" in tools and last_role == "user":
            content = event({
                "role": "assistant",
                "tool_calls": [{
                    "index": 0, "id": "mcp-test", "type": "function",
                    "function": {"name": "mcp_catalog", "arguments": "{}"},
                }],
            })
            content += event({}, "tool_calls")
        else:
            content = event({"role": "assistant", "content": "magicmtn-smoke-ok"})
            content += event({}, "stop")
        content += "data: [DONE]\n\n"
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(content)))
        self.end_headers()
        self.wfile.write(content.encode())

    def log_message(self, *_):
        pass


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_OPENCODE"), "requires the OpenCode CLI")
class OpenCodeTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory(prefix="magicmtn-opencode-test-")
        self.addCleanup(directory.cleanup)
        self.directory = pathlib.Path(directory.name).resolve()
        self.workspace = self.directory / "workspace"
        self.workspace.mkdir()
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), ModelHandler)
        self.server.calls = []
        self.server.broker_calls = []
        worker = threading.Thread(target=self.server.serve_forever, daemon=True)
        worker.start()
        self.addCleanup(worker.join, 5)
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.origin = f"http://127.0.0.1:{self.server.server_port}"

    def command(self, *args, environment):
        result = subprocess.run(
            [os.environ["MAGICMTN_TEST_OPENCODE"], *args, "--standalone"],
            env={**os.environ, "HOME": str(self.directory), "PWD": str(self.workspace), **environment},
            cwd=self.workspace, capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return result.stdout

    def test_managed_inference_and_session_resume(self):
        harness = OpenCodeHarness(self.directory, {
            "origin": self.origin, "token": "scoped-runtime", "workspace": str(self.workspace),
            "model": {"id": "managed-model", "name": "Managed Model"}, "error_reporting": False,
        }, str(uuid.uuid4()), atomic_json)
        broker_config = self.directory / "broker.json"
        atomic_json(broker_config, {"origin": self.origin, "token": "scoped-broker"})
        environment = {**harness.environment(), "MAGICMTN_INTEGRATIONS_CONFIG": str(broker_config)}
        for prompt in ["first managed prompt", "follow-up after restart"]:
            reply = self.command("run", "--session", harness.session_id, "--format", "json", prompt,
                                 environment=environment)
            self.assertIn("magicmtn-smoke-ok", reply)
        self.assertEqual({(path, auth, body["model"]) for path, auth, body in self.server.calls},
                         {("/v1/harness/agent/chat/completions", "Bearer scoped-runtime", "managed-model")})
        self.assertEqual(self.server.broker_calls,
                         [("/v1/mcp/runtime", "Bearer scoped-broker")] * 2)
        exported = self.command("session", "export", harness.session_id, environment=environment)
        self.assertIn("first managed prompt", exported)
        self.assertIn("follow-up after restart", exported)
        journal, = tuple(harness.artifacts())
        events = [json.loads(line) for line in journal.read_text().splitlines()]
        self.assertTrue(any(event["type"] == "session.text.ended" for event in events))

    def test_host_plugin_routes_magicmtn_inference(self):
        environment = {f"XDG_{key}_HOME": str(self.directory / key.lower())
                       for key in ("CONFIG", "DATA", "STATE", "CACHE")}
        plugin = pathlib.Path(environment["XDG_CONFIG_HOME"]) / "opencode/plugins/magicmtn"
        plugin.mkdir(parents=True)
        source = RUNTIME.parent / "interfaces/opencode"
        for name in ("index.js", "tui.js", "magic.js", "package.json"):
            (plugin / name).write_bytes((source / name).read_bytes())
        fake = self.directory / "magic"
        grant = {
            "api": "openai-completions", "apiKey": "scoped-host",
            "baseUrl": self.origin + "/v1/harness/agent",
            "models": [{"id": "host-model"}], "expiresIn": 60,
        }
        responses = {
            ("opencode", "account"): {"signed_in": True},
            ("opencode", "catalog"): {"models": [{"id": "host-model", "display_name": "Host Model"}]},
            ("opencode", "inference", "--model", "host-model"): grant,
        }
        fake.write_text(
            "#!/usr/bin/env python3\nimport json,sys\n"
            f"print(json.dumps({responses!r}[tuple(sys.argv[1:])]))\n"
        )
        fake.chmod(0o700)
        (plugin / "config.js").write_text(f"export const executable = {json.dumps(str(fake))};\n")
        reply = self.command("run", "--model", "magicmtn/host-model", "--format", "json", "host prompt",
                             environment=environment)
        self.assertIn("magicmtn-smoke-ok", reply)
        self.assertEqual({(path, auth, body["model"]) for path, auth, body in self.server.calls},
                         {("/v1/harness/agent/chat/completions", "Bearer scoped-host", "host-model")})
