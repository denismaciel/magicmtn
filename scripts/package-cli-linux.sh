#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
podman build --layers --ulimit nofile=65536:65536 --ignorefile images/cli.containerignore \
  -f images/cli.Containerfile -t localhost/magicmtn-cli-linux .
container=$(podman create localhost/magicmtn-cli-linux /magic-linux-arm64)
trap 'podman rm "$container" >/dev/null' EXIT HUP INT TERM
mkdir -p dist/bin
for platform in linux-x86_64 linux-arm64; do
  podman cp "$container:/magic-$platform" "dist/bin/magic-$platform.new"
  chmod 755 "dist/bin/magic-$platform.new"
  mv -f "dist/bin/magic-$platform.new" "dist/bin/magic-$platform"
done
printf 'Packaged production Linux CLIs in dist/bin\n'
