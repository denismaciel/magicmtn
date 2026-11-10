"""Remote relay status and extraction; file transport is owned by sandbox SCP."""
import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile


identity, action, name, repository = sys.argv[1:]
if repository in ("", ".", "..") or Path(repository).name != repository:
    raise ValueError("invalid repository directory name")
status = Path("/tmp") / f"magicmtn-relay-{identity}.json"
workspace = Path.cwd()
if workspace in (Path("/"), Path.home()) and Path("/workspace").is_dir():
    workspace = Path("/workspace")
archive = workspace / name
directory = workspace / repository


def report(state):
    temporary = status.with_suffix(".tmp")
    temporary.write_text(json.dumps({"state": state, "directory": str(directory.resolve())}))
    os.replace(temporary, status)


if action == "begin":
    report("copying")
elif action == "extract":
    if os.path.lexists(directory):
        raise FileExistsError(f"repository destination already exists: {directory}")
    # The data filter prevents traversal and links escaping the destination.
    # Publish the repository only after all files and metadata are extracted.
    with tempfile.TemporaryDirectory(prefix=".magic-relay-", dir=workspace) as temporary:
        staged = Path(temporary) / repository
        staged.mkdir()
        with tarfile.open(archive, "r:gz") as source:
            members = source.getmembers()
            for member in members:
                tarfile.data_filter(member, str(staged))
            source.extractall(staged, members=members, filter="data")
        staged.rename(directory)
    archive.unlink()
    report("complete")
elif action == "fail":
    archive.unlink(missing_ok=True)
    report("failed")
else:
    raise ValueError("unknown relay action")
