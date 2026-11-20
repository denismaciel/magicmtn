"""Credential-free CLI terminal smoke test against a local HTTP fixture.

Build magic, then MAGICMTN_TEST_MAGIC=target/debug/magic python3 -m unittest
 discover -s cli/tests -v
"""
import base64
import fcntl
import hashlib
import http.server
import json
import os
import pathlib
import pty
import select
import struct
import subprocess
import tempfile
import termios
import threading
import time
import unittest


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        if self.headers.get("Authorization") != "Bearer test-cli-session":
            self.send_error(401)
            return
        self.server.paths.append(self.path)
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path.endswith("/terminal"):
            response = {"client_id": "a748c687-ddb5-44d2-a0b8-330a67aff13a", "cursor": 0}
        else:
            self.server.frames.append(body)
            self.server.received.extend(base64.b64decode(body["input"]))
            data = b"remote terminal ready\r\n" if body["cursor"] == 0 else b""
            response = {"output": base64.b64encode(data).decode(), "cursor": body["cursor"] + len(data), "lost": False, "exit_code": None}
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(response).encode())


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_MAGIC"), "set MAGICMTN_TEST_MAGIC to a built magic executable")
class TerminalTest(unittest.TestCase):
    def test_raw_terminal_input_resize_detach_and_restore(self):
        self.run_terminal(["pi", "attach", "2240676a-44f4-4d55-8790-f659af72074a"], "/v1/pi/workspaces/2240676a-44f4-4d55-8790-f659af72074a")

    def test_sandbox_ssh_input_resize_detach_and_restore(self):
        self.run_terminal(["sandbox", "ssh", "2240676a"], "/v1/sandboxes/2240676a")

    def run_terminal(self, command, expected_path):
        binary = str(pathlib.Path(os.environ["MAGICMTN_TEST_MAGIC"]).resolve())
        with tempfile.TemporaryDirectory() as root:
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            server.frames = []
            server.paths = []
            server.received = bytearray()
            server.daemon_threads = True
            threading.Thread(target=server.serve_forever, daemon=True).start()
            origin = f"http://127.0.0.1:{server.server_port}/"
            credentials = pathlib.Path(root) / ".config/magicmtn/cli"
            credentials.mkdir(parents=True)
            path = credentials / (hashlib.sha256(origin.encode()).hexdigest() + ".json")
            path.write_text(json.dumps({"server": origin, "token": "test-cli-session"}))
            master, slave = pty.openpty()
            previous = termios.tcgetattr(slave)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            process = subprocess.Popen([binary, "--server-url", origin, *command], stdin=slave, stdout=slave, stderr=slave, env=dict(os.environ, HOME=root))
            output = bytearray()
            try:
                deadline = time.monotonic() + 10
                sent = False
                detached = False
                while process.poll() is None and time.monotonic() < deadline:
                    ready, _, _ = select.select([master], [], [], 0.1)
                    if ready:
                        output.extend(os.read(master, 8192))
                    if b"remote terminal ready" in output and not sent:
                        os.write(master, "typed 雪\r".encode())
                        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 100, 0, 0))
                        sent = True
                    if "typed 雪\r".encode() in server.received and any(f["cols"] == 100 for f in server.frames) and not detached:
                        # Modern Kitty encoding, not just a legacy control byte.
                        os.write(master, b"\x1b[93;5u")
                        detached = True
                self.assertEqual(process.wait(timeout=2), 0, output.decode(errors="replace"))
                self.assertEqual(set(server.paths), {expected_path + "/terminal", expected_path + "/terminal/poll"})
                self.assertTrue(sent and detached)
                self.assertTrue(any(f["cols"] == 100 and f["rows"] == 32 for f in server.frames))
                self.assertNotIn(b"\x1b[93;5u", server.received)
                self.assertEqual(termios.tcgetattr(slave), previous)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                os.close(master)
                os.close(slave)
                server.shutdown()
                server.server_close()


if __name__ == "__main__":
    unittest.main()
