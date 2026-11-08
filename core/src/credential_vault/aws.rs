//! AWS KMS data-key provider. Enabled only for trusted hosts via the
//! `credential-vault-aws` feature. No credentials are loaded by sandbox runtimes.

use super::*;
use aws_sdk_kms::{
    config::{retry::RetryConfig, timeout::TimeoutConfig},
    primitives::Blob,
    types::DataKeySpec,
    Client,
};
use std::{collections::BTreeSet, time::Duration};

pub struct AwsKmsDataKeyProvider {
    client: Client,
    active_key_arn: String,
    decryption_keys: BTreeSet<String>,
}

impl AwsKmsDataKeyProvider {
    /// Keep old key ARNs allowed until all credentials/backups encrypted with
    /// them have expired or been migrated. IAM must allow only these keys too.
    pub fn new(
        client: Client,
        active_key_arn: String,
        additional_decryption_keys: Vec<String>,
    ) -> Result<Self> {
        if !key_arn(&active_key_arn) || !additional_decryption_keys.iter().all(|key| key_arn(key)) {
            return Err(VaultError::InvalidInput);
        }
        let mut decryption_keys: BTreeSet<_> = additional_decryption_keys.into_iter().collect();
        decryption_keys.insert(active_key_arn.clone());
        Ok(Self {
            client,
            active_key_arn,
            decryption_keys,
        })
    }
}

fn key_arn(value: &str) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 6
        && parts[0] == "arn"
        && parts[1].starts_with("aws")
        && parts[2] == "kms"
        && !parts[3].is_empty()
        && parts[4].len() == 12
        && parts[4].bytes().all(|b| b.is_ascii_digit())
        && parts[5].starts_with("key/")
        && parts[5].len() > 4
}

#[async_trait]
impl DataKeyProvider for AwsKmsDataKeyProvider {
    async fn generate(&self, context: &EncryptionContext) -> Result<DataKey> {
        let mut output = self
            .client
            .generate_data_key()
            .key_id(&self.active_key_arn)
            .key_spec(DataKeySpec::Aes256)
            .set_encryption_context(Some(context.values().clone().into_iter().collect()))
            .send()
            .await
            .map_err(|_| VaultError::Unavailable)?;
        let plaintext = SecretBytes::new(
            output
                .plaintext
                .take()
                .ok_or(VaultError::Integrity)?
                .into_inner(),
        );
        let key_id = output.key_id.take().ok_or(VaultError::Integrity)?;
        let encrypted = output
            .ciphertext_blob
            .take()
            .ok_or(VaultError::Integrity)?
            .into_inner();
        if key_id != self.active_key_arn
            || plaintext.expose_secret().len() != 32
            || encrypted.is_empty()
        {
            return Err(VaultError::Integrity);
        }
        Ok(DataKey {
            plaintext,
            encrypted,
            key_id,
        })
    }

    async fn decrypt(
        &self,
        context: &EncryptionContext,
        key_id: &str,
        encrypted: &[u8],
    ) -> Result<SecretBytes> {
        if !self.decryption_keys.contains(key_id) {
            return Err(VaultError::Integrity);
        }
        let mut output = self
            .client
            .decrypt()
            .key_id(key_id)
            .ciphertext_blob(Blob::new(encrypted))
            .set_encryption_context(Some(context.values().clone().into_iter().collect()))
            .send()
            .await
            .map_err(|error| match error.as_service_error() {
                Some(service)
                    if service.is_invalid_ciphertext_exception()
                        || service.is_incorrect_key_exception() =>
                {
                    VaultError::Integrity
                }
                _ => VaultError::Unavailable,
            })?;
        let plaintext = SecretBytes::new(
            output
                .plaintext
                .take()
                .ok_or(VaultError::Integrity)?
                .into_inner(),
        );
        if output.key_id.as_deref() != Some(key_id) || plaintext.expose_secret().len() != 32 {
            return Err(VaultError::Integrity);
        }
        Ok(plaintext)
    }
}

/// Explicit opt-in for the future integration broker; not invoked by the public
/// control plane or existing inference credential path. Uses the AWS SDK's
/// default region/credential chain (including workload roles).
///
/// Required: MAGICMTN_CREDENTIAL_VAULT_KMS_KEY_ARN.
/// Optional: MAGICMTN_CREDENTIAL_VAULT_KMS_DECRYPT_KEY_ARNS, a JSON string array.
pub async fn from_environment(pool: PgPool) -> Result<PostgresKmsVault> {
    let active = std::env::var("MAGICMTN_CREDENTIAL_VAULT_KMS_KEY_ARN")
        .map_err(|_| VaultError::InvalidInput)?;
    let additional = match std::env::var("MAGICMTN_CREDENTIAL_VAULT_KMS_DECRYPT_KEY_ARNS") {
        Ok(value) => serde_json::from_str(&value).map_err(|_| VaultError::InvalidInput)?,
        Err(std::env::VarError::NotPresent) => vec![],
        Err(_) => return Err(VaultError::InvalidInput),
    };
    // Validate before invoking the SDK credential provider chain.
    if !key_arn(&active) || !additional.iter().all(|key: &String| key_arn(key)) {
        return Err(VaultError::InvalidInput);
    }
    let configuration = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .retry_config(RetryConfig::standard().with_max_attempts(3))
        .timeout_config(
            TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(10))
                .build(),
        )
        .load()
        .await;
    let keys = AwsKmsDataKeyProvider::new(Client::new(&configuration), active, additional)?;
    Ok(PostgresKmsVault::new(pool, Arc::new(keys)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_kms::config::{BehaviorVersion, Credentials, Region};
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        routing::post,
        Json, Router,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde_json::{json, Value};
    use tokio::sync::Mutex;

    const KEY: &str = "arn:aws:kms:us-east-1:111122223333:key/fixture";
    const OLD: &str = "arn:aws:kms:us-east-1:111122223333:key/previous";

    #[derive(Default)]
    struct MockKms {
        calls: Mutex<Vec<(HeaderMap, Value)>>,
        response_key: Mutex<Option<String>>,
        denied: std::sync::atomic::AtomicBool,
        corrupt: std::sync::atomic::AtomicBool,
    }

    async fn respond(
        State(mock): State<Arc<MockKms>>,
        headers: HeaderMap,
        bytes: Bytes,
    ) -> (StatusCode, Json<Value>) {
        // AWS uses application/x-amz-json-1.1 rather than application/json.
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let generate = headers["x-amz-target"]
            .to_str()
            .unwrap()
            .ends_with("GenerateDataKey");
        let key = mock
            .response_key
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| body["KeyId"].as_str().unwrap().into());
        mock.calls.lock().await.push((headers, body));
        if mock.corrupt.load(std::sync::atomic::Ordering::SeqCst) {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({"__type":"InvalidCiphertextException","message":"do-not-reflect-this"}),
                ),
            );
        }
        if mock.denied.load(std::sync::atomic::Ordering::SeqCst) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"__type":"AccessDeniedException","message":"do-not-reflect-this"})),
            );
        }
        let mut output = json!({"Plaintext":STANDARD.encode([1u8;32]),"KeyId":key});
        if generate {
            output["CiphertextBlob"] = json!(STANDARD.encode(b"wrapped-key"));
        }
        (StatusCode::OK, Json(output))
    }

    #[tokio::test]
    async fn sdk_signing_context_key_allowlist_and_redacted_failures() {
        let mock = Arc::new(MockKms::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/", post(respond))
            .with_state(mock.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let configuration = aws_sdk_kms::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                "fixture-access",
                "fixture-secret",
                None,
                None,
                "test",
            ))
            .endpoint_url(format!("http://{address}"))
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(3))
                    .build(),
            )
            .build();
        let keys = AwsKmsDataKeyProvider::new(
            Client::from_conf(configuration),
            KEY.into(),
            vec![OLD.into()],
        )
        .unwrap();
        let scope = CredentialScope {
            organization_id: Uuid::new_v4(),
            owner_user_id: None,
            connection_id: Uuid::new_v4(),
            purpose: "github.oauth".into(),
        };
        let context = EncryptionContext::new(&scope, Uuid::new_v4(), 3);
        let generated = keys.generate(&context).await.unwrap();
        assert_eq!(generated.key_id, KEY);
        assert_eq!(generated.plaintext.expose_secret(), &[1u8; 32]);
        assert_eq!(
            keys.decrypt(&context, KEY, &generated.encrypted)
                .await
                .unwrap()
                .expose_secret(),
            &[1u8; 32]
        );
        keys.decrypt(&context, OLD, &generated.encrypted)
            .await
            .unwrap();
        {
            let calls = mock.calls.lock().await;
            assert_eq!(calls.len(), 3);
            for (headers, body) in calls.iter() {
                assert!(headers["authorization"]
                    .to_str()
                    .unwrap()
                    .starts_with("AWS4-HMAC-SHA256"));
                assert_eq!(body["EncryptionContext"], json!(context.values()));
            }
            assert_eq!(calls[0].1["KeySpec"], "AES_256");
            assert_eq!(
                calls[1].1["CiphertextBlob"],
                STANDARD.encode(b"wrapped-key")
            );
        }
        assert_eq!(
            keys.decrypt(
                &context,
                "arn:aws:kms:us-east-1:111122223333:key/unapproved",
                b"anything"
            )
            .await
            .unwrap_err(),
            VaultError::Integrity
        );
        assert_eq!(mock.calls.lock().await.len(), 3);
        *mock.response_key.lock().await = Some(OLD.into());
        assert!(matches!(
            keys.generate(&context).await,
            Err(VaultError::Integrity)
        ));
        assert_eq!(
            keys.decrypt(&context, KEY, b"anything").await.unwrap_err(),
            VaultError::Integrity
        );
        mock.denied.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            keys.generate(&context).await,
            Err(VaultError::Unavailable)
        ));
        assert_eq!(
            keys.decrypt(&context, KEY, b"anything").await.unwrap_err(),
            VaultError::Unavailable
        );
        mock.corrupt
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            keys.decrypt(&context, KEY, b"anything").await.unwrap_err(),
            VaultError::Integrity
        );
        assert!(!VaultError::Unavailable
            .to_string()
            .contains("do-not-reflect-this"));
        assert!(!key_arn("alias/magicmtn"));
        server.abort();
    }
}
