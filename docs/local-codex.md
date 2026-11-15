# Use Codex in a local sandbox

Run Codex with your ChatGPT subscription inside a MagicMtn sandbox on your Mac. Codex works with the sandbox's files, shell, and MCP tools. Your Codex login stays in the local sandbox.

## Start MagicMtn

Install the [MagicMtn CLI](../README.md#install) and [Apple Container](https://github.com/apple/container), then run:

```sh
container system start
magic provider install apple-container
magic logout
magic server start --network default
```

Keep the server terminal open. The network setting gives new sandboxes internet access. To change it on an existing local installation, stop the server first with `magic server stop`, then start it with the command above.

## Create a sandbox and sign in

In another terminal:

```sh
sandbox_id=$(magic --local sandbox new --image node:24-alpine)
magic --local sandbox ssh "$sandbox_id"
```

Inside the sandbox:

```sh
npm install --global @openai/codex@0.169.0
codex login --device-auth
```

Open the link Codex prints in your Mac's browser and enter the code. Enable device-code login in your ChatGPT security settings or workspace permissions before signing in.

Start working:

```sh
mkdir -p /workspace
cd /workspace
codex
```

Use `/quit` to leave Codex and `exit` to return to your Mac. Reopen the same sandbox to keep using its files and login:

```sh
magic --local sandbox ssh "$sandbox_id"
```

## Add MCP tools

Inside the sandbox, add an MCP server to Codex. For example, connect OpenAI's documentation server:

```sh
codex mcp add openai-docs --url https://developers.openai.com/mcp
codex mcp list
codex
```

Use your chosen MCP server's URL to connect other tools.

## Finish

Run `codex logout` inside the sandbox to sign out. To delete the sandbox and its files, run this on your Mac:

```sh
magic --local sandbox terminate "$sandbox_id" --yes
```

Sandboxes have a 24-hour default lifetime. See [Run MagicMtn locally](local-server.md) for server settings and cleanup.
