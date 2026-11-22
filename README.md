<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="MagicMtn logo" width="180" height="180">
  </picture>
</p>

# MagicMtn

Run coding agents in isolated workspaces with MagicMtn's CLI, sandbox engine, and provider SDK.

## Install from source

Use Rust 1.99.0 or a later stable release. From the repository root, install the CLI and check its version.

```sh
cargo install --path cli --locked --bin magic
magic --version
```

Cargo installs the executable into `~/.cargo/bin` by default. Add that directory to your PATH if your shell cannot find `magic`.

## Run a local sandbox

On Apple silicon, use Apple Container and the built-in SQLite server to create, execute a command in, and terminate a sandbox. Follow the [local quickstart](docs/local-server.md).

The local server also exposes sandbox tools to Pi and OpenCode. With the local server running, use `magic agents install pi` or `magic agents install opencode`. Start your agent and use `/magic new` and `/magic view` to work with sandboxes.

## Connect to hosted MagicMtn

Use `magic login` to connect your CLI to a hosted account. Then install the integration for your agent with `magic agents install pi` or `magic agents install opencode`. For OpenCode, use version 2.3.2 or later. Hosted and local modes have separate credentials.

CLI [error reporting](docs/error-reporting.md) is opt-in.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) to build and test the engine and SDK. Send vulnerability reports through the private channel in [SECURITY.md](SECURITY.md).

MagicMtn is licensed under the [Apache License 2.0](LICENSE). Third-party components retain their own licenses.
