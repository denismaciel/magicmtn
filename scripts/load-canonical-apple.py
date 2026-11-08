#!/usr/bin/env python3
"""Load the verified ARM64 sandbox into Apple Container under its immutable reference."""
import argparse
import json
from pathlib import Path
import subprocess


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, help="Repository in the default-image registration")
    args = parser.parse_args()
    if not args.repository or args.repository.startswith("-") or any(c.isspace() for c in args.repository) or "@" in args.repository:
        parser.error("provide a repository without a tag or digest")
    metadata = json.loads((root / "dist/images/canonical-arm64.oci.json").read_text())
    if metadata["platform"] != "linux/arm64":
        raise SystemExit("the local image must be the verified ARM64 build")
    subprocess.run(["container", "image", "load", "--input", metadata["archive"]], check=True)
    image = json.loads(subprocess.check_output(["container", "image", "inspect", metadata["tag"]]))[0]
    if not any(v["digest"] == metadata["digest"] and v["platform"]["architecture"] == "arm64"
               for v in image["variants"]):
        raise SystemExit("loaded image does not contain the verified ARM64 manifest")
    reference = args.repository + "@" + metadata["digest"]
    subprocess.run(["container", "image", "tag", metadata["tag"], reference], check=True)
    print("Loaded MagicMtn Canonical locally:", reference)
    print("This does not publish the image to a remote registry or enable its database registration.")


if __name__ == "__main__":
    main()
