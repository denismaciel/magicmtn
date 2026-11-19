#!/usr/bin/env python3
"""Harness supervisor. Network attachment and sync never own the child lifetime.

Only scoped runtime identities enter this supervisor. Long-lived upstream secrets
remain in trusted services, and integration requests go through the GitHub
gateway. Packaged adapters configure Pi and OpenCode.
"""
import base64
import collections
import fcntl
import hashlib
import json
import os
import pathlib
import pty
import queue
import select
import signal
import ssl
import struct
import subprocess
import sys
import termios
import threading
import time
import urllib.error
import urllib.request
import uuid
from harnesses.opencode import OpenCodeHarness
from harnesses.pi import PiHarness
import environment as sandbox_environment

LIMIT = 1024 * 1024
CHUNK = 24 * 1024
START_FAILURE_EXIT_CODE = -1000


class Supervisor:
    def __init__(self, config):
        self.config = config
        self.id = str(uuid.UUID(config["session_id"]))
        self.directory = pathlib.Path.home() / ".magicmtn" / "sessions" / self.id
        self.directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.lock_file = open(self.directory / "supervisor.lock", "a")
        try:
            fcntl.flock(self.lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError:
            self.lock_file.close()
            raise
        self.lock = threading.Lock()
        self.output = bytearray()
        self.base = self.end = self.input_ack = 0
        self.generation = 1
        self.revision = 0
        self.size = (config.get("cols", 80), config.get("rows", 24))
        self.child = self.master = None
        self.exit_code = None
        self.state = "starting"
        self.desired = "running"
        self.closed = threading.Event()
        self.output_ready = threading.Event()
        self.errors = collections.Counter()
        self.harness = self.configure()
        self.command = None
        self.runtime_environment = sandbox_environment.configure(
            self.directory, self.config, self.harness.environment(), atomic_json)

    def configure(self):
        harness = self.config.get("harness", "pi")
        adapter = {"pi": PiHarness, "opencode": OpenCodeHarness}.get(harness)
        if adapter is None:
            raise ValueError("harness adapter is not installed in this image")
        return adapter(self.directory, self.config, self.id, atomic_json)

    def environment(self):
        environment = sandbox_environment.apply(self.runtime_environment)
        environment.update(TERM="xterm-256color", COLORTERM="truecolor")
        return environment

    def start_child(self):
        with self.lock:
            if self.child is not None and self.child.poll() is None:
                return
            command = self.command or self.harness.command()
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", self.size[1], self.size[0], 0, 0))
            # Avoid Python preexec_fn after threads have started. The tiny
            # session-leader wrapper acquires the controlling PTY before exec.
            wrapper = [sys.executable, "-c", "import fcntl,os,sys,termios;fcntl.ioctl(0,termios.TIOCSCTTY,0);os.execvp(sys.argv[1],sys.argv[1:])"]
            workspace = self.config.get("workspace", os.path.realpath("/workspace"))
            self.child = subprocess.Popen(wrapper + command, stdin=slave, stdout=slave, stderr=slave,
                                          cwd=workspace,
                                          env=dict(self.environment(), PWD=os.path.abspath(workspace)),
                                          start_new_session=True, close_fds=True)
            os.close(slave)
            os.set_blocking(master, False)
            self.master = master
            self.exit_code = None
            self.state = "running"
        threading.Thread(target=self.read_output, args=(master, self.child), daemon=True).start()

    def read_output(self, master, child):
        try:
            while not self.closed.is_set():
                if not select.select([master], [], [], 0.1)[0]:
                    if child.poll() is not None:
                        break
                    continue
                try:
                    data = os.read(master, CHUNK)
                except BlockingIOError:
                    continue
                except OSError:
                    break
                if not data:
                    break
                with self.lock:
                    self.output.extend(data)
                    self.end += len(data)
                    if len(self.output) > LIMIT:
                        remove = len(self.output) - LIMIT
                        del self.output[:remove]
                        self.base += remove
                self.output_ready.set()
        finally:
            code = child.wait()
            with self.lock:
                if self.child is child:
                    self.exit_code = code
                    self.state = "stopped" if self.desired == "stopped" else "exited"
                    self.master = None
            self.output_ready.set()
            os.close(master)

    def request(self, path, body=None):
        payload = json.dumps(body or {}).encode()
        request = urllib.request.Request(self.config["origin"] + path, payload,
            {"Authorization": "Bearer " + self.config["token"], "Content-Type": "application/json"}, method="POST")
        context = ssl.create_default_context()
        if self.config.get("ca_certificate"):
            context.load_verify_locations(cadata=self.config["ca_certificate"])
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, *args):
                return None
        with urllib.request.build_opener(NoRedirect, urllib.request.HTTPSHandler(context=context)).open(request, timeout=10) as response:
            return json.load(response)

    def write_pending_input(self, pending_input):
        # Called with self.lock held. PTYs are nonblocking: retain only the
        # unwritten suffix and acknowledge the frame after its final byte.
        seq, data = pending_input
        try:
            data = data[os.write(self.master, data):]
        except BlockingIOError:
            pass
        if data:
            return seq, data
        self.input_ack = seq
        return None

    def apply_response(self, result, pending_input):
        control = result.get("control", {})
        with self.lock:
            size = (result["cols"], result["rows"])
            if self.master is not None:
                if size != self.size or result.get("revision") != self.revision:
                    self.size = size
                    self.revision = result.get("revision")
                    fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", size[1], size[0], 0, 0))
                    os.killpg(self.child.pid, signal.SIGWINCH)
                for entry in result.get("input", []):
                    if entry["seq"] <= self.input_ack:
                        continue
                    if entry["seq"] != self.input_ack + 1:
                        raise ValueError("terminal input sequence mismatch")
                    if pending_input is None:
                        pending_input = (entry["seq"], base64.b64decode(entry["data"], validate=True))
                    pending_input = self.write_pending_input(pending_input)
                    if pending_input is not None:
                        break
        next_generation = control.get("generation", self.generation)
        if next_generation != self.generation:
            self.desired = control.get("desired_state", "stopped")
            self.generation = next_generation
            if self.desired == "stopped":
                self.stop_child()
            else:
                # Explicit restart; never replay a creation or tool call.
                self.stop_child()
                pending_input = None
                try:
                    self.start_child()
                except (OSError, ValueError, RuntimeError):
                    with self.lock:
                        self.child = None
                        self.master = None
                        self.exit_code = START_FAILURE_EXIT_CODE
                        self.state = "exited"
                    self.output_ready.set()
                    raise
        return pending_input

    def poll_transport(self):
        acknowledged = 0
        renewed = 0
        pending_input = None
        while not self.closed.is_set():
            self.output_ready.clear()
            with self.lock:
                offset = max(acknowledged, self.base)
                data = bytes(self.output[offset-self.base:offset-self.base+CHUNK])
                frame = {"offset": offset, "output": base64.b64encode(data).decode(),
                         "input_ack": self.input_ack, "exit_code": self.exit_code,
                         "runtime_state": self.state, "generation": self.generation}
            try:
                if time.monotonic() - renewed > 300:
                    self.request("/v1/harness/agent/renew")
                    renewed = time.monotonic()
                result = self.request("/v1/harness/agent/poll", frame)
                if result["offset"] != offset + len(data):
                    raise ValueError("terminal acknowledgement mismatch")
                acknowledged = result["offset"]
                pending_input = self.apply_response(result, pending_input)
            except (OSError, ValueError, KeyError, RuntimeError):
                self.errors["transport"] += 1
                # No SIGHUP, no child shutdown, no implicit sandbox deletion.
                self.closed.wait(1)
            self.output_ready.wait(0.15)

    def transport(self):
        if not self.config.get("terminal_stream"):
            return self.poll_transport()
        # A failed attachment never owns the child lifetime. Keep draining its
        # PTY and retry this connection without restarting Pi or replaying work.
        from websockets.sync.client import connect
        from websockets.exceptions import WebSocketException
        acknowledged = 0
        pending_input = None
        inflight = None
        last_sent = None
        while not self.closed.is_set():
            try:
                context = ssl.create_default_context()
                if self.config.get("ca_certificate"):
                    context.load_verify_locations(cadata=self.config["ca_certificate"])
                origin = self.config["origin"]
                url = ("wss" + origin[5:] if origin.startswith("https:") else "ws" + origin[4:])
                options = {"ssl": context} if url.startswith("wss:") else {}
                with connect(url + "/v1/harness/agent/stream",
                             additional_headers={"Authorization": "Bearer " + self.config["token"]},
                             proxy=None, open_timeout=10, close_timeout=1,
                             max_size=65536, max_queue=16, **options) as socket:
                    # The gateway sends lease/control updates every second.
                    # Bound both stalled writes and a dead connection without
                    # blocking PTY draining or waiting on a keepalive send lock.
                    socket.socket.settimeout(5)
                    last_sent = None
                    responses = queue.Queue(maxsize=16)
                    disconnected = threading.Event()

                    def receive(socket=socket, responses=responses, disconnected=disconnected):
                        try:
                            for message in socket:
                                result = json.loads(message)
                                while not disconnected.is_set() and not self.closed.is_set():
                                    try:
                                        responses.put(result, timeout=0.1)
                                        self.output_ready.set()
                                        break
                                    except queue.Full:
                                        continue
                        except (OSError, ValueError, WebSocketException):
                            pass
                        finally:
                            disconnected.set()
                            self.output_ready.set()

                    reader = threading.Thread(target=receive, daemon=True)
                    reader.start()
                    # Preserve the exact unacknowledged frame across reconnects.
                    # It may no longer be reconstructible after the ring grows.
                    if inflight is not None:
                        socket.send(json.dumps(inflight))
                    sent_at = time.monotonic()
                    received_at = sent_at
                    try:
                        while not self.closed.is_set() and not disconnected.is_set():
                            self.output_ready.clear()
                            while True:
                                try:
                                    result = responses.get_nowait()
                                except queue.Empty:
                                    break
                                received_at = time.monotonic()
                                if "error" in result:
                                    raise ValueError("terminal stream rejected")
                                offset = result["offset"]
                                if inflight is not None:
                                    end = inflight["offset"] + len(base64.b64decode(inflight["output"]))
                                    if offset == end:
                                        acknowledged = end
                                        inflight = None
                                    elif offset > acknowledged:
                                        raise ValueError("terminal acknowledgement mismatch")
                                elif offset > acknowledged:
                                    raise ValueError("unexpected terminal acknowledgement")
                                pending_input = self.apply_response(result, pending_input)
                            if pending_input is not None:
                                with self.lock:
                                    if self.master is not None:
                                        pending_input = self.write_pending_input(pending_input)
                            now = time.monotonic()
                            if now - received_at > 10:
                                raise OSError("terminal stream timed out")
                            if inflight is None:
                                with self.lock:
                                    offset = max(acknowledged, self.base)
                                    data = bytes(self.output[offset-self.base:offset-self.base+CHUNK])
                                    state = (self.input_ack, self.exit_code, self.state, self.generation)
                                    frame = {"offset": offset, "output": base64.b64encode(data).decode(),
                                             "input_ack": self.input_ack, "exit_code": self.exit_code,
                                             "runtime_state": self.state, "generation": self.generation}
                                if data or state != last_sent or now - sent_at >= 5:
                                    inflight = frame
                                    socket.send(json.dumps(frame))
                                    last_sent = state
                                    sent_at = now
                            # Both PTY output and pushed gateway input wake this
                            # event. This timeout is a heartbeat, not input polling.
                            self.output_ready.wait(0.01 if pending_input is not None else 1)
                    finally:
                        disconnected.set()
                        socket.close()
                        reader.join(timeout=2)
                self.closed.wait(0.25)
            except (OSError, ValueError, KeyError, RuntimeError, WebSocketException):
                self.errors["transport"] += 1
                self.closed.wait(1)

    def synchronize(self):
        renewed = 0
        offsets_file = self.directory / "upload-offsets.json"
        offsets = json.loads(offsets_file.read_text()) if offsets_file.exists() else {}
        while not self.closed.is_set():
            try:
                # Credential renewal can perform a fresh HTTPS handshake; keep
                # it off the terminal's input/output path along with uploads.
                if self.config.get("terminal_stream") and time.monotonic() - renewed > 300:
                    self.request("/v1/harness/agent/renew")
                    renewed = time.monotonic()
                for path in self.harness.artifacts():
                    relative = path.relative_to(self.harness.sessions).as_posix()
                    file_id = hashlib.sha256(relative.encode()).hexdigest()
                    offset = offsets.get(file_id, 0)
                    with path.open("rb") as source:
                        source.seek(offset)
                        content = source.read(65536)
                    if not content:
                        continue
                    response = self.request("/v1/harness/agent/chunks", {
                        "file_id": file_id, "offset": offset,
                        "content": base64.b64encode(content).decode()})
                    if not offset < response["next_offset"] <= offset + len(content):
                        raise ValueError("upload acknowledgement mismatch")
                    offsets[file_id] = response["next_offset"]
                    atomic_json(offsets_file, offsets)
            except (OSError, ValueError, KeyError):
                self.errors["sync"] += 1
            self.closed.wait(1)

    def stop_child(self):
        with self.lock:
            child = self.child
        if child is not None and child.poll() is None:
            try:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    def run(self):
        try:
            self.start_child()
        except (OSError, ValueError, RuntimeError):
            # Report a startup failure through the existing authenticated relay.
            # The exception text can contain paths or environment details.
            try:
                self.request("/v1/harness/agent/poll", {
                    "offset": 0, "output": "", "input_ack": 0,
                    "exit_code": START_FAILURE_EXIT_CODE, "runtime_state": "exited",
                    "generation": self.generation,
                })
            except (OSError, ValueError, KeyError):
                pass
            raise
        threading.Thread(target=self.transport, daemon=True).start()
        threading.Thread(target=self.synchronize, daemon=True).start()
        try:
            self.closed.wait()
        finally:
            self.stop_child()
            self.lock_file.close()


def atomic_json(path, value):
    temporary = path.with_suffix(".tmp")
    with temporary.open("w") as output:
        os.chmod(temporary, 0o600)
        json.dump(value, output)
        output.flush()
        os.fsync(output.fileno())
    temporary.replace(path)


def main():
    os.umask(0o077)
    config = json.load(sys.stdin)
    supervisor = Supervisor(config)
    def shutdown(*_):
        supervisor.closed.set()
    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGINT, shutdown)
    supervisor.run()


if __name__ == "__main__":
    main()
