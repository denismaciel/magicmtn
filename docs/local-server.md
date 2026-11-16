# Run MagicMtn locally

Run sandboxes on your Mac through a single-user MagicMtn server. The server stores resource records in bundled SQLite and uses providers installed on your computer.

## Prepare your machine

Install the [MagicMtn CLI](../README.md#install) and [Apple Container](https://github.com/apple/container). Start the container runtime:

```sh
container system start
```

Install the Apple Container provider:

```sh
magic provider install apple-container
```

## Start the server

Sign out of MagicMtn Cloud with `magic logout` before starting the local server. Cloud login and the local server use mutually exclusive modes.

```sh
magic server start
```

The server creates `~/.config/magicmtn/local/magicmtn.sqlite3` automatically and listens on `127.0.0.1:47821`. Keep this terminal open. No database service or separate SQLite installation is required.

Local commands use a private credential saved in `~/.config/magicmtn/local/connection.json`. No MagicMtn account or login is required. Use `--local` to select this server; commands without it use MagicMtn Cloud unless you select another server with `--server-url` or `MAGICMTN_SERVER_URL`.

## Create and use a sandbox

In another terminal:

```sh
magic --local sandbox new --image alpine:3.24.1 |
  xargs -I {} magic --local sandbox exec {} -- /bin/sh -lc 'printf "hello from the sandbox\n"'
magic --local sandbox list
```

The execution command prints the exit code, output, and timeout information. Apple Container sandboxes use an isolated network by default. Images are pulled by the host runtime when needed.

Use an ID from `magic --local sandbox list` in `magic --local sandbox show <ID> --format json` to inspect a sandbox, or `magic --local sandbox ssh <ID>` to open an interactive shell. Type `exit` to return to your Mac. To use your ChatGPT subscription and MCP tools inside a sandbox, follow [Use Codex in a local sandbox](local-codex.md).

## Restart and clean up

Run `magic server stop` or press Ctrl+C in the server terminal, then run `magic server start` again. Sandbox records persist, and the lifecycle worker resumes inspection and pending cleanup. Stopping the server leaves running sandboxes available for the next startup.

Delete a sandbox and its files when finished:

```sh
magic --local sandbox terminate <ID> --yes
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
