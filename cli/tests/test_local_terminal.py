"""Exercise local shell handoff with a real controlling terminal and pinned provider."""
import fcntl
import hashlib
import http.server
import json
import os
from pathlib import Path
import pty
import select
import subprocess
import termios
import tempfile
import threading
import time
import unittest


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_MAGIC"), "set MAGICMTN_TEST_MAGIC to a built magic executable")
class LocalTerminalTest(unittest.TestCase):
    def test_local_shell_uses_pinned_provider_and_controlling_terminal(self):
        binary = str(Path(os.environ["MAGICMTN_TEST_MAGIC"]).resolve())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            providers = root / "providers"
            providers.mkdir()
            (providers / ".lock").touch()
            executable = providers / "provider"
            events = root / "events"
            manifest = {"manifest_version": 1, "provider_version": "0.1.0", "capabilities": {
                "inference_tasks": [], "sandbox_operations": ["ssh"],
                "editable_surfaces": [], "observe": False, "evaluate": False, "deploy": False}}
            executable.write_text(f'''#!/usr/bin/env python3
import json, os, sys
for line in sys.stdin:
    request = json.loads(line)
    operation = request['operation']['name']
    with open({str(events)!r}, 'a') as log:
        log.write(operation + '\\n')
    if operation == 'describe':
        result = {{'kind': 'description', 'value': {manifest!r}}}
    elif operation == 'sandbox_ssh':
        assert request['operation']['arguments']['sandbox']['provider_id'] == 'local-test-container'
        assert os.environ['MAGICMTN_APPLE_CONTAINER_NETWORK'] == 'default'
        assert 'OPENAI_API_KEY' not in os.environ
        with open('/dev/tty', 'r') as reader, open('/dev/tty', 'w') as writer:
            writer.write('local shell ready\\n'); writer.flush()
            assert reader.readline().strip() == 'hello'
            writer.write('local shell complete\\n'); writer.flush()
        result = {{'kind': 'done'}}
    else:
        result = {{'kind': 'done'}}
    print(json.dumps({{'id': request['id'], 'protocol_version': 1, 'provider': 'apple-container', 'outcome': {{'ok': result}}}}), flush=True)
    if operation == 'shutdown': break
''')
            executable.chmod(0o700)
            pin = {"name": "apple-container", "version": "0.1.0",
                   "sha256": hashlib.sha256(executable.read_bytes()).hexdigest()}
            package = dict(pin, protocol_version=1, os="macos" if os.uname().sysname == "Darwin" else "linux",
                           arch="aarch64" if os.uname().machine == "arm64" else os.uname().machine)
            # An inactive retained release is still the sandbox's exact provider pin.
            (providers / "catalog.json").write_text(json.dumps([{
                "package": package, "manifest": manifest, "active": False, "executable": str(executable)}]))
            paths = []

            class Handler(http.server.BaseHTTPRequestHandler):
                def log_message(self, *args):
                    pass

                def do_POST(self):
                    paths.append(self.path)
                    if self.path != "/v1/sandboxes/2240676a/shell" or self.headers.get("Authorization") != "Bearer local-test-token":
                        self.send_error(403)
                        return
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.end_headers()
                    self.wfile.write(json.dumps({"pin": pin,
                        "configuration": {"MAGICMTN_APPLE_CONTAINER_NETWORK": "default"},
                        "sandbox": {"provider_id": "local-test-container", "status": "running",
                                    "create_time_ms": None, "metadata": {}}}).encode())

            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            connection = root / "connection.json"
            connection.write_text(json.dumps({"url": f"http://127.0.0.1:{server.server_port}", "token": "local-test-token"}))
            connection.chmod(0o600)
            master, slave = pty.openpty()

            def controlling_terminal():
                os.setsid()
                fcntl.ioctl(0, termios.TIOCSCTTY, 0)

            process = subprocess.Popen([binary, "--local", "sandbox", "ssh", "2240676a"],
                stdin=slave, stdout=slave, stderr=slave, preexec_fn=controlling_terminal,
                env=dict(os.environ, HOME=directory, MAGICMTN_CONFIG_DIRECTORY=directory,
                         MAGICMTN_LOCAL_DIRECTORY=directory, OPENAI_API_KEY="must-not-reach-provider"))
            output = bytearray()
            try:
                deadline = time.monotonic() + 10
                sent = False
                while process.poll() is None and time.monotonic() < deadline:
                    if select.select([master], [], [], 0.1)[0]:
                        output.extend(os.read(master, 8192))
                    if b"local shell ready" in output and not sent:
                        os.write(master, b"hello\n")
                        sent = True
                self.assertEqual(process.wait(timeout=2), 0, output.decode(errors="replace"))
                self.assertTrue(sent)
                self.assertEqual(paths, ["/v1/sandboxes/2240676a/shell"])
                self.assertEqual(events.read_text().splitlines(), ["describe", "sandbox_ssh", "shutdown"])
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()
                os.close(master)
                os.close(slave)
                server.shutdown()
                server.server_close()
