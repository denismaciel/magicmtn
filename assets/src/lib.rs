pub const PI_WORKSPACE_EXTENSION: &str = include_str!("../../interfaces/pi/workspace.js");
pub const PROVIDER_CATALOG: &str = include_str!("../../registry/providers.json");
pub const CLI_INSTALLER: &str = include_str!("../../scripts/install-public.sh");
pub const CLI_TRUST: &str = include_str!("../../release/cli-trust.json");

#[cfg(feature = "test-fixtures")]
pub mod fixtures {
    pub const COMPUTE_VECTORS: &str = include_str!("../../tests/fixtures/compute-vectors.json");
    pub const MULTIMODEL_ARTIFACTS: &str =
        include_str!("../../tests/fixtures/harness-models-multimodel-artifacts.example.json");
}
