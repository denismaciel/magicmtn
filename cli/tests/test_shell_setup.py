"""Verify shell consent with piped stdin and an actual controlling terminal."""
import errno
import os
from pathlib import Path
import pty
import select
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get("MAGICMTN_TEST_MAGIC"), "set MAGICMTN_TEST_MAGIC to a built magic executable")
class ShellSetupTest(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.TemporaryDirectory(prefix="magic shell '")
        self.addCleanup(self.root.cleanup)
        self.home = Path(self.root.name)
        self.binary = str(Path(os.environ["MAGICMTN_TEST_MAGIC"]).resolve())
        self.env = {**os.environ, "HOME": str(self.home), "ZDOTDIR": str(self.home / "zsh"),
                    "XDG_CONFIG_HOME": str(self.home / "config")}
        self.bin_dir = self.home / "bin with ' quotes"

    def run_setup(self, shell, answer):
        pid, terminal = pty.fork()
        if pid == 0:
            # Model curl | sh: stdin is unavailable for a consent response.
            os.dup2(os.open(os.devnull, os.O_RDONLY), 0)
            os.execve(self.binary, [self.binary, "completion", shell, "--path", str(self.bin_dir)], self.env)
        output = b""
        sent = False
        deadline = time.monotonic() + 10
        try:
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
                if b"[Y/n]" in output and not sent:
                    os.write(terminal, answer)
                    sent = True
            else:
                os.kill(pid, 9)
                self.fail(f"shell setup timed out: {output!r}")
        finally:
            os.close(terminal)
            _, status = os.waitpid(pid, 0)
        self.assertEqual(status, 0, output.decode())
        return output.decode()

    def test_declining_leaves_configuration_untouched(self):
        for shell in ("bash", "zsh", "fish"):
            with self.subTest(shell=shell):
                self.assertIn("Shell setup skipped", self.run_setup(shell, b"n\n"))
                self.assertEqual(list(self.home.iterdir()), [])

    def test_no_terminal_skips_configuration(self):
        result = subprocess.run([self.binary, "completion", "zsh", "--path", str(self.bin_dir)],
                                env=self.env, stdin=subprocess.DEVNULL, capture_output=True,
                                start_new_session=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(b"Shell setup skipped", result.stdout)
        self.assertEqual(list(self.home.iterdir()), [])

    def test_accepting_configures_path_once_and_preserves_zshrc(self):
        rc = self.home / "zsh/.zshrc"
        rc.parent.mkdir()
        original = "# My shell settings\nautoload -Uz compinit\ncompinit\n"
        rc.write_text(original)
        self.run_setup("zsh", b"\n")
        first = rc.read_text()
        self.assertTrue(first.endswith(original))
        self.assertLess(first.index("fpath="), first.index("compinit"))
        self.assertIn("export PATH=", first)
        self.assertNotIn("[Y/n]", self.run_setup("zsh", b"n\n"))
        self.assertEqual(rc.read_text(), first)
        # Execute just the PATH line to verify quoting, including apostrophes.
        result = subprocess.run(["sh", "-c", first.splitlines()[0] + '\nprintf "%s" "$PATH"'],
                                env=self.env, capture_output=True, check=True)
        self.assertTrue(result.stdout.decode().startswith(str(self.bin_dir) + ":"))

    def test_accepting_configures_bash_path_and_completions(self):
        self.run_setup("bash", b"yes\n")
        rc = self.home / (".bash_profile" if os.uname().sysname == "Darwin" else ".bashrc")
        first = rc.read_text()
        self.assertIn("source '", first)
        self.assertNotIn("[Y/n]", self.run_setup("bash", b"n\n"))
        self.assertEqual(rc.read_text(), first)

    @unittest.skipUnless(os.uname().sysname == "Darwin", "macOS Bash uses a login profile")
    def test_bash_reuses_existing_login_profile(self):
        profile = self.home / ".profile"
        profile.write_text("# Existing login configuration\n")
        self.run_setup("bash", b"y\n")
        self.assertTrue(profile.read_text().startswith("# Existing login configuration\n"))
        self.assertIn("export PATH=", profile.read_text())
        self.assertFalse((self.home / ".bash_profile").exists())

    def test_accepting_configures_fish_in_xdg_directory(self):
        self.run_setup("fish", b"y\n")
        self.assertTrue((self.home / "config/fish/completions/magic.fish").is_file())
        rc = self.home / "config/fish/conf.d/magic-path.fish"
        first = rc.read_text()
        self.assertIn("fish_add_path -- ", first)
        self.assertNotIn("[Y/n]", self.run_setup("fish", b"n\n"))
        self.assertEqual(rc.read_text(), first)


if __name__ == "__main__":
    unittest.main()
