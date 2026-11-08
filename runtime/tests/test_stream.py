"""Real WebSocket/PTY tests; no credentials, inference, or cloud services."""
import base64
import json
import os
import pathlib
import sys
import tempfile
import threading
import time
import unittest
import uuid
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from agent import Supervisor
try:
    from websockets.sync.server import serve
    from websockets.exceptions import ConnectionClosed
except ImportError:
    serve = None


def wait_for(predicate, timeout=5):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.005)
    raise AssertionError("condition was not reached")


@unittest.skipUnless(serve, "install runtime/requirements.txt for WebSocket tests")
class StreamTests(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.TemporaryDirectory()
        self.env = patch.dict(os.environ, HOME=self.home.name)
        self.env.start()
        self.offset = 0
        self.output = bytearray()
        self.input = []
        self.ack = 0
        self.frames = []
        self.connections = []
        self.lock = threading.Lock()
        self.drop_next = False
        self.control = {"generation": 1, "desired_state": "running"}
        self.size = (80, 24)
        self.revision = 0
        self.last_input = 0
        self.server = serve(self.handle, "127.0.0.1", 0)
        self.port = self.server.socket.getsockname()[1]
        self.server_thread = threading.Thread(target=self.server.serve_forever)
        self.server_thread.start()
        self.supervisor = Supervisor({
            "session_id": str(uuid.uuid4()), "origin": f"http://127.0.0.1:{self.port}",
            "token": "test-only", "workspace": self.home.name,
            "model": {"id": "test"}, "terminal_stream": True,
        })
        self.supervisor.request = lambda *args: {"expires_in": 86400}
        self.supervisor.command = [sys.executable, "-c", "import os,tty;tty.setraw(0);os.write(1,b'READY');\nwhile True: os.write(1,os.read(0,4096))"]
        self.thread = threading.Thread(target=self.supervisor.run)
        self.thread.start()
        wait_for(lambda: b"READY" in self.output)

    def tearDown(self):
        self.supervisor.closed.set()
        self.supervisor.output_ready.set()
        self.thread.join(timeout=8)
        self.assertFalse(self.thread.is_alive())
        for connection in self.connections:
            connection.close()
        self.server.shutdown()
        self.server_thread.join(timeout=3)
        self.env.stop()
        self.home.cleanup()

    def response(self):
        with self.lock:
            return {"offset": self.offset, "input": [frame for frame in self.input if frame["seq"] > self.ack],
                    "cols": self.size[0], "rows": self.size[1], "revision": self.revision,
                    "control": dict(self.control)}

    def handle(self, connection):
        self.assertEqual(connection.request.headers["Authorization"], "Bearer test-only")
        self.assertEqual(connection.request.path, "/v1/harness/agent/stream")
        self.connections.append(connection)
        try:
            for text in connection:
                frame = json.loads(text)
                data = base64.b64decode(frame["output"])
                with self.lock:
                    self.frames.append(frame)
                    if frame["offset"] == self.offset:
                        self.offset += len(data)
                        self.output.extend(data)
                    else:
                        self.assertEqual(frame["offset"] + len(data), self.offset)
                    self.ack = frame["input_ack"]
                    drop = self.drop_next and bool(data)
                    if drop:
                        self.drop_next = False
                if drop:
                    # The gateway accepted output, but its acknowledgement was
                    # lost. A reconnect must resend the identical frame once.
                    connection.close(code=1012)
                    return
                connection.send(json.dumps(self.response()))
        except ConnectionClosed:
            pass

    def push(self, data):
        with self.lock:
            self.last_input += 1
            self.input.append({"seq": self.last_input, "data": base64.b64encode(data).decode()})
        self.connections[-1].send(json.dumps(self.response()))

    def test_idle_input_is_pushed_and_echoed_without_another_poll(self):
        # No periodic traffic is required to deliver the first key after idle.
        time.sleep(0.05)
        before = len(self.frames)
        self.push(b"typed")
        wait_for(lambda: b"typed" in self.output)
        self.assertGreater(len(self.frames), before)
        self.assertEqual(len(self.connections), 1)
        self.assertEqual(self.supervisor.input_ack, 1)

    def test_lost_output_ack_reconnects_without_restarting_or_duplicating(self):
        child = self.supervisor.child
        self.drop_next = True
        self.push(b"first")
        wait_for(lambda: len(self.connections) >= 2)
        wait_for(lambda: self.supervisor.errors["transport"] > 0 or self.ack == 1)
        # Allow the replay acknowledgement to reach the sender, then new input.
        wait_for(lambda: sum(base64.b64decode(frame["output"]) == b"first" for frame in self.frames) >= 2)
        self.push(b"second")
        wait_for(lambda: b"firstsecond" in self.output)
        self.assertEqual(self.output.count(b"first"), 1)
        self.assertIs(self.supervisor.child, child)
        self.assertIsNone(child.poll())

    def test_partial_pty_writes_retry_the_suffix_without_replaying_prefix(self):
        real_write = os.write
        attempts = []

        def limited_write(fd, data):
            if fd == self.supervisor.master:
                attempts.append(bytes(data))
                if len(attempts) == 1:
                    raise BlockingIOError()
                if len(attempts) == 2:
                    return real_write(fd, data[:2])
            return real_write(fd, data)

        with patch("agent.os.write", side_effect=limited_write):
            self.push(b"abcdef")
            wait_for(lambda: b"abcdef" in self.output)
            wait_for(lambda: self.ack == 1)
        self.assertEqual(self.output.count(b"abcdef"), 1)
        self.assertEqual(attempts[:3], [b"abcdef", b"abcdef", b"cdef"])

    def test_pushed_resize_and_stop_restart_preserve_supervisor(self):
        self.size = (100, 32)
        self.revision += 1
        self.connections[-1].send(json.dumps(self.response()))
        wait_for(lambda: self.supervisor.size == (100, 32))
        child = self.supervisor.child
        self.control = {"generation": 2, "desired_state": "stopped"}
        self.connections[-1].send(json.dumps(self.response()))
        wait_for(lambda: self.supervisor.state == "stopped")
        self.assertIsNotNone(child.poll())
        self.control = {"generation": 3, "desired_state": "running"}
        self.connections[-1].send(json.dumps(self.response()))
        wait_for(lambda: self.supervisor.child is not child)
        self.assertTrue(self.thread.is_alive())
