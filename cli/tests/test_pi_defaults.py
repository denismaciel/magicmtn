"""Credential-free CLI preference persistence and account isolation checks."""
import hashlib
import http.server
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path != "/v1/pi/catalog" or self.headers.get("Authorization") != "Bearer test-session":
            self.send_error(401)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(self.server.catalog).encode())


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_MAGIC"), "set MAGICMTN_TEST_MAGIC to a built magic executable")
class DefaultsTest(unittest.TestCase):
    def test_defaults_persist_fall_back_and_do_not_cross_account_boundaries(self):
        ids = [f"00000000-0000-4000-8000-{n:012d}" for n in range(1, 9)]
        model, other_model, apple, exe, replacement, user, other_user, org = ids
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.catalog = {"models": [{"id": model}, {"id": other_model}],
                          "connections": [{"id": apple, "provider": "apple-container"}, {"id": exe, "provider": "exe"}],
                          "defaults": {"model_id": model},
                          "scope": {"user_id": user, "organization_id": org}}
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            with tempfile.TemporaryDirectory() as home:
                base = f"http://127.0.0.1:{server.server_port}/"
                directory = Path(home) / ".config/magicmtn/cli"
                directory.mkdir(parents=True, mode=0o700)
                (directory / (hashlib.sha256(base.encode()).hexdigest() + ".json")).write_text(json.dumps({"server": base, "token": "test-session"}))
                binary = str(Path(os.environ["MAGICMTN_TEST_MAGIC"]).resolve())

                def call(*args, success=True):
                    result = subprocess.run([binary, "--server-url", base, "pi", *args], env=dict(os.environ, HOME=home), capture_output=True, text=True)
                    self.assertEqual(result.returncode == 0, success, result.stderr)
                    return json.loads(result.stdout) if success else None

                call("defaults", "--model", other_model)
                self.assertEqual(call("catalog")["defaults"], {"model_id": other_model, "inference_source": "magicmtn"})
                self.assertEqual(next(directory.glob("defaults-*.json")).stat().st_mode & 0o777, 0o600)
                call("defaults", "--model", replacement, success=False)
                preferences = next(directory.glob("defaults-*.json"))
                saved = json.loads(preferences.read_text())
                self.assertNotIn("connection_id", saved)
                saved.update(connection_id=exe, sandbox_provider="exe")
                preferences.write_text(json.dumps(saved))
                server.catalog["models"] = [{"id": model}]
                server.catalog["connections"][1]["id"] = replacement
                self.assertEqual(call("catalog")["defaults"], {"model_id": model, "inference_source": "magicmtn"})
                server.catalog["scope"]["user_id"] = other_user
                self.assertEqual(call("catalog")["defaults"], {"model_id": model})
                server.catalog["scope"]["user_id"] = user
                server.catalog["scope"]["organization_id"] = other_user
                self.assertEqual(call("catalog")["defaults"], {"model_id": model})
        finally:
            server.shutdown()
            server.server_close()
