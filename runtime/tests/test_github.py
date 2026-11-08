import importlib.util
import io
import json
import os
import pathlib
import tempfile
import unittest
import uuid
from unittest.mock import patch

runtime = pathlib.Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('git_credential', runtime / 'github.py')
github = importlib.util.module_from_spec(spec)
spec.loader.exec_module(github)
launcher_spec = importlib.util.spec_from_file_location('gh_launcher', runtime / 'gh.py')
launcher = importlib.util.module_from_spec(launcher_spec)
launcher_spec.loader.exec_module(launcher)


class GithubTests(unittest.TestCase):
    def test_gh_launcher_uses_installed_cli_and_harness_credential(self):
        with tempfile.TemporaryDirectory() as root:
            config = pathlib.Path(root) / 'integrations.json'
            config.write_text(json.dumps({'origin': 'https://control.magicmtn.test', 'token': 'runtime-only'}))
            selected = str(uuid.uuid4())
            with patch.dict(os.environ, {'MAGICMTN_INTEGRATIONS_CONFIG': str(config),
                                         'MAGICMTN_GITHUB_CONNECTION_ID': selected,
                                         'GH_TOKEN': 'should-not-be-forwarded'}), \
                 patch.object(launcher.sys, 'argv', ['gh', 'api', 'repos/org/repo']), \
                 patch.object(launcher.os, 'execve') as execute:
                launcher.main()
                executable, args, environment = execute.call_args.args
                self.assertEqual(executable, '/usr/bin/gh')
                self.assertEqual(args, ['gh', 'api', 'repos/org/repo'])
                self.assertEqual(environment['GH_HOST'], 'control.magicmtn.test')
                self.assertEqual(environment['GH_ENTERPRISE_TOKEN'], 'runtime-only~' + selected)
                self.assertNotIn('GH_TOKEN', environment)

    def test_git_credential_is_limited_to_gateway_repo_path(self):
        with tempfile.TemporaryDirectory() as root:
            config = pathlib.Path(root) / 'integrations.json'
            config.write_text(json.dumps({'origin': 'https://control.magicmtn.test', 'token': 'runtime-only'}))
            selected = str(uuid.uuid4())
            with patch.dict(os.environ, {'MAGICMTN_INTEGRATIONS_CONFIG': str(config),
                                         'MAGICMTN_GITHUB_CONNECTION_ID': selected}):
                for host, path, allowed in [
                    ('control.magicmtn.test', 'org/repo.git', True),
                    ('control.magicmtn.test', 'org/repo.git/info/refs', True),
                    ('evil.test', 'org/repo.git', False),
                    ('control.magicmtn.test', 'v1/sessions', False),
                    ('control.magicmtn.test', 'org/repo%2fsecret.git', False),
                ]:
                    with patch.object(github.sys, 'stdin', io.StringIO(f'protocol=https\nhost={host}\npath={path}\n')), \
                         patch.object(github.sys, 'stdout', new_callable=io.StringIO) as output:
                        github.git_credential()
                        self.assertEqual('password=runtime-only~' + selected in output.getvalue(), allowed)

    def test_bad_origin_or_selected_connection_is_rejected(self):
        with tempfile.TemporaryDirectory() as root:
            config = pathlib.Path(root) / 'integrations.json'
            config.write_text(json.dumps({'origin': 'https://evil.test/path', 'token': 'runtime-only'}))
            with patch.dict(os.environ, {'MAGICMTN_INTEGRATIONS_CONFIG': str(config)}):
                with self.assertRaises(RuntimeError):
                    github.configuration()
            config.write_text(json.dumps({'origin': 'https://control.magicmtn.test', 'token': 'runtime-only'}))
            with patch.dict(os.environ, {'MAGICMTN_INTEGRATIONS_CONFIG': str(config),
                                         'MAGICMTN_GITHUB_CONNECTION_ID': 'invalid'}), \
                 patch.object(launcher.os, 'execve'):
                with self.assertRaises(ValueError):
                    launcher.main()
