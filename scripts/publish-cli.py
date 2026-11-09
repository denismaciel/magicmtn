#!/usr/bin/env python3
"""Build and publish standalone CLI binaries with SHA-256 checksums."""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
BUCKET = "magicmtn-cli-beta-993499103295"
PLATFORMS = ("darwin-arm64", "linux-x86_64", "linux-arm64")


def prepare(artifacts, output, channel):
    hashes = {}
    for platform in PLATFORMS:
        binary = artifacts / ("magic-" + platform)
        if not binary.is_file() or not binary.stat().st_size:
            raise ValueError("Missing or empty artifact: " + str(binary))
        hashes[platform] = hashlib.sha256(binary.read_bytes()).hexdigest()
    identifier = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%d") + "-" + channel + "-" + str(time.time_ns())
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "diff", "HEAD", "--", "cli", "sdk/rust", "Cargo.toml", "Cargo.lock"], cwd=ROOT))
    manifest = {"release": identifier, "platforms": hashes, "source_commit": source, "source_dirty": dirty}
    output.mkdir(parents=True, exist_ok=True)
    (output / "release.json").write_text(json.dumps(manifest, indent=2) + "\n")
    for platform, digest in hashes.items():
        (output / ("magic-" + platform + ".sha256")).write_text(digest + "  magic\n")
    (output / "SHA256SUMS").write_text("".join(digest + "  magic-" + platform + "\n" for platform, digest in hashes.items()))
    installer = (ROOT / "scripts/install-checksum.sh").read_text()
    (output / "install.sh").write_text(installer.replace("@RELEASE@", identifier))
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--skip-build", action="store_true", help="Publish previously built and checked artifacts")
    parser.add_argument("--artifacts", type=Path, default=ROOT / "dist/bin")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/public-release")
    parser.add_argument("--channel", choices=("alpha", "beta", "prerelease", "stable"), default="beta")
    parser.add_argument("--aws-profile")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    os.environ["AWS_CLI_AUTO_PROMPT"] = "off"
    os.environ["AWS_PAGER"] = ""
    if not args.skip_build:
        for script in ("package-cli.sh", "package-cli-linux.sh", "check-cli-linux.sh"):
            subprocess.run(["sh", "scripts/" + script], cwd=ROOT, check=True)
        shutil.copy2(args.artifacts / "magic", args.artifacts / "magic-darwin-arm64")
    payload = prepare(args.artifacts, args.output, args.channel)
    if args.dry_run:
        print("Release prepared and checksums generated; no objects uploaded")
        return
    aws = ["aws"] + (["--profile", args.aws_profile] if args.aws_profile else [])

    def upload(path, key, content_type, cache):
        subprocess.run([*aws, "s3", "cp", str(path), f"s3://{BUCKET}/{key}",
                        "--region", "us-east-2", "--content-type", content_type,
                        "--cache-control", cache, "--only-show-errors"], check=True)

    prefix = "releases/" + payload["release"] + "/"
    immutable = "public,max-age=31536000,immutable"
    for platform in PLATFORMS:
        name = "magic-" + platform
        upload(args.artifacts / name, prefix + name, "application/octet-stream", immutable)
        upload(args.output / (name + ".sha256"), prefix + name + ".sha256", "text/plain", immutable)
    upload(args.output / "SHA256SUMS", prefix + "SHA256SUMS", "text/plain", immutable)
    upload(args.output / "release.json", prefix + "release.json", "application/json", immutable)
    # Promote only after every immutable artifact has uploaded successfully.
    upload(args.output / "release.json", "latest.json", "application/json", "no-cache")
    upload(args.output / "install.sh", "install.sh", "text/plain; charset=utf-8", "no-cache")
    print(f"Published {args.channel} release {payload['release']}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        raise SystemExit("CLI publication failed: " + str(error)) from None
