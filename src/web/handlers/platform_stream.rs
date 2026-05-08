//! Streaming platform inference endpoint — SSE variant of platform_ingest.
//!
//! Returns Server-Sent Events as inference progresses, enabling the platform
//! router to post thinking/tool/audit updates to the Discord thinking thread
//! in real-time rather than all-at-once.

use crate::web::state::AppState;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use futures_util::stream::Stream;
use futures_util::FutureExt;
use std::convert::Infallible;
use std::pin::Pin;

/// SSE streaming platform ingest endpoint.
pub async fn platform_ingest_stream(
    State(state): State<AppState>,
    axum::Json(msg): axum::Json<crate::platform::adapter::PlatformMessage>,
) -> Sse<Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    let error_tx = tx.clone();
    tokio::spawn(async move {
        let result = std::panic::AssertUnwindSafe(
            run_streaming_pipeline(state, msg, tx)
        )
        .catch_unwind()
        .await;

        if let Err(panic) = result {
            let panic_msg = panic.downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            tracing::error!(
                panic = %panic_msg,
                "SSE streaming pipeline PANICKED — this message will fail"
            );
            let _ = error_tx.send(Ok(
                Event::default()
                    .event("error")
                    .data(format!("{{\"error\": \"Pipeline panic: {}\"}}", panic_msg))
            )).await;
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(Box::pin(stream))
}

/// Run the full pipeline, emitting SSE events as each stage completes.
async fn run_streaming_pipeline(
    state: AppState,
    msg: crate::platform::adapter::PlatformMessage,
    tx: tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    tracing::info!(
        platform = %msg.platform, user = %msg.user_name,
        content_len = msg.content.len(),
        "SSE pipeline: starting"
    );

    // Bug fix: session ID must match platform_ingest.rs (3-part format)
    let session_id = format!("{}_{}_{}", msg.platform, msg.user_id, msg.channel_id);

    tracing::info!(session = %session_id, "SSE pipeline: ensuring session");
    // Bug fix: create/load session — was completely missing from streaming path
    super::platform_ingest::ensure_session(&state, &session_id, &msg).await;
    tracing::info!(session = %session_id, "SSE pipeline: session ready");

    tracing::info!(session = %session_id, "SSE pipeline: processing attachments");
    // Process platform attachments (security-scoped: admin=disk, non-admin=memory)
    let processed = crate::web::attachment_ingest::process_attachments(
        &msg.attachments, msg.is_admin,
    ).await;
    let (images, attachment_text) = crate::web::attachment_ingest::split_processed(
        &processed, state.model_spec.context_length,
    );

    // Deep-read: if admin attachment exceeds inline budget, summarise page-by-page.
    // Track which attachments get deep-read so we can REPLACE their inline text with the digest.
    let mut deep_read_digests: Vec<(String, String)> = Vec::new();
    if msg.is_admin {
        for att in &processed {
            if let (Some(ref path), true) = (&att.saved_path, att.exceeds_budget(state.model_spec.context_length)) {
                let config = crate::web::attachment_reader::DeepReadConfig {
                    path: path.clone(),
                    filename: att.filename.clone(),
                    context_length: state.model_spec.context_length,
                };
                let digest = crate::web::attachment_reader::deep_read(
                    config, state.provider.as_ref(), &state.memory, Some(&tx),
                ).await;
                deep_read_digests.push((att.filename.clone(), digest));
            }
        }
    }

    // Build the final content: user message + attachments.
    // For deep-read files, the digest REPLACES inline text (not stacked on top).
    let content_with_attachments = if deep_read_digests.is_empty() {
        // No deep-read — include inline attachment text as before
        if attachment_text.is_empty() {
            msg.content.clone()
        } else {
            format!("{}\n\n[ATTACHED FILES]\n{}", msg.content, attachment_text)
        }
    } else {
        // Deep-read triggered — use digests instead of raw inline text
        let mut content = msg.content.clone();
        for (filename, digest) in &deep_read_digests {
            content.push_str(&format!("\n\n[FILE: {} — processed via deep-read]\n{}", filename, digest));
        }
        content
    };

    // Select tools BEFORE building context so consolidation can account for tool overhead
    let tools = super::platform_ingest::select_tools(msg.is_admin);
    let tools_chars = tools.to_string().len();

    tracing::info!(session = %session_id, tools_chars, "SSE pipeline: building context");
    let ctx = crate::web::ws_context::build_chat_context(
        &state, &content_with_attachments, &session_id, None, images, &msg.platform, tools_chars,
    ).await;
    let mut messages = ctx.messages;
    let provider = state.provider.as_ref();

    // Start inference
    tracing::info!(
        msg_count = messages.len(),
        tools_count = tools.as_array().map(|a| a.len()).unwrap_or(0),
        thinking = state.config.prompt.thinking_enabled,
        session = %session_id,
        "Platform stream: calling provider.chat()"
    );
    let chat_start = std::time::Instant::now();
    let rx_stream = match provider.chat(
        &messages, Some(&tools), state.config.prompt.thinking_enabled,
    ).await {
        Ok(rx) => {
            tracing::info!(
                elapsed_ms = chat_start.elapsed().as_millis() as u64,
                "Platform stream: provider.chat() returned OK"
            );
            rx
        }
        Err(e) => {
            tracing::error!(
                elapsed_ms = chat_start.elapsed().as_millis() as u64,
                error = %e,
                "Platform stream: provider.chat() FAILED"
            );
            let _ = emit(&tx, "error", &serde_json::json!({"error": e.to_string()})).await;
            return;
        }
    };


    // Keepalive emitter — sends a heartbeat every 30 seconds to prevent
    // client-side timeouts during long inference prefill phases.
    let keepalive_tx = tx.clone();
    let keepalive_cancel = tokio_util::sync::CancellationToken::new();
    let keepalive_token = keepalive_cancel.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => {
                    if keepalive_tx.send(Ok(
                        Event::default().event("keepalive").data("{}")
                    )).await.is_err() {
                        break; // receiver dropped
                    }
                }
                _ = keepalive_token.cancelled() => break,
            }
        }
    });

    // Consume stream using the unified consumer with SSE sink
    use crate::inference::stream_consumer::{self, ConsumeResult, SseSink};
    let mut sink = SseSink::new(&tx);
    let result = stream_consumer::consume_stream(rx_stream, &mut sink).await;

    // Handle spiral: re-prompt with thinking disabled
    let result = match result {
        ConsumeResult::Spiral { .. } => {
            tracing::info!("Platform stream: re-prompting after spiral");
            let _ = emit(&tx, "spiral_reprompt", &serde_json::json!({})).await;
            stream_consumer::reprompt_after_spiral(
                provider, &mut messages, Some(&tools), &mut sink,
            ).await
        }
        other => other,
    };

    // Stop keepalive — inference is complete
    keepalive_cancel.cancel();

    // Dispatch result
    match result {
        ConsumeResult::Reply { ref text, ref thinking } => {
            if let Some(ref t) = thinking {
                let _ = emit(&tx, "thinking_complete", &serde_json::json!({"length": t.len()})).await;
            }
            crate::tools::introspect_tool::log_reasoning_event(
                &state.config.general.data_dir, &session_id,
                &serde_json::json!({"type":"inference","result":"reply","text_len":text.len(),"thinking_len":thinking.as_ref().map(|t|t.len()).unwrap_or(0)}),
                thinking.as_deref());
            if text.trim().is_empty() {
                let total_chars: usize = messages.iter().map(|m| m.text_content().len()).sum();
                let has_thinking = thinking.as_ref().map_or(false, |t| !t.is_empty());
                if has_thinking {
                    // Thinking consumed entire generation budget — retry with thinking disabled
                    tracing::warn!(
                        total_chars, msg_count = messages.len(),
                        thinking_chars = thinking.as_ref().map_or(0, |t| t.len()),
                        context_length = state.model_spec.context_length,
                        "Thinking consumed entire generation budget — retrying with thinking disabled"
                    );
                    let retry_rx = match provider.chat(&messages, Some(&tools), false).await {
                        Ok(rx) => rx,
                        Err(e) => {
                            tracing::error!(error = %e, "Thinking recovery: provider.chat() failed");
                            let _ = emit(&tx, "error", &serde_json::json!({"error": e.to_string()})).await;
                            let _ = emit(&tx, "done", &serde_json::json!({})).await;
                            return;
                        }
                    };
                    let mut retry_sink = SseSink::new(&tx);
                    let retry_result = stream_consumer::consume_stream(retry_rx, &mut retry_sink).await;
                    if let ConsumeResult::Reply { ref text, .. } = retry_result {
                        if !text.trim().is_empty() {
                            emit_reply(&state, provider, &mut messages, &tools, &msg, &session_id, text, &tx).await;
                            let _ = emit(&tx, "done", &serde_json::json!({})).await;
                            return;
                        }
                    }
                    tracing::error!(
                        "Thinking recovery also produced empty output — reporting to user"
                    );
                    let _ = emit(&tx, "error", &serde_json::json!({
                        "error": "Model could not produce visible output after retry. Context may be exhausted."
                    })).await;
                } else {
                    // Genuine empty response — likely context overflow.
                    tracing::error!(
                        total_chars,
                        msg_count = messages.len(),
                        context_length = state.model_spec.context_length,
                        "Model returned completely empty response — possible context overflow"
                    );
                    let _ = emit(&tx, "error", &serde_json::json!({
                        "error": format!(
                            "Empty response ({} messages, context {}). \
                             Context may be too large — consolidation should have triggered.",
                            messages.len(), state.model_spec.context_length
                        ),
                    })).await;
                }
            } else {
                emit_reply(&state, provider, &mut messages, &tools, &msg, &session_id, text, &tx).await;
            }
        }
        ConsumeResult::PlanProposal { title, plan_markdown, estimated_turns } => {
            emit_plan(&state.config.general.data_dir, &session_id, &title, &plan_markdown, estimated_turns, &tx).await;
        }
        ConsumeResult::ToolCall { id, name, arguments } => {
            emit_tool_chain(&state, provider, &mut messages, &tools, &msg, &session_id, id, name, arguments, &tx).await;
        }
        ConsumeResult::Escalate { objective, plan, .. } => {
            emit_escalation(&state, provider, messages, &msg, &session_id, &objective, plan, &tx).await;
        }
        ConsumeResult::Error(ref e) if e.contains("Stream stalled") => {
            // Stream stalled during thinking — retry with thinking disabled
            // Same recovery pattern as spiral detection (line 186)
            tracing::warn!(error = %e, "Platform stream: retrying after stall with thinking disabled");
            let _ = emit(&tx, "stall_reprompt", &serde_json::json!({})).await;
            let retry_rx = match provider.chat(&messages, Some(&tools), false).await {
                Ok(rx) => rx,
                Err(retry_err) => {
                    tracing::error!(error = %retry_err, "Stall recovery: provider.chat() failed");
                    let _ = emit(&tx, "error", &serde_json::json!({"error": retry_err.to_string()})).await;
                    let _ = emit(&tx, "done", &serde_json::json!({})).await;
                    return;
                }
            };
            let mut retry_sink = SseSink::new(&tx);
            let retry_result = stream_consumer::consume_stream(retry_rx, &mut retry_sink).await;
            if let ConsumeResult::Reply { ref text, .. } = retry_result {
                if !text.trim().is_empty() {
                    emit_reply(&state, provider, &mut messages, &tools, &msg, &session_id, text, &tx).await;
                    let _ = emit(&tx, "done", &serde_json::json!({})).await;
                    return;
                }
            }
            tracing::error!("Stall recovery also failed — reporting to user");
            let _ = emit(&tx, "error", &serde_json::json!({
                "error": "Inference stalled during thinking. Retry with thinking disabled also failed."
            })).await;
        }
        ConsumeResult::Error(e) => {
            let _ = emit(&tx, "error", &serde_json::json!({"error": e})).await;
        }
        _ => {}
    }

    let _ = emit(&tx, "done", &serde_json::json!({})).await;
}


// ── Emitters ──────────────────────────────────────────────────────

async fn emit(tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>, event: &str, data: &serde_json::Value) {
    let _ = tx.send(Ok(Event::default().event(event).data(data.to_string()))).await;
}

async fn emit_reply(
    state: &AppState, provider: &dyn crate::provider::Provider,
    messages: &mut Vec<crate::provider::Message>, tools: &serde_json::Value,
    msg: &crate::platform::adapter::PlatformMessage, session_id: &str,
    text: &str, tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    let (audited, audit) = super::platform_ingest::audit_and_capture(
        state, provider, messages, tools, &msg.content, text, session_id,
    ).await;
    let _ = emit(tx, "audit", &serde_json::json!({
        "verdict": audit.verdict, "confidence": audit.confidence,
    })).await;

    // Persist BEFORE emitting response — this lets us include the accurate
    // message_count so the router builds buttons targeting the correct turn.
    crate::web::ws_learning::ingest_assistant_turn(state, &audited, session_id).await;
    let msg_count = get_session_message_count(state, session_id).await;

    let _ = emit(tx, "response", &serde_json::json!({
        "text": audited, "session_id": session_id, "has_plan": false,
        "message_count": msg_count,
    })).await;

    // SAE: extract activations from response
    super::sae_capture::spawn_activation_capture(state, &audited);
}

async fn emit_plan(
    data_dir: &std::path::Path,
    session_id: &str, title: &str, plan_md: &str, turns: usize,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    let plan = crate::web::ws_plans::save_pending_plan(data_dir, session_id, title, plan_md, turns);
    let _ = emit(tx, "plan", &serde_json::json!({
        "title": plan.title, "plan_markdown": plan.plan_markdown,
    })).await;
    let response = format!(
        "📋 **{}**\n\nI've prepared a plan. Review the details in the thinking thread.",
        plan.title,
    );
    let _ = emit(tx, "response", &serde_json::json!({
        "text": response, "session_id": session_id,
        "has_plan": true, "plan_markdown": plan.plan_markdown,
    })).await;
}

async fn emit_tool_chain(
    state: &AppState, provider: &dyn crate::provider::Provider,
    messages: &mut Vec<crate::provider::Message>, tools: &serde_json::Value,
    msg: &crate::platform::adapter::PlatformMessage, session_id: &str,
    id: String, name: String, arguments: String,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    let _ = emit(tx, "tool_start", &serde_json::json!({"name": &name})).await;
    let tc = crate::tools::schema::ToolCall { id, name: name.clone(), arguments };

    // Use the proper tool chain that loops until the model produces a reply
    let (reply, _events, audit) = super::platform_exec::run_platform_tool_chain(
        state, provider, messages, tools, &msg.content, session_id, tc, Some(tx),
    ).await;
    // Tool events already emitted live via sse_tx during the loop

    // Emit audit if present
    if let Some(ref audit) = audit {
        let _ = emit(tx, "audit", &serde_json::json!({
            "verdict": audit.verdict, "confidence": audit.confidence,
        })).await;
    }

    // Emit the actual reply — persist first for accurate message_count
    crate::web::ws_learning::ingest_assistant_turn(state, &reply, session_id).await;
    let msg_count = get_session_message_count(state, session_id).await;

    let _ = emit(tx, "response", &serde_json::json!({
        "text": reply, "session_id": session_id, "has_plan": false,
        "message_count": msg_count,
    })).await;

    // SAE: extract activations from response
    super::sae_capture::spawn_activation_capture(state, &reply);
}

async fn emit_escalation(
    state: &AppState, provider: &dyn crate::provider::Provider,
    messages: Vec<crate::provider::Message>,
    msg: &crate::platform::adapter::PlatformMessage, session_id: &str,
    objective: &str, plan: Option<String>,
    tx: &tokio::sync::mpsc::Sender<Result<Event, Infallible>>,
) {
    let _ = emit(tx, "escalate", &serde_json::json!({"objective": objective})).await;
    let (reply, _events, _audit) = super::platform_exec::run_platform_react(
        state, provider, messages, objective, plan.as_deref(), &msg.content, session_id, Some(tx),
    ).await;
    // Tool events already emitted live via sse_tx during the loop
    // Persist escalation reply first for accurate message_count
    crate::web::ws_learning::ingest_assistant_turn(state, &reply, session_id).await;
    let msg_count = get_session_message_count(state, session_id).await;

    let _ = emit(tx, "response", &serde_json::json!({
        "text": reply, "session_id": session_id, "has_plan": false,
        "message_count": msg_count,
    })).await;

    // SAE: extract activations from response
    super::sae_capture::spawn_activation_capture(state, &reply);
}

/// Get the current message count for a session.
/// Used to build response buttons that target the correct message index.
async fn get_session_message_count(state: &AppState, session_id: &str) -> usize {
    let sessions = state.sessions.read().await;
    sessions.get(session_id)
        .map(|s| s.messages.len())
        .unwrap_or(0)
}
