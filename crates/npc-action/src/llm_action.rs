//! Natural-language-to-actions chat flow (ports Go `ChatAgent.ChatWithAction`
//! plus `buildSystemPrompt`/`updateSystemPrompt` in `internal/agent/agent.go`).

use std::sync::atomic::Ordering;
use std::sync::Arc;

use npc_core::Config;
use npc_llm::{ChatMessage, ChatRequest, ResponseFormat};

use crate::dispatcher::execute_actions;
use crate::state::ActionState;
use crate::types::ActionResponse;

/// Full command-name/args reference handed to the LLM, one line per
/// dispatcher command from `dispatcher.rs`. The Go original's
/// `buildSystemPrompt` only advertised a 9-command subset (`goto`, `home`,
/// `mv`, `mb`, `rt`, `lt`, `jump`, `stop`, `route`) and had its locations
/// listing commented out; this port intentionally lists every dispatcher
/// command and both configured locations and routes, per this crate's spec.
const ACTION_DESCRIPTIONS: &[&str] = &[
    r#""w" args: [] - 前進を開始する(停止するまで継続)"#,
    r#""s" args: [] - 後退を開始する(停止するまで継続)"#,
    r#""a" args: [] - 左移動を開始する(停止するまで継続)"#,
    r#""d" args: [] - 右移動を開始する(停止するまで継続)"#,
    r#""fw" args: ["seconds"] - 指定秒数(既定2秒)前進する"#,
    r#""tr" args: ["seconds"] - 指定秒数(既定1秒)右に旋回する"#,
    r#""tl" args: ["seconds"] - 指定秒数(既定1秒)左に旋回する"#,
    r#""jump" args: [] - ジャンプする"#,
    r#""mv" args: ["meters"] - 前方に指定メートル(既定1m)移動する"#,
    r#""mb" args: ["meters"] - 後方に指定メートル(既定1m)移動する"#,
    r#""rt" args: ["degrees"] - 右に指定角度(既定90°)回転する"#,
    r#""lt" args: ["degrees"] - 左に指定角度(既定90°)回転する"#,
    r#""mvt" args: ["meters", "seconds"] - 指定秒数(既定4秒)で指定メートル(既定2m)移動する"#,
    r#""rtt" args: ["degrees", "seconds"] - 指定秒数(既定3秒)で右に指定角度(既定180°)回転する"#,
    r#""ltt" args: ["degrees", "seconds"] - 指定秒数(既定3秒)で左に指定角度(既定180°)回転する"#,
    r#""stop" args: [] - すべての移動を停止する"#,
    r#""pos" args: [] - 現在位置を表示する"#,
    r#""reset" args: [] - 現在位置を原点にリセットする"#,
    r#""goto" args: ["x", "y", "heading", "seconds"] - 指定座標へ移動する(headingとsecondsは省略可、既定10秒)"#,
    r#""home" args: ["seconds"] - 原点に帰還する(既定10秒)"#,
    r#""locations" args: [] - 登録済みロケーション一覧を表示する"#,
    r#""go" args: ["location_name", "seconds"] - 登録済みロケーションへ移動する(既定10秒)"#,
    r#""routes" args: [] - 登録済みルート一覧を表示する"#,
    r#""route" args: ["route_name"] - 登録済みルートを実行する"#,
];

/// Ports `buildSystemPrompt`'s static portion (the position status line is
/// appended fresh every turn by [`dynamic_system_prompt`], porting
/// `updateSystemPrompt`).
pub fn build_static_system_prompt(cfg: &Config) -> String {
    let mut s = String::new();
    s.push_str(
        "あなたはVRChat内のアバターを制御するアシスタントです。\n\
         ユーザーの指示に従って移動や回転を行います。\n\
         必ずJSON形式で応答してください。\n\n\
         応答フォーマット:\n\
         {\n  \"message\": \"ユーザーへの返答メッセージ\",\n  \"actions\": [\n    \
         {\"name\": \"アクション名1\", \"args\": [\"引数1\", ...]},\n    \
         {\"name\": \"アクション名2\", \"args\": [\"引数1\", ...]}\n  ]\n}\n\n\
         利用可能なアクション:\n",
    );
    for line in ACTION_DESCRIPTIONS {
        s.push_str("- ");
        s.push_str(line);
        s.push('\n');
    }

    if !cfg.action.locations.is_empty() {
        s.push_str("\n登録済みロケーション:\n");
        for loc in &cfg.action.locations {
            s.push_str(&format!(
                "- {} (x={:.1}, y={:.1}, heading={:.1}\u{b0})\n",
                loc.name, loc.x, loc.y, loc.heading
            ));
        }
    }

    if !cfg.action.routes.is_empty() {
        s.push_str("\n登録済みルート:\n");
        for r in &cfg.action.routes {
            let waypoints: Vec<&str> = r.waypoints.iter().map(|w| w.location.as_str()).collect();
            let loop_str = if r.r#loop { " [loop]" } else { "" };
            s.push_str(&format!("- {}: {}{}\n", r.name, waypoints.join(" -> "), loop_str));
        }
    }

    s
}

fn dynamic_system_prompt(state: &ActionState) -> String {
    format!(
        "{}\n\n現在のステータス:\n- {}",
        state.static_system_prompt,
        state.navigator.pos.get().display()
    )
}

/// Ports the history-trim block duplicated in Go's `Chat`/`ChatWithAction`:
/// keep the system message (index 0) plus the most recent `history_size`
/// entries.
fn trim_history(history: &mut Vec<ChatMessage>, history_size: usize) {
    if history.len() > history_size {
        let start = history.len().saturating_sub(history_size).max(1);
        let mut trimmed = Vec::with_capacity(history.len() - start + 1);
        trimmed.push(history[0].clone());
        trimmed.extend_from_slice(&history[start..]);
        *history = trimmed;
    }
}

/// Ports `ChatAgent.ChatWithAction` plus the Redis-triggered-chat
/// concurrency guard from `cli/handler.go` (`actionSem`, size-1 semaphore):
/// only one NL round-trip runs at a time. A request arriving while one is in
/// flight is logged and dropped rather than queued.
pub async fn chat_with_action(state: Arc<ActionState>, query: String) {
    if state
        .chat_busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        state.log(format!(
            "[Chat] 別の指示を処理中のため、このメッセージは破棄されました: {query}"
        ));
        return;
    }

    let result = do_chat_with_action(&state, &query).await;
    state.chat_busy.store(false, Ordering::SeqCst);

    match result {
        Ok(resp) => {
            state.log(resp.message.clone());
            if !resp.actions.is_empty() {
                execute_actions(state.clone(), resp.actions).await;
            }
        }
        Err(e) => state.log(format!("Error: {e}")),
    }
}

async fn do_chat_with_action(state: &Arc<ActionState>, query: &str) -> anyhow::Result<ActionResponse> {
    let history_size = (state.config.talk.history_size as usize).max(1);

    let mut history = state.chat_history.lock().await;
    history[0] = ChatMessage::system(dynamic_system_prompt(state));
    history.push(ChatMessage::user(query.to_string()));
    trim_history(&mut history, history_size);

    let mut req = ChatRequest::new(state.config.api.model.clone(), history.clone());
    req.response_format = Some(ResponseFormat::json_object());
    if !state.config.api.reasoning_effort.is_empty() {
        req.reasoning_effort = Some(state.config.api.reasoning_effort.clone());
    }

    let resp = state
        .llm
        .chat(req)
        .await
        .map_err(|e| anyhow::anyhow!("chat error: {e}"))?;
    let content = resp.content().unwrap_or_default().to_string();

    history.push(ChatMessage::assistant(content.clone()));
    drop(history);

    // A malformed/non-JSON reply falls back to a plain message with no
    // actions, matching Go's `ChatWithAction` fallback on unmarshal error.
    Ok(serde_json::from_str::<ActionResponse>(&content)
        .unwrap_or(ActionResponse { message: content, actions: Vec::new() }))
}
