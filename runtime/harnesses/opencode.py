"""Configure the OpenCode CLI and persist its managed session on the sandbox disk."""
import json
import pathlib


PACKAGES = {
    "openai-completions": "@opencode/ai/providers/openai-compatible",
    "openai-responses": "@opencode/ai/providers/openai-compatible/responses",
    "anthropic-messages": "@opencode/ai/providers/anthropic-compatible",
}


class OpenCodeHarness:
    def __init__(self, directory, config, session_id, atomic_json):
        self.directory = directory / "opencode"
        self.directory.mkdir(mode=0o700, exist_ok=True)
        self.sessions = self.directory / "sessions"
        self.sessions.mkdir(mode=0o700, exist_ok=True)
        self.session_id = "ses_" + session_id.replace("-", "")
        self.workspace = config.get("workspace", "/workspace")
        self.workspace_info = config.get("workspace_info")
        self.config_home = self.directory / "config"
        global_config = self.config_home / "opencode"
        global_config.mkdir(parents=True, exist_ok=True)
        plugin = global_config / "plugins" / "magicmtn"
        plugin.mkdir(parents=True, exist_ok=True)
        runtime = pathlib.Path(__file__).resolve().parents[1]
        for source, destination in (
            ("opencode.js", "index.js"),
            ("opencode-tui.js", "tui.js"),
            ("broker.js", "broker.js"),
            ("error-worker.js", "error-worker.js"),
        ):
            (plugin / destination).write_text((runtime / source).read_text())
        atomic_json(plugin / "package.json", {
            "name": "magicmtn-opencode-runtime", "type": "module",
            "exports": {".": "./index.js", "./tui": "./tui.js"},
        })
        atomic_json(plugin / "config.json", {
            "origin": config["origin"], "token": config["token"],
            "ca_certificate": config.get("ca_certificate"),
            "error_reporting": config.get("error_reporting", True),
            "session_id": self.session_id,
            "journal": str(self.sessions / "history.jsonl"),
        })

        model = config["model"]
        protocol = model["api"] if config.get("native") else "openai-completions"
        context = model.get("contextWindow", 32768)
        endpoint = config["origin"] + "/v1/harness/agent"
        if protocol == "anthropic-messages":
            endpoint += "/v1"
        atomic_json(global_config / "opencode.json", {
            "model": "magicmtn/" + model["id"],
            "providers": {
                "magicmtn": {
                    "name": "MagicMtn",
                    "package": PACKAGES[protocol],
                    "activation": "enabled",
                    "settings": {"baseURL": endpoint, "apiKey": config["token"]},
                    "models": {model["id"]: {
                        "name": model.get("name", model["id"]),
                        "tool_call": True,
                        "reasoning": model.get("reasoning", False),
                        "limit": {"context": context, "output": min(context, model.get("maxTokens", 4096))},
                    }},
                },
            },
        })

    def command(self):
        return ["opencode", "--standalone", "--session", self.session_id, self.workspace]

    def environment(self):
        values = {
            "XDG_CONFIG_HOME": str(self.config_home),
            "XDG_DATA_HOME": str(self.directory / "data"),
            "XDG_STATE_HOME": str(self.directory / "state"),
            "XDG_CACHE_HOME": str(self.directory / "cache"),
        }
        if self.workspace_info:
            values["MAGICMTN_WORKSPACE_INFO"] = json.dumps(self.workspace_info)
        return values

    def artifacts(self):
        return self.sessions.glob("*.jsonl")
