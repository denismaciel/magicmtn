# Run MagicMtn locally

Run sandboxes on your Mac through a single-user MagicMtn server. The server stores resource records in bundled SQLite and uses providers installed on your computer.

## Prepare your machine

Install the [MagicMtn CLI](../README.md#install-from-source) and [Apple Container](https://github.com/apple/container). Start the container runtime:

```sh
container system start
```

Install the Apple Container provider and MagicMtn Canonical image:

```sh
magic provider install apple-container
```

## Start the server

If you previously signed in to hosted MagicMtn, run `magic logout`. Start the local server in its own terminal.

```sh
magic server start
```

The server creates `~/.config/magicmtn/local/magicmtn.sqlite3` automatically and listens on `127.0.0.1:47821`. Keep this terminal open. No database service or separate SQLite installation is required.

Starting the server selects local mode for the CLI and Pi extension. Commands use the server address and credential saved in `~/.config/magicmtn/local/connection.json`. Local mode uses your machine and works without a hosted account.

## Create and use a sandbox

In another terminal, create a sandbox with a small Alpine image, run a command, and clean up.

```sh
sandbox_id=$(magic --local sandbox new --image alpine:3.24.1)
magic --local sandbox exec "$sandbox_id" -- /bin/echo "hello from the sandbox"
magic --local sandbox terminate "$sandbox_id" --yes
```

To use the bundled Canonical image, run `magic --local sandbox new`. Use `magic --local images list` to list available images and `magic --local sandbox list` to list running sandboxes. To inspect a specific sandbox, run `magic --local sandbox show SANDBOX_ID --format json`.

Apple Container sandboxes use an isolated network by default. The host pulls sandbox images as needed. A custom image reference can use a registry tag or an immutable digest.

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

Run `magic server stop` before signing in to hosted MagicMtn with `magic login`.

Stop the server before installing or uninstalling providers. Keep provider releases referenced by active sandboxes until those sandboxes are deleted.

## Configuration

| Setting                     | Purpose                                                                                                                  |
| --------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| `--network default`         | Give new Apple Container sandboxes internet access. Use `--network none` for an isolated network.                        |
| `--bind 127.0.0.1:PORT`     | Choose the local server port. The listener must use a loopback address.                                                  |
| `MAGICMTN_CONFIG_DIRECTORY` | Change the CLI configuration and installed-provider directory from `~/.config/magicmtn`.                                 |
| `MAGICMTN_LOCAL_DIRECTORY`  | Override the local server's database, credential, and lock directory. Set it for both the server and local CLI commands. |

Keep the local directory to retain your sandbox records and server credential. For a backup, stop the server and copy the entire directory. Each directory belongs to one local installation.
