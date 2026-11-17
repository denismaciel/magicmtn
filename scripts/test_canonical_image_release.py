import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]


def module(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts' / (name + '.py'))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


publisher = module('publish-canonical-image')
builder = module('build-canonical-image')


class CanonicalReleaseTests(unittest.TestCase):
    def archive(self, root, architecture='arm64', corrupt=False):
        blobs = {}

        def descriptor(value):
            data = json.dumps(value).encode()
            digest = hashlib.sha256(data).hexdigest()
            blobs['blobs/sha256/' + digest] = data
            return {'digest': 'sha256:' + digest, 'size': len(data)}

        configuration = descriptor({'os': 'linux', 'architecture': architecture})
        manifest = descriptor({'config': configuration, 'layers': []})
        archive = root / ('canonical-' + architecture + '.oci.tar')
        index = json.dumps({'manifests': [manifest]}).encode()
        if corrupt:
            blobs['blobs/sha256/' + manifest['digest'].split(':')[1]] += b' '
        with tarfile.open(archive, 'w') as output:
            for name, data in {'index.json': index, **blobs}.items():
                member = tarfile.TarInfo(name)
                member.size = len(data)
                output.addfile(member, io.BytesIO(data))
        metadata = dict(platform='linux/' + architecture, digest=manifest['digest'],
                        tag='localhost/canonical:test-' + architecture,
                        archive='/build-machine/' + archive.name,
                        runtime_sources=builder.runtime_sources(ROOT),
                        emulated_smoke_skips=[])
        for key, relative in [('containerfile_sha256', 'images/canonical/Containerfile'),
                              ('package_lock_sha256', 'images/canonical/package-lock.json')]:
            metadata[key] = hashlib.sha256((ROOT / relative).read_bytes()).hexdigest()
        path = archive.with_suffix('.json')
        path.write_text(json.dumps(metadata))
        return path

    def test_archive_requires_verified_blobs_native_smoke_and_matching_sources(self):
        with tempfile.TemporaryDirectory() as directory:
            path = self.archive(Path(directory))
            publisher.inspect_archive(path)
            original = json.loads(path.read_text())
            for changed in [dict(emulated_smoke_skips=['fd']),
                            dict(runtime_sources={}), dict(digest='sha256:' + '0' * 64),
                            dict(platform='linux/amd64')]:
                path.write_text(json.dumps({**original, **changed}))
                with self.assertRaises(ValueError):
                    publisher.inspect_archive(path)
            path = self.archive(Path(directory), corrupt=True)
            with self.assertRaises(ValueError):
                publisher.inspect_archive(path)

    def test_release_verifies_committed_pins_without_modifying_source(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'lock.json'
            value = {'repository': publisher.REPOSITORY, 'manifests': {
                'linux/arm64': 'sha256:' + '1' * 64, 'linux/amd64': 'sha256:' + '2' * 64}}
            path.write_text(json.dumps(value))
            original = path.read_bytes()
            with mock.patch.object(publisher, 'anonymous_manifest') as verify:
                publisher.verify_lock(path)
                self.assertEqual(verify.call_count, 2)
            self.assertEqual(path.read_bytes(), original)
            value['manifests']['linux/arm64'] = 'latest'
            path.write_text(json.dumps(value))
            with mock.patch.object(publisher, 'anonymous_manifest') as verify:
                with self.assertRaises(ValueError):
                    publisher.verify_lock(path)
                verify.assert_not_called()

    def test_failed_public_verification_preserves_previous_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [self.archive(root, architecture) for architecture in ['arm64', 'amd64']]
            output = root / 'lock.json'
            output.write_text('previous release\n')

            def run(*args):
                if '--digestfile' in args:
                    Path(args[args.index('--digestfile') + 1]).write_text('sha256:' + '1' * 64)

            with mock.patch.object(publisher, 'run', side_effect=run), \
                 mock.patch.object(publisher, 'anonymous_manifest', side_effect=RuntimeError('private package')):
                with self.assertRaisesRegex(RuntimeError, 'private package'):
                    publisher.publish(paths, 'release-test', output)
            self.assertEqual(output.read_text(), 'previous release\n')

    def test_success_records_both_platforms_after_public_verification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [self.archive(root, architecture) for architecture in ['arm64', 'amd64']]
            output = root / 'lock.json'

            def run(*args):
                if '--digestfile' in args:
                    Path(args[args.index('--digestfile') + 1]).write_text('sha256:' + '1' * 64)

            with mock.patch.object(publisher, 'run', side_effect=run), \
                 mock.patch.object(publisher, 'anonymous_manifest') as verify:
                publisher.publish(paths, 'release-test', output)
                self.assertEqual(verify.call_count, 3)
            result = json.loads(output.read_text())
            self.assertEqual(result['repository'], publisher.REPOSITORY)
            self.assertEqual(set(result['manifests']), {'linux/arm64', 'linux/amd64'})


if __name__ == '__main__':
    unittest.main()
