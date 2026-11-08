#!/usr/bin/env python3
"""Build only the authority-curated Registry adapters; never publish source code."""
import hashlib
import gzip
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
SOURCES = ROOT.parent
OUT = ROOT / 'dist/registry/packages'


def package(name, version, os_name, arch, executable, license_path):
    payload = executable.read_bytes()
    manifest = dict(name=name, version=version, protocol_version=1, os=os_name, arch=arch,
                    sha256=hashlib.sha256(payload).hexdigest())
    OUT.mkdir(parents=True, exist_ok=True)
    target = OUT / f'{name}-{version}-{os_name}-{arch}.tar.gz'
    with target.open('wb') as raw, gzip.GzipFile(filename='', mode='wb', fileobj=raw, mtime=0) as compressed, tarfile.open(fileobj=compressed, mode='w', format=tarfile.USTAR_FORMAT) as archive:
        for filename, data, mode in [('package.json', json.dumps(manifest).encode(), 0o644),
                                     ('provider', payload, 0o755), ('LICENSE', license_path.read_bytes(), 0o644)]:
            info = tarfile.TarInfo(filename)
            info.size, info.mode = len(data), mode
            archive.addfile(info, io.BytesIO(data))
    return target


def main():
    apple = SOURCES / 'provider-apple-container'
    subprocess.run(['cargo', 'build', '--release', '--locked'], cwd=apple, check=True)
    version = tomllib.loads((apple / 'Cargo.toml').read_text())['package']['version']
    package('apple-container', version, 'macos', 'aarch64', apple / 'target/release/magicmtn-provider-apple-container', apple / 'LICENSE')
    firecracker = SOURCES / 'provider-firecracker'
    version = tomllib.loads((firecracker / 'Cargo.toml').read_text())['package']['version']
    with tempfile.TemporaryDirectory(prefix='magic-registry-build-') as temporary:
        context = Path(temporary)
        source = context / 'provider'
        source.mkdir()
        for name in ['Cargo.toml', 'Cargo.lock', 'LICENSE']:
            shutil.copy2(firecracker / name, source / name)
        shutil.copytree(firecracker / 'src', source / 'src')
        for name in ['magicmtn-sdk', 'magicmtn-provider-framework']:
            shutil.copytree(ROOT / 'sdk/rust' / name, context / 'sdk' / name)
        (source / '.cargo').mkdir()
        (source / '.cargo/config.toml').write_text('[patch.crates-io]\nmagicmtn-sdk = { path = "../sdk/magicmtn-sdk" }\nmagicmtn-provider-framework = { path = "../sdk/magicmtn-provider-framework" }\n')
        (context / 'Containerfile').write_text('''FROM docker.io/library/rust:1.99.0-bookworm@sha256:114c7a4425406451c2866b6aafe69fe29b1b298832db1277d411ac73c82d04d6 AS builder
RUN apt-get update && apt-get install -y --no-install-recommends python3-venv && rm -rf /var/lib/apt/lists/*
RUN python3 -m venv /opt/cross && /opt/cross/bin/pip install --no-cache-dir cargo-zigbuild==0.23.4 ziglang==0.14.1
ENV PATH=/opt/cross/bin:$PATH
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
COPY sdk /build/sdk
COPY provider /build/provider
WORKDIR /build/provider
RUN cargo zigbuild --locked --release --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl
FROM scratch
COPY --from=builder /build/provider/target/x86_64-unknown-linux-musl/release/magicmtn-provider-firecracker /firecracker-x86_64
COPY --from=builder /build/provider/target/aarch64-unknown-linux-musl/release/magicmtn-provider-firecracker /firecracker-aarch64
''')
        subprocess.run(['podman', 'build', '--platform', 'linux/arm64', '--layers', '-t', 'localhost/magicmtn-registry-build', str(context)], check=True)
        container = subprocess.check_output(['podman', 'create', 'localhost/magicmtn-registry-build', '/firecracker-aarch64'], text=True).strip()
        try:
            for arch in ['x86_64', 'aarch64']:
                binary = context / f'firecracker-{arch}'
                subprocess.run(['podman', 'cp', f'{container}:/firecracker-{arch}', str(binary)], check=True)
                package('firecracker', version, 'linux', arch, binary, firecracker / 'LICENSE')
        finally:
            subprocess.run(['podman', 'rm', container], check=True)
    print(OUT)


if __name__ == '__main__':
    main()
