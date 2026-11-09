<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="MagicMtn" width="180">
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

## Development

```sh
cargo check --workspace --all-targets --locked
cargo test --workspace --lib --locked
```

The Rust workspace contains the shared engine, provider SDK, CLI, and sandbox runtime. The CLI currently connects to MagicMtn Cloud; local server startup is not available yet.
