# Run MagicMtn locally

Run sandboxes on your Mac through a single-user MagicMtn server. The server stores resource records in bundled SQLite and uses providers installed on your computer.

## Prepare your machine

Install the [MagicMtn CLI](../README.md#install) and [Apple Container](https://github.com/apple/container). Start the container runtime:

```sh
container system start
```

Install the Apple Container provider and MagicMtn Canonical image:

```sh
magic provider install apple-container
```

## Start the server

Sign out of MagicMtn Cloud with `magic logout` before starting the local server. Cloud login and the local server use mutually exclusive modes.

```sh
magic server start
```

The server creates `~/.config/magicmtn/local/magicmtn.sqlite3` automatically and listens on `127.0.0.1:47821`. Keep this terminal open. No database service or separate SQLite installation is required.

Starting the server selects local mode for the CLI and Pi extension. Commands automatically use its saved address and private credential from `~/.config/magicmtn/local/connection.json`. No MagicMtn account, subscription, or login is required. Local mode stays selected after the server stops; stop the server and run `magic login` to switch to Cloud.

A developer’s Cloud server is a separate application. Use `--server-url` to target it explicitly. Its login and saved credentials are independent of the single-user local server. Choosing a local sandbox provider while using Cloud changes workload placement and keeps the Cloud server selected.

## Create and use a sandbox

List images available on your machine:

```sh
magic images list
```

In another terminal:

```sh
magic sandbox new |
  xargs -I {} magic sandbox exec {} -- /bin/sh -lc 'printf "hello from the sandbox\n"'
magic sandbox list
```

New sandboxes use the MagicMtn Canonical image bundled with your CLI release. Canonical includes Python, Node.js, Pi, Codex, Git, Jujutsu, and GitHub CLI. Provider installation downloads it into Apple Container's local image store. Sandbox creation restores the image automatically if it has been removed.

Use a custom image with `magic sandbox new --image alpine:3.24.1`. Image references can use registry tags or immutable digests. Each Canonical sandbox records its exact image digest and retains it across restarts and CLI upgrades.

The execution command prints the exit code, output, and timeout information. Apple Container sandboxes use an isolated network by default. Images are pulled by the host runtime when needed.

Use an ID from `magic sandbox list` in `magic sandbox show <ID> --format json` to inspect a sandbox, or `magic sandbox ssh <ID>` to open an interactive shell. Type `exit` to return to your Mac. To use your ChatGPT subscription and MCP tools inside a sandbox, follow [Use Codex in a local sandbox](local-codex.md).

## Use Pi with sandbox tools

Install the Pi extension in local mode, then start Pi:

```sh
magic agents install pi
pi
```

Pi stays on your computer and uses its own model and provider login. Select your model with `/model`. Use `/magic new` to create a sandbox and `/magic view` to manage existing sandboxes. Pi can create, inspect, execute commands in, read and write files in, and terminate sandboxes through the MagicMtn tools. Its built-in shell and file tools continue to work on your computer.

## Restart and clean up

Run `magic server stop` or press Ctrl+C in the server terminal, then run `magic server start` again. Sandbox records persist, and the lifecycle worker resumes inspection and pending cleanup. Stopping the server leaves running sandboxes available for the next startup.

Delete a sandbox and its files when finished:

```sh
magic sandbox terminate <ID> --yes
```

Sandboxes have a 24-hour default lifetime. Set `magic server start --ttl-seconds SECONDS` to choose a lifetime between 1 second and 24 hours for new sandboxes. The lifecycle worker cleans up expired sandboxes while the server is running and resumes that work after restart.

Run `magic server stop` before signing in to MagicMtn Cloud with `magic login`.

Stop the server before installing or uninstalling providers. Keep provider releases referenced by active sandboxes until those sandboxes are deleted.

## Configuration

| Setting | Purpose |
| --- | --- |
| `--network default` | Give new Apple Container sandboxes internet access. Use `--network none` for an isolated network. |
| `--bind 127.0.0.1:PORT` | Choose the local server port. The listener must use a loopback address. |
| `MAGICMTN_CONFIG_DIRECTORY` | Change the CLI configuration and installed-provider directory from `~/.config/magicmtn`. |
| `MAGICMTN_LOCAL_DIRECTORY` | Override the local server's database, credential, and lock directory. Set it for both the server and local CLI commands. |

Keep the local directory to retain your sandbox records and server credential. For a backup, stop the server and copy the entire directory. Each directory belongs to one local installation.
