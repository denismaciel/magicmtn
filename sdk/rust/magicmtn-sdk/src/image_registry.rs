//! Provider-neutral image storage for MagicMtn.
//!
//! A configured backend scopes its repository inventory and owns credentials.
//! The caller authorizes user selections, freezes resolved images in harness
//! revisions, and owns staging directories. Building and sandbox execution are
//! separate compute operations. These traits do not change the executable wire
//! protocol or automatically integrate registry retrieval with session startup.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::BoxFuture;

/// An immutable image identity, suitable for freezing in a harness revision.
///
/// `repository` is a non-secret logical name or registry/repository locator.
/// `digest` identifies the image manifest or multi-platform index, never a tag.
/// Implementations validate both fields and reject repositories outside their
/// configured scope. Possession of this value does not grant access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub repository: String,
    pub digest: String,
}

/// Store and retrieve built container images through a provider-owned registry.
pub trait ImageRegistryBackend: Send + Sync {
    /// List selectable repository/tag or repository/digest references.
    /// Provider pagination is handled internally; ordering is provider-defined.
    fn list<'a>(&'a self) -> BoxFuture<'a, Vec<String>>;

    /// Resolve an explicit `repository:tag` or `repository@digest` to an image.
    /// No implicit `latest`; invalid or missing references return an error.
    fn resolve<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Image>;

    /// Retrieve the exact image as a complete OCI image layout.
    ///
    /// `destination` must be absent or an empty directory with an existing
    /// parent. Success means one root descriptor and all referenced manifests,
    /// configurations, and layers are present with verified digests and sizes.
    /// Multi-platform indexes retain all their referenced platform images.
    /// No unrelated files are overwritten; failure must not expose a partial
    /// layout at the destination. The caller owns the completed directory.
    fn pull<'a>(&'a self, image: &'a Image, destination: &'a Path) -> BoxFuture<'a, ()>;

    /// Publish a complete OCI layout containing exactly one root descriptor.
    ///
    /// `reference` is an explicit tag or digest in a configured repository.
    /// Tags cannot be rebound to different contents. Publishing the same image
    /// to the same reference is idempotent; return the verified image identity.
    /// A digest reference must match the source manifest/index digest.
    fn publish<'a>(&'a self, source: &'a Path, reference: &'a str) -> BoxFuture<'a, Image>;

    /// Delete the image and all of its tag bindings, preserving other images.
    /// Missing images return an error. The caller checks harness retention and
    /// deletion permissions; the registry has no knowledge of harness revisions.
    fn delete<'a>(&'a self, image: &'a Image) -> BoxFuture<'a, ()>;
}
