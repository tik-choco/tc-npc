//! VLM analysis + tool-call flow (ports Go `agent-vision/internal/vision`
//! and `internal/tool/action.go`).

use npc_core::{msg, topic, Bus};
use npc_llm::{
    ChatContent, ChatMessage, ChatRequest, LlmClient, ResponseMessage, Tool, ToolCall,
};

const TOOL_NAME: &str = "robot_command";
/// Only offered to the model when `config.vision.person_detection` is
/// `true` (see [`build_tools`]) — a person-memory feature, not part of the
/// original Go `agent-vision` port.
const PERSON_TOOL_NAME: &str = "observe_person";
const MAX_TOKENS: u32 = 500;

/// Run one VLM analysis cycle: send the captured image (+ optional
/// short-term memory context) to the chat model, and if it requests
/// `robot_command` tool calls, publish each command to `agent:action` and do
/// a single tools-less follow-up completion for the final narrative. When
/// `person_detection` is `true`, the model may also call `observe_person`
/// (person-memory contract §2), publishing an observation to `agent:sense`/
/// `person_seen` for npc-memory to merge into a person record.
///
/// Returns the observation text to publish on `agent:sense`/`vision`.
pub async fn analyze_image(
    client: &LlmClient,
    model: &str,
    system_prompt: &str,
    image_data_url: String,
    memory_context: Option<&str>,
    bus: &Bus,
    person_detection: bool,
) -> anyhow::Result<String> {
    let user_text = match memory_context {
        Some(ctx) if !ctx.is_empty() => format!(
            "【短期記憶】: {ctx}\n\n現在の視界を分析し、必要に応じて行動を選択してください。"
        ),
        _ => "現在の視界を分析し、必要に応じて行動を選択してください。".to_string(),
    };

    let mut messages = vec![
        ChatMessage::system(system_prompt),
        ChatMessage::user_parts(vec![
            npc_llm::ContentPart::image_data_url(image_data_url),
            npc_llm::ContentPart::text(user_text),
        ]),
    ];

    let mut req = ChatRequest::new(model, messages.clone());
    req.max_tokens = Some(MAX_TOKENS);
    req.tools = Some(build_tools(person_detection));

    let resp = client.chat(req).await?;
    let initial: ResponseMessage = resp
        .choices
        .first()
        .map(|c| c.message.clone())
        .ok_or_else(|| anyhow::anyhow!("no response from vlm"))?;

    let tool_calls = initial.tool_calls.clone().unwrap_or_default();
    if tool_calls.is_empty() {
        return Ok(initial.content.unwrap_or_default());
    }

    // Turn 1 -> assistant message requesting tool calls.
    messages.push(ChatMessage {
        role: "assistant".to_string(),
        content: ChatContent::Text(initial.content.clone().unwrap_or_default()),
        tool_calls: Some(tool_calls.clone()),
        tool_call_id: None,
    });

    let mut action_results: Vec<String> = Vec::new();
    for call in &tool_calls {
        let result = execute_tool_call(call, bus);
        action_results.push(result.clone());
        messages.push(ChatMessage::tool_result(call.id.clone(), result));
    }

    // Turn 2: one follow-up completion, no tools, for the final narrative.
    let mut follow_up = ChatRequest::new(model, messages);
    follow_up.max_tokens = Some(MAX_TOKENS);

    match client.chat(follow_up).await {
        Ok(final_resp) => match final_resp.content() {
            Some(content) if !content.is_empty() => Ok(content.to_string()),
            _ => Ok(fallback_narrative(&initial, &action_results)),
        },
        Err(err) => {
            tracing::warn!(error = %err, "vision: follow-up completion failed, using fallback narrative");
            Ok(fallback_narrative(&initial, &action_results))
        }
    }
}

/// Build the tool list offered to the model: `robot_command` always, plus
/// `observe_person` when `person_detection` is enabled (kept out of the list
/// entirely when disabled so a model that doesn't do person detection never
/// spends tokens on the schema).
fn build_tools(person_detection: bool) -> Vec<Tool> {
    let mut tools = vec![Tool::function(
        TOOL_NAME,
        "Send a natural language command to the robot to perform actions (move, turn, jump, stop, etc.)",
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command in natural language, e.g., 'jump', 'move forward 1 meter', 'turn right 90 degrees'"
                }
            },
            "required": ["command"]
        }),
    )];

    if person_detection {
        tools.push(Tool::function(
            PERSON_TOOL_NAME,
            "Record an observation about a person visible in the image, if any. Call this once per distinct person noticed.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The person's name if identifiable from a name tag, caption, or prior context; empty string if unknown"
                    },
                    "appearance": {
                        "type": "string",
                        "description": "Short visual description, e.g. 'blue hoodie, short black hair'"
                    },
                    "note": {
                        "type": "string",
                        "description": "What this person is doing or any notable detail, if any"
                    }
                },
                "required": ["appearance"]
            }),
        ));
    }

    tools
}

/// Dispatch a single tool call requested by the model to its executor,
/// returning the text fed back to the model as the tool result. Unknown tool
/// names (anything but `robot_command`/`observe_person`) get the same error
/// shape either way.
fn execute_tool_call(call: &ToolCall, bus: &Bus) -> String {
    match call.function.name.as_str() {
        TOOL_NAME => execute_robot_command(call, bus),
        PERSON_TOOL_NAME => execute_observe_person(call, bus),
        other => format!("Error executing tool: unknown tool '{other}'"),
    }
}

/// Execute a single `robot_command` tool call: publish the bare command
/// string to `agent:action`/`action` (matching the Go wire format, where
/// `Payload` was the command string itself, not an object), and return the
/// text fed back to the model as the tool result.
fn execute_robot_command(call: &ToolCall, bus: &Bus) -> String {
    #[derive(serde::Deserialize)]
    struct Args {
        command: String,
    }

    match serde_json::from_str::<Args>(&call.function.arguments) {
        Ok(args) => {
            bus.publish(topic::ACTION, msg::ACTION, args.command.clone());
            format!("Successfully sent command: {}", args.command)
        }
        Err(err) => format!("Error executing tool: {err}"),
    }
}

/// Execute a single `observe_person` tool call: publish a `person_seen`
/// observation to `agent:sense` (person-memory contract §2) for npc-memory
/// to merge into a person record, and return the text fed back to the model
/// as the tool result.
fn execute_observe_person(call: &ToolCall, bus: &Bus) -> String {
    #[derive(serde::Deserialize)]
    struct Args {
        #[serde(default)]
        name: String,
        appearance: String,
        #[serde(default)]
        note: String,
    }

    match serde_json::from_str::<Args>(&call.function.arguments) {
        Ok(args) => {
            bus.publish(
                topic::SENSE,
                msg::PERSON_SEEN,
                serde_json::json!({
                    "name": args.name,
                    "appearance": args.appearance,
                    "note": args.note,
                    "source": "vision",
                }),
            );
            format!("Recorded observation of person: {}", args.appearance)
        }
        Err(err) => format!("Error executing tool: {err}"),
    }
}

fn fallback_narrative(initial: &ResponseMessage, action_results: &[String]) -> String {
    format!(
        "{}\n\n【アクション実行結果】\n{}",
        initial.content.clone().unwrap_or_default(),
        action_results.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use npc_llm::FunctionCall;

    fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "call-1".to_string(),
            r#type: "function".to_string(),
            function: FunctionCall {
                name: name.to_string(),
                arguments: arguments.to_string(),
            },
        }
    }

    #[test]
    fn build_tools_omits_observe_person_when_disabled() {
        let tools = build_tools(false);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function.name, TOOL_NAME);
        assert!(tools.iter().all(|t| t.function.name != PERSON_TOOL_NAME));
    }

    #[test]
    fn build_tools_includes_observe_person_when_enabled() {
        let tools = build_tools(true);
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().any(|t| t.function.name == PERSON_TOOL_NAME));
    }

    #[test]
    fn execute_observe_person_publishes_person_seen() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();

        let call = tool_call(
            PERSON_TOOL_NAME,
            serde_json::json!({
                "name": "太郎",
                "appearance": "blue hoodie, short black hair",
                "note": "waving"
            }),
        );
        let result = execute_tool_call(&call, &bus);
        assert!(result.contains("blue hoodie, short black hair"));

        let received = rx.try_recv().expect("expected a published message");
        assert_eq!(received.topic, topic::SENSE);
        assert_eq!(received.env.r#type, msg::PERSON_SEEN);
        assert_eq!(received.env.payload["name"], "太郎");
        assert_eq!(received.env.payload["appearance"], "blue hoodie, short black hair");
        assert_eq!(received.env.payload["note"], "waving");
        assert_eq!(received.env.payload["source"], "vision");
    }

    #[test]
    fn execute_observe_person_defaults_name_and_note_to_empty() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();

        let call = tool_call(PERSON_TOOL_NAME, serde_json::json!({ "appearance": "red cap" }));
        execute_tool_call(&call, &bus);

        let received = rx.try_recv().expect("expected a published message");
        assert_eq!(received.env.payload["name"], "");
        assert_eq!(received.env.payload["note"], "");
    }

    #[test]
    fn execute_robot_command_still_publishes_to_action_topic() {
        let bus = Bus::new();
        let mut rx = bus.subscribe();

        let call = tool_call(TOOL_NAME, serde_json::json!({ "command": "jump" }));
        let result = execute_tool_call(&call, &bus);
        assert_eq!(result, "Successfully sent command: jump");

        let received = rx.try_recv().expect("expected a published message");
        assert_eq!(received.topic, topic::ACTION);
        assert_eq!(received.env.r#type, msg::ACTION);
        assert_eq!(received.env.payload, "jump");
    }

    #[test]
    fn unknown_tool_name_returns_error_string_unchanged() {
        let bus = Bus::new();
        let call = tool_call("bogus_tool", serde_json::json!({}));
        let result = execute_tool_call(&call, &bus);
        assert_eq!(result, "Error executing tool: unknown tool 'bogus_tool'");
    }
}
