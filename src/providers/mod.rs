use anyhow::Result;
use serde_json::Value;

use crate::telemetry::{self, EventStatus, EventType, IngestRequest};

pub mod codex;
pub mod copilot;
pub mod local;
pub mod pi;

/// Dispatches the three first-class hook adapters while keeping their common
/// envelope normalization in the telemetry layer.
pub fn parse_hook(source: &str, raw: &[u8], account_id: String) -> Result<IngestRequest> {
    let mut request = telemetry::parse_hook(source, raw, account_id)?;
    request.usage.sum_total_tokens();
    let payload = request.payload.clone();
    match request.source_system.as_str() {
        "claude_code" => normalize_claude(&mut request, &payload),
        "codex" => normalize_codex(&mut request, &payload),
        "opencode" => normalize_opencode(&mut request, &payload),
        _ => unreachable!("telemetry parser validates source names"),
    }
    Ok(request)
}

fn normalize_claude(request: &mut IngestRequest, payload: &Value) {
    request.provider_id = Some("anthropic".into());
    request
        .account_id
        .get_or_insert_with(|| "claude-code".into());
    request.source_schema_version = "claude_hook_v1".into();
    request.usage.requests.get_or_insert(1);
    let event_name = first_string(payload, &["hook_event_name", "hook_event", "event", "type"])
        .unwrap_or_default()
        .to_ascii_lowercase();
    if event_name.contains("tool") {
        request.event_type = EventType::ToolUsage;
        request.tool_name = first_string(
            payload,
            &["tool_name", "tool.name", "tool_input.name", "tool"],
        )
        .map(|value| value.to_ascii_lowercase());
        request.tool_call_id = first_string(payload, &["tool_call_id", "toolUseID", "tool_use_id"]);
    } else if request.usage.total_tokens.is_none() {
        request.event_type = EventType::TurnCompleted;
        if matches!(
            first_string(payload, &["decision", "status"]).as_deref(),
            Some("block" | "blocked" | "error" | "failed")
        ) {
            request.status = EventStatus::Error;
        }
    } else {
        request.event_type = EventType::MessageUsage;
    }
}

fn normalize_codex(request: &mut IngestRequest, payload: &Value) {
    request.provider_id = Some("codex".into());
    request.account_id.get_or_insert_with(|| "codex-cli".into());
    request.source_schema_version = "codex_notify_v1".into();
    request.usage.requests.get_or_insert(1);
    if let Some(tool) = first_string(
        payload,
        &["tool_name", "tool.name", "tool", "tool_call.name"],
    ) {
        request.event_type = EventType::ToolUsage;
        request.tool_name = Some(tool.to_ascii_lowercase());
        request.tool_call_id = first_string(payload, &["tool_call_id", "toolCallId", "call_id"]);
    } else if request.usage.total_tokens.is_some() {
        request.event_type = EventType::MessageUsage;
    } else {
        request.event_type = EventType::TurnCompleted;
    }
}

fn normalize_opencode(request: &mut IngestRequest, payload: &Value) {
    request.provider_id = first_string(
        payload,
        &["provider_id", "providerID", "message.providerID"],
    )
    .or_else(|| Some("opencode".into()));
    request.source_schema_version = "opencode_hook_v1".into();
    request.usage.requests.get_or_insert(1);
    let hook = first_string(payload, &["hook"]).unwrap_or_default();
    let event = first_string(payload, &["event", "type"]).unwrap_or_default();
    if hook == "tool.execute.after" || event == "tool.execute.after" {
        request.event_type = EventType::ToolUsage;
        request.tool_name = first_string(payload, &["input.tool", "tool", "tool_name"])
            .map(|value| value.to_ascii_lowercase());
        request.tool_call_id =
            first_string(payload, &["input.callID", "input.call_id", "tool_call_id"]);
        request.session_id = request
            .session_id
            .clone()
            .or_else(|| first_string(payload, &["input.sessionID", "input.session_id"]));
    } else if event == "message.updated" || hook == "chat.message" {
        request.event_type = EventType::MessageUsage;
        request.message_id = request
            .message_id
            .clone()
            .or_else(|| first_string(payload, &["message.id", "output.message.id"]));
    }
}

fn first_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .pointer(&format!("/{}", key.replace('.', "/")))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_specific_defaults_are_preserved() {
        let claude = parse_hook(
            "claude_code",
            br#"{"type":"tool_use","tool_name":"Bash"}"#,
            String::new(),
        )
        .unwrap();
        assert_eq!(claude.provider_id.as_deref(), Some("anthropic"));
        assert_eq!(claude.event_type, EventType::ToolUsage);

        let codex = parse_hook(
            "codex",
            br#"{"turn_id":"turn-1","usage":{"input_tokens":10,"output_tokens":4}}"#,
            String::new(),
        )
        .unwrap();
        assert_eq!(codex.provider_id.as_deref(), Some("codex"));
        assert_eq!(codex.event_type, EventType::MessageUsage);

        let opencode = parse_hook(
            "opencode",
            br#"{"hook":"tool.execute.after","input":{"callID":"call-1","tool":"read"}}"#,
            String::new(),
        )
        .unwrap();
        assert_eq!(opencode.event_type, EventType::ToolUsage);
        assert_eq!(opencode.tool_name.as_deref(), Some("read"));
    }
}
