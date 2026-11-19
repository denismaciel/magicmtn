import base64
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile


def default_directory():
    directory = Path.cwd()
    workspace = Path("/workspace")
    if directory in (Path("/"), Path.home()) and workspace.is_dir():
        directory = workspace.resolve()
    return directory


def transfer(operation):
    action = operation["action"]
    if action == "begin":
        name = operation["name"]
        if name in ("", ".", "..") or Path(name).name != name:
            raise ValueError("invalid destination filename")
        directory = default_directory()
        descriptor, temporary = tempfile.mkstemp(prefix=".magic-scp-", dir=directory)
        os.close(descriptor)
        return {"temporary": temporary, "destination": str(directory / name)}

    temporary = Path(operation["temporary"])
    if action == "write":
        data = base64.b64decode(operation["data"], validate=True)
        with temporary.open("r+b") as output:
            output.seek(0, os.SEEK_END)
            if output.tell() != operation["offset"]:
                raise ValueError("upload offset differs from acknowledged data")
            output.write(data)
        return {"offset": operation["offset"] + len(data)}
    if action == "commit":
        checksum = hashlib.sha256()
        with temporary.open("rb") as source:
            if os.fstat(source.fileno()).st_size != operation["size"]:
                raise ValueError("uploaded file size differs")
            for chunk in iter(lambda: source.read(65536), b""):
                checksum.update(chunk)
        if checksum.hexdigest() != operation["sha256"]:
            raise ValueError("uploaded file checksum differs")
        os.chmod(temporary, operation["mode"] & 0o777)
        os.replace(temporary, operation["destination"])
        return {"copied": True}
    if action == "abort":
        temporary.unlink(missing_ok=True)
        return {"removed": True}
    raise ValueError("unknown upload operation")


if __name__ == "__main__":
    print(json.dumps(transfer(json.loads(sys.argv[1]))))
