//! Platform inference pipeline — processes messages from platform adapters.
//!
//! Runs the full inference pipeline (context assembly, L1/L2 inference,
//! tool execution, observer audit) for messages ingested from Discord,
//! Telegram, or any other platform adapter. Returns JSON including
//! tool execution metadata and audit results for thinking thread display.

use crate::web::state::AppState;
use axum::{extract::State, Json};
use serde::Serialize;

/// A single tool execution event, captured for thinking thread display.
#[derive(Debug, Clone, Serialize)]
pub struct ToolEvent {
    pub name: String,
    pub success: bool,
    pub elapsed_ms: u64,
    pub output_preview: String,
}

/// Observer audit summary, captured for thinking thread display.
#[derive(Debug, Clone, Serialize)]
pub struct AuditSummary {
    pub verdict: String,
    pub confidence: f32,
    pub retries: usize,
    pub active_topic: String,
}

impl AuditSummary {
    fn skipped() -> Self {
        Self { verdict: "Skipped".into(), confidence: 0.0, retries: 0, active_topic: String::new() }
    }
    fn error(retries: usize) -> Self {
        Self { verdict: "Error".into(), confidence: 0.0, retries, active_topic: String::new() }
    }
}

/// POST /api/chat/platform — ingest a message from a platform adapter.
/// Full inference pipeline: context assembly, tool execution, observer audit.
/// Returns tool events and audit metadata for thinking thread observability.
pub async fn platform_ingest(
    State(state): State<AppState>,
    Json(msg): Json<crate::platform::adapter::PlatformMessage>,
) -> Json<serde_json::Value> {
    tracing::info!(
        platform = %msg.platform,
        user = %msg.user_name,
        is_admin = msg.is_admin,
        content_len = msg.content.len(),
        "Platform message ingested"
    );

    let session_id = format!("{}_{}_{}", msg.platform, msg.user_id, msg.channel_id);
    ensure_session(&state, &session_id, &msg).await;

    // Process platform attachments (security-scoped: admin=disk, non-admin=memory)
    let processed = crate::web::attachment_ingest::process_attachments(
        &msg.attachments, msg.is_admin,
    ).await;
    let (images, attachment_text) = crate::web::attachment_ingest::split_processed(
        &processed,
    );
    // Select tools early so their token cost is included in the base context measurement below.
    let tools = select_tools(msg.is_admin);
    let tools_chars = tools.to_string().len();

    // Deep-read: if admin attachment token cost exceeds the remaining context budget,
    // summarise page-by-page instead of injecting inline.
    // Budget = context_length minus the measured cost of the base context (no attachments).
    // Both values derived from provider.count_tokens() — no heuristics (§2.1, §8.3).
    let provider = state.provider.as_ref();
    let mut deep_read_digests: Vec<(String, String)> = Vec::new();
    if msg.is_admin {
        let base_messages = crate::web::ws_context::build_chat_context(
            &state, &msg.content, &session_id, None, vec![], &msg.platform, tools_chars,
        ).await.messages;
        let base_token_cost = match provider.count_tokens(
            &base_messages, Some(&tools), state.config.prompt.thinking_enabled,
        ).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e,
                    "count_tokens failed for base context — defaulting to deep-read for all admin attachments");
                state.model_spec.context_length
            }
        };
        let remaining_tokens = state.model_spec.context_length.saturating_sub(base_token_cost);
        tracing::info!(
            context_length = state.model_spec.context_length,
            base_token_cost, remaining_tokens,
            "Deep-read gate: measured base context cost"
        );
        for att in &processed {
            if att.image_data_url.is_some() { continue; }
            if let Some(ref path) = att.saved_path {
                let att_content = att.content_text.as_deref().unwrap_or("");
                let att_tokens = match provider.count_tokens(
                    &[crate::provider::Message::text("user", att_content)],
                    None,
                    false,
                ).await {
                    Ok(n) => n,
                    Err(e) => {
                        tracing::warn!(filename = %att.filename, error = %e,
                            "count_tokens failed for attachment — defaulting to deep-read");
                        usize::MAX
                    }
                };
                tracing::info!(
                    filename = %att.filename, att_tokens, remaining_tokens,
                    "Deep-read gate: attachment token cost measured"
                );
                if att_tokens > remaining_tokens {
                    use crate::web::handlers::background_digest::{DigestStatus, spawn_background_deep_read};

                    // Path A: Cache hit — full digest already computed. Inject immediately.
                    if let Some(entry) = state.digest_store.get(path) {
                        if let DigestStatus::Complete { ref digest, .. } = *entry {
                            tracing::info!(filename = %att.filename, "Deep-read gate: cache hit — using stored digest");
                            deep_read_digests.push((att.filename.clone(), digest.clone()));
                            continue;
                        }
                    }

                    // Path B/C: Cache miss or pending — spawn background task if not already running.
                    let is_pending = state.digest_store.get(path)
                        .map(|e| matches!(*e, DigestStatus::Pending { .. }))
                        .unwrap_or(false);

                    if !is_pending {
                        tracing::info!(filename = %att.filename, "Deep-read gate: spawning background deep-read");
                        let bg_config = crate::web::attachment_reader::DeepReadConfig {
                            path: path.clone(),
                            filename: att.filename.clone(),
                            context_length: state.model_spec.context_length,
                        };
                        spawn_background_deep_read(
                            state.clone(), bg_config, path.clone(),
                            msg.channel_id.clone(), msg.platform.clone(),
                            session_id.clone(), msg.content.clone(),
                        );
                    } else {
                        tracing::info!(filename = %att.filename, "Deep-read gate: background read already in progress");
                    }

                    // Immediate turn: inject acknowledgment system note so model understands
                    // the two-turn architecture and does not attempt file_read itself.
                    let note = crate::web::attachment_reader::peek_acknowledgment_note(
                        &att.filename, att.content_text.as_deref().unwrap_or("").len(),
                    );
                    deep_read_digests.push((att.filename.clone(), note));
                }
            }
        }
    }

    // Inject any complete digests for this session not already provided via attachment.
    // This covers follow-up messages (no attachment) after Turn 2 has already fired,
    // ensuring the model retains document context across the full conversation.
    {
        use crate::web::handlers::background_digest::DigestStatus;
        let already_injected: std::collections::HashSet<String> =
            deep_read_digests.iter().map(|(f, _)| f.clone()).collect();
        let mut to_inject: Vec<(String, String)> = Vec::new();
        for entry in state.digest_store.iter() {
            if let DigestStatus::Complete { ref digest, session_id: ref entry_session } = *entry.value() {
                if entry_session == &session_id && !already_injected.contains(entry.key().as_str()) {
                    let filename = std::path::Path::new(entry.key())
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(entry.key());
                    tracing::info!(
                        filename, session = %session_id,
                        "Context builder: injecting cached digest for follow-up turn"
                    );
                    to_inject.push((filename.to_string(), digest.clone()));
                }
            }
        }
        deep_read_digests.extend(to_inject);
    }

    // Build the final content: user message + attachments.
    // For deep-read files, the digest REPLACES inline text (not stacked on top).
    let content_with_attachments = if deep_read_digests.is_empty() {
        if attachment_text.is_empty() {
            msg.content.clone()
        } else {
            format!("{}\n\n[ATTACHED FILES]\n{}", msg.content, attachment_text)
        }
    } else {
        let mut content = msg.content.clone();
        for (filename, digest) in &deep_read_digests {
            content.push_str(&format!("\n\n[FILE: {} — processed via deep-read]\n{}", filename, digest));
        }
        content
    };

    let tools_chars = tools.to_string().len();

    let ctx = crate::web::ws_context::build_chat_context(
        &state, &content_with_attachments, &session_id, None, images, &msg.platform, tools_chars,
    ).await;
    let mut messages = ctx.messages;

    let provider = state.provider.as_ref();

    let rx = match provider.chat(&messages, Some(&tools), state.config.prompt.thinking_enabled).await {
        Ok(rx) => rx,
        Err(e) => return build_error_response(&msg.platform, &e),
    };

    use crate::inference::stream_consumer::{self as sc, NullSink};
    let mut sink = NullSink;
    let result = sc::consume_stream(rx, &mut sink).await;

    // Handle spiral: re-prompt
    let result = match result {
        sc::ConsumeResult::Spiral { .. } => {
            sc::reprompt_after_spiral(provider, &mut messages, Some(&tools), &mut sink).await
        }
        other => other,
    };

    let (response, thinking_content, tool_events, audit_summary, has_plan, plan_markdown) = dispatch_result(
        &state, provider, &mut messages, &tools, &msg, &content_with_attachments, &session_id, result,
    ).await;

    // Read session message count after dispatch (dispatch persists the assistant turn)
    let msg_count = {
        let sessions = state.sessions.read().await;
        sessions.get(&session_id).map(|s| s.messages.len()).unwrap_or(0)
    };

    Json(serde_json::json!({
        "success": true,
        "response": response,
        "thinking": thinking_content,
        "tool_events": tool_events,
        "audit": audit_summary,
        "session_id": session_id,
        "has_plan": has_plan,
        "plan_markdown": plan_markdown,
        "message_count": msg_count,
        "platform": msg.platform,
        "channel_id": msg.channel_id,
        "message_id": msg.message_id,
    }))
}

/// Build error response when initial inference fails.
fn build_error_response(platform: &str, e: &anyhow::Error) -> Json<serde_json::Value> {
    tracing::error!(error = %e, platform = %platform, "Platform inference failed");
    Json(serde_json::json!({
        "success": false,
        "error": e.to_string(),
    }))
}

/// Dispatch the L1 inference result to the appropriate handler.
async fn dispatch_result(
    state: &AppState,
    provider: &dyn crate::provider::Provider,
    messages: &mut Vec<crate::provider::Message>,
    tools: &serde_json::Value,
    msg: &crate::platform::adapter::PlatformMessage,
    user_query: &str,
    session_id: &str,
    result: crate::inference::stream_consumer::ConsumeResult,
) -> (String, Option<String>, Vec<ToolEvent>, Option<AuditSummary>, bool, Option<String>) {
    use crate::inference::stream_consumer::ConsumeResult;
    match result {
        ConsumeResult::Reply { text, thinking } => {
            let (audited, audit) = audit_and_capture(
                state, provider, messages, tools, user_query, &text, thinking.as_deref(), session_id,
            ).await;
            crate::web::ws_learning::ingest_assistant_turn(state, &audited, session_id).await;
            crate::web::ws_learning::spawn_insight_extraction(state, user_query, &audited);
            (audited, thinking, Vec::new(), Some(audit), false, None)
        }
        ConsumeResult::Escalate { objective, plan, .. } => {
            let (reply, thinking, events, audit) = handle_escalation(
                state, provider, messages.clone(), msg, user_query, &objective, plan.as_deref(), session_id,
            ).await;
            (reply, thinking, events, audit, plan.is_some(), None)
        }
        ConsumeResult::ToolCall { id, name, arguments } => {
            // Intercept propose_plan: save the plan and surface it for the platform
            if name == "propose_plan" {
                return handle_plan_proposal(state, &arguments, session_id).await;
            }
            let tc = crate::tools::schema::ToolCall { id, name, arguments };
            let (reply, events, audit) = super::platform_exec::run_platform_tool_chain(
                state, provider, messages, tools, user_query, session_id, tc, None,
            ).await;
            crate::web::ws_learning::ingest_assistant_turn(state, &reply, session_id).await;
            (reply, None, events, audit, false, None)
        }
        ConsumeResult::ToolCalls(calls) => {
            // Execute all tool calls, then continue with tool chain from the last one
            let mut all_events = Vec::new();
            let mut tcs: Vec<crate::tools::schema::ToolCall> = calls.into_iter()
                .map(|(id, name, arguments)| crate::tools::schema::ToolCall { id, name, arguments })
                .collect();
            let last_tc = match tcs.pop() {
                Some(tc) => tc,
                None => return ("No tool calls.".to_string(), None, Vec::new(), None, false, None),
            };
            // Execute all but the last
            for tc in &tcs {
                let (result, event) = super::platform_exec::execute_and_capture(state, tc).await;
                all_events.push(event);
                super::platform_exec::append_tool_messages(messages, tc, &result);
            }
            // Run tool chain starting from the last tool call
            let (reply, events, audit) = super::platform_exec::run_platform_tool_chain(
                state, provider, messages, tools, user_query, session_id, last_tc, None,
            ).await;
            all_events.extend(events);
            crate::web::ws_learning::ingest_assistant_turn(state, &reply, session_id).await;
            (reply, None, all_events, audit, false, None)
        }
        ConsumeResult::Error(e) => {
            tracing::error!(error = %e, platform = %msg.platform, "Stream consumption failed");
            (format!("An error occurred: {}", e), None, Vec::new(), None, false, None)
        }
        _ => {
            ("Unexpected result.".to_string(), None, Vec::new(), None, false, None)
        }
    }
}

/// Handle a propose_plan tool call: save the plan and return it for the platform.
async fn handle_plan_proposal(
    state: &AppState,
    arguments: &str,
    session_id: &str,
) -> (String, Option<String>, Vec<ToolEvent>, Option<AuditSummary>, bool, Option<String>) {
    let parsed: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let title = parsed["title"].as_str().unwrap_or("Plan");
    let plan_md = parsed["plan_markdown"].as_str().unwrap_or("");
    let turns = parsed["estimated_turns"].as_u64().unwrap_or(10) as usize;

    let plan = crate::web::ws_plans::save_pending_plan(
        &state.config.general.data_dir, session_id, title, plan_md, turns.max(3).min(50),
    );
    tracing::info!(
        title = %plan.title, turns = plan.estimated_turns,
        "Platform plan proposal saved — awaiting approval"
    );

    let response = format!(
        "📋 **{}**\n\nI've prepared a plan. Review the details in the thinking thread.",
        plan.title,
    );
    (response, None, Vec::new(), None, true, Some(plan.plan_markdown))
}

/// Handle L2 escalation from L1 inference.
async fn handle_escalation(
    state: &AppState,
    provider: &dyn crate::provider::Provider,
    messages: Vec<crate::provider::Message>,
    msg: &crate::platform::adapter::PlatformMessage,
    user_query: &str,
    objective: &str,
    plan: Option<&str>,
    session_id: &str,
) -> (String, Option<String>, Vec<ToolEvent>, Option<AuditSummary>) {
    if !msg.is_admin {
        return ("I can't perform complex multi-step tasks for non-admin users. Please ask an admin.".to_string(),
                None, Vec::new(), None);
    }
    let (reply, events, audit) = super::platform_exec::run_platform_react(
        state, provider, messages, objective, plan, user_query, session_id, None,
    ).await;
    crate::web::ws_learning::ingest_assistant_turn(state, &reply, session_id).await;
    (reply, None, events, audit)
}

/// Ensure a session exists for this platform + user + channel combination.
pub(crate) async fn ensure_session(
    state: &AppState,
    session_id: &str,
    msg: &crate::platform::adapter::PlatformMessage,
) {
    let mut sessions = state.sessions.write().await;
    if sessions.get(session_id).is_none() {
        let mut session = crate::session::Session::new();
        session.id = session_id.to_string();
        session.title = format!("{} — {}", msg.platform, msg.user_name);
        if let Err(e) = sessions.update(&session) {
            tracing::warn!(error = %e, "Failed to persist new platform session");
        }
        sessions.list(); // ensure loaded
    }
}

/// Select tool schema based on admin status.
pub(crate) fn select_tools(is_admin: bool) -> serde_json::Value {
    if is_admin {
        crate::tools::schema::layer1_tools()
    } else {
        crate::tools::schema::platform_safe_tools()
    }
}

/// Run observer audit and capture training signals + conversation stack.
/// Returns the audited text and an audit summary for the thinking thread.
pub async fn audit_and_capture(
    state: &AppState,
    provider: &dyn crate::provider::Provider,
    messages: &mut Vec<crate::provider::Message>,
    tools: &serde_json::Value,
    user_query: &str,
    initial_text: &str,
    thinking: Option<&str>,
    session_id: &str,
) -> (String, AuditSummary) {
    if !state.config.observer.enabled || initial_text.is_empty() {
        return (initial_text.to_string(), AuditSummary::skipped());
    }

    // Reconstruct the raw model output (thinking + text) for the first observer call.
    // The llama-server KV cache holds the full token sequence including thinking tokens.
    // Passing the thinking-stripped text breaks the prefix match and forces a cold
    // recompute of the entire conversation history on every turn.
    let raw_first_candidate = match thinking {
        Some(t) if !t.is_empty() => format!("<think>{}</think>\n{}", t, initial_text),
        _ => initial_text.to_string(),
    };

    let tool_context = super::platform_exec::build_tool_context(messages);
    let mut current_text = initial_text.to_string();
    let mut retries: usize = 0;

    // No cap — the model MUST produce an approved response.
    // If this loops, it means the observer feedback isn't being followed,
    // which is a deeper bug to fix — not mask with a bailout.
    loop {
        // First call uses the raw candidate (with thinking) to match the KV cache prefix.
        // Retries use current_text (which is the regenerated clean response).
        let candidate = if retries == 0 { &raw_first_candidate } else { &current_text };
        match crate::observer::audit_response(
            provider, messages, candidate, &tool_context, user_query,
        ).await {
            Ok(output) if output.result.verdict.is_allowed() => {
                crate::observer::persist_audit_result(&state.config.general.data_dir, &output.result);
                return handle_approved(state, user_query, &current_text, session_id, &output.result, retries);
            }
            Ok(output) => {
                retries += 1;
                let rejected = current_text.clone();
                tracing::warn!(
                    retries,
                    category = %output.result.failure_category,
                    what_went_wrong = %output.result.what_went_wrong,
                    how_to_fix = %output.result.how_to_fix,
                    "Observer BLOCKED — re-inferring with feedback"
                );
                crate::observer::persist_audit_result(&state.config.general.data_dir, &output.result);
                let (new_text, pre_audited) = retry_after_rejection(
                    state, provider, messages, tools, user_query, session_id, &rejected, &output.result,
                ).await;
                if pre_audited {
                    // Tool chain already audited this text internally — accept directly.
                    // Re-auditing causes context divergence (the outer loop lacks the
                    // tool results that the inner audit had), producing false rejections.
                    tracing::info!(
                        retries,
                        "Observer retry: tool chain returned pre-audited reply — accepting"
                    );
                    return (new_text, AuditSummary {
                        verdict: "Allowed".to_string(),
                        confidence: 0.0,
                        retries,
                        active_topic: String::new(),
                    });
                }
                current_text = new_text;
            }
            Err(e) => {
                // §4: Fail-CLOSED — observer down means response blocked.
                tracing::error!(error = %e, "Platform observer failed — fail-CLOSED (response blocked)");
                return ("[Observer infrastructure error — response blocked. Please retry.]".to_string(), AuditSummary::error(retries));
            }
        }
    }
}

/// Handle approved audit — capture training signal and save conversation stack.
fn handle_approved(
    state: &AppState,
    user_query: &str,
    text: &str,
    session_id: &str,
    result: &crate::observer::AuditResult,
    retries: usize,
) -> (String, AuditSummary) {
    crate::web::training_capture::capture_approved_with_flags(
        state, user_query, text, result.confidence, &result.positive_flags,
    );
    crate::web::ws_stream::save_conversation_stack(state, session_id, result);
    (text.to_string(), AuditSummary {
        verdict: "Allowed".to_string(),
        confidence: result.confidence,
        retries,
        active_topic: result.active_topic.clone(),
    })
}


/// Retry inference after observer rejection.
/// Handles tool calls in the retry response — the model may follow observer
/// guidance by calling file_read/codebase_search instead of generating text.
///
/// Returns `(text, pre_audited)` — when the retry dispatches through
/// `run_platform_tool_chain`, the tool chain already runs `audit_and_capture`
/// internally. The `pre_audited = true` flag tells the outer loop to accept
/// the result directly, preventing the double-audit that causes infinite loops.
///
/// Uses Box::pin indirection because this is part of a recursive async cycle:
/// retry_after_rejection → run_platform_tool_chain → audit_and_capture → retry_after_rejection
fn retry_after_rejection<'a>(
    state: &'a AppState,
    provider: &'a dyn crate::provider::Provider,
    messages: &'a mut Vec<crate::provider::Message>,
    tools: &'a serde_json::Value,
    user_query: &'a str,
    session_id: &'a str,
    rejected_text: &'a str,
    result: &'a crate::observer::AuditResult,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = (String, bool)> + Send + 'a>> {
    Box::pin(async move {
        let feedback = crate::observer::format_rejection_feedback(result);

        // Prune stale rejection pairs before appending new ones.
        // Without this, repeated confabulation retries stack N copies of the
        // rejected text + feedback, polluting the context with the very content
        // the observer told the model to stop generating.
        prune_stale_rejection_pairs(messages);

        messages.push(crate::provider::Message::text("assistant", rejected_text));
        messages.push(crate::provider::Message::text("system", &feedback));
        super::platform_exec::enforce_context_budget(provider, messages, Some(tools), state.model_spec.context_length, true).await;

        if let Ok(rx) = provider.chat(messages, Some(tools), true).await {
            use crate::inference::stream_consumer::{self as sc, NullSink};
            let mut sink = NullSink;
            match sc::consume_stream(rx, &mut sink).await {
                sc::ConsumeResult::Reply { text, .. } => {
                    crate::web::training_capture::capture_rejection(
                        state, user_query, rejected_text, &text, &result.what_went_wrong,
                    );
                    return (text, false);
                }
                sc::ConsumeResult::ToolCall { id, name, arguments } => {
                    // Model is following observer guidance — execute the tool chain.
                    // The tool chain runs audit_and_capture internally → pre-audited.
                    tracing::info!(
                        tool = %name, retries = ?result.failure_category,
                        "Observer retry: model called tool (following feedback) — executing"
                    );
                    let tc = crate::tools::schema::ToolCall { id, name, arguments };
                    let (reply, _events, _audit) = super::platform_exec::run_platform_tool_chain(
                        state, provider, messages, tools, user_query, session_id, tc, None,
                    ).await;
                    return (reply, true);
                }
                sc::ConsumeResult::ToolCalls(calls) => {
                    tracing::info!(
                        count = calls.len(),
                        "Observer retry: model called multiple tools (following feedback) — executing"
                    );
                    let mut tcs: Vec<crate::tools::schema::ToolCall> = calls.into_iter()
                        .map(|(id, name, arguments)| crate::tools::schema::ToolCall { id, name, arguments })
                        .collect();
                    if let Some(last_tc) = tcs.pop() {
                        for tc in &tcs {
                            let (result, _event) = super::platform_exec::execute_and_capture(state, tc).await;
                            super::platform_exec::append_tool_messages(messages, tc, &result);
                        }
                        let (reply, _events, _audit) = super::platform_exec::run_platform_tool_chain(
                            state, provider, messages, tools, user_query, session_id, last_tc, None,
                        ).await;
                        return (reply, true);
                    }
                }
                _ => {} // Empty/Error — fall through to rejected_text
            }
        }

        (rejected_text.to_string(), false)
    })
}

/// Remove prior observer rejection pairs (assistant rejected text + system feedback)
/// from the message history. Keeps only the latest pair to prevent context pollution.
///
/// During confabulation retries, each rejection appends the rejected text and feedback.
/// After 8 retries, the context contains 8 copies of hallucinated content (e.g. "ErnieBook")
/// which the model sees 16 times and treats as real. Pruning ensures only the most recent
/// rejection context is present.
fn prune_stale_rejection_pairs(messages: &mut Vec<crate::provider::Message>) {
    // Walk backwards and remove system messages that contain the rejection marker
    let mut i = messages.len();
    while i > 0 {
        i -= 1;
        if messages[i].role == "system"
            && messages[i].text_content().contains("[SELF-CHECK FAIL: INVISIBLE TO USER]")
        {
            // Remove the feedback message and the preceding assistant rejected text
            messages.remove(i);
            if i > 0 && messages[i - 1].role == "assistant" {
                messages.remove(i - 1);
                i -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_id_scoping() {
        let session_id = format!("{}_{}_{}", "discord", "user123", "channel456");
        assert_eq!(session_id, "discord_user123_channel456");
    }

    #[test]
    fn test_select_tools_admin() {
        let tools = select_tools(true);
        assert!(tools.is_array());
    }

    #[test]
    fn test_select_tools_non_admin() {
        let tools = select_tools(false);
        assert!(tools.is_array());
    }

    #[test]
    fn test_tool_event_serialize() {
        let event = ToolEvent {
            name: "web_search".to_string(),
            success: true,
            elapsed_ms: 1234,
            output_preview: "Found 5 results".to_string(),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["name"], "web_search");
        assert_eq!(json["success"], true);
        assert_eq!(json["elapsed_ms"], 1234);
    }

    #[test]
    fn test_audit_summary_serialize() {
        let summary = AuditSummary {
            verdict: "Allowed".to_string(),
            confidence: 8.5,
            retries: 0,
            active_topic: "Testing serialization".to_string(),
        };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["verdict"], "Allowed");
        assert_eq!(json["confidence"], 8.5);
    }

    #[test]
    fn test_prune_stale_rejection_pairs_removes_feedback() {
        let mut messages = vec![
            crate::provider::Message::text("system", "You are Ernos."),
            crate::provider::Message::text("user", "Hello"),
            crate::provider::Message::text("assistant", "rejected response 1"),
            crate::provider::Message::text("system", "[SELF-CHECK FAIL: INVISIBLE TO USER] Category: confabulation"),
            crate::provider::Message::text("assistant", "rejected response 2"),
            crate::provider::Message::text("system", "[SELF-CHECK FAIL: INVISIBLE TO USER] Category: confabulation"),
        ];
        prune_stale_rejection_pairs(&mut messages);
        // Should remove both rejection pairs, leaving only system + user
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[1].role, "user");
    }

    #[test]
    fn test_prune_stale_rejection_pairs_preserves_non_rejection() {
        let mut messages = vec![
            crate::provider::Message::text("system", "You are Ernos."),
            crate::provider::Message::text("user", "Hello"),
            crate::provider::Message::text("assistant", "Good response"),
            crate::provider::Message::text("system", "Normal system message"),
        ];
        prune_stale_rejection_pairs(&mut messages);
        // Nothing removed — no rejection markers
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn test_prune_stale_rejection_pairs_empty() {
        let mut messages: Vec<crate::provider::Message> = Vec::new();
        prune_stale_rejection_pairs(&mut messages);
        assert!(messages.is_empty());
    }
}
