"""One sandbox environment for the supervised agent and interactive commands."""
import json
import os
import pathlib
from urllib.parse import urlsplit


def configure(directory, config, harness_environment, atomic_json):
    values = dict(harness_environment)
    # Clear the previous runtime's optional settings even when invoked from an
    # existing managed shell after a configuration refresh.
    unset = ["MAGICMTN_WORKSPACE_INFO", "MAGICMTN_INTEGRATIONS_CONFIG",
             "NODE_EXTRA_CA_CERTS", "GIT_CONFIG_GLOBAL", "GIT_CONFIG_NOSYSTEM", "GIT_CONFIG_COUNT"]
    for index in range(5):
        unset.extend([f"GIT_CONFIG_KEY_{index}", f"GIT_CONFIG_VALUE_{index}"])
    if config.get("integrations_origin"):
        path = directory / "integrations.json"
        atomic_json(path, {
            "origin": config["integrations_origin"], "token": config["token"],
            "github_enabled": config.get("github_enabled", True),
            "ca_certificate": config.get("ca_certificate"),
        })
        values["MAGICMTN_INTEGRATIONS_CONFIG"] = str(path)
        if config.get("github_enabled", True):
            unset.extend(["GH_TOKEN", "GITHUB_TOKEN"])
            origin = urlsplit(config["integrations_origin"])
            if origin.hostname:
                gitconfig = directory / "gitconfig"
                gitconfig.touch(mode=0o600)
                values.update(GIT_CONFIG_GLOBAL=str(gitconfig), GIT_CONFIG_NOSYSTEM="1",
                              GIT_CONFIG_COUNT="5",
                              GIT_CONFIG_KEY_0=f"credential.{origin.scheme}://{origin.netloc}.helper",
                              GIT_CONFIG_VALUE_0="!magicmtn-git-credential",
                              GIT_CONFIG_KEY_1="credential.useHttpPath", GIT_CONFIG_VALUE_1="true")
                gateway = config["integrations_origin"].rstrip("/") + "/"
                for index, source in enumerate(("https://github.com/", "git@github.com:", "ssh://git@github.com/"), start=2):
                    values[f"GIT_CONFIG_KEY_{index}"] = f"url.{gateway}.insteadOf"
                    values[f"GIT_CONFIG_VALUE_{index}"] = source
    if config.get("ca_certificate"):
        certificate = directory / "control-plane-ca.pem"
        certificate.write_text(config["ca_certificate"])
        values["NODE_EXTRA_CA_CERTS"] = str(certificate)
    specification = {"set": values, "unset": unset}
    atomic_json(pathlib.Path.home() / ".magicmtn" / "environment.json", specification)
    return specification


def apply(specification, base=None):
    values = dict(os.environ if base is None else base)
    for key in specification["unset"]:
        values.pop(key, None)
    values.update(specification["set"])
    return values


def load():
    path = pathlib.Path.home() / ".magicmtn" / "environment.json"
    return apply(json.loads(path.read_text()))
