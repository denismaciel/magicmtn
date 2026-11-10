import base64
import importlib.util
import json
import os
import pathlib
import sys
import subprocess
import tempfile
import threading
import time
import unittest
import uuid
from unittest.mock import patch

AGENT = pathlib.Path(__file__).resolve().parents[1] / "agent.py"
sys.path.insert(0, str(AGENT.parent))
spec = importlib.util.spec_from_file_location("sandbox_agent", AGENT)
agent = importlib.util.module_from_spec(spec)
spec.loader.exec_module(agent)


def wait_for(predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.025)
    raise AssertionError("condition was not reached")


class SupervisorTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.env = patch.dict(os.environ, {"HOME": self.directory.name})
        self.env.start()
        self.config = {"session_id": str(uuid.uuid4()), "origin": "http://localhost:8081", "token": "scoped-runtime-only", "workspace": self.directory.name, "model": {"id": "test-model"}, "cols": 80, "rows": 24}
        self.supervisor = agent.Supervisor(self.config)
        self.thread = None
        self.control = {"generation": 1, "desired_state": "running"}
        self.offline = True
        self.chunks = {}
        self.frames = []
        self.supervisor.request = self.request

    def request(self, path, body=None):
        if self.offline:
            raise OSError("simulated gateway outage")
        if path.endswith("/renew"):
            return {"expires_in": 86400}
        if path.endswith("/chunks"):
            key = (body["file_id"], body["offset"])
            data = base64.b64decode(body["content"])
            self.assertEqual(self.chunks.setdefault(key, data), data)
            return {"next_offset": body["offset"] + len(data)}
        self.frames.append(body)
        return {"offset": body["offset"] + len(base64.b64decode(body["output"])), "input": [], "cols": 80, "rows": 24, "revision": 1, "control": dict(self.control)}

    def run_supervisor(self, code):
        self.supervisor.command = [sys.executable, "-c", code]
        self.thread = threading.Thread(target=self.supervisor.run)
        self.thread.start()

    def tearDown(self):
        self.supervisor.closed.set()
        if self.thread:
            self.thread.join(timeout=8)
            self.assertFalse(self.thread.is_alive())
        else:
            self.supervisor.lock_file.close()
        self.env.stop()
        self.directory.cleanup()

    def test_startup_failure_reports_only_structured_state(self):
        self.offline = False
        with patch.object(self.supervisor, "start_child", side_effect=OSError("secret path")):
            with self.assertRaises(OSError):
                self.supervisor.run()
        self.assertEqual(len(self.frames), 1)
        self.assertEqual(self.frames[0]["exit_code"], -1000)
        self.assertEqual(self.frames[0]["output"], "")
        self.assertNotIn("secret path", json.dumps(self.frames[0]))

    def test_completed_relay_resumes_in_destination_and_preserves_transcript(self):
        destination = pathlib.Path(self.directory.name).resolve() / "copied project"
        destination.mkdir()
        status = pathlib.Path("/tmp") / f"magicmtn-relay-{self.supervisor.id}.json"
        self.addCleanup(lambda: status.unlink(missing_ok=True))
        self.supervisor.harness.sessions.mkdir()
        session = self.supervisor.harness.sessions / "session.jsonl"
        transcript = '{"type":"message","message":{"role":"user","content":"keep me"}}\n'
        session.write_text(json.dumps({"type": "session", "cwd": self.directory.name}) + "\n" + transcript)
        # Model Pi's graceful exit after the completion marker appears.
        self.run_supervisor("import pathlib,time,os; p=pathlib.Path('launched'); p.write_text(os.getcwd()); time.sleep(0.2)")
        wait_for(lambda: (pathlib.Path(self.directory.name) / "launched").exists())
        agent.atomic_json(status, {"state": "complete", "directory": str(destination)})
        wait_for(lambda: (destination / "launched").exists())
        self.assertEqual((destination / "launched").read_text(), str(destination))
        # The real Pi command also updates its saved cwd before resuming.
        command = self.supervisor.harness.command(str(destination))
        self.assertFalse(session.exists())
        session = next(self.supervisor.harness.sessions.glob("*.jsonl"))
        self.assertEqual(command[1:3], ["--session", str(session)])
        header, _, contents = session.read_text().partition("\n")
        self.assertEqual(json.loads(header)["cwd"], str(destination))
        self.assertEqual(contents, transcript)

    def test_ambiguous_saved_session_reports_resume_failure(self):
        self.offline = False
        self.supervisor.harness.sessions.mkdir(exist_ok=True)
        for name in ("first.jsonl", "second.jsonl"):
            (self.supervisor.harness.sessions / name).write_text("")
        with self.assertRaisesRegex(RuntimeError, "ambiguous Pi session"):
            self.supervisor.run()
        self.assertEqual(self.frames[0]["exit_code"], -1000)
        self.assertEqual(self.frames[0]["output"], "")

    def test_github_git_remotes_use_integration_gateway(self):
        self.supervisor.config["integrations_origin"] = "https://control.magicmtn.test"
        environment = self.supervisor.environment()
        for remote in (
            "https://github.com/acme/project.git",
            "git@github.com:acme/project.git",
            "ssh://git@github.com/acme/project.git",
        ):
            resolved = subprocess.check_output(
                ["git", "ls-remote", "--get-url", remote], env=environment, text=True
            ).strip()
            self.assertEqual(resolved, "https://control.magicmtn.test/acme/project.git")
        self.assertEqual(environment["GIT_CONFIG_VALUE_0"], "!magicmtn-git-credential")

    def test_integration_config_is_private_and_bound_to_runtime_identity(self):
        config = dict(self.config, session_id=str(uuid.uuid4()), integrations_origin="https://control.magicmtn.test")
        supervisor = agent.Supervisor(config)
        try:
            path = pathlib.Path(supervisor.environment()["MAGICMTN_INTEGRATIONS_CONFIG"])
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads(path.read_text())["token"], config["token"])
            self.assertEqual(json.loads(path.read_text())["origin"], config["integrations_origin"])
            extension = supervisor.directory / "pi" / "extensions" / "magicmtn-mcp" / "index.js"
            self.assertTrue(extension.exists())
            self.assertIn("mcp_catalog", extension.read_text())
        finally:
            supervisor.lock_file.close()

    def test_work_and_bounded_terminal_drain_survive_gateway_outage(self):
        self.run_supervisor("import pathlib,time;print('x'*2097152,flush=True);pathlib.Path('completed').write_text('done');time.sleep(30)")
        wait_for(lambda: pathlib.Path(self.directory.name, "completed").exists())
        # A remote PTY can accept the whole write before the reader thread has
        # drained it. Wait for the drain condition, not just the child's file.
        wait_for(lambda: self.supervisor.end > agent.LIMIT)
        self.assertIsNone(self.supervisor.child.poll())
        self.assertLessEqual(len(self.supervisor.output), agent.LIMIT)
        self.assertGreater(self.supervisor.end, agent.LIMIT)
        self.offline = False
        wait_for(lambda: bool(self.frames))
        self.assertGreater(self.frames[0]["offset"], 0)
        self.assertIsNone(self.supervisor.child.poll())

    def test_stop_and_explicit_restart_do_not_replay_launches(self):
        self.offline = False
        self.run_supervisor("import pathlib,time;p=pathlib.Path('launches');p.write_text(p.read_text()+'x' if p.exists() else 'x');time.sleep(30)")
        path = pathlib.Path(self.directory.name, "launches")
        wait_for(path.exists)
        self.control.update(generation=2, desired_state="stopped")
        wait_for(lambda: self.supervisor.state == "stopped")
        self.assertEqual(path.read_text(), "x")
        self.control.update(generation=3, desired_state="running")
        wait_for(lambda: path.read_text() == "xx")
        time.sleep(0.4)
        self.assertEqual(path.read_text(), "xx")

    def test_jsonl_sync_retries_from_durable_acknowledged_offsets(self):
        sessions = self.supervisor.directory / "pi" / "sessions"
        sessions.mkdir()
        (sessions / "conversation.jsonl").write_bytes(b'{"type":"message"}\n' * 5000)
        self.run_supervisor("import time;time.sleep(30)")
        time.sleep(0.2)
        self.assertFalse(self.chunks)
        self.offline = False
        wait_for(lambda: sum(len(v) for v in self.chunks.values()) == (sessions / "conversation.jsonl").stat().st_size)
        # The mock server records the chunk before returning its acknowledgement.
        # Wait for the uploader to persist that acknowledgement before checking it.
        offsets_path = self.supervisor.directory / "upload-offsets.json"
        expected_size = (sessions / "conversation.jsonl").stat().st_size
        wait_for(lambda: offsets_path.exists() and list(json.loads(offsets_path.read_text()).values()) == [expected_size])
        offsets = json.loads(offsets_path.read_text())
        self.assertEqual(list(offsets.values()), [expected_size])
        self.assertEqual(len(self.chunks), 2)

    def test_duplicate_supervisor_is_fenced_and_only_scoped_credentials_are_written(self):
        with self.assertRaises(BlockingIOError):
            agent.Supervisor(self.config)
        model = json.loads((self.supervisor.directory / "pi" / "models.json").read_text())
        self.assertEqual(model["providers"]["magicmtn"]["apiKey"], "scoped-runtime-only")
        self.assertEqual((self.supervisor.directory / "pi" / "models.json").stat().st_mode & 0o777, 0o600)

    @unittest.skipUnless(os.environ.get("MAGICMTN_TEST_PI"), "set MAGICMTN_TEST_PI to pinned Pi 1.2 executable")
    def test_real_pi_tool_loop_history_resume_and_async_sync(self):
        self.run_real_pi_loop()

    @unittest.skipUnless(os.environ.get("MAGICMTN_TEST_PI"), "set MAGICMTN_TEST_PI to pinned Pi 1.2 executable")
    def test_real_pi_native_selected_model_uses_gateway_and_retains_history(self):
        self.run_real_pi_loop(native=True)

    def run_real_pi_loop(self, native=False):
        legacy_path = AGENT.parents[1] / "server" / "runtime" / "tests" / "test_pi_agent.py"
        legacy_spec = importlib.util.spec_from_file_location("legacy_transport_fixture", legacy_path)
        legacy = importlib.util.module_from_spec(legacy_spec)
        legacy_spec.loader.exec_module(legacy)
        handler = legacy.Handler.do_POST
        supervisor = self.supervisor
        uploads = self.chunks
        class Handler(legacy.Handler):
            def do_POST(request):
                if request.path.endswith(("/renew", "/chunks")):
                    if request.headers.get("Authorization") != "Bearer test-scoped-token":
                        request.send_error(401)
                        return
                    body = json.loads(request.rfile.read(int(request.headers["Content-Length"])))
                    response = {"expires_in":86400}
                    if request.path.endswith("/chunks"):
                        content = base64.b64decode(body["content"])
                        uploads[(body["file_id"],body["offset"])] = content
                        response = {"next_offset":body["offset"]+len(content)}
                    request.send_response(200)
                    request.end_headers()
                    request.wfile.write(json.dumps(response).encode())
                else:
                    handler(request)
        # Reuse only the credential-free mock inference protocol from the
        # previous adapter tests. This runs the actual pinned Pi executable.
        original_handler = legacy.Handler
        legacy.Handler = Handler
        bridge = legacy.Bridge()
        legacy.Handler = original_handler
        bridge.real_pi = True
        self.config.update(origin=f"http://127.0.0.1:{bridge.server_port}", token="test-scoped-token", model={"id":"test-model","name":"Test","reasoning":False,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0},"contextWindow":32768,"maxTokens":4096})
        if native:
            self.config["native"] = True
            self.config["model"].update(api="openai-completions", provider="original-selected-provider", baseUrl="https://original-selected-upstream.invalid/v1")
        self.supervisor.configure()
        command = self.supervisor.harness.command
        self.supervisor.harness.command = lambda: [os.environ["MAGICMTN_TEST_PI"], *command()[1:], "--offline", "--no-context-files", "--no-extensions"]
        # Retain adapter command refresh on explicit restart.
        self.supervisor.command = ["pi"]
        self.supervisor.request = agent.Supervisor.request.__get__(self.supervisor)
        self.thread = threading.Thread(target=self.supervisor.run)
        self.thread.start()
        try:
            wait_for(lambda: bridge.finished.is_set(), 35)
            self.assertEqual(pathlib.Path(self.directory.name,"evidence.txt").read_text(), "VM evidence")
            self.assertTrue(any(m["role"] == "tool" for m in bridge.requests[1]["messages"]))
            wait_for(lambda: supervisor.state == "exited", 15)
            self.assertTrue(self.thread.is_alive(), "Pi exiting must not terminate supervisor")
            wait_for(lambda: bool(uploads), 8)
            history = list(self.supervisor.harness.artifacts())
            self.assertEqual(len(history), 1)
            resumed = self.supervisor.harness.command()
            self.assertIn("--session", resumed)
            self.assertIn(str(history[0]), resumed)
            self.supervisor.start_child()
            wait_for(lambda: self.supervisor.child.poll() is None)
            self.assertEqual(list(self.supervisor.harness.artifacts()), history)
        except AssertionError as error:
            raise AssertionError(str(error) + "\nterminal=" + bridge.output.decode(errors="replace")[-6000:]) from error
        finally:
            self.supervisor.closed.set()
            self.thread.join(timeout=8)
            bridge.shutdown()
            bridge.server_close()


if __name__ == "__main__":
    unittest.main()
