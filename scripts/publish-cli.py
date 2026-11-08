#!/usr/bin/env python3
"""Build, authenticate, and publish CLI releases; identical policy for every channel."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

import cli_release as release

BUCKET = "magicmtn-cli-beta-993499103295"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true", help="Use CI-built artifacts; signing and verification still run")
    parser.add_argument("--artifacts", type=Path, default=release.ROOT / "dist/bin")
    parser.add_argument("--output", type=Path, default=release.ROOT / "dist/public-release")
    parser.add_argument("--channel", choices=release.CHANNELS, default="beta")
    parser.add_argument("--signing-key", type=Path, default=os.environ.get("MAGICMTN_RELEASE_SIGNING_KEY_FILE"))
    parser.add_argument("--aws-profile", help="Omit in CI to use ambient AWS/OIDC credentials")
    parser.add_argument("--dry-run", action="store_true", help="Run all signing and verification checks without uploading")
    args = parser.parse_args()
    os.environ["AWS_CLI_AUTO_PROMPT"] = "off"
    os.environ["AWS_PAGER"] = ""
    configuration = release.trust()
    if sys.platform != "darwin":
        raise ValueError("Publish on a macOS runner so Developer ID and notarization can be verified")
    if args.signing_key is None or not args.signing_key.is_file():
        raise ValueError("Set MAGICMTN_RELEASE_SIGNING_KEY_FILE or --signing-key")
    if args.signing_key.stat().st_mode & 0o077:
        raise ValueError("The private release signing key must have permissions 0600")
    release.notary_credentials()
    release.source_commit()
    release.verify_bootstrap(configuration)
    if not args.skip_build:
        subprocess.run(["sh", "scripts/package-cli.sh"], cwd=release.ROOT, check=True)
        subprocess.run(["sh", "scripts/package-cli-linux.sh"], cwd=release.ROOT, check=True)
        subprocess.run(["sh", "scripts/check-cli-linux.sh"], cwd=release.ROOT, check=True)
    macos = args.artifacts / "magic-darwin-arm64"
    if not args.skip_build or not macos.exists():
        shutil.copy2(args.artifacts / "magic", macos)
    release.notarize(macos, args.artifacts / "notarization.json",
                     os.environ.get("MAGICMTN_APPLE_SIGNING_IDENTITY"), configuration)
    payload = release.prepare(args.artifacts, args.output, args.channel, configuration)
    manifest = args.output / "release.json"
    manifest.write_text(release.sign_payload(payload, args.signing_key, configuration))
    release.verify_artifacts(release.verify_envelope(manifest.read_bytes(), configuration), args.artifacts)
    for platform, digest in payload["platforms"].items():
        (args.output / ("magic-" + platform + ".sha256")).write_text(digest + "  magic\n")
    # The old S3 bootstrap is retired. The authenticated application deployment
    # serves the installer and its pinned public key independently of this bucket.
    legacy = args.output / "install.sh"
    legacy.write_text("#!/bin/sh\nset -eu\nprintf '%s\\n' 'Use the authenticated installer: curl -fsSL " +
                      release.INSTALLER_URL + " | sh' >&2\nexit 1\n")
    if args.dry_run:
        print("All release checks passed; no objects uploaded")
        return
    aws = ["aws"] + (["--profile", args.aws_profile] if args.aws_profile else [])

    def upload(path, key, content_type, cache):
        subprocess.run([*aws, "s3", "cp", str(path), f"s3://{BUCKET}/{key}",
                        "--region", "us-east-2", "--content-type", content_type,
                        "--cache-control", cache, "--only-show-errors"], check=True)

    prefix = "releases/" + payload["release"] + "/"
    for platform in release.PLATFORMS:
        name = "magic-" + platform
        upload(args.artifacts / name, prefix + name, "application/octet-stream", "public,max-age=31536000,immutable")
        upload(args.output / (name + ".sha256"), prefix + name + ".sha256", "text/plain", "public,max-age=31536000,immutable")
    upload(manifest, prefix + "release.json", "application/json", "public,max-age=31536000,immutable")
    upload(legacy, "install.sh", "text/plain; charset=utf-8", "no-cache")
    # One signed object is the atomic promotion point. No signature/pointer race.
    upload(manifest, "latest.json", "application/json", "no-cache")
    print(f"Published signed {args.channel} release {payload['release']}\ncurl -fsSL {release.INSTALLER_URL} | sh")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        raise SystemExit("CLI publication failed: " + (str(error) if not isinstance(error, subprocess.CalledProcessError) else "a required build/signing/verification command failed")) from None
