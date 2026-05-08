//! WebSocket stream consumption and observer audit utilities.
//!
//! NOTE: Stream consumption is now centralized in `inference::stream_consumer`.
//! This module retains: `send_ws`, `audit_and_retry`, `save_conversation_stack`,
//! and the observer retry infrastructure.

use crate::observer;
use crate::provider::Message;
use crate::web::state::AppState;
use crate::web::training_capture;
use axum::extract::ws::{Message as WsMessage, WebSocket};
use futures_util::SinkExt;

/// Observer-gated audit loop: audit response, retry with feedback if rejected.
/// Returns the approved text. No cap — the model MUST produce an approved response.
/// If this loops, the observer feedback isn't being followed, which is a deeper bug.
pub async fn audit_and_retry(
    state: &AppState,
    provider: &dyn crate::provider::Provider,
    sender: &mut futures_util::stream::SplitSink<WebSocket, WsMessage>,
    messages: &mut Vec<Message>,
    tools: &serde_json::Value,
    user_query: &str,
    initial_text: &str,
    session_id: &str,
) -> String {
    let mut current_text = initial_text.to_string();

    if !state.config.observer.enabled || current_text.is_empty() {
        return current_text;
    }

    let mut retries: usize = 0;

    loop {
        send_ws(sender, "audit_running", &serde_json::json!({})).await;
        let tool_ctx = build_l1_tool_context(messages);

        match observer::audit_response(provider, messages, &current_text, &tool_ctx, user_query).await {
            Ok(output) if output.result.verdict.is_allowed() => {
                send_ws(sender, "audit_completed", &serde_json::json!({
                    "approved": true, "confidence": output.result.confidence,
                    "category": &output.result.failure_category,
                })).await;
                observer::persist_audit_result(&state.config.general.data_dir, &output.result);
                training_capture::capture_approved_with_flags(
                    state, user_query, &current_text,
                    output.result.confidence, &output.result.positive_flags,
                );
                save_conversation_stack(state, session_id, &output.result);
                return current_text;
            }
            Ok(output) => {
                retries += 1;
                let rejected_text = current_text.clone();
                let reason = output.result.what_went_wrong.clone();

                tracing::warn!(
                    retries,
                    category = %output.result.failure_category,
                    what_went_wrong = %reason,
                    how_to_fix = %output.result.how_to_fix,
                    "Observer BLOCKED — re-inferring with feedback"
                );
                send_ws(sender, "audit_completed", &serde_json::json!({
                    "approved": false, "category": &output.result.failure_category, "reason": &reason,
                })).await;
                observer::persist_audit_result(&state.config.general.data_dir, &output.result);

                current_text = retry_with_feedback(
                    state, provider, sender, messages, tools,
                    user_query, &rejected_text, &reason, &output.result,
                ).await;
            }
            Err(e) => {
                // §4: Fail-CLOSED — observer down means response blocked.
                // Return error message instead of the unaudited response.
                tracing::error!(error = %e, "Observer failed — fail-CLOSED (response blocked)");
                return "[Observer infrastructure error — response blocked. Please retry.]".to_string();
            }
        }
    }
}

/// Retry inference with structured rejection feedback and execute any tool calls.
async fn retry_with_feedback(
    state: &AppState,
    provider: &dyn crate::provider::Provider,
    sender: &mut futures_util::stream::SplitSink<WebSocket, WsMessage>,
    messages: &mut Vec<Message>,
    tools: &serde_json::Value,
    user_query: &str,
    rejected_text: &str,
    reject_reason: &str,
    result: &observer::AuditResult,
) -> String {
    let feedback = observer::format_rejection_feedback(result);
    messages.push(Message::text("assistant", rejected_text));
    messages.push(Message::text("system", &feedback));

    let rx = match provider.chat(messages, Some(tools), true).await {
        Ok(rx) => rx,
        Err(e) => {
            tracing::error!(error = %e, "Observer retry: inference error");
            return rejected_text.to_string();
        }
    };

    use crate::inference::stream_consumer::{self as sc, NullSink};
    let mut sink = NullSink;
    match sc::consume_stream(rx, &mut sink).await {
        sc::ConsumeResult::Reply { text, .. } => {
            training_capture::capture_rejection(state, user_query, rejected_text, &text, reject_reason);
            text
        }
        sc::ConsumeResult::ToolCall { id, name, arguments } => {
            let tc = crate::tools::schema::ToolCall { id, name, arguments };
            execute_retry_tool_chain(state, provider, sender, messages, tools, tc).await
        }
        sc::ConsumeResult::Error(e) => {
            tracing::error!(error = %e, "Observer retry: inference error");
            rejected_text.to_string()
        }
        _ => {
            tracing::warn!("Observer retry: unexpected result type — continuing");
            rejected_text.to_string()
        }
    }
}

/// Execute a chain of tool calls during observer retry (up to 10 iterations).
async fn execute_retry_tool_chain(
    state: &AppState,
    provider: &dyn crate::provider::Provider,
    sender: &mut futures_util::stream::SplitSink<WebSocket, WsMessage>,
    messages: &mut Vec<Message>,
    tools: &serde_json::Value,
    mut tc: crate::tools::schema::ToolCall,
) -> String {
    let max_chain = 10;

    for iteration in 0..max_chain {
        tracing::info!(tool = %tc.name, iteration, "Observer retry: executing tool");
        send_ws(sender, "tool_executing", &serde_json::json!({"name": &tc.name, "id": &tc.id})).await;

        let result = crate::web::tool_dispatch::execute_tool_with_state(state, &tc).await;
        send_ws(sender, "tool_completed", &serde_json::json!({
            "id": &tc.id, "name": &tc.name, "result": &result.output, "success": result.success,
        })).await;

        messages.push(Message::assistant_tool_call(&tc.id, &tc.name, &tc.arguments));
        if result.images.is_empty() {
            messages.push(Message::tool_result(&tc.id, &result.output));
        } else {
            messages.push(Message::tool_result_multipart(&tc.id, &result.output, result.images));
        }

        let rx = match provider.chat(messages, Some(tools), true).await {
            Ok(rx) => rx,
            Err(e) => {
                tracing::error!(error = %e, "Observer retry: re-inference failed");
                return String::new();
            }
        };

        use crate::inference::stream_consumer::{self as sc, NullSink};
        let mut sink = NullSink;
        match sc::consume_stream(rx, &mut sink).await {
            sc::ConsumeResult::Reply { text, .. } => return text,
            sc::ConsumeResult::ToolCall { id, name, arguments } => {
                tracing::info!(tool = %name, iteration = iteration + 1, "Observer retry: chaining tool");
                tc = crate::tools::schema::ToolCall { id, name, arguments };
            }
            sc::ConsumeResult::Error(e) => {
                tracing::error!(error = %e, "Observer retry: tool chain error");
                return String::new();
            }
            _ => break,
        }
    }

    String::new()
}

/// Send a typed JSON message to the WebSocket client.
pub async fn send_ws(
    sender: &mut futures_util::stream::SplitSink<WebSocket, WsMessage>,
    msg_type: &str,
    payload: &serde_json::Value,
) {
    let mut msg = payload.clone();
    msg["type"] = serde_json::json!(msg_type);
    let _ = sender.send(WsMessage::Text(msg.to_string().into())).await;
}

/// Build tool context from L1 message history for observer audit.
fn build_l1_tool_context(messages: &[Message]) -> String {
    let mut entries: Vec<String> = Vec::new();
    for (i, msg) in messages.iter().enumerate() {
        if msg.role == "tool" {
            let tool_call_id = msg.tool_call_id.as_deref().unwrap_or("");
            let result_text = msg.text_content();
            let preview = if result_text.len() > 200 {
                let b = result_text.char_indices().take_while(|(i,_)| *i <= 200).last().map(|(i,_)| i).unwrap_or(0);
                format!("{}...", &result_text[..b])
            } else {
                result_text.clone()
            };

            let mut tool_name = "unknown".to_string();
            for j in (0..i).rev() {
                if messages[j].role == "assistant" {
                    if let Some(tcs) = &messages[j].tool_calls {
                        for tc in tcs {
                            if tc["id"].as_str() == Some(tool_call_id) {
                                tool_name = tc["function"]["name"]
                                    .as_str()
                                    .unwrap_or("unknown")
                                    .to_string();
                                break;
                            }
                        }
                        if tool_name != "unknown" { break; }
                    }
                }
            }
            entries.push(format!("[{}] {} → {}", entries.len() + 1, tool_name, preview));
        }
    }
    if entries.is_empty() {
        String::new()
    } else {
        format!("L1 tools executed ({} calls):\n{}", entries.len(), entries.join("\n"))
    }
}

/// Save the conversation stack from an observer audit result.
/// Called after every successful audit — the observer generates topic classification
/// as part of its standard verdict JSON (zero additional inference cost).
pub fn save_conversation_stack(
    state: &AppState,
    session_id: &str,
    result: &crate::observer::AuditResult,
) {
    if result.active_topic.is_empty() {
        return;
    }
    let store = crate::prompt::conversation_stack::ConversationStackStore::new(
        std::path::Path::new(&state.config.general.data_dir),
    );
    if let Err(e) = store.update_from_audit(
        session_id,
        &result.active_topic,
        &result.topic_transition,
        &result.topic_context,
    ) {
        tracing::warn!(error = %e, "Failed to save conversation stack");
    }
}
