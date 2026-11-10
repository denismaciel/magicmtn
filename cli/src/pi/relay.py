"""Remote relay status and extraction; file transport is owned by sandbox SCP."""
import json
import os
from pathlib import Path
import sys
import tarfile


identity, action, name = sys.argv[1:]
status = Path("/tmp") / f"magicmtn-relay-{identity}.json"
directory = Path.cwd()
if directory in (Path("/"), Path.home()) and Path("/workspace").is_dir():
    directory = Path("/workspace")
archive = directory / name


def report(state):
    temporary = status.with_suffix(".tmp")
    temporary.write_text(json.dumps({"state": state}))
    os.replace(temporary, status)


if action == "begin":
    report("copying")
elif action == "extract":
    # The data filter prevents traversal and links escaping the destination.
    # Validate the whole archive before writing any of its contents.
    with tarfile.open(archive, "r:gz") as source:
        members = source.getmembers()
        for member in members:
            tarfile.data_filter(member, str(directory))
        source.extractall(directory, members=members, filter="data")
    archive.unlink()
    status.unlink(missing_ok=True)
elif action == "fail":
    archive.unlink(missing_ok=True)
    report("failed")
else:
    raise ValueError("unknown relay action")
