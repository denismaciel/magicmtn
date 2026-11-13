#!/usr/bin/env python3
"""Build, smoke-test and export the canonical sandbox with Podman."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import uuid


def run(*args, **kwargs):
    return subprocess.run(list(args), check=True, **kwargs)


def runtime_sources(root):
    runtime = root / "runtime"
    files = [*sorted(runtime.glob("*.py")), runtime / "requirements.txt", runtime / "mcp.js", runtime / "install.sh", *sorted((runtime / "harnesses").glob("*.py"))]
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() for path in files}


def main():
    root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default="localhost/magicmtn/sandbox:0.1.0")
    parser.add_argument("--platform", help="For example linux/arm64 or linux/amd64; defaults to native")
    parser.add_argument("--emulated", action="store_true", help="Skip fd and ripgrep smoke checks under QEMU; validate those on a native runtime before rollout")
    parser.add_argument("--output", type=Path, default=root / "dist/images")
    args = parser.parse_args()
    context = root / "images/canonical"
    command = ["podman", "build", "--format", "oci", "--tag", args.tag, "--file", str(context / "Containerfile"),
               "--ignorefile", str(context / ".containerignore")]
    if args.platform:
        command += ["--platform", args.platform]
    run(*command, str(root))
    platform_options = ["--platform", args.platform] if args.platform else []
    expected_pi = json.loads((context / "package.json").read_text())["dependencies"]["@earendil-works/pi-coding-agent"]
    installed_pi = subprocess.check_output(["podman", "run", *platform_options, "--rm", "--network", "none",
                                            args.tag, "pi", "--version"], text=True).strip()
    assert installed_pi == expected_pi, f"Pi version mismatch: expected {expected_pi}, got {installed_pi}"
    # No provider credentials or network access are needed for these checks.
    smoke = r'''
test "$(id -u)" = 10001
test "$PWD" = /workspace
test -w "$HOME"
python -c 'import ssl, sqlite3, venv; print("Python", __import__("sys").version.split()[0])'
python -m pip --version
node --version
pi --version
gh --version
fd --version
gh help > /tmp/gh-help.txt
test -x /usr/local/bin/magicmtn-sandbox-agent
test -x /usr/local/bin/magicmtn-shell
test -x /usr/local/bin/magicmtn-git-credential
/opt/magicmtn/venv/bin/python3 -c 'import websockets.sync.client; import sys; sys.path.insert(0, "/opt/magicmtn/runtime"); import agent; from harnesses.pi import PiHarness'
pi --help > /tmp/pi-help.txt
grep -q -- '--mode' /tmp/pi-help.txt
git init --quiet /workspace/smoke-repo
printf 'sandbox smoke test\n' > /workspace/smoke-repo/test.txt
rg 'sandbox smoke test' /workspace/smoke-repo
python -m venv /workspace/project-venv
/workspace/project-venv/bin/python -c 'print("Project virtualenv works")'
'''
    if args.emulated:
        smoke = smoke.replace("fd --version\n", "").replace("rg 'sandbox smoke test' /workspace/smoke-repo\n", "")
    run("podman", "run", *platform_options, "--rm", "--network", "none", "--cap-drop", "all",
        "--security-opt", "no-new-privileges", args.tag, "bash", "-euc", smoke)
    fixture = {"session_id": str(uuid.uuid4()), "origin": "http://127.0.0.1:8080",
               "integrations_origin": "http://127.0.0.1:8080", "token": "image-smoke-scoped-token",
               "workspace_info": {"id": str(uuid.uuid4())},
               "workspace_extension": (root / "interfaces/pi/workspace.js").read_text(),
               "model": {"id": "smoke-model", "name": "Smoke Model", "reasoning": False,
                         "input": ["text"], "contextWindow": 32768, "maxTokens": 4096,
                         "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}}}
    setup = "import os,sys;sys.path.insert(0,'/opt/magicmtn/runtime');from agent import Supervisor;"
    setup += f"supervisor=Supervisor({fixture!r});supervisor.lock_file.close();"
    setup += "os.execvp('pi',['pi','--mode','rpc','--no-session','--offline'])"
    rpc = run("podman", "run", *platform_options, "--interactive", "--rm", "--network", "none", args.tag,
              "python3", "-c", setup, input='{"id":"smoke-state","type":"get_state"}\n',
              text=True, stdout=subprocess.PIPE, timeout=30)
    responses = [json.loads(line) for line in rpc.stdout.splitlines() if line.startswith("{")]
    assert any(r.get("id") == "smoke-state" and r.get("success") is True for r in responses), "Pi RPC initialization failed"
    state = next(r["data"] for r in responses if r.get("id") == "smoke-state")
    assert state["model"]["provider"] == "magicmtn" and state["model"]["id"] == "smoke-model", "Shell Pi lost the sandbox inference configuration"
    name = "magicmtn-image-smoke-" + uuid.uuid4().hex[:12]
    try:
        run("podman", "run", *platform_options, "--detach", "--name", name, "--network", "none", args.tag,
            stdout=subprocess.DEVNULL)
        run("podman", "exec", name, "pi", "--version")
        run("podman", "stop", "--time", "3", name, stdout=subprocess.DEVNULL)
        stopped = json.loads(subprocess.check_output(["podman", "inspect", name]))[0]
        assert stopped["State"]["ExitCode"] != 137, "sandbox needed SIGKILL to stop"
    finally:
        run("podman", "rm", "--force", name, stdout=subprocess.DEVNULL)
    info = json.loads(subprocess.check_output(["podman", "image", "inspect", args.tag]))[0]
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / ("canonical-" + info["Architecture"] + ".oci.tar")
    run("podman", "save", "--format", "oci-archive", "--output", str(archive), args.tag)
    # An image ID identifies its configuration, not the runnable OCI manifest.
    # Take the exact manifest digest from the exported OCI index instead.
    with tarfile.open(archive) as layout:
        index = json.load(layout.extractfile("index.json"))
        assert len(index["manifests"]) == 1, "expected one root image descriptor"
        digest = index["manifests"][0]["digest"]
        for member in layout.getmembers():
            if member.isfile() and member.name.startswith("blobs/sha256/"):
                checksum = hashlib.sha256()
                stream = layout.extractfile(member)
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    checksum.update(chunk)
                assert checksum.hexdigest() == member.name.rsplit("/", 1)[-1], "OCI blob digest mismatch"
    metadata = {
        "runtime_sources": runtime_sources(root),
        "tag": args.tag, "platform": info["Os"] + "/" + info["Architecture"],
        "digest": digest, "archive": str(archive), "image_size_bytes": info["Size"], "image_id": info["Id"],
        "containerfile_sha256": hashlib.sha256((context / "Containerfile").read_bytes()).hexdigest(),
        "package_lock_sha256": hashlib.sha256((context / "package-lock.json").read_bytes()).hexdigest(),
        "emulated_smoke_skips": ["fd", "rg"] if args.emulated else [],
    }
    archive.with_suffix(".json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps(metadata, indent=2))


if __name__ == "__main__":
    main()
