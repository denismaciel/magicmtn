<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="MagicMtn logo" width="180" height="180">
  </picture>
</p>

<p align="center"><strong>MagicMtn - Secure inference and compute for coding agents</strong></p>

## Install

Install the command-line interface:

```sh
curl -fsSL https://control.magicmtn.ai/install.sh | sh
```

Or via Homebrew:

```sh
brew install UseMagicMtn/tap/magic
```

The installer verifies the release and installs `magic` in `~/.local/bin`. It prompts to configure your shell's PATH and completions. Restart your shell after configuration, then sign in:

```sh
magic login
```

To use MagicMtn with Pi, install the integration, then run `/reload` in Pi:

```sh
magic agents install pi
```

In Pi, `/magic new` starts a workspace using your saved defaults. Use
`/magic view` to reconnect to an existing sandbox.

## Run locally

Run a single-user server and Apple Container sandboxes using your own machine and bundled SQLite. Follow [Run MagicMtn locally](docs/local-server.md) to install the provider, start the server, and execute commands in a sandbox.

## Development

Run the engine and SDK tests with [Bazelisk](https://github.com/bazelbuild/bazelisk#installation):

```sh
bazel test //...
```

See [the Bazel guide](docs/bazel.md) for reusable targets and cross-repository builds.
For Cargo development:

```sh
cargo check --workspace --all-targets --locked
cargo test --workspace --lib --locked
```

The Rust workspace contains the shared engine, provider SDK, CLI, local server, and sandbox runtime. MagicMtn Cloud consumes the same engine and sandbox API.
