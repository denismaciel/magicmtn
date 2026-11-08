//! Remove internal ECR locations from account-facing JSON responses.
use serde_json::Value;

fn is_ecr_reference(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.contains(".dkr.ecr.") || value.contains("public.ecr.aws/")
}

pub fn ecr_references(value: &mut Value) {
    match value {
        Value::String(text) if is_ecr_reference(text) => {
            *text = "[redacted image reference]".into();
        }
        Value::Array(items) => {
            for item in items {
                ecr_references(item);
            }
        }
        Value::Object(fields) => {
            let original = std::mem::take(fields);
            for (key, mut item) in original {
                ecr_references(&mut item);
                let key = if is_ecr_reference(&key) {
                    "[redacted image reference]".into()
                } else {
                    key
                };
                fields.insert(key, item);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_nested_ecr_references_without_changing_other_fields() {
        let mut response = json!({
            "template":"image:123456789012.dkr.ecr.us-east-2.amazonaws.com/app@sha256:abc",
            "metadata":{"image":"public.ecr.aws/example/app:latest","status":"running"},
            "templates":{"123456789012.dkr.ecr.us-east-2.amazonaws.com/app@sha256:abc":"template-id"},
            "items":["registry.example.com/app:latest"]
        });
        ecr_references(&mut response);
        let serialized = response.to_string();
        assert!(!serialized.contains("dkr.ecr"));
        assert!(!serialized.contains("public.ecr.aws"));
        assert_eq!(response["metadata"]["status"], "running");
        assert_eq!(response["items"][0], "registry.example.com/app:latest");
    }
}
