#!/usr/bin/env python3
"""Run the installed GitHub CLI against the MagicMtn gateway."""
import os
import pathlib
import sys
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from github import configuration


def main():
    if "MAGICMTN_INTEGRATIONS_CONFIG" not in os.environ:
        os.execv("/usr/bin/gh", ["gh", *sys.argv[1:]])
    config, origin = configuration()
    if not config.get("github_enabled", True):
        os.execv("/usr/bin/gh", ["gh", *sys.argv[1:]])
    selected = os.environ.get("MAGICMTN_GITHUB_CONNECTION_ID", "")
    if selected:
        import uuid
        selected = str(uuid.UUID(selected))
    environment = os.environ.copy()
    environment.pop("GH_TOKEN", None)
    environment.pop("GITHUB_TOKEN", None)
    environment["GH_HOST"] = origin.hostname
    environment["GH_ENTERPRISE_TOKEN"] = config["token"] + ("~" + selected if selected else "")
    environment["GH_NO_UPDATE_NOTIFIER"] = "1"
    os.execve("/usr/bin/gh", ["gh", *sys.argv[1:]], environment)


if __name__ == "__main__":
    main()
