"""Pi 1.2 adapter; no agent loop is implemented by MagicMtn."""
import datetime
import json
import os
import pathlib


class PiHarness:
    def __init__(self, directory, config, session_id, atomic_json):
        self.config = config
        self.id = session_id
        self.atomic_json = atomic_json
        self.directory = directory / "pi"
        self.directory.mkdir(exist_ok=True, mode=0o700)
        self.sessions = self.directory / "sessions"
        model = dict(config["model"])
        endpoint = config["origin"] + "/v1/harness/agent"
        api = model.get("api", "openai-completions") if config.get("native") else "openai-completions"
        model["baseUrl"] = endpoint
        atomic_json(self.directory / "models.json", {"providers": {"magicmtn": {
            "baseUrl": endpoint, "api": api, "apiKey": config["token"], "models": [model]}}})
        settings_path = self.directory / "settings.json"
        settings = json.loads(settings_path.read_text()) if settings_path.exists() else {}
        settings.update(defaultProvider="magicmtn", defaultModel=model["id"], hideThinkingBlock=True)
        atomic_json(settings_path, settings)
        if config.get("workspace_extension"):
            extension = self.directory / "extensions" / "magicmtn-workspace"
            extension.mkdir(parents=True, exist_ok=True)
            (extension / "index.js").write_text(config["workspace_extension"])
            (extension / "package.json").write_text('{"type":"module"}')
        extension = self.directory / "extensions" / "magicmtn-errors"
        extension.mkdir(parents=True, exist_ok=True)
        runtime = pathlib.Path(__file__).resolve().parents[1]
        for source, destination in (("errors.js", "index.js"), ("error-worker.js", "error-worker.js")):
            (extension / destination).write_text((runtime / source).read_text())
        (extension / "package.json").write_text('{"type":"module"}')
        atomic_json(extension / "config.json", {
            "origin": config["origin"], "token": config["token"],
            "ca_certificate": config.get("ca_certificate"),
        })
        extension = self.directory / "extensions" / "magicmtn-context"
        extension.mkdir(parents=True, exist_ok=True)
        for source, destination in (("context.js", "index.js"), ("context.md", "context.md")):
            (extension / destination).write_text((runtime / source).read_text())
        (extension / "package.json").write_text('{"type":"module"}')
        if config.get("integrations_origin"):
            extension = self.directory / "extensions" / "magicmtn-mcp"
            extension.mkdir(parents=True, exist_ok=True)
            (extension / "index.js").write_text((runtime / "mcp.js").read_text())
            (extension / "broker.js").write_text((runtime / "broker.js").read_text())
            (extension / "package.json").write_text('{"type":"module"}')

    def command(self):
        files = list(self.sessions.glob("*.jsonl"))
        if len(files) > 1:
            raise RuntimeError("ambiguous Pi session files; operator recovery required")
        if files:
            session_file = files[0]
        else:
            # Seed Pi's JSONL header so it can open the allocated ID without a lookup warning.
            timestamp = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="milliseconds")
            timestamp = timestamp.replace("+00:00", "Z")
            filename_timestamp = timestamp.replace(":", "-").replace(".", "-")
            self.sessions.mkdir(exist_ok=True, mode=0o700)
            session_file = self.sessions / f"{filename_timestamp}_{self.id}.jsonl"
            self.atomic_json(session_file, {
                "type": "session",
                "version": 3,
                "id": self.id,
                "timestamp": timestamp,
                "cwd": os.path.realpath(self.config.get("workspace", "/workspace")),
            })
        return ["pi", "--session", str(session_file), "--approve", "--provider", "magicmtn", "--model", self.config["model"]["id"]]

    def environment(self):
        values = {"PI_CODING_AGENT_DIR": str(self.directory)}
        if self.config.get("workspace_info"):
            values["MAGICMTN_WORKSPACE_INFO"] = json.dumps(self.config["workspace_info"])
        return values

    def artifacts(self):
        # Interactive Pi sessions use cwd-specific subdirectories. They share
        # the runtime identity and synchronization with the supervised session.
        return self.sessions.rglob("*.jsonl")
