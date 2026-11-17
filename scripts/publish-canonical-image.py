#!/usr/bin/env python3
"""Publish smoke-tested Canonical OCI archives and emit a CLI image lock."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "ghcr.io/usemagicmtn/canonical"
LOCK = ROOT / "sdk/rust/magicmtn-sdk/src/canonical-image.json"


def run(*args):
    subprocess.run(list(map(str, args)), check=True)


def inspect_archive(metadata_path):
    metadata = json.loads(metadata_path.read_text())
    archive = metadata_path.parent / Path(metadata["archive"]).name
    platform = metadata["platform"]
    if platform not in ("linux/amd64", "linux/arm64") or metadata["emulated_smoke_skips"]:
        raise ValueError("Canonical releases require native smoke tests on both architectures")
    # Check that the image was tested against these exact runtime/build sources.
    spec = importlib.util.spec_from_file_location("canonical_builder", ROOT / "scripts/build-canonical-image.py")
    builder = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(builder)
    expected = builder.runtime_sources(ROOT)
    if metadata["runtime_sources"] != expected:
        raise ValueError("runtime sources differ from the smoke-tested image")
    for key, relative in (("containerfile_sha256", "images/canonical/Containerfile"),
                          ("package_lock_sha256", "images/canonical/package-lock.json")):
        if metadata[key] != hashlib.sha256((ROOT / relative).read_bytes()).hexdigest():
            raise ValueError("image build sources differ from the smoke-tested image")
    with tarfile.open(archive) as layout:
        index = json.load(layout.extractfile("index.json"))
        if len(index["manifests"]) != 1 or index["manifests"][0]["digest"] != metadata["digest"]:
            raise ValueError("archive manifest differs from smoke-test metadata")
        descriptor = index["manifests"][0]
        def blob(descriptor):
            algorithm, checksum = descriptor["digest"].split(":", 1)
            if algorithm != "sha256" or not re.fullmatch(r"[0-9a-f]{64}", checksum):
                raise ValueError("invalid OCI descriptor digest")
            member = layout.getmember("blobs/sha256/" + checksum)
            if not member.isfile() or member.size != descriptor["size"]:
                raise ValueError("OCI descriptor size differs")
            return layout.extractfile(member)
        manifest = json.load(blob(descriptor))
        configuration = json.load(blob(manifest["config"]))
        if configuration["os"] + "/" + configuration["architecture"] != platform:
            raise ValueError("archive platform differs from native smoke-test metadata")
        for layer in manifest["layers"]:
            blob(layer).close()
        for member in layout.getmembers():
            if member.isfile() and member.name.startswith("blobs/sha256/"):
                checksum = hashlib.sha256()
                stream = layout.extractfile(member)
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    checksum.update(chunk)
                if checksum.hexdigest() != member.name.rsplit("/", 1)[-1]:
                    raise ValueError("OCI blob checksum mismatch")
    return metadata, archive


def anonymous_manifest(digest):
    """Verify that a new installation can retrieve the immutable public image."""
    scope = "repository:usemagicmtn/canonical:pull"
    query = urllib.parse.urlencode({"service": "ghcr.io", "scope": scope})
    try:
        with urllib.request.urlopen("https://ghcr.io/token?" + query, timeout=30) as response:
            token = json.load(response)["token"]
        request = urllib.request.Request(
            "https://ghcr.io/v2/usemagicmtn/canonical/manifests/" + digest,
            headers={"Authorization": "Bearer " + token,
                     "Accept": "application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json"})
        with urllib.request.urlopen(request, timeout=30) as response:
            manifest = response.read(4 * 1024 * 1024)
    except urllib.error.HTTPError as error:
        raise RuntimeError("Set the GHCR canonical package visibility to Public, then retry publication") from error
    if "sha256:" + hashlib.sha256(manifest).hexdigest() != digest:
        raise ValueError("public registry manifest digest differs")


def publish(metadata_paths, tag, output):
    # Validate every artifact before publishing any of them.
    images = [inspect_archive(path) for path in metadata_paths]
    if len(images) != 2 or {metadata["platform"] for metadata, _ in images} != {"linux/arm64", "linux/amd64"}:
        raise ValueError("provide one native image for each supported platform")
    manifests = {}
    with tempfile.TemporaryDirectory(prefix="magicmtn-canonical-") as temporary:
        for metadata, archive in images:
            run("podman", "load", "--input", archive)
            architecture = metadata["platform"].split("/")[1]
            digest_path = Path(temporary) / (architecture + ".digest")
            run("podman", "push", "--format", "oci", "--digestfile", digest_path,
                metadata["tag"], "docker://" + REPOSITORY + ":" + tag + "-" + architecture)
            digest = digest_path.read_text().strip()
            if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
                raise ValueError("invalid published digest")
            anonymous_manifest(digest)
            manifests[metadata["platform"]] = digest
        name = "localhost/magicmtn-canonical-" + uuid.uuid4().hex
        run("podman", "manifest", "create", name)
        try:
            for platform, digest in sorted(manifests.items()):
                run("podman", "manifest", "add", name, "docker://" + REPOSITORY + "@" + digest)
            index_digest = Path(temporary) / "index.digest"
            run("podman", "manifest", "push", "--all", "--format", "oci", "--digestfile", index_digest,
                name, "docker://" + REPOSITORY + ":" + tag)
            anonymous_manifest(index_digest.read_text().strip())
        finally:
            run("podman", "manifest", "rm", name)
    output.parent.mkdir(parents=True, exist_ok=True)
    # The completed lock is the release commit point. Existing locks survive failures.
    with tempfile.NamedTemporaryFile(mode="w", dir=output.parent, delete=False) as handle:
        json.dump({"repository": REPOSITORY, "manifests": manifests}, handle, indent=2)
        handle.write("\n")
        candidate = Path(handle.name)
    candidate.replace(output)
    print("Published MagicMtn Canonical; image lock:", output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("metadata", nargs="+", type=Path)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--output", type=Path, default=LOCK)
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]{0,100}", args.tag):
        parser.error("invalid release tag")
    publish(args.metadata, args.tag, args.output)


if __name__ == "__main__":
    main()
