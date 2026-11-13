//! Small stdio MCP bridge over the authenticated sandbox API. No inference runs here.
use crate::Api;
use anyhow::{bail, ensure, Context, Result};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

#[derive(Subcommand)]
pub enum Commands {
    /// Expose sandbox tools over MCP stdio (stdout is reserved for JSON-RPC)
    Serve,
}

const FILE_LIMIT: usize = 16 * 1024;
const MESSAGE_LIMIT: u64 = 1024 * 1024;

fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read: bool,
    destructive: bool,
) -> Value {
    json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":read,"destructiveHint":destructive,"idempotentHint":read,"openWorldHint":true}})
}
fn tools() -> Value {
    let id = json!({"type":"string","description":"Sandbox UUID or unique hexadecimal prefix"});
    let path = json!({"type":"string","description":"Absolute path inside the sandbox"});
    json!([
        tool("sandbox_list", "List MagicMtn sandboxes. No files or credentials are copied locally.", json!({}), &[], true, false),
        tool("sandbox_create", "Create a managed sandbox using MagicMtn Canonical and automatic placement. Keep the ID. On uncertainty, inspect that ID instead of creating a replacement.", json!({"idempotency_key":{"type":"string","format":"uuid","description":"Reuse the same UUID when retrying one creation intent; omitted generates a new intent"}}), &[], false, false),
        tool("sandbox_show", "Inspect a sandbox's state and lifetime.", json!({"id":id}), &["id"], true, false),
        tool("sandbox_exec", "Run a shell command remotely in the sandbox. Each call starts a fresh shell. Inspect exit_code, timed_out and truncated. Never blindly retry a mutating command after a timeout or transport failure.", json!({"id":id,"command":{"type":"string"},"cwd":{"type":"string","default":"/workspace"},"timeout_ms":{"type":"integer","minimum":1,"maximum":30000,"default":30000}}), &["id","command"], false, true),
        tool("sandbox_read_file", "Read a UTF-8 file inside the sandbox (maximum 16 KiB).", json!({"id":id,"path":path}), &["id","path"], true, false),
        tool("sandbox_write_file", "Write UTF-8 text inside the sandbox (maximum 16 KiB), replacing the file. Parent directories must exist. Does not read any local file.", json!({"id":id,"path":path,"content":{"type":"string"}}), &["id","path","content"], false, true),
        tool("sandbox_terminate", "Delete a sandbox and its files. Only use when the user authorizes deletion.", json!({"id":id}), &["id"], false, true)
    ])
}
fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("{key} must be a string"))
}
fn summary(record: &Value) -> Value {
    // Never expose internal connection snapshots or provider credentials to the model.
    let mut result = serde_json::Map::new();
    for key in [
        "id",
        "status",
        "notes",
        "created_at",
        "expires_at",
        "runtime_state",
        "shutdown_reason",
    ] {
        if let Some(value) = record.get(key) {
            result.insert(key.into(), value.clone());
        }
    }
    Value::Object(result)
}
fn validate_args(name: &str, args: &Value) -> Result<()> {
    let definitions = tools();
    let definition = definitions
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == name)
        .context("unknown tool")?;
    let object = args.as_object().context("arguments must be an object")?;
    let properties = definition["inputSchema"]["properties"].as_object().unwrap();
    for (key, value) in object {
        let schema = properties
            .get(key)
            .with_context(|| format!("unknown argument: {key}"))?;
        match schema["type"].as_str() {
            Some("string") => ensure!(value.is_string(), "{key} must be a string"),
            Some("integer") => {
                ensure!(value.as_u64().is_some(), "{key} must be a positive integer")
            }
            _ => bail!("unsupported argument type"),
        }
    }
    for key in definition["inputSchema"]["required"].as_array().unwrap() {
        ensure!(
            object.contains_key(key.as_str().unwrap()),
            "missing argument: {key}"
        );
    }
    Ok(())
}
async fn call(api: &Api, name: &str, args: &Value) -> Result<Value> {
    validate_args(name, args)?;
    ensure!(
        api.token.is_some(),
        "Run `magic login` in your terminal, then reconnect the MCP server"
    );
    match name {
        "sandbox_list" => {
            let records = api.request(Method::GET, "v1/sandboxes", None).await?;
            Ok(json!(records
                .as_array()
                .context("invalid sandbox list")?
                .iter()
                .map(summary)
                .collect::<Vec<_>>()))
        }
        "sandbox_create" => {
            let key = match args.get("idempotency_key") {
                Some(value) => {
                    uuid::Uuid::parse_str(value.as_str().context("invalid idempotency key")?)?
                }
                None => uuid::Uuid::new_v4(),
            };
            let record = api.request(Method::POST, "v1/sandboxes/managed", Some(crate::pi::managed_creation_body(api, None, None, key).await?)).await
                .with_context(|| format!("Creation intent {key}; inspect sandbox_show with this ID before retrying. Reuse idempotency_key {key} for the same intent"))?;
            Ok(summary(&record))
        }
        _ => {
            let id = string(args, "id")?;
            crate::validate_id(id)?;
            let route = format!("v1/sandboxes/{id}");
            match name {
                "sandbox_show" => Ok(summary(&api.request(Method::GET, &route, None).await?)),
                "sandbox_terminate" => Ok(summary(
                    &api.request(Method::POST, &format!("{route}/terminate"), Some(json!({})))
                        .await?,
                )),
                "sandbox_exec" | "sandbox_read_file" | "sandbox_write_file" => {
                    let body = execution(name, args)?;
                    api.request(Method::POST, &format!("{route}/exec"), Some(body))
                        .await
                }
                _ => bail!("unknown tool"),
            }
        }
    }
}
fn execution(name: &str, args: &Value) -> Result<Value> {
    let (script, argv, timeout) = match name {
        "sandbox_exec" => {
            let cwd = args
                .get("cwd")
                .and_then(Value::as_str)
                .unwrap_or("/workspace");
            ensure!(cwd.starts_with('/'), "cwd must be absolute");
            let timeout = args
                .get("timeout_ms")
                .and_then(Value::as_u64)
                .unwrap_or(30_000);
            ensure!(
                (1..=30_000).contains(&timeout),
                "timeout_ms must be between 1 and 30000"
            );
            ("import os,sys; os.chdir(sys.argv[1]); os.execv('/bin/sh',['sh','-lc',sys.argv[2]])", vec![cwd.to_owned(), string(args,"command")?.to_owned()], timeout)
        }
        "sandbox_read_file" => {
            let path = string(args, "path")?;
            ensure!(path.starts_with('/'), "path must be absolute");
            ("import sys\nwith open(sys.argv[1], 'rb') as f: data=f.read(16385)\nif len(data)>16384: raise ValueError('file exceeds 16 KiB; read a range with sandbox_exec')\nsys.stdout.write(data.decode('utf-8'))", vec![path.to_owned()], 30_000)
        }
        "sandbox_write_file" => {
            let path = string(args, "path")?;
            let content = string(args, "content")?;
            ensure!(path.starts_with('/'), "path must be absolute");
            ensure!(content.len() <= FILE_LIMIT, "content exceeds 16 KiB");
            // Base64 keeps NULs and arbitrary text out of process arguments.
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD.encode(content);
            ("import base64,sys\ndata=base64.b64decode(sys.argv[2])\nwith open(sys.argv[1], 'wb') as f: f.write(data)\nprint('Wrote',len(data),'bytes')", vec![path.to_owned(), data], 30_000)
        }
        _ => bail!("unknown execution tool"),
    };
    crate::execution::request(script, &argv, timeout)
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
async fn response(api: &Api, request: Value) -> Option<Value> {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request["jsonrpc"] != "2.0" || !request["method"].is_string() {
        return Some(error(id, -32600, "Invalid Request"));
    }
    if request.get("id").is_none() {
        return None;
    }
    let result = match request["method"].as_str().unwrap() {
        "initialize" => {
            let requested = request["params"]["protocolVersion"].as_str().unwrap_or("");
            let version =
                if ["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"].contains(&requested) {
                    requested
                } else {
                    "2025-11-25"
                };
            json!({"protocolVersion":version,"capabilities":{"tools":{}},"serverInfo":{"name":"magicmtn","version":env!("CARGO_PKG_VERSION")},"instructions":"Use these tools for remote sandbox work; built-in shell and file tools remain local. Never blindly retry uncertain mutations."})
        }
        "ping" => json!({}),
        "tools/list" => json!({"tools":tools()}),
        "tools/call" => {
            let Some(name) = request["params"]["name"].as_str() else {
                return Some(error(id, -32602, "tool name required"));
            };
            let args = request["params"]
                .get("arguments")
                .cloned()
                .unwrap_or(json!({}));
            match call(api, name, &args).await {
                Ok(value) => {
                    let failed = value.get("exit_code").is_some_and(|code| code != 0)
                        || value["timed_out"] == true;
                    json!({"content":[{"type":"text","text":serde_json::to_string(&value).unwrap()}],"isError":failed})
                }
                Err(err) => {
                    json!({"content":[{"type":"text","text":format!("{err:#}")}],"isError":true})
                }
            }
        }
        _ => return Some(error(id, -32601, "Method not found")),
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub async fn run(api: &Api, _: Commands) -> Result<()> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = Vec::new();
        let read =
            std::io::Read::take(&mut input, MESSAGE_LIMIT + 1).read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        ensure!(
            line.len() as u64 <= MESSAGE_LIMIT,
            "MCP message exceeds 1 MiB"
        );
        let reply = match serde_json::from_slice(&line) {
            Ok(request) => response(api, request).await,
            Err(_) => Some(error(Value::Null, -32700, "Parse error")),
        };
        if let Some(reply) = reply {
            serde_json::to_writer(&mut output, &reply)?;
            writeln!(output)?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn discovery_works_without_login_and_notifications_are_silent() {
        let api = Api::new("http://localhost:8080").unwrap();
        let init = response(&api,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}})).await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(response(
            &api,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .await
        .is_none());
        let result = response(&api, json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .await
            .unwrap();
        assert_eq!(result["result"]["tools"].as_array().unwrap().len(), 7);
        let result = response(
            &api,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sandbox_list"}}),
        )
        .await
        .unwrap();
        assert_eq!(result["result"]["isError"], true);
    }
    #[tokio::test]
    async fn bridge_preserves_auth_execution_results_and_creation_identity() {
        use axum::{
            extract::State,
            http::HeaderMap,
            routing::{get, post},
            Json, Router,
        };
        use std::sync::{Arc, Mutex};
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        async fn handler(
            State(log): State<Arc<Mutex<Vec<Value>>>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            assert_eq!(headers["authorization"], "Bearer test-account-token");
            log.lock().unwrap().push(body.clone());
            if body.get("idempotency_key").is_some() {
                Json(
                    json!({"id":body["idempotency_key"],"status":"running","metadata":{"credential":"never expose"}}),
                )
            } else {
                Json(
                    json!({"stdout":"remote stdout","stderr":"failure detail","exit_code":7,"timed_out":false,"truncated":false}),
                )
            }
        }
        let app = Router::new()
            .route("/v1/pi/catalog", get(|| async {
                Json(json!({"scope":{"organization_id":"11111111-1111-4111-8111-111111111111","user_id":"22222222-2222-4222-8222-222222222222"},"defaults":{"model_id":"33333333-3333-4333-8333-333333333333"}}))
            }))
            .route("/v1/sandboxes/managed", post(handler))
            .route("/v1/sandboxes/abc/exec", post(handler))
            .with_state(requests.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut api = Api::new(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        api.token = Some("test-account-token".into());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let key = uuid::Uuid::new_v4();
        let created = call(&api, "sandbox_create", &json!({"idempotency_key":key}))
            .await
            .unwrap();
        assert_eq!(created["id"], key.to_string());
        assert!(created.get("metadata").is_none());
        let result = response(&api,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"sandbox_exec","arguments":{"id":"abc","command":"exit 7"}}})).await.unwrap();
        assert_eq!(result["result"]["isError"], true);
        assert!(result["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("remote stdout"));
        let log = requests.lock().unwrap();
        assert_eq!(log[0]["idempotency_key"], key.to_string());
        assert_eq!(log[0]["model_id"], "33333333-3333-4333-8333-333333333333");
        assert_eq!(log[1]["command"], "python3");
        assert!(log[1]["timeout_ms"].as_u64().unwrap() <= 30_000);
        assert!(log[1]["max_output_bytes"].as_u64().unwrap() <= 16_384);
        use base64::Engine;
        let encoded = log[1]["args"].as_array().unwrap()[2..]
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<String>();
        let payload: Value = serde_json::from_slice(
            &base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(payload["argv"][1], "exit 7");
        server.abort();
    }

    #[test]
    fn arguments_are_validated_and_internal_metadata_is_not_exposed() {
        assert!(validate_args(
            "sandbox_exec",
            &json!({"id":"abc","command":"true","timeout_ms":-1})
        )
        .is_err());
        assert!(validate_args("sandbox_list", &json!({"url":"http://evil"})).is_err());
        assert!(validate_args("unknown", &json!({})).is_err());
        assert_eq!(
            summary(&json!({"id":"abc","metadata":{"secret":"hidden"}})),
            json!({"id":"abc"})
        );
        assert!(execution(
            "sandbox_exec",
            &json!({"command":"true","timeout_ms":120001})
        )
        .is_err());
        assert!(execution(
            "sandbox_write_file",
            &json!({"path":"/tmp/file","content":"x".repeat(FILE_LIMIT+1)})
        )
        .is_err());
    }
    #[test]
    fn remote_file_arguments_are_literal_and_round_trip_unicode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quotes' $dollar ; space");
        let contents = format!("hello\n世界\0{}", "x".repeat(FILE_LIMIT - 13));
        let body = execution(
            "sandbox_write_file",
            &json!({"path":path,"content":contents}),
        )
        .unwrap();
        let args: Vec<&str> = body["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(std::process::Command::new("python3")
            .args(&args)
            .output()
            .unwrap()
            .status
            .success());
        let body = execution("sandbox_read_file", &json!({"path":path})).unwrap();
        let args: Vec<&str> = body["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let output = std::process::Command::new("python3")
            .args(&args)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, contents.as_bytes());
        assert!(args.len() <= 32);
        assert!(args.iter().all(|a| a.len() <= 2048));
    }
}
