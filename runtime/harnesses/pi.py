"""Pi 1.2 adapter; no agent loop is implemented by MagicMtn."""
import json
import hashlib
import os
import pathlib


class PiHarness:
    def __init__(self, directory, config, session_id, atomic_json):
        self.config = config
        self.id = session_id
        self.directory = directory / "pi"
        self.directory.mkdir(exist_ok=True, mode=0o700)
        self.sessions = self.directory / "sessions"
        model = dict(config["model"])
        endpoint = config["origin"] + "/v1/harness/agent"
        api = model.get("api", "openai-completions") if config.get("native") else "openai-completions"
        model["baseUrl"] = endpoint
        atomic_json(self.directory / "models.json", {"providers": {"magicmtn": {
            "baseUrl": endpoint, "api": api, "apiKey": config["token"], "models": [model]}}})
        if config.get("workspace_extension"):
            extension = self.directory / "extensions" / "magicmtn-workspace"
            extension.mkdir(parents=True, exist_ok=True)
            (extension / "index.js").write_text(config["workspace_extension"])
            (extension / "package.json").write_text('{"type":"module"}')
        if config.get("integrations_origin"):
            extension = self.directory / "extensions" / "magicmtn-mcp"
            extension.mkdir(parents=True, exist_ok=True)
            (extension / "index.js").write_text(pathlib.Path(__file__).resolve().parents[1].joinpath("mcp.js").read_text())
            (extension / "package.json").write_text('{"type":"module"}')

    def command(self, cwd=None):
        files = list(self.sessions.glob("*.jsonl"))
        if len(files) > 1:
            raise RuntimeError("ambiguous Pi session files; operator recovery required")
        if files and cwd:
            # Pi restores cwd from the session header. Preserve every transcript
            # entry while moving the resumed session to the relay destination.
            header, separator, transcript = files[0].read_text().partition("\n")
            metadata = json.loads(header)
            if metadata["cwd"] != cwd:
                metadata["cwd"] = cwd
                temporary = files[0].with_suffix(".tmp")
                temporary.write_text(json.dumps(metadata) + separator + transcript)
                # Upload offsets are append-only and keyed by filename. Give
                # the rewritten header a new artifact identity.
                resumed = files[0].with_name(hashlib.sha256(cwd.encode()).hexdigest() + ".jsonl")
                temporary.replace(resumed)
                files[0].unlink()
                files = [resumed]
        selector = ["--session", str(files[0])] if files else ["--session-id", self.id, "--session-dir", str(self.sessions)]
        return ["pi", *selector, "--approve", "--provider", "magicmtn", "--model", self.config["model"]["id"]]

    def environment(self):
        values = {"PI_CODING_AGENT_DIR": str(self.directory)}
        if self.config.get("workspace_info"):
            values["MAGICMTN_WORKSPACE_INFO"] = json.dumps(self.config["workspace_info"])
        return values

    def artifacts(self):
        return self.sessions.glob("*.jsonl")
