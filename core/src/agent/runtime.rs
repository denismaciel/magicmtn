use serde_json::json;

pub fn interactive_runtime(image: &str) -> magicmtn_contracts::configuration::RuntimeChange {
    use magicmtn_contracts::configuration::RuntimeChange;
    RuntimeChange {
        runtime_version: "interactive-v1".into(),
        tool_manifest: vec![
            json!({"type":"function","function":{"name":"run_command","description":"Execute one program with literal arguments in the sandbox. Use /bin/sh with args [\"-c\",\"script\"] for shell syntax. Read changed files back before reporting success.","parameters":{"type":"object","properties":{"program":{"type":"string"},"args":{"type":"array","items":{"type":"string"}}},"required":["program","args"],"additionalProperties":false}}}),
        ],
        runtime_configuration: json!({"state":"configured","sandbox_image":image,"limits":{"max_model_calls":6,"max_tool_calls":4,"max_turn_seconds":120,"command_timeout_ms":10000,"max_output_bytes":4096,"max_context_messages":40}}),
    }
}
