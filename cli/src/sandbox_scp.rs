use crate::execution;
use crate::Api;
use anyhow::{ensure, Context, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use magicmtn_sdk::ExecResult;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;
use uuid::Uuid;

const CHUNK_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
struct Upload {
    temporary: String,
    destination: String,
}

async fn remote(api: &Api, id: Uuid, operation: Value) -> Result<Value> {
    let body = execution::request(
        include_str!("sandbox_scp.py"),
        &[serde_json::to_string(&operation)?],
        30_000,
    )?;
    let result: ExecResult = serde_json::from_value(
        api.request(Method::POST, &format!("v1/sandboxes/{id}/exec"), Some(body))
            .await?,
    )?;
    ensure!(
        !result.timed_out && !result.truncated && result.exit_code == Some(0),
        "sandbox file transfer failed: {}",
        if result.stderr.is_empty() {
            "execution timed out or returned an incomplete result"
        } else {
            &result.stderr
        }
    );
    serde_json::from_str(&result.stdout).context("invalid sandbox upload response")
}

pub(crate) async fn run(api: &Api, id: &str, path: &Path) -> Result<()> {
    crate::validate_id(id)?;
    ensure!(
        fs::metadata(path)?.is_file(),
        "source must be a regular file"
    );
    let mut file = fs::File::open(path).context("opening local file")?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "source must be a regular file");
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("source filename must be valid UTF-8")?;
    let sandbox = api
        .request(Method::GET, &format!("v1/sandboxes/{id}"), None)
        .await?;
    let id: Uuid =
        serde_json::from_value(sandbox["id"].clone()).context("invalid sandbox identity")?;
    let upload: Upload =
        serde_json::from_value(remote(api, id, json!({"action":"begin","name":name})).await?)?;
    let result = copy(api, id, &mut file, &metadata, &upload).await;
    if result.is_err()
        && remote(
            api,
            id,
            json!({"action":"abort","temporary":upload.temporary}),
        )
        .await
        .is_err()
    {
        eprintln!("Could not remove incomplete upload {}", upload.temporary);
    }
    result.with_context(|| {
        format!(
            "copy to {} failed or could not be confirmed",
            upload.destination
        )
    })?;
    println!("Copied {} to {}", path.display(), upload.destination);
    Ok(())
}

async fn copy(
    api: &Api,
    id: Uuid,
    file: &mut fs::File,
    metadata: &fs::Metadata,
    upload: &Upload,
) -> Result<()> {
    let mut checksum = Sha256::new();
    let mut buffer = [0; CHUNK_BYTES];
    let mut offset = 0_u64;
    loop {
        let count = file.read(&mut buffer).context("reading local file")?;
        if count == 0 {
            break;
        }
        let response = remote(
            api,
            id,
            json!({
                "action":"write", "temporary":upload.temporary, "offset":offset,
                "data":STANDARD.encode(&buffer[..count]),
            }),
        )
        .await?;
        offset += count as u64;
        ensure!(
            response["offset"] == offset,
            "sandbox acknowledged a different upload offset"
        );
        checksum.update(&buffer[..count]);
    }
    ensure!(offset == metadata.len(), "local file changed during copy");
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    };
    #[cfg(not(unix))]
    let mode = 0o644;
    let response = remote(
        api,
        id,
        json!({
            "action":"commit", "temporary":upload.temporary, "destination":upload.destination,
            "size":offset, "sha256":format!("{:x}",checksum.finalize()), "mode":mode,
        }),
    )
    .await?;
    ensure!(
        response["copied"] == true,
        "sandbox did not confirm the file copy"
    );
    Ok(())
}
