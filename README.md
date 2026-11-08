<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/logo-dark.svg">
    <img src=".github/assets/logo-light.svg" alt="MagicMtn" width="180">
  </picture>
</p>

<p align="center"><strong>MagicMtn - Secure inference and compute for coding agents</strong></p>

## Development

```sh
cargo check --workspace --all-targets --locked
cargo test --workspace --lib --locked
```

The Rust workspace contains the shared engine, provider SDK, CLI, and sandbox runtime. The CLI currently connects to MagicMtn Cloud; local server startup is not available yet.
