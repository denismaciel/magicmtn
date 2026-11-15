"""A shell launch and a supervised Pi launch must see the same sandbox."""
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import unittest
import uuid
from unittest.mock import patch

RUNTIME = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(RUNTIME))
import agent
import environment


class EnvironmentTests(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.addCleanup(self.home.cleanup)
        patcher = patch.dict(os.environ, HOME=self.home.name)
        patcher.start()
        self.addCleanup(patcher.stop)

    def supervisor(self, **options):
        config = {
            "session_id": str(uuid.uuid4()), "origin": "https://gateway.example.test",
            "integrations_origin": "https://integrations.example.test", "token": "scoped-test-token",
            "workspace": self.home.name, "workspace_info": {"id": str(uuid.uuid4())},
            "workspace_extension": (RUNTIME.parent / "interfaces/pi/workspace.js").read_text(),
            "model": {"id": "test-model", "name": "Test Model", "reasoning": False,
                      "input": ["text"], "contextWindow": 32768, "maxTokens": 4096,
                      "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}},
        }
        config.update(options)
        supervisor = agent.Supervisor(config)
        self.addCleanup(supervisor.lock_file.close)
        return supervisor

    def test_shell_and_agent_share_configuration_without_copying_host_credentials(self):
        supervisor = self.supervisor(ca_certificate="test CA")
        with patch.dict(os.environ, GH_TOKEN="host-secret", GITHUB_TOKEN="other-secret"):
            managed = supervisor.environment()
            shell = json.loads(subprocess.check_output(
                [sys.executable, str(RUNTIME / "launch.py"), sys.executable, "-c",
                 "import json,os; print(json.dumps(dict(os.environ)))"], text=True))
        for key, value in supervisor.runtime_environment["set"].items():
            self.assertEqual(managed[key], value)
            self.assertEqual(shell[key], value)
        for key in ("GH_TOKEN", "GITHUB_TOKEN"):
            self.assertNotIn(key, managed)
            self.assertNotIn(key, shell)
        self.assertNotIn("host-secret", json.dumps(supervisor.runtime_environment))
        config_dir = pathlib.Path(shell["PI_CODING_AGENT_DIR"])
        self.assertEqual(json.loads((config_dir / "settings.json").read_text()),
                         {"defaultProvider": "magicmtn", "defaultModel": "test-model"})
        self.assertTrue((config_dir / "extensions/magicmtn-mcp/index.js").is_file())
        self.assertTrue((config_dir / "extensions/magicmtn-workspace/index.js").is_file())
        context = config_dir / "extensions/magicmtn-context"
        self.assertEqual((context / "context.md").read_text(), (RUNTIME / "context.md").read_text())
        self.assertTrue((context / "index.js").is_file())
        reporter = config_dir / "extensions/magicmtn-errors"
        self.assertTrue((reporter / "index.js").is_file())
        self.assertTrue((reporter / "error-worker.js").is_file())
        self.assertEqual(json.loads((reporter / "config.json").read_text()), {
            "origin": "https://gateway.example.test", "token": "scoped-test-token",
            "ca_certificate": "test CA",
        })
        self.assertEqual((reporter / "config.json").stat().st_mode & 0o777, 0o600)
        self.assertFalse(any("ERROR_REPORTING" in key or "SENTRY" in key for key in managed))
        self.assertEqual(json.loads(pathlib.Path(shell["MAGICMTN_INTEGRATIONS_CONFIG"]).read_text())["token"],
                         "scoped-test-token")
        self.assertEqual(pathlib.Path(shell["NODE_EXTRA_CA_CERTS"]).read_text(), "test CA")
        for values in (managed, shell):
            self.assertEqual(subprocess.check_output(
                ["git", "ls-remote", "--get-url", "git@github.com:acme/project.git"],
                env=values, text=True).strip(), "https://integrations.example.test/acme/project.git")
        self.assertEqual((pathlib.Path(self.home.name) / ".magicmtn/environment.json").stat().st_mode & 0o777, 0o600)

    def test_reconfiguration_publishes_current_settings_without_stale_integrations(self):
        first = self.supervisor(ca_certificate="old CA")
        first.lock_file.close()
        second = self.supervisor(session_id=first.id, integrations_origin=None, github_enabled=False)
        with patch.dict(os.environ, first.runtime_environment["set"]):
            loaded = environment.load()
        self.assertEqual(loaded["PI_CODING_AGENT_DIR"], str(second.harness.directory))
        for key in ("MAGICMTN_INTEGRATIONS_CONFIG", "NODE_EXTRA_CA_CERTS", "GIT_CONFIG_GLOBAL"):
            self.assertNotIn(key, second.runtime_environment["set"])
            self.assertNotIn(key, loaded)

    @unittest.skipUnless(os.environ.get("MAGICMTN_TEST_PI"), "set MAGICMTN_TEST_PI to the pinned Pi executable")
    def test_real_pi_uses_identical_inference_and_mcp_tools_from_both_launches(self):
        calls = []
        mcp_calls = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_GET(self):
                mcp_calls.append((self.path, self.headers.get("Authorization")))
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'[]')

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                calls.append((self.path, self.headers.get("Authorization"), body))
                if body["messages"][-1]["role"] != "tool":
                    delta = {"role": "assistant", "content": "", "tool_calls": [{"index": 0,
                        "id": "call_catalog", "type": "function", "function": {
                            "name": "mcp_catalog", "arguments": "{}"}}]}
                    finish = "tool_calls"
                else:
                    delta = {"role": "assistant", "content": "Sandbox parity verified."}
                    finish = "stop"
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                for change, reason in ((delta, None), ({}, finish)):
                    chunk = {"id": "test", "object": "chat.completion.chunk", "created": 0,
                             "model": body["model"], "choices": [{"index": 0, "delta": change,
                             "finish_reason": reason}]}
                    self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                self.wfile.write(b"data: [DONE]\n\n")

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        origin = f"http://127.0.0.1:{server.server_port}"
        supervisor = self.supervisor(origin=origin, integrations_origin=origin)
        binary = str(pathlib.Path(os.environ["MAGICMTN_TEST_PI"]).resolve())
        managed = supervisor.harness.command()
        managed[0] = binary
        for command, values in (
            (managed, supervisor.environment()),
            ([sys.executable, str(RUNTIME / "launch.py"), binary, "--approve"], dict(os.environ)),
        ):
            result = subprocess.run(command + ["--mode", "json", "--offline", "-p", "List MCP services."],
                                    cwd=self.home.name, env=values, stdin=subprocess.DEVNULL,
                                    capture_output=True, text=True, timeout=40)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("Sandbox parity verified.", result.stdout)
            self.assertNotIn("Failed to load extension", result.stderr)
        self.assertEqual(len(calls), 4)
        for path, authorization, body in calls:
            self.assertEqual(path, "/v1/harness/agent/chat/completions")
            self.assertEqual(authorization, "Bearer scoped-test-token")
            self.assertEqual(body["model"], "test-model")
            system = "\n".join(message["content"] for message in body["messages"]
                               if message["role"] == "system")
            self.assertIn((RUNTIME / "context.md").read_text(), system)
            self.assertIn("You are a coding assistant", system)
            tools = {tool["function"]["name"] for tool in body["tools"]}
            self.assertTrue({"mcp_catalog", "mcp_call"} <= tools)
        self.assertEqual(mcp_calls, [("/v1/mcp/runtime", "Bearer scoped-test-token")] * 2)


if __name__ == "__main__":
    unittest.main()
