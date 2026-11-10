"""Exercise relay through a real CLI attachment and the sandbox SCP transport."""
import base64
import hashlib
import http.server
import json
import os
import pathlib
import pty
import select
import subprocess
import tempfile
import threading
import time
import unittest
import uuid

from test_pi_terminal import Handler


class RelayHandler(Handler):
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"id": self.server.identity}).encode())

    def do_POST(self):
        if not self.path.endswith("/exec"):
            return super().do_POST()
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        self.server.exec_bodies.append(body)
        payload = json.loads(base64.b64decode("".join(body["args"][2:])))
        if '"action":"write"' in "".join(payload["argv"]):
            self.server.upload_started.set()
            self.server.allow_upload.wait(15)
            if self.server.fail_transfer:
                self.respond({"stdout": "", "stderr": "simulated upload failure", "exit_code": 1,
                              "timed_out": False, "truncated": False, "elapsed_ms": 0})
                return
        output = subprocess.run(["python3", *body["args"]], cwd=self.server.guest, capture_output=True)
        self.respond({"stdout": output.stdout.decode(), "stderr": output.stderr.decode(),
                      "exit_code": output.returncode, "timed_out": False,
                      "truncated": False, "elapsed_ms": 0})

    def respond(self, value):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(value).encode())


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_MAGIC"), "set MAGICMTN_TEST_MAGIC to a built magic executable")
class RelayTest(unittest.TestCase):
    def test_attach_and_detach_do_not_wait_for_scp(self):
        self.run_relay(False)

    def test_upload_failure_is_visible_and_partial_data_is_cleaned(self):
        self.run_relay(True)

    def run_relay(self, fail_transfer):
        binary = str(pathlib.Path(os.environ["MAGICMTN_TEST_MAGIC"]).resolve())
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            local, guest, home = (root / name for name in ("local", "guest", "home"))
            for directory in (local, guest, home):
                directory.mkdir()
            subprocess.run(["git", "init", "-q", str(local)], check=True)
            (local / "tracked").write_text("staged contents")
            subprocess.run(["git", "-C", str(local), "add", "."], check=True)
            (local / ".jj/repo/store").mkdir(parents=True)
            (local / ".jj/repo/store/type").write_text("git")
            (local / "tracked").write_text("current modified contents")
            (local / "untracked").write_text("do not upload")
            identity = str(uuid.uuid4())
            status = pathlib.Path("/tmp") / f"magicmtn-relay-{identity}.json"
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), RelayHandler)
            server.daemon_threads = True
            server.identity, server.guest = identity, guest
            server.paths, server.frames, server.exec_bodies = [], [], []
            server.received = bytearray()
            server.upload_started, server.allow_upload = threading.Event(), threading.Event()
            server.fail_transfer = fail_transfer
            threading.Thread(target=server.serve_forever, daemon=True).start()
            origin = f"http://127.0.0.1:{server.server_port}/"
            credentials = home / ".config/magicmtn/cli"
            credentials.mkdir(parents=True)
            (credentials / (hashlib.sha256(origin.encode()).hexdigest() + ".json")).write_text(
                json.dumps({"server": origin, "token": "test-cli-session"}))
            master, slave = pty.openpty()
            process = subprocess.Popen([binary, "--server-url", origin, "pi", "attach", identity,
                                        "--relay-from", str(local)], stdin=slave, stdout=slave,
                                       stderr=slave, env=dict(os.environ, HOME=str(home)))
            output = bytearray()
            try:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    ready, _, _ = select.select([master], [], [], 0.1)
                    if ready:
                        output.extend(os.read(master, 8192))
                    if b"remote terminal ready" in output and server.upload_started.is_set():
                        break
                self.assertIn(b"remote terminal ready", output)
                self.assertTrue(server.upload_started.is_set(), output.decode(errors="replace"))
                self.assertEqual(json.loads(status.read_text())["state"], "copying")
                self.assertFalse((guest / "tracked").exists())
                # Detach while the SCP write is still blocked on the fixture.
                os.write(master, b"\x1d")
                self.assertEqual(process.wait(timeout=3), 0)
                server.allow_upload.set()
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    if fail_transfer:
                        if status.exists() and json.loads(status.read_text())["state"] == "failed":
                            break
                    elif status.exists() and json.loads(status.read_text())["state"] == "complete":
                        break
                    time.sleep(0.05)
                if fail_transfer:
                    self.assertEqual(json.loads(status.read_text())["state"], "failed")
                    self.assertEqual(list(guest.iterdir()), [])
                else:
                    self.assertEqual((guest / "tracked").read_text(), "current modified contents")
                    self.assertEqual({entry.name for entry in guest.iterdir()}, {"tracked", ".git", ".jj"})
                    self.assertEqual((guest / ".jj/repo/store/type").read_text(), "git")
                    self.assertEqual(
                        subprocess.check_output(["git", "-C", str(guest), "diff", "--", "tracked"]),
                        subprocess.check_output(["git", "-C", str(local), "diff", "--", "tracked"]),
                    )
                    self.assertEqual(json.loads(status.read_text()), {"state": "complete", "directory": str(guest.resolve())})
                self.assertTrue(server.exec_bodies)
            finally:
                server.allow_upload.set()
                if process.poll() is None:
                    process.kill()
                    process.wait()
                os.close(master)
                os.close(slave)
                server.shutdown()
                server.server_close()
                status.unlink(missing_ok=True)
