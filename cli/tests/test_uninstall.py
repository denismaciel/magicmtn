"""Exercise uninstall using disposable binary copies, homes and an HTTP fixture."""
import errno
import hashlib
import http.server
import json
import os
from pathlib import Path
import pty
import select
import shutil
import subprocess
import tempfile
import threading
import time
import unittest


class AuthHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        self.server.calls.append((self.command, self.path))
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"origin": self.server.origin}).encode())

    def do_POST(self):
        self.server.calls.append((self.command, self.path))
        self.server.authorization = self.headers.get("Authorization")
        self.send_response(self.server.logout_status)
        self.end_headers()
        self.wfile.write(b"{}")


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_MAGIC"), "set MAGICMTN_TEST_MAGIC to a development-server build")
class UninstallTest(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory(prefix="magic uninstall '")
        self.addCleanup(self.root.cleanup)
        self.home = Path(self.root.name)
        self.binary = self.home / "custom bin ' directory/magic"
        self.binary.parent.mkdir()
        shutil.copy2(Path(os.environ["MAGICMTN_TEST_MAGIC"]).resolve(), self.binary)
        self.env = {**os.environ, "HOME": str(self.home), "ZDOTDIR": str(self.home / "dotfiles"),
                    "XDG_CONFIG_HOME": str(self.home / "xdg"), "PI_CODING_AGENT_DIR": str(self.home / "pi")}
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), AuthHandler)
        self.server.origin = f"http://127.0.0.1:{self.server.server_port}"
        self.server.calls = []
        self.server.logout_status = 200
        self.addCleanup(self.server.server_close)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.shutdown)
        if "--server-url" not in subprocess.check_output([str(self.binary), "--help"], text=True):
            self.skipTest("uninstall fixture requires --features development-server")
        self.args = ["--server-url", self.server.origin, "uninstall"]

    def write(self, relative, value="fixture\n"):
        path = self.home / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(value)
        return path

    def save_session(self):
        digest = hashlib.sha256((self.server.origin + "/").encode()).hexdigest()
        return self.write(f".config/magicmtn/cli/{digest}.json",
                          json.dumps({"server": self.server.origin + "/", "token": "test-cli-session"}))

    def setup_integration(self, shared=False):
        if shared:
            self.write("custom bin ' directory/other-tool")
        self.write(".bashrc", "# Keep my Bash settings\n")
        self.write("dotfiles/.zshrc", "# Keep my Zsh settings\nautoload -Uz compinit\ncompinit\n")
        for shell in ("bash", "zsh", "fish"):
            subprocess.run([str(self.binary), "completion", shell, "--yes", "--path", str(self.binary.parent)],
                           env=self.env, check=True, capture_output=True)
        self.write("pi/extensions/magicmtn/index.js")
        self.write("pi/extensions/other/index.js", "keep\n")
        self.write(".config/magicmtn/pi-preferences.json", "keep\n")
        self.write(".local/share/magic/providers/keep", "keep\n")
        # Uninstallation must preserve a user's symlinked shell configuration.
        target = self.home / "dotfiles/.zshrc"
        target.rename(self.home / "dotfiles/zshrc-source")
        target.symlink_to("zshrc-source")
        return self.save_session()

    def snapshot(self):
        return {str(path.relative_to(self.home)): path.read_bytes()
                for path in self.home.rglob("*") if path.is_file()}

    def uninstall(self, answer, executable=None):
        pid, terminal = pty.fork()
        if pid == 0:
            # Confirmation must work when stdin is a pipe or unavailable.
            os.dup2(os.open(os.devnull, os.O_RDONLY), 0)
            binary = str(executable or self.binary)
            os.execve(binary, [binary, *self.args], self.env)
        output = b""
        sent = False
        try:
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if not select.select([terminal], [], [], 0.1)[0]:
                    continue
                try:
                    data = os.read(terminal, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                if not data:
                    break
                output += data
                if b"[y/N]" in output and not sent:
                    # No network or file mutation may precede confirmation.
                    if self.server.calls:
                        os.kill(pid, 9)
                        self.fail("uninstall contacted the server before confirmation")
                    os.write(terminal, answer)
                    sent = True
            else:
                os.kill(pid, 9)
                self.fail(f"uninstall timed out: {output!r}")
        finally:
            os.close(terminal)
            _, status = os.waitpid(pid, 0)
        return status, output.decode()

    def test_declining_or_pressing_enter_changes_nothing(self):
        self.setup_integration()
        before = self.snapshot()
        for answer in (b"n\n", b"\n", b"anything\n"):
            status, output = self.uninstall(answer)
            self.assertEqual(status, 0, output)
            self.assertIn("Uninstall cancelled", output)
            self.assertEqual(self.snapshot(), before)
            self.assertEqual(self.server.calls, [])

    def test_no_terminal_cannot_confirm(self):
        self.setup_integration()
        before = self.snapshot()
        result = subprocess.run([str(self.binary), *self.args], env=self.env, input="yes\n",
                                text=True, capture_output=True, start_new_session=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("interactive terminal", result.stderr)
        self.assertEqual(self.snapshot(), before)
        self.assertEqual(self.server.calls, [])

    def test_confirmed_uninstall_signs_out_and_cleans_up_custom_locations(self):
        credentials = self.setup_integration()
        status, output = self.uninstall(b"yes\n")
        self.assertEqual(status, 0, output)
        self.assertFalse(self.binary.exists())
        self.assertFalse(credentials.exists())
        self.assertFalse((self.home / "pi/extensions/magicmtn").exists())
        for relative in (".local/share/magic/completions/magic.bash", ".local/share/magic/completions/_magic",
                         "xdg/fish/completions/magic.fish", "xdg/fish/conf.d/magic-path.fish"):
            self.assertFalse((self.home / relative).exists(), relative)
        for path in [self.home / ".bashrc", self.home / ".bash_profile", self.home / "dotfiles/.zshrc"]:
            if path.exists():
                self.assertNotIn("source '", path.read_text())
                self.assertNotIn("fpath=(", path.read_text())
                self.assertNotIn("export PATH=", path.read_text())
        self.assertEqual((self.home / "dotfiles/.zshrc").read_text(),
                         "# Keep my Zsh settings\nautoload -Uz compinit\ncompinit\n")
        self.assertTrue((self.home / "dotfiles/.zshrc").is_symlink())
        self.assertEqual((self.home / "pi/extensions/other/index.js").read_text(), "keep\n")
        self.assertTrue((self.home / ".config/magicmtn/pi-preferences.json").exists())
        self.assertTrue((self.home / ".local/share/magic/providers/keep").exists())
        self.assertEqual(self.server.calls, [("GET", "/auth/config"), ("POST", "/auth/logout")])
        self.assertEqual(self.server.authorization, "Bearer test-cli-session")

    def test_shared_path_and_user_configuration_survive(self):
        self.setup_integration(shared=True)
        self.write(".local/bin/another-tool")
        status, output = self.uninstall(b"y\n")
        self.assertEqual(status, 0, output)
        self.assertTrue((self.home / "custom bin ' directory/other-tool").exists())
        self.assertIn("export PATH=", (self.home / "dotfiles/.zshrc").read_text())
        self.assertIn("fish_add_path", (self.home / "xdg/fish/conf.d/magic-path.fish").read_text())

    def test_failed_logout_preserves_installation_and_credentials(self):
        self.setup_integration()
        before = self.snapshot()
        self.server.logout_status = 503
        status, output = self.uninstall(b"y\n")
        self.assertNotEqual(status, 0)
        self.assertIn("no installation files were removed", output)
        self.assertEqual(self.snapshot(), before)

    def test_symlink_install_removes_link_and_keeps_target(self):
        link = self.home / ".local/bin/magic"
        link.parent.mkdir(parents=True)
        link.symlink_to(self.binary)
        self.env["PATH"] = str(link.parent) + os.pathsep + self.env["PATH"]
        status, output = self.uninstall(b"y\n", executable=link)
        self.assertEqual(status, 0, output)
        self.assertFalse(link.is_symlink())
        self.assertTrue(self.binary.exists())
        self.assertEqual(self.server.calls, [])

    def test_pi_extension_symlink_does_not_delete_its_target(self):
        target = self.home / "extension-source"
        target.mkdir()
        (target / "index.js").write_text("keep\n")
        link = self.home / "pi/extensions/magicmtn"
        link.parent.mkdir(parents=True)
        link.symlink_to(target)
        status, output = self.uninstall(b"y\n")
        self.assertEqual(status, 0, output)
        self.assertFalse(link.is_symlink())
        self.assertTrue((target / "index.js").exists())

    def test_uninstall_is_last_command_in_help(self):
        help_text = subprocess.check_output([str(self.binary), "help"], text=True)
        commands = help_text.split("Commands:\n", 1)[1].split("Options:", 1)[0]
        last = [line.split()[0] for line in commands.splitlines() if line.strip()][-1]
        self.assertEqual(last, "uninstall")


if __name__ == "__main__":
    unittest.main()
