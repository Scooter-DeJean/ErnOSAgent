//! Stream parser utilities — stall detection, tool accumulation, and token stripping.
//! Split from `stream_parser.rs` per §10.1 (max 500 lines per file, excluding tests).

use crate::provider::StreamEvent;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Accumulated tool call from streaming deltas.
#[derive(Debug, Default, Clone)]
pub(super) struct ToolCallAccumulator {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) arguments: String,
}

/// Server stall detection info.
pub(super) struct StallInfo {
    pub(super) n_decoded: u64,
    pub(super) n_predicted: u64,
}

/// Check if llama-server is generating tokens that aren't reaching the HTTP stream.
///
/// Queries the server's `/slots` endpoint (model-derived data per §2.1) to read
/// generation progress. A stall is detected when:
/// - The server is actively processing on the target slot (`is_processing == true`)
/// - The server has **generated** (n_predicted) tokens that the client hasn't received
/// - No HTTP chunks have arrived in the recent interval
///
/// CRITICAL: Uses `n_predicted` (generated content tokens), NOT `n_decoded` (which
/// includes prompt prefill tokens). The old `n_decoded` heuristic caused false
/// positives during large prompt prefill (~49K tokens at ~5K tok/s = ~10s), which
/// then poisoned the KV cache and broke all subsequent inferences on the slot.
pub(super) async fn check_server_stall(
    slots_url: &str,
    client_chunks: u64,
    last_chunk_time: &Instant,
    target_slot_id: i32,
) -> Option<StallInfo> {
    // Minimum 30s before declaring a stall — large prompts (49K+ tokens) take
    // 10-20s to prefill. No content tokens arrive during prefill, which is normal.
    if last_chunk_time.elapsed() < Duration::from_secs(30) {
        return None;
    }

    let resp = reqwest::get(slots_url).await.ok()?;
    let slots: Vec<serde_json::Value> = resp.json().await.ok()?;

    // Find the specific slot we're streaming from — never assume slot ordering.
    let slot = slots.iter().find(|s| {
        s["id"].as_i64().unwrap_or(-1) == target_slot_id as i64
    })?;

    if !slot["is_processing"].as_bool().unwrap_or(false) {
        return None; // Server isn't processing on this slot — not a stall
    }

    let n_decoded = slot["n_decoded"].as_u64()
        .or_else(|| slot["next_token"].get(0).and_then(|t| t["n_decoded"].as_u64()))
        .unwrap_or(0);

    // n_predicted = tokens the server has GENERATED (content/thinking output).
    // This excludes prompt prefill tokens, so it only counts real output.
    let n_predicted = slot["n_predicted"].as_u64().unwrap_or(0);

    // A real stall: the server has generated >50 content tokens but the client
    // received almost none. During normal prefill, n_predicted stays at 0.
    if n_predicted > 50 && client_chunks < 10 {
        tracing::debug!(
            target_slot_id, n_decoded, n_predicted, client_chunks,
            "Stall check: generation stall confirmed (n_predicted > threshold)"
        );
        return Some(StallInfo { n_decoded, n_predicted });
    }

    // Fallback: if n_predicted is not available (older llama-server builds),
    // use n_decoded but with a much higher threshold that can't fire during
    // normal prefill. At ~5K tok/s prefill and 30s minimum elapsed, the server
    // has processed ~150K prompt tokens. We only flag if n_decoded exceeds what
    // could be prompt prefill (context_length is typically 262K, so use 200K).
    if n_predicted == 0 && n_decoded > 200_000 && client_chunks < 10 {
        tracing::warn!(
            target_slot_id, n_decoded, client_chunks,
            "Stall check: fallback n_decoded heuristic triggered (n_predicted unavailable)"
        );
        return Some(StallInfo { n_decoded, n_predicted: 0 });
    }

    None
}

/// Erase a slot's KV cache after a stall abort to prevent cache poisoning.
///
/// When a streaming inference is interrupted mid-generation, the slot retains
/// partial KV cache entries. If the next request's prompt prefix matches these
/// stale entries, the server loads corrupted hidden states and the model
/// immediately stops with zero output (finish_reason="stop", no content).
///
/// Calling this after a stall ensures the next inference starts with a clean cache.
pub async fn erase_slot_cache(base_url: &str, slot_id: i32) {
    let url = format!("{}/slots/{}?action=erase", base_url, slot_id);
    match reqwest::Client::new()
        .post(&url)
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            tracing::info!(
                slot_id, status = %r.status(),
                "Erased slot KV cache after stall — preventing cache poisoning"
            );
        }
        Ok(r) => {
            tracing::warn!(
                slot_id, status = %r.status(),
                "Slot cache erase returned non-success — cache may be poisoned"
            );
        }
        Err(e) => {
            tracing::warn!(
                slot_id, error = %e,
                "Failed to erase slot cache after stall — cache may be poisoned"
            );
        }
    }
}

/// Calculate how many bytes can be safely emitted without splitting a potential tag.
pub(super) fn safe_emit_length(buffer: &str) -> usize {
    // Keep enough chars to detect partial `<|channel>thought` (17 chars) or `<think>` (7 chars)
    let reserve = 20;
    let raw = buffer.len().saturating_sub(reserve);
    // Snap to a valid char boundary — never slice mid-codepoint.
    let mut pos = raw;
    while pos > 0 && !buffer.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// OpenAI SSE tool call delta structure.
#[derive(Debug, serde::Deserialize)]
pub(super) struct SseToolCallDelta {
    pub(super) index: Option<usize>,
    pub(super) id: Option<String>,
    pub(super) function: Option<SseFunctionDelta>,
}

/// OpenAI SSE function delta structure.
#[derive(Debug, serde::Deserialize)]
pub(super) struct SseFunctionDelta {
    pub(super) name: Option<String>,
    pub(super) arguments: Option<String>,
}

/// Accumulate tool call deltas by index.
pub(super) fn accumulate_tool_call(
    tool_calls: &mut Vec<ToolCallAccumulator>,
    delta: &SseToolCallDelta,
) {
    let idx = delta.index.unwrap_or(0);

    // Extend the vector if needed
    while tool_calls.len() <= idx {
        tool_calls.push(ToolCallAccumulator::default());
    }

    let tc = &mut tool_calls[idx];

    if let Some(id) = &delta.id {
        tc.id.clone_from(id);
    }
    if let Some(func) = &delta.function {
        if let Some(name) = &func.name {
            tc.name.clone_from(name);
        }
        if let Some(args) = &func.arguments {
            let clean = strip_special_tokens(args);
            if !clean.is_empty() {
                tc.arguments.push_str(&clean);
            }
        }
    }
}

/// Strip model-specific control tokens that should never appear in tool call arguments.
/// These are generation artifacts from Gemma 4's channel/tool_call system, not valid JSON.
pub(super) fn strip_special_tokens(text: &str) -> String {
    text.replace("<tool_call|>", "")
        .replace("<|tool_call>", "")
        .replace("<|channel>thought", "")
        .replace("<|channel>", "")
        .replace("<channel|>", "")
}

/// Emit all accumulated tool calls as StreamEvents.
/// Validates that accumulated arguments are parseable JSON — skips corrupt calls.
pub(super) async fn emit_accumulated_tools(
    tool_calls: &[ToolCallAccumulator],
    tx: &mpsc::Sender<StreamEvent>,
) {
    for tc in tool_calls {
        if !tc.name.is_empty() {
            let args = tc.arguments.trim();
            if !args.is_empty() && serde_json::from_str::<serde_json::Value>(args).is_err() {
                tracing::error!(
                    tool = %tc.name,
                    args_len = args.len(),
                    args_preview = %args.chars().take(200).collect::<String>(),
                    "Corrupt tool call arguments — skipping (model emitted control tokens in args)"
                );
                continue;
            }
            let _ = tx
                .send(StreamEvent::ToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                })
                .await;
        }
    }
}
