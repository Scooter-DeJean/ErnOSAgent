// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! SSE stream parser — handles OpenAI-compatible SSE streams with
//! Gemma 4 thinking block extraction and tool call accumulation.

use crate::provider::StreamEvent;
use anyhow::Result;
use futures_util::StreamExt;
use reqwest::Response;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// Tool call accumulation, stall detection, and token stripping utilities
// moved to stream_parser_util.rs (§10.1).
use super::stream_parser_util::{
    ToolCallAccumulator, SseToolCallDelta,
    check_server_stall, safe_emit_length, accumulate_tool_call,
    emit_accumulated_tools, erase_slot_cache,
};
#[cfg(test)]
use super::stream_parser_util::SseFunctionDelta;

/// OpenAI SSE delta chunk structure.
#[derive(Debug, Deserialize)]
struct SseChunk {
    choices: Option<Vec<SseChoice>>,
}

#[derive(Debug, Deserialize)]
struct SseChoice {
    delta: Option<SseDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SseDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    tool_calls: Option<Vec<SseToolCallDelta>>,
}

/// State machine for extracting thinking blocks from content stream.
#[derive(Debug, PartialEq)]
enum ThinkingState {
    /// Normal text output
    Normal,
    /// Inside a `<|channel>thought` ... `<channel|>` block (Gemma 4)
    InsideGemma4,
    /// Inside a `<think>` ... `</think>` block (legacy)
    InsideLegacy,
}

/// Parse an SSE stream from an OpenAI-compatible endpoint.
///
/// Handles:
/// - Text content deltas
/// - Gemma 4 thinking: `<|channel>thought` ... `<channel|>` extraction
/// - Legacy thinking: `<think>` ... `</think>` extraction
/// - `reasoning_content` field (direct thinking support)
/// - Tool call delta accumulation by index
/// - Stream completion
pub async fn parse_sse_stream(
    response: Response,
    tx: mpsc::Sender<StreamEvent>,
    slots_url: Option<String>,
) -> Result<()> {
    let mut tool_calls: Vec<ToolCallAccumulator> = Vec::new();
    let mut thinking_state = ThinkingState::Normal;
    let mut content_buffer = String::new();

    let mut stream = response.bytes_stream();
    let mut chunk_count: u64 = 0;
    let mut has_content = false; // track if any real content was generated
    let start = Instant::now();
    let mut last_chunk_time = Instant::now();
    let mut line_buffer = String::new(); // Buffer for partial lines split across HTTP chunks
    let mut parsed_lines: Vec<String> = Vec::new(); // Track raw lines for empty response diagnostics
    let mut stall_interval = tokio::time::interval(Duration::from_secs(10));
    stall_interval.tick().await; // consume first immediate tick

    loop {
        let chunk_result = tokio::select! {
            chunk_opt = stream.next() => {
                match chunk_opt {
                    Some(r) => r,
                    None => break, // stream ended
                }
            }
            _ = stall_interval.tick() => {
                if let Some(ref url) = slots_url {
                    // Slot 0 = main inference (chat() always targets slot 0)
                    if let Some(stall) = check_server_stall(url, chunk_count, &last_chunk_time, 0).await {
                        tracing::warn!(
                            server_decoded = stall.n_decoded,
                            server_predicted = stall.n_predicted,
                            client_chunks = chunk_count,
                            secs_since_last_chunk = last_chunk_time.elapsed().as_secs(),
                            "SSE stream: generation stall detected (n_predicted confirms server IS generating)"
                        );
                        // Erase the slot's KV cache before returning — prevents cache poisoning
                        // that causes all subsequent requests to get empty responses.
                        let base_url = url.trim_end_matches("/slots");
                        erase_slot_cache(base_url, 0).await;
                        let _ = tx.send(StreamEvent::Error(
                            "Stream stalled: server generating tokens but HTTP stream not flushing".into()
                        )).await;
                        return Ok(());
                    }
                }
                continue;
            }
        };
        // If the consumer dropped the receiver (e.g. spiral detection killed the stream),
        // abort immediately. This drops the HTTP response, freeing the llama-server slot
        // so the recovery request can proceed without blocking.
        if tx.is_closed() {
            tracing::info!(
                chunks = chunk_count,
                elapsed_ms = start.elapsed().as_millis() as u64,
                "SSE stream: consumer disconnected — aborting to free server slot"
            );
            return Ok(());
        }

        let chunk = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, chunks = chunk_count, "SSE stream: chunk read error");
                let _ = tx.send(StreamEvent::Error(e.to_string())).await;
                break;
            }
        };

        chunk_count += 1;
        last_chunk_time = Instant::now();
        let text = String::from_utf8_lossy(&chunk);

        // Diagnostics: log raw content of early chunks at debug level
        // to diagnose the recurring empty response pattern.
        if chunk_count <= 5 {
            tracing::debug!(
                chunk_num = chunk_count,
                len = text.len(),
                content = %text.chars().take(300).collect::<String>(),
                "SSE raw chunk"
            );
        }

        // Append to line buffer and process complete lines
        line_buffer.push_str(&text);

        // Process all complete lines (terminated by \n)
        while let Some(newline_pos) = line_buffer.find('\n') {
            let line = line_buffer[..newline_pos].trim().to_string();
            line_buffer = line_buffer[newline_pos + 1..].to_string();

            if line.is_empty() || line.starts_with(':') {
                continue;
            }

            // Track raw lines for diagnostics when response is empty
            if parsed_lines.len() < 10 {
                parsed_lines.push(line.chars().take(200).collect::<String>());
            }

            if line == "data: [DONE]" {
                // CRITICAL: flush remaining content buffer before Done
                if !content_buffer.is_empty() {
                    let _ = tx.send(StreamEvent::TextDelta(content_buffer.clone())).await;
                    content_buffer.clear();
                }
                emit_accumulated_tools(&tool_calls, &tx).await;
                let _ = tx.send(StreamEvent::Done).await;
                if !has_content && tool_calls.is_empty() {
                    tracing::error!(
                        chunks = chunk_count,
                        content_buffer_len = content_buffer.len(),
                        elapsed_ms = start.elapsed().as_millis() as u64,
                        raw_lines = ?parsed_lines,
                        "SSE stream: [DONE] with NO content and NO tool calls — raw lines logged for diagnostics"
                    );
                }
                tracing::debug!(
                    chunks = chunk_count,
                    tool_calls = tool_calls.len(),
                    has_content,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "SSE stream: [DONE] received"
                );
                return Ok(());
            }

            if let Some(json_str) = line.strip_prefix("data: ") {
                if let Ok(chunk) = serde_json::from_str::<SseChunk>(json_str) {
                    let produced = process_chunk(
                        &chunk,
                        &mut tool_calls,
                        &mut thinking_state,
                        &mut content_buffer,
                        &tx,
                    )
                    .await;
                    if produced {
                        has_content = true;
                    }
                }
            }
        }
    }

    // Stream ended without [DONE] — flush remaining content buffer
    tracing::warn!(
        chunks = chunk_count,
        content_len = content_buffer.len(),
        tool_calls = tool_calls.len(),
        elapsed_ms = start.elapsed().as_millis() as u64,
        "SSE stream: ended WITHOUT [DONE]"
    );
    if !content_buffer.is_empty() {
        let _ = tx.send(StreamEvent::TextDelta(content_buffer)).await;
    }
    emit_accumulated_tools(&tool_calls, &tx).await;
    let _ = tx.send(StreamEvent::Done).await;
    Ok(())
}

/// Server stall detection result — carries the server's own reported state.
// check_server_stall and StallInfo moved to stream_parser_util.rs (§10.1).


/// Process a single SSE chunk.
/// Returns `true` if the chunk contained any content or tool call data.
async fn process_chunk(
    chunk: &SseChunk,
    tool_calls: &mut Vec<ToolCallAccumulator>,
    thinking_state: &mut ThinkingState,
    content_buffer: &mut String,
    tx: &mpsc::Sender<StreamEvent>,
) -> bool {
    let choices = match &chunk.choices {
        Some(c) => c,
        None => return false,
    };

    let mut produced = false;

    for choice in choices {
        let delta = match &choice.delta {
            Some(d) => d,
            None => {
                // No delta — this is a finish-only chunk
                if let Some(reason) = &choice.finish_reason {
                    match reason.as_str() {
                        "length" => tracing::warn!("SSE: finish_reason=length — model hit max_tokens"),
                        "stop" => tracing::debug!("SSE: finish_reason=stop"),
                        other => tracing::info!(finish_reason = other, "SSE: finish_reason received"),
                    }
                }
                continue;
            }
        };

        // Direct reasoning_content field (some providers)
        if let Some(reasoning) = &delta.reasoning_content {
            if !reasoning.is_empty() {
                produced = true;
                let _ = tx.send(StreamEvent::ThinkingDelta(reasoning.clone())).await;
            }
        }

        // Content with thinking block extraction
        if let Some(content) = &delta.content {
            if !content.is_empty() {
                produced = true;
            }
            process_content_delta(
                content,
                thinking_state,
                content_buffer,
                tx,
            )
            .await;
        }

        // Tool call delta accumulation
        if let Some(tc_deltas) = &delta.tool_calls {
            produced = true;
            for tc_delta in tc_deltas {
                accumulate_tool_call(tool_calls, tc_delta);
            }
        }

        // Check finish_reason for truncation detection
        if let Some(reason) = &choice.finish_reason {
            match reason.as_str() {
                "length" => tracing::warn!("SSE: finish_reason=length — response truncated"),
                _ => {}
            }
        }
    }
    produced
}

/// Process content delta with thinking block extraction.
///
/// Handles both Gemma 4 (`<|channel>thought` ... `<channel|>`) and
/// legacy (`<think>` ... `</think>`) thinking formats.
async fn process_content_delta(
    content: &str,
    state: &mut ThinkingState,
    buffer: &mut String,
    tx: &mpsc::Sender<StreamEvent>,
) {
    buffer.push_str(content);

    loop {
        match state {
            ThinkingState::Normal => {
                // Check for Gemma 4 thinking start
                if let Some(pos) = buffer.find("<|channel>thought") {
                    let before = &buffer[..pos];
                    if !before.is_empty() {
                        let _ = tx.send(StreamEvent::TextDelta(before.to_string())).await;
                    }
                    *buffer = buffer[pos + "<|channel>thought".len()..].to_string();
                    *state = ThinkingState::InsideGemma4;
                    continue;
                }

                // Check for legacy thinking start
                if let Some(pos) = buffer.find("<think>") {
                    let before = &buffer[..pos];
                    if !before.is_empty() {
                        let _ = tx.send(StreamEvent::TextDelta(before.to_string())).await;
                    }
                    *buffer = buffer[pos + "<think>".len()..].to_string();
                    *state = ThinkingState::InsideLegacy;
                    continue;
                }

                // No thinking tags — check if buffer might contain partial tag
                let safe_len = safe_emit_length(buffer);
                if safe_len > 0 {
                    let emit = buffer[..safe_len].to_string();
                    let _ = tx.send(StreamEvent::TextDelta(emit)).await;
                    *buffer = buffer[safe_len..].to_string();
                }
                break;
            }

            ThinkingState::InsideGemma4 => {
                if let Some(pos) = buffer.find("<channel|>") {
                    let thinking = &buffer[..pos];
                    if !thinking.is_empty() {
                        let _ = tx.send(StreamEvent::ThinkingDelta(thinking.to_string())).await;
                    }
                    *buffer = buffer[pos + "<channel|>".len()..].to_string();
                    *state = ThinkingState::Normal;
                    continue;
                }

                // Emit partial thinking, keep last 15 chars for tag detection
                let safe = buffer.len().saturating_sub(15);
                if safe > 0 {
                    let emit = buffer[..safe].to_string();
                    let _ = tx.send(StreamEvent::ThinkingDelta(emit)).await;
                    *buffer = buffer[safe..].to_string();
                }
                break;
            }

            ThinkingState::InsideLegacy => {
                if let Some(pos) = buffer.find("</think>") {
                    let thinking = &buffer[..pos];
                    if !thinking.is_empty() {
                        let _ = tx.send(StreamEvent::ThinkingDelta(thinking.to_string())).await;
                    }
                    *buffer = buffer[pos + "</think>".len()..].to_string();
                    *state = ThinkingState::Normal;
                    continue;
                }

                let safe = buffer.len().saturating_sub(10);
                if safe > 0 {
                    let emit = buffer[..safe].to_string();
                    let _ = tx.send(StreamEvent::ThinkingDelta(emit)).await;
                    *buffer = buffer[safe..].to_string();
                }
                break;
            }
        }
    }
}

// safe_emit_length, accumulate_tool_call, strip_special_tokens, and
// emit_accumulated_tools moved to stream_parser_util.rs (§10.1).

/// Re-export spiral detection from dedicated module.
pub use super::spiral_detector::detect_thought_spiral;

include!("stream_parser_tests.rs");
