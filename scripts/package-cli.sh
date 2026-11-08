#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
repository=$(pwd -P)
cargo build -p magicmtn-cli --bin magic --bin magic-provider-runner --release --locked --no-default-features \
  --target-dir "$repository/target/cli-production"
mkdir -p dist/bin
cp target/cli-production/release/magic dist/bin/magic.new
mv -f dist/bin/magic.new dist/bin/magic
printf 'Packaged production CLI: %s/dist/bin/magic\n' "$repository"

cp target/cli-production/release/magic-provider-runner dist/bin/magic-provider-runner.new
mv -f dist/bin/magic-provider-runner.new dist/bin/magic-provider-runner
