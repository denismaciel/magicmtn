# Build and test with Bazel

Run the shared engine, contracts, provider framework, and SDK tests from a clean
checkout. Bazel downloads the pinned Rust toolchain and Cargo dependencies; the
build needs no private repository or cloud credentials.

Install [Bazelisk](https://github.com/bazelbuild/bazelisk#installation), then run:

```sh
bazel test //...
```

`.bazelversion` selects Bazel. `MODULE.bazel` selects the Rust and Python
toolchains and build rules. Cargo remains the dependency declaration for the
public Rust workspace. CLI release verification uses its existing Cargo CI job.

## Build a specific component

```sh
bazel build //core:engine
bazel test //sdk/rust/magicmtn-sdk:wire_test
```

The reusable library targets are `//core:engine`,
`//sdk/rust/magicmtn-contracts:contracts`, `//sdk/rust/magicmtn-sdk:sdk`, and
`//sdk/rust/magicmtn-provider-framework:provider_framework`. The framework target
includes its `host` feature. Engine targets use the default Cargo feature set.

## Consume the core from another repository

Declare a `magicmtn` dependency in the consumer's `MODULE.bazel` and pin its source
with `git_override` to a full commit SHA containing the Bazel files. Reference
the exported targets with the `@magicmtn` prefix, for example
`@magicmtn//core:engine`.

Use `crate_deps` from `@magicmtn//bazel/rust:deps.bzl` for the pilot's shared
third-party Rust dependencies. It resolves names against the core's locked crate
universe, so SDK and consumer types have the same crate identity. The available
names are the public workspace's direct third-party dependencies. Changes to
their features or versions belong in Cargo manifests and the generated lock.

Test a local core checkout from the consumer:

```sh
bazel test --override_module=magicmtn=../magicmtn //...
```

This override applies only to the invocation. Commit source pins for CI;
keep local overrides in an ignored `.bazelrc.local` when useful.

## Update Rust dependencies

Update the Cargo manifests and `Cargo.lock`, then regenerate the Bazel rendering:

```sh
CARGO_BAZEL_REPIN=1 bazel test //...
```

Review and commit `Cargo.lock`, `Cargo.Bazel.lock`, and `MODULE.bazel.lock` with
the manifest changes. The generated `Cargo.Bazel.lock` lets downstream consumers
use the core without running Cargo dependency resolution. Bazel may add
platform-specific extension entries to `MODULE.bazel.lock` on another host.

## CI

The `Bazel core` workflow runs on pull requests and pushes to `main`, using native
Linux x64, Linux ARM64, and macOS ARM64 runners. It tests the public repository
independently. Only pushes to `main` save the workflow's build caches.
