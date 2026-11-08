#!/bin/sh
# The installer tests use ephemeral signing keys and never access production.
set -eu
cd "$(dirname "$0")/.."
exec python3 -m unittest discover -s scripts -p test_cli_release.py
