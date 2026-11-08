use anyhow::{ensure, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};

pub(crate) fn request(script: &str, argv: &[String], timeout_ms: u64) -> Result<Value> {
    ensure!(
        argv.iter().all(|argument| !argument.contains('\0')),
        "arguments cannot contain NUL bytes"
    );
    let encoded = STANDARD.encode(serde_json::to_vec(&json!({"script":script,"argv":argv}))?);
    let launcher = "import base64,json,sys; p=json.loads(base64.b64decode(''.join(sys.argv[1:]))); sys.argv=['magicmtn-workload',*p['argv']]; exec(compile(p['script'],'<magicmtn-workload>','exec'))";
    let mut args = vec!["-c".to_owned(), launcher.to_owned()];
    args.extend(
        encoded
            .as_bytes()
            .chunks(1600)
            .map(|part| String::from_utf8(part.to_vec()).unwrap()),
    );
    ensure!(
        args.len() <= 32,
        "command payload exceeds provider limits; split the workload into smaller commands or files"
    );
    Ok(json!({
        "command": "python3",
        "args": args,
        "timeout_ms": timeout_ms,
        "max_output_bytes": 16384,
    }))
}
