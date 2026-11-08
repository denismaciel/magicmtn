#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
repository=$(pwd -P)
cargo build -p magicmtn-cli --features development-server --locked --target-dir "$repository/target"
sh scripts/package-cli.sh
mkdir -p "$HOME/.local/bin"
ln -sfn "$repository/dist/bin/magic-provider-runner" "$HOME/.local/bin/magic-provider-runner"
ln -sfn "$repository/dist/bin/magic" "$HOME/.local/bin/magic"
ln -sfn "$repository/target/debug/magic" "$HOME/.local/bin/magic-dev"
if ! "$repository/dist/bin/magic" completion --path "$HOME/.local/bin"; then
  printf 'Could not install shell completions; run magic completion <shell> to retry.\n' >&2
fi
printf 'Next: run magic login. To install Pi, run magic agents install pi.\n'
printf 'Installed magic -> %s/dist/bin/magic\n' "$repository"
printf 'Installed magic-dev -> %s/target/debug/magic\n' "$repository"
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) printf 'Add %s/.local/bin to your PATH to run magic and magic-dev.\n' "$HOME" ;;
esac
