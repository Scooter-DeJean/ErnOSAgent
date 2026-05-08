//! WebSocket resume handler — delivers post-recompile wakeup messages.
//! Split from `ws.rs` per §10.1 (max 500 lines per file, excluding tests).

use crate::web::state::AppState;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::SinkExt;

/// Deliver post-recompile resume message if one is pending for the web platform.
/// Loads the prior session context and runs real inference for a contextual wakeup.
/// Discord/Telegram resumes are handled by their own platform adapters.
pub(crate) async fn deliver_resume_if_pending(
    state: &AppState,
    sender: &mut futures_util::stream::SplitSink<WebSocket, WsMessage>,
) {
    // Only consume if platform is "web" — leave Discord/Telegram resumes for their adapters
    let resume = {
        let guard = state.resume_message.read().await;
        match guard.as_ref() {
            Some((_, _, platform)) if platform == "web" => true,
            _ => false,
        }
    };
    if !resume {
        return;
    }
    let (msg, session_id, _platform) = match state.resume_message.write().await.take() {
        Some(r) => r,
        None => return,
    };

    tracing::info!(msg_len = msg.len(), session_id = %session_id, "Delivering post-recompile resume (web)");

    // Tell the client to switch to the correct session BEFORE streaming text
    if !session_id.is_empty() {
        let switch_cmd = serde_json::json!({"type": "session_switch", "session_id": session_id});
        let _ = sender.send(WsMessage::Text(switch_cmd.to_string().into())).await;
        // Brief delay to let the client switch
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    // If we have a session_id, load it and re-infer with full context
    if !session_id.is_empty() {
        let session_messages = {
            let sessions = state.sessions.read().await;
            sessions.get(&session_id).map(|s| s.messages.clone())
        };

        if let Some(history) = session_messages {
            // Build context: system prompt + session history + recompile notification
            let mut messages = history;
            messages.push(crate::provider::Message::text("system", &format!(
                "[SYSTEM] You have just been recompiled successfully. \
                 Review the conversation above and greet the user, confirming the recompile \
                 succeeded and briefly summarising what you were working on before the restart."
            )));

            // Stream the response to the WebSocket
            let provider = match crate::provider::create_provider(&state.config) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to create provider for resume re-inference");
                    let payload = serde_json::json!({"type": "text_delta", "content": format!("✅ {}", msg)});
                    let _ = sender.send(WsMessage::Text(payload.to_string().into())).await;
                    let done = serde_json::json!({"type": "done"});
                    let _ = sender.send(WsMessage::Text(done.to_string().into())).await;
                    return;
                }
            };
            match provider.chat(&messages, None, true).await {
                Ok(rx) => {
                    use crate::inference::stream_consumer::{self, WebSocketSink};
                    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let mut sink = WebSocketSink { sender, cancel };
                    let _ = stream_consumer::consume_stream(rx, &mut sink).await;
                    let done = serde_json::json!({"type": "done"});
                    let _ = sender.send(WsMessage::Text(done.to_string().into())).await;
                    tracing::info!("Post-recompile resume delivered via re-inference");
                    return;
                }
                Err(e) => {
                    tracing::error!(error = %e, "Resume re-inference failed — falling back to canned message");
                }
            }
        }
    }

    // Fallback: canned message (no session context available)
    let payload = serde_json::json!({"type": "text_delta", "content": format!("✅ {}", msg)});
    let _ = sender.send(WsMessage::Text(payload.to_string().into())).await;
    let done = serde_json::json!({"type": "done"});
    let _ = sender.send(WsMessage::Text(done.to_string().into())).await;
}
