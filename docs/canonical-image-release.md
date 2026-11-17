# Release MagicMtn Canonical

Canonical is built from `images/canonical/Containerfile` and published to the public OCI repository `ghcr.io/usemagicmtn/canonical`. It includes the sandbox runtime and pinned Pi and Codex installations.

## Release with the CLI

Run the **Publish CLI binaries** workflow on `main`. It calls **Publish Canonical image**, builds and smoke-tests Linux ARM64 and AMD64 images on native runners, publishes them to GHCR, and verifies anonymous access to their manifests. The publication produces a `canonical-image-lock` artifact. Both the Mac and Linux CLI build jobs download that same lock before compiling, so every binary in a release embeds the same image selection.

The lock is `sdk/rust/magicmtn-sdk/src/canonical-image.json`. It maps OCI platforms to immutable manifest digests. The container runtime selects and stores the host's image locally. Provider installation pulls the pinned ARM64 image into Apple Container. Local CLI and MCP creation use the server's default, which is frozen in the sandbox record before the provider launches it. Replaying an existing creation intent retains its saved digest.

## GHCR access

The workflow uses its repository's `GITHUB_TOKEN` with `packages: write`. Set the `canonical` container package under the UseMagicMtn organization to **Public** in GitHub's package settings. Publication verifies access without publisher credentials before emitting the lock; the CLI release depends on successful publication.

Release tags contain the source commit, workflow run ID, and attempt. A multi-platform tag identifies both native builds. CLI binaries launch the pinned platform manifests directly.

## Publish from a release machine

Authenticate Podman to GHCR with a publisher credential that has `write:packages`. Build each architecture on a native host:

```sh
python3 scripts/build-canonical-image.py \
  --platform linux/arm64 --tag localhost/magicmtn/canonical:release-arm64 \
  --output dist/canonical
```

Use `linux/amd64` and `release-amd64` on the AMD64 host. Collect both OCI archives and metadata files into `dist/canonical`, then publish:

```sh
python3 scripts/publish-canonical-image.py \
  dist/canonical/canonical-arm64.oci.json \
  dist/canonical/canonical-amd64.oci.json \
  --tag release-VERSION
```

The publisher validates the archives and source hashes before uploading. It writes the lock atomically after both platform images and their index pass anonymous manifest verification. Commit the resulting lock to select that image release for development builds.

## Custom images and providers

`magic sandbox new --image REFERENCE` passes the user's selection to the provider. Explicit selections bypass Canonical setup. Apple Container owns registry authentication, downloads, and storage for custom OCI images.

Firecracker owns prepared VM root filesystems and its image mapping through `MAGICMTN_FIRECRACKER_IMAGES`; map the Canonical reference to a prepared VM image to select it on that provider. `magic images list` shows the local provider's inventory.
