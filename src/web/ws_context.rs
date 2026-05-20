//! WebSocket context builder — assembles the full message context for inference.

use crate::memory::consolidation::ConsolidationEngine;
use crate::provider::Message;
use crate::web::state::AppState;
use super::hud_data;

/// Assembled chat context ready for inference.
pub struct ChatContext {
    pub messages: Vec<Message>,
    pub session_id: String,
    pub user_query: String,
}

/// Build the full inference context: system prompt, memory, session history, consolidation.
/// `tools_chars` is the measured byte length of the serialised tool definitions JSON.
pub async fn build_chat_context(
    state: &AppState,
    content: &str,
    session_id: &str,
    agent_id: Option<&str>,
    images: Vec<String>,
    platform: &str,
    tools_chars: usize,
) -> ChatContext {
    let mut messages = Vec::new();

    // ── Load session conversation history ──
    let session_history = {
        let sessions = state.sessions.read().await;
        sessions.get(session_id).map(|s| s.messages.clone()).unwrap_or_default()
    };
    let turn_count = session_history.len();

    // ── Multi-part system prompt assembly (agent-aware) ──
    let (core_prompt, identity_prompt) = resolve_prompts(state, agent_id).await;

    // Embed user query for RAG document retrieval (best-effort)
    let query_embedding = state.provider.embed(content).await.ok();

    let (memory_context, memory_counts, hud_data) = {
        let memory = state.memory.read().await;
        let ctx = memory.recall_context(content, 2000, query_embedding.as_deref(), None);
        let counts = (
            memory.timeline.entry_count(),
            memory.lessons.count(),
            memory.procedures.count(),
            memory.scratchpad.count(),
            memory.documents.count(),
        );
        let lessons = hud_data::match_relevant_lessons(&memory.lessons, content, &session_history);
        let procedures = hud_data::match_relevant_procedures(&memory.procedures, content, &session_history);
        let scratchpad = hud_data::format_scratchpad_content(&memory.scratchpad);
        let kg = hud_data::format_kg_snapshot(&memory.synaptic);
        let narrative = hud_data::format_timeline_narrative(&memory.timeline);
        (ctx, counts, (lessons, procedures, scratchpad, kg, narrative))
    };

    let golden_count = state.golden_buffer.read().await.count();
    let rejection_count = state.rejection_buffer.read().await.count();
    let consolidation_count = {
        let memory = state.memory.read().await;
        memory.consolidation.consolidation_count()
    };

    // ── Load conversation stack (generated retroactively by observer audit) ──
    let conversation_stack = {
        let stack_store = crate::prompt::conversation_stack::ConversationStackStore::new(
            std::path::Path::new(&state.config.general.data_dir),
        );
        let stack = stack_store.load(session_id);
        if stack.active_topic.is_empty() { None } else { Some(stack) }
    };

    let curriculum_count = state.curriculum.read().await.course_count();
    let (review_total, review_due) = {
        let deck = state.review_deck.read().await;
        (deck.count(), deck.due_count(chrono::Utc::now()))
    };
    let quarantine_count = state.quarantine.read().await.count();

    // ── Phase 3: Pre-HUD context usage estimate ──
    let history_chars: usize = session_history.iter().map(|m| m.text_content().len()).sum();
    let est_tokens = (history_chars + content.len() + tools_chars) / 4;
    let context_usage_pct = est_tokens as f32 / state.model_spec.context_length.max(1) as f32;

    // ── Phase 5: System log tail (WARN/ERROR only) ──
    let system_log_tail = hud_data::read_log_tail(&state.config.general.data_dir);

    // ── Phase 7: Reasoning traces from persisted thinking logs ──
    let reasoning_traces = hud_data::extract_recent_reasoning(&state.config.general.data_dir, session_id);

    // ── Phase 8: Active steering vectors ──
    let active_steering = hud_data::format_active_steering(&state.config.general.data_dir);

    // ── Phase 9: Platform connection status ──
    let platform_status = state.platforms.read().await.status_summary();

    // ── Phase 11: User preferences ──
    let user_preferences = hud_data::load_user_preferences(&state.config.general.data_dir, session_id);

    // ── Phase 12: Scheduler status ──
    let scheduler_status = hud_data::format_scheduler_status(&*state.scheduler.read().await);

    let hud = crate::prompt::hud::build_hud(&crate::prompt::hud::HudContext {
        model_name: state.model_spec.name.clone(),
        provider: state.config.general.active_provider.clone(),
        context_length: state.model_spec.context_length,
        session_id: session_id.to_string(),
        turn_count,
        platform: platform.to_string(),
        timeline_count: memory_counts.0,
        lesson_count: memory_counts.1,
        procedure_count: memory_counts.2,
        scratchpad_count: memory_counts.3,
        document_count: memory_counts.4,
        golden_count,
        rejection_count,
        curriculum_count,
        review_total,
        review_due,
        quarantine_count,
        observer_enabled: state.config.observer.enabled,
        conversation_stack,
        relevant_lessons: hud_data.0,
        relevant_procedures: hud_data.1,
        context_usage_pct,
        scratchpad_content: hud_data.2,
        system_log_tail,
        kg_snapshot: hud_data.3,
        reasoning_traces,
        active_steering,
        platform_status,
        timeline_narrative: hud_data.4,
        user_preferences,
        scheduler_status,
        consolidation_count,
    });

    let system_prompt = crate::prompt::assemble(&core_prompt, &identity_prompt, &memory_context, &hud);

    // ── Context Consolidation ──
    let working_history = consolidate_if_needed(state, session_history, &system_prompt, content, tools_chars).await;

    // ── Message Assembly: Attention-Optimized Ordering ──
    for msg in &working_history {
        messages.push(msg.clone());
    }
    messages.push(Message::text("system", &system_prompt));

    let current_user_msg = if images.is_empty() {
        Message::text("user", content)
    } else {
        tracing::info!(count = images.len(), "Multimodal message with images");
        Message::multipart("user", content, images)
    };
    messages.push(current_user_msg.clone());

    // ── Persist to session ──
    {
        let mut sessions = state.sessions.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            session.messages.push(current_user_msg);
            session.updated_at = chrono::Utc::now();
            if session.messages.len() == 1 { session.auto_title(); }
            let updated = session.clone();
            let _ = sessions.update(&updated);
        }
    }

    // Ingest turn into timeline memory
    {
        let mut memory = state.memory.write().await;
        memory.ingest_turn("user", content, session_id, None);
    }

    ChatContext {
        messages,
        session_id: session_id.to_string(),
        user_query: content.to_string(),
    }
}

/// Resolve agent-specific or default prompts.
async fn resolve_prompts(state: &AppState, agent_id: Option<&str>) -> (String, String) {
    if let Some(aid) = agent_id {
        let agents = state.agents.read().await;
        let agent = agents.get(aid);
        let core_custom = agent.map_or(false, |a| a.has_custom_prompt("core"));
        let identity_custom = agent.map_or(false, |a| a.has_custom_prompt("identity"));
        let core = agents.resolve_prompt(aid, "core")
            .unwrap_or_else(|_| crate::prompt::load_core(std::path::Path::new(&state.config.general.data_dir)));
        let identity = agents.resolve_prompt(aid, "identity")
            .unwrap_or_else(|_| crate::prompt::load_identity(std::path::Path::new(&state.config.general.data_dir)));
        tracing::info!(
            agent = %aid,
            core_custom, identity_custom,
            "Agent prompt resolution"
        );
        (core, identity)
    } else {
        let core = crate::prompt::load_core(std::path::Path::new(&state.config.general.data_dir));
        let identity = crate::prompt::load_identity(std::path::Path::new(&state.config.general.data_dir));
        (core, identity)
    }
}

/// Consolidate session history if context usage exceeds 80%.
/// `tools_chars` is the measured byte length of serialised tool definitions.
async fn consolidate_if_needed(
    state: &AppState,
    session_history: Vec<Message>,
    system_prompt: &str,
    content: &str,
    _tools_chars: usize,
) -> Vec<Message> {
    let context_length = state.model_spec.context_length;

    // Fast path: if char-estimated tokens are well below trim threshold, skip count_tokens.
    // This is a gate (same pattern as enforce_context_budget:42-51), not a measurement.
    // The real count_tokens fires when context is near threshold.
    let total_chars: usize = session_history.iter().map(|m| m.text_content().len()).sum::<usize>()
        + system_prompt.len() + content.len();
    let tools_json = crate::tools::schema::layer1_tools();
    let tool_chars = tools_json.to_string().len();
    let char_estimated_tokens = (total_chars + tool_chars) / 4;
    let trim_budget = (context_length as f64 * state.config.context.trim_threshold) as usize;

    if char_estimated_tokens < trim_budget {
        tracing::debug!(
            char_estimated_tokens,
            trim_budget,
            "Consolidation fast-path: provably under threshold — skipping count_tokens"
        );
        return session_history;
    }

    // Near or over threshold — measure exactly via server tokenizer.
    let mut temp_messages = session_history.clone();
    temp_messages.push(Message::text("system", system_prompt));
    temp_messages.push(Message::text("user", content));
    let estimated_tokens = match state.provider.count_tokens(&temp_messages, Some(&tools_json), state.config.prompt.thinking_enabled).await {
        Ok(t) => t,
        Err(e) => {
            // §2.7: fail to OFF — consolidation disabled for this turn
            tracing::error!(
                error = %e,
                "count_tokens failed — consolidation disabled for this turn"
            );
            return session_history;
        }
    };
    let usage_pct = estimated_tokens as f32 / context_length as f32;

    tracing::debug!(
        estimated_tokens,
        context_length,
        usage_pct = format!("{:.1}%", usage_pct * 100.0),
        "Context usage accounting (server-side tokenization)"
    );

    if usage_pct < state.config.context.trim_threshold as f32 {
        return session_history;
    }

    // ── Stage 1: Progressive trim at trim–consolidation% — compress verbose tool results ──
    if usage_pct < state.config.context.consolidation_threshold as f32 {
        tracing::info!(
            usage_pct = format!("{:.0}%", usage_pct * 100.0),
            "Context at trim threshold — progressive trimming tool results"
        );
        return trim_verbose_tool_results(
            session_history,
            state.config.context.trim_keep_recent,
            state.config.context.trim_tool_result_chars,
        );
    }

    tracing::info!(
        usage_pct = format!("{:.0}%", usage_pct * 100.0),
        history_msgs = session_history.len(),
        "Context usage above consolidation threshold — triggering memory sort + LLM consolidation"
    );

    let (old_messages, recent_messages) = {
        let memory = state.memory.read().await;
        memory.consolidation.split_for_consolidation(
            &session_history,
            state.config.context.consolidation_split_ratio,
        )
    };

    if old_messages.is_empty() {
        return session_history;
    }

    // ── Step 1: Model sorts old messages into memory tiers before discarding ──
    // Uses digest_provider (slot 1) — does not block slot 0 inference.
    // §2.4: if sort fails, abort consolidation — keep full history, discard nothing.
    if let Err(e) = run_memory_sort_pass(state, &old_messages).await {
        tracing::error!(
            error = %e,
            msgs = old_messages.len(),
            "Memory sort pass failed — consolidation aborted, keeping full history"
        );
        return session_history;
    }

    // ── Step 2: LLM summarisation (slot 1) — runs AFTER memory is safe ──
    let old_text: String = old_messages.iter()
        .map(|m| format!("{}: {}", m.role, m.text_content()))
        .collect::<Vec<_>>().join("\n");

    let summary_prompt = vec![
        Message::text("system",
            "You are a context consolidation engine. Summarize the following conversation \
             into a dense, factual summary preserving all key information, decisions, \
             code changes, facts discussed, and user preferences. Be thorough but concise. \
             CRITICAL: Always preserve any [FILE SAVED: ...] references and file paths \
             EXACTLY as they appear — the model needs these to re-read files later. \
             Output ONLY the summary, no preamble."),
        Message::text("user", &format!(
            "Summarize this conversation segment ({} messages):\n\n{}",
            old_messages.len(), old_text
        )),
    ];

    let summary = match state.digest_provider.chat_sync(&summary_prompt, None).await {
        Ok(s) => {
            tracing::info!(input_chars = old_text.len(), summary_chars = s.len(), "LLM consolidation generated");
            s
        }
        Err(e) => {
            tracing::warn!(error = %e, "LLM consolidation failed, keeping full content");
            old_text.clone()
        }
    };

    {
        let mut memory = state.memory.write().await;
        let _ = memory.consolidation.record_consolidation(old_messages.len(), &summary, old_text.len());
    }

    tracing::info!(
        consolidated = old_messages.len(), kept = recent_messages.len(),
        "Session context consolidated via LLM"
    );

    let mut working_history = Vec::new();
    working_history.push(ConsolidationEngine::summary_message(&summary));
    working_history.extend(recent_messages);
    working_history
}

/// Run the model-driven memory sorting inference pass over old_messages.
/// The model issues tool calls sorting entities, relationships, lessons, and
/// pinned facts into the correct memory tiers before they are discarded.
/// Uses digest_provider (slot 1) to avoid blocking slot 0 inference.
/// Returns Err only on total failure — partial tool call success is acceptable.
async fn run_memory_sort_pass(state: &AppState, old_messages: &[Message]) -> anyhow::Result<()> {
    use crate::inference::react_loop::{ReactContext, run_iteration};
    use crate::inference::react_loop::IterationResult;

    let sort_prompt = crate::memory::consolidation::pre_consolidation_sort_prompt(old_messages.len());
    let old_text: String = old_messages.iter()
        .map(|m| format!("{}: {}", m.role, m.text_content()))
        .collect::<Vec<_>>().join("\n");

    let base = vec![
        Message::text("system", &sort_prompt),
        Message::text("user", &format!(
            "Here are the {} messages to sort into memory before they are lost:\n\n{}",
            old_messages.len(), old_text
        )),
    ];

    let limit = state.config.context.sort_pass_tool_call_limit;
    let mut calls_made = 0usize;

    // Build a ReactContext for the memory-sort pass.
    // run_iteration uses layer2_tools internally which includes all memory tools.
    let mut ctx = ReactContext::new(
        "Sort old conversation messages into memory tiers before consolidation.",
        None,
        base,
    );

    loop {
        if calls_made >= limit {
            tracing::warn!(calls_made, limit, "Memory sort pass hit tool call limit");
            break;
        }

        match run_iteration(state.digest_provider.as_ref(), &ctx, false).await {
            Ok(IterationResult::Reply(text, _)) | Ok(IterationResult::ImplicitReply(text, _)) => {
                tracing::info!(calls_made, summary = %text.chars().take(200).collect::<String>(), "Memory sort pass complete");
                break;
            }
            Ok(IterationResult::Refuse(reason)) => {
                tracing::warn!(reason = %reason, "Memory sort pass refused — treating as complete");
                break;
            }
            Ok(IterationResult::ToolCall(tc)) => {
                if tc.name == "reply_request" {
                    tracing::info!(calls_made, "Memory sort pass: reply_request");
                    break;
                }
                let result = crate::web::tool_dispatch::execute_tool_with_state(state, &tc).await;
                tracing::debug!(tool = %tc.name, output = %result.output.chars().take(120).collect::<String>(), "Sort pass tool executed");
                ctx.add_tool_result(&tc, result);
                calls_made += 1;
            }
            Ok(IterationResult::ToolCalls(tcs)) => {
                for tc in &tcs {
                    if tc.name == "reply_request" {
                        tracing::info!(calls_made, "Memory sort pass: reply_request in batch");
                        return Ok(());
                    }
                    let result = crate::web::tool_dispatch::execute_tool_with_state(state, tc).await;
                    tracing::debug!(tool = %tc.name, "Sort pass batch tool executed");
                    ctx.add_tool_result(tc, result);
                    calls_made += 1;
                }
            }
            Ok(IterationResult::ExtendTurns { .. }) => {
                // Not applicable in sort pass — treat as done
                tracing::info!("Memory sort pass: extend_turns received — treating as done");
                break;
            }
            Err(e) => {
                return Err(anyhow::anyhow!("Memory sort pass inference failed: {}", e));
            }
        }
    }

    Ok(())
}

/// Trim verbose tool results with lossless bookmarks (PR5).
/// Keeps `keep_recent` messages verbatim. Trims older tool results
/// to `trim_chars` with a [BOOKMARK] for recovery via file_read.
fn trim_verbose_tool_results(
    history: Vec<Message>,
    keep_recent: usize,
    trim_chars: usize,
) -> Vec<Message> {
    let len = history.len();
    if len <= keep_recent {
        return history;
    }
    let boundary = len - keep_recent;
    let mut trimmed = Vec::with_capacity(len);
    for (i, msg) in history.into_iter().enumerate() {
        if i < boundary && msg.role == "tool" {
            let text = msg.text_content();
            if text.len() > trim_chars {
                let cut = text.char_indices()
                    .take_while(|(idx, _)| *idx <= trim_chars)
                    .last()
                    .map(|(idx, _)| idx)
                    .unwrap_or(trim_chars.min(text.len()));
                let shown_lines = text[..cut].lines().count();
                let total_lines = text.lines().count();
                let new_content = format!(
                    "{}\n[TRIMMED — {}/{} chars shown, {} total lines. \
                     BOOKMARK: use file_read start_line={} to recover remaining content]",
                    &text[..cut], cut, text.len(), total_lines, shown_lines + 1
                );
                trimmed.push(Message::tool_result(
                    msg.tool_call_id.as_deref().unwrap_or(""),
                    &new_content,
                ));
                continue;
            }
        }
        trimmed.push(msg);
    }
    trimmed
}
