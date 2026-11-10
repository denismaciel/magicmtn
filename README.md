<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="MagicMtn mountain" width="180" height="180">
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
`/magic relay` from a Git working tree to start a workspace and copy the current
tracked files in the background using sandbox SCP. The copy includes staged and
unstaged changes and the `.git` and `.jj` metadata directories within the current
directory; it excludes untracked files, deleted files, and submodule contents.
Pi opens without waiting for the upload and
shows a temporary copying notice until the files arrive. Transfer failures stay
visible in Pi, with details in the local transfer log printed during connection.

## Development

```sh
cargo check --workspace --all-targets --locked
cargo test --workspace --lib --locked
```

The Rust workspace contains the shared engine, provider SDK, CLI, and sandbox runtime. The CLI currently connects to MagicMtn Cloud; local server startup is not available yet.
