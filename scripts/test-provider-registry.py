import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('publisher', Path(__file__).with_name('publish-provider-registry.py'))
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)


class RegistryReleaseTests(unittest.TestCase):
    def package(self, root, name='firecracker', version='0.1.0', arch='aarch64', duplicate=False):
        binary = bytearray(64)
        binary[:6] = b'\x7fELF\x02\x01'
        binary[18:20] = (183).to_bytes(2, 'little')
        manifest = dict(name=name, version=version, protocol_version=1, os='linux', arch=arch, sha256=publisher.digest(binary))
        path = root / 'package.tar.gz'
        with tarfile.open(path, 'w:gz') as archive:
            for name, body in [('package.json', json.dumps(manifest).encode()), ('provider', binary), ('LICENSE', b'license')]+([('provider', binary)] if duplicate else []):
                info = tarfile.TarInfo(name); info.size = len(body)
                archive.addfile(info, io.BytesIO(body))
        return path

    def test_unsupported_names_cannot_publish(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'not approved'):
                publisher.prepare([self.package(root, name='private-provider')], root / 'out')

    def test_incomplete_releases_fail_before_index_creation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'all approved platform builds'):
                publisher.prepare([self.package(root)], root / 'out')
            self.assertFalse((root / 'out/index.json').exists())

    def test_architecture_and_duplicate_entries_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, 'architecture mismatch'):
                publisher.inspect_package(self.package(root, arch='x86_64'))
            with self.assertRaisesRegex(ValueError, 'invalid package entries'):
                publisher.inspect_package(self.package(root, duplicate=True))

    def test_prune_errors_are_not_silently_ignored(self):
        # Verify cleanup is after publication and scoped to obsolete provider objects.
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary); index=root/'index.json'; index.write_text('{"providers":[]}')
            missing = publisher.subprocess.CalledProcessError(1, 'aws', stderr='NoSuchKey')
            calls=[]
            def aws(*args):
                calls.append(args)
                if args[1]=='list-objects-v2': return json.dumps({'Contents':[{'Key':'providers/old/1/linux.tar.gz'}]})
                if args[1]=='delete-objects': return '{"Errors":[{"Code":"AccessDenied"}]}'
                return '{}'
            with patch.object(publisher.subprocess, 'run', side_effect=missing), patch.object(publisher, 'aws', side_effect=aws), patch.object(publisher, 'record_publication') as record:
                with self.assertRaisesRegex(RuntimeError, 'could not be removed'):
                    publisher.publish(index, {}, {'providers': []})
            self.assertEqual(record.call_count, 2)
            self.assertEqual([args[1] for args in calls], ['put-object','list-objects-v2','delete-objects'])

    def test_publication_requires_documentation_for_each_release(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            index = root / 'index.json'
            index.write_text(json.dumps({'providers': [{'name': 'new-provider', 'version': '1.0.0'}]}))
            docs = root / 'docs.json'
            docs.write_text('{"providers": []}')
            with self.assertRaisesRegex(ValueError, 'repo_url are required'):
                publisher.publication(index, docs)
            docs.write_text(json.dumps({'providers': [{'name': 'new-provider', 'requirements': 'Linux', 'documentation': 'Use it', 'repo_url': 'https://github.com/example/new-provider'}]}))
            self.assertEqual(publisher.publication(index, docs)['providers'][0]['documentation'], 'Use it')
            self.assertEqual(publisher.publication(index, docs)['providers'][0]['repo_url'], 'https://github.com/example/new-provider')

    def test_database_failure_fails_publication_before_cleanup(self):
        with tempfile.TemporaryDirectory() as temporary:
            index = Path(temporary) / 'index.json'
            index.write_text('{"providers": []}')
            missing = publisher.subprocess.CalledProcessError(1, 'aws', stderr='NoSuchKey')
            failure = publisher.subprocess.CalledProcessError(1, 'magicmtn-publish-provider')
            with patch.object(publisher.subprocess, 'run', side_effect=missing), patch.object(publisher, 'aws', return_value='{}') as aws, patch.object(publisher, 'record_publication', side_effect=[None, failure]):
                with self.assertRaises(publisher.subprocess.CalledProcessError):
                    publisher.publish(index, {}, {'providers': []})
            self.assertEqual([call.args[1] for call in aws.call_args_list], ['put-object'])


if __name__ == '__main__':
    unittest.main()
