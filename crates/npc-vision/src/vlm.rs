//! VLM analysis + tool-call flow (ports Go `agent-vision/internal/vision`
//! and `internal/tool/action.go`).

use npc_core::{msg, topic, Bus};
use npc_llm::{
    ChatContent, ChatMessage, ChatRequest, LlmClient, ResponseMessage, Tool, ToolCall,
};

const TOOL_NAME: &str = "robot_command";
const MAX_TOKENS: u32 = 500;

/// Run one VLM analysis cycle: send the captured image (+ optional
/// short-term memory context) to the chat model, and if it requests
/// `robot_command` tool calls, publish each command to `agent:action` and do
/// a single tools-less follow-up completion for the final narrative.
///
/// Returns the observation text to publish on `agent:sense`/`vision`.
pub async fn analyze_image(
    client: &LlmClient,
    model: &str,
    system_prompt: &str,
    image_data_url: String,
    memory_context: Option<&str>,
    bus: &Bus,
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

    let tool = Tool::function(
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
    );

    let mut req = ChatRequest::new(model, messages.clone());
    req.max_tokens = Some(MAX_TOKENS);
    req.tools = Some(vec![tool]);

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

/// Execute a single `robot_command` tool call: publish the bare command
/// string to `agent:action`/`action` (matching the Go wire format, where
/// `Payload` was the command string itself, not an object), and return the
/// text fed back to the model as the tool result.
fn execute_tool_call(call: &ToolCall, bus: &Bus) -> String {
    if call.function.name != TOOL_NAME {
        return format!("Error executing tool: unknown tool '{}'", call.function.name);
    }

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

fn fallback_narrative(initial: &ResponseMessage, action_results: &[String]) -> String {
    format!(
        "{}\n\n【アクション実行結果】\n{}",
        initial.content.clone().unwrap_or_default(),
        action_results.join("\n")
    )
}
