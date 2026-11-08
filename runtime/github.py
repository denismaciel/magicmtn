#!/usr/bin/env python3
"""Supply Git with a sandbox credential for the MagicMtn gateway host."""
import json
import os
import pathlib
import sys
import urllib.parse
import uuid


def configuration():
    config = json.loads(pathlib.Path(os.environ["MAGICMTN_INTEGRATIONS_CONFIG"]).read_text())
    origin = urllib.parse.urlsplit(config["origin"])
    if ((origin.scheme != "https" and not (origin.scheme == "http" and origin.hostname == "localhost"))
            or not origin.hostname or origin.username or origin.password or origin.query or origin.fragment
            or origin.path not in ("", "/") or not config.get("token")):
        raise RuntimeError("invalid integration configuration")
    return config, origin


def git_credential():
    fields = dict(line.rstrip("\n").split("=", 1) for line in sys.stdin if "=" in line)
    config, origin = configuration()
    if fields.get("protocol") != origin.scheme or fields.get("host") != origin.netloc:
        return
    parts = fields.get("path", "").lstrip("/").split("/")
    if len(parts) < 2 or not parts[1].endswith(".git") or not all(
            p and p not in (".", "..") and all(c.isascii() and (c.isalnum() or c in "._-") for c in p)
            for p in parts[:2]):
        return
    selected = os.environ.get("MAGICMTN_GITHUB_CONNECTION_ID")
    if selected:
        selected = str(uuid.UUID(selected))
    print("username=magicmtn\npassword=" + config["token"] + ("~" + selected if selected else "") + "\n")


if __name__ == "__main__":
    try:
        if sys.argv[1:] == ["get"]:
            git_credential()
    except Exception:
        sys.exit(1)
