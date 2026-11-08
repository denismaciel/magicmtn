//! Shared text-chat conventions: function calls carry IDs and JSON-encoded arguments.
//! Retained object arguments and tool_name messages are accepted at provider boundaries.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};

pub fn normalize_message(value: &Value, id_prefix: &str) -> Result<Value> {
    let mut message = value.clone();
    ensure!(message.is_object(), "chat message must be an object");
    ensure!(message["role"].is_string(), "chat role required");
    if message["role"] == "assistant" && message["content"].is_null() {
        message["content"] = json!("");
    }
    if let Some(calls) = message.get_mut("tool_calls") {
        if calls.is_null() {
            *calls = json!([]);
        }
        let calls = calls
            .as_array_mut()
            .context("tool calls must be an array")?;
        for (index, call) in calls.iter_mut().enumerate() {
            ensure!(call.is_object(), "tool call must be an object");
            ensure!(
                call.get("type").is_none() || call["type"] == "function",
                "unsupported tool call type"
            );
            // Some compatible servers retain chunk indices in completed calls.
            call.as_object_mut().unwrap().remove("index");
            call["type"] = json!("function");
            if call.get("id").is_none() {
                call["id"] = json!(format!("{id_prefix}_{index}"));
            }
            ensure!(
                call["id"].as_str().is_some_and(|id| !id.is_empty()),
                "tool call ID required"
            );
            ensure!(
                call["function"]["name"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty()),
                "tool function name required"
            );
            let arguments = &call["function"]["arguments"];
            let parsed = if let Some(encoded) = arguments.as_str() {
                serde_json::from_str::<Value>(encoded).context("invalid tool arguments JSON")?
            } else {
                arguments.clone()
            };
            ensure!(parsed.is_object(), "tool arguments must encode an object");
            call["function"]["arguments"] = json!(serde_json::to_string(&parsed)?);
        }
    }
    Ok(message)
}

/// Normalize historical messages without rewriting their stored copies.
/// Legacy tool results are paired in order with the preceding named function calls.
pub fn normalize_input(value: &Value) -> Result<Value> {
    let mut input = value.clone();
    ensure!(input.is_object(), "chat input must be an object");
    let messages = input["messages"]
        .as_array_mut()
        .context("chat messages required")?;
    let mut pending: Vec<(String, String)> = Vec::new();
    for (index, message) in messages.iter_mut().enumerate() {
        *message = normalize_message(message, &format!("call_history_{index}"))?;
        if message["role"] == "assistant" {
            pending.clear();
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    pending.push((
                        call["id"].as_str().unwrap().into(),
                        call["function"]["name"].as_str().unwrap().into(),
                    ));
                }
            }
        } else if message["role"] == "tool" {
            let name = message
                .get("name")
                .or_else(|| message.get("tool_name"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            if message.get("tool_call_id").is_none() {
                if let Some(position) = pending
                    .iter()
                    .position(|(_, n)| name.as_ref().is_none_or(|name| name == n))
                {
                    let (id, function) = pending.remove(position);
                    message["tool_call_id"] = json!(id);
                    message["name"] = json!(function);
                }
            } else if let Some(position) = pending
                .iter()
                .position(|(id, _)| message["tool_call_id"] == *id)
            {
                let (_, function) = pending.remove(position);
                if name.is_none() {
                    message["name"] = json!(function);
                }
            }
            if let Some(name) = name {
                message["name"] = json!(name);
            }
            message.as_object_mut().unwrap().remove("tool_name");
        }
    }
    Ok(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_legacy_arguments_and_pairs_multiple_results() {
        let original = json!({"messages":[{"role":"assistant","content":"","tool_calls":[
            {"function":{"name":"run_command","arguments":{"program":"echo","args":["a\\b","雪"]}}},
            {"function":{"name":"run_command","arguments":{"program":"pwd","args":[]}}}]},
            {"role":"tool","tool_name":"run_command","content":"first"},
            {"role":"tool","tool_name":"run_command","content":"second"}]});
        let normalized = normalize_input(&original).unwrap();
        let messages = &normalized["messages"];
        for index in 0..2 {
            let call = &messages[0]["tool_calls"][index];
            assert_eq!(call["type"], "function");
            assert_eq!(call["id"], messages[index + 1]["tool_call_id"]);
            assert_eq!(
                serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap())
                    .unwrap(),
                original["messages"][0]["tool_calls"][index]["function"]["arguments"]
            );
        }
        assert_eq!(normalize_input(&normalized).unwrap(), normalized);
        assert!(original["messages"][1].get("tool_call_id").is_none());
    }
    #[test]
    fn preserves_native_ids_and_rejects_malformed_calls() {
        let call = json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_original","type":"function","index":0,"function":{"name":"f","arguments":"{}"}}]});
        let normalized = normalize_message(&call, "generated").unwrap();
        assert_eq!(normalized["tool_calls"][0]["id"], "call_original");
        assert!(normalized["tool_calls"][0].get("index").is_none());
        let mut bad = call;
        bad["tool_calls"][0]["function"]["arguments"] = json!("not JSON");
        assert!(normalize_message(&bad, "generated").is_err());
    }
}
