"""Shared crate identities for the engine, SDK, and downstream providers."""

def crate_deps(names):
    """Returns dependencies from the core's Cargo.lock-backed crate universe.

    Resolve labels here so downstream modules share the exact same Rust crates
    as the SDK, including types such as anyhow::Error and serde_json::Value.
    """
    return [Label("@crates//:" + name) for name in names]
