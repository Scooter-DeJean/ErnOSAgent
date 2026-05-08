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
}

/// Check if llama-server is generating tokens that aren't reaching the HTTP stream.
///
/// Queries the server's `/slots` endpoint (model-derived data per §2.1) to read
/// `n_decoded` — the server's real decoded token count. A stall is detected when:
/// - The server is actively processing (`is_processing == true`)
/// - The server has decoded significantly more tokens than the client received
/// - No HTTP chunks have arrived in the recent interval
///
/// HEURISTIC: The `n_decoded > 500 && client_chunks < 10` thresholds are derived
/// from observed minimum generation speed (~13 tok/s on M3 Ultra). At 10-second
/// intervals, the server generates ≥130 tokens. 500 provides ~4x margin against
/// prompt-processing spikes. The `<10 chunks` guard prevents false-positives
/// during normal streaming. Error margin: could false-positive during initial
/// KV cache fill on very large prompts (>200K tokens), mitigated by the
/// `is_processing` check and the `last_chunk_time` age requirement.
pub(super) async fn check_server_stall(
    slots_url: &str,
    client_chunks: u64,
    last_chunk_time: &Instant,
) -> Option<StallInfo> {
    // Only check if we haven't received data for at least 8 seconds
    if last_chunk_time.elapsed() < Duration::from_secs(8) {
        return None;
    }

    let resp = reqwest::get(slots_url).await.ok()?;
    let slots: Vec<serde_json::Value> = resp.json().await.ok()?;
    let slot = slots.first()?;

    if !slot["is_processing"].as_bool().unwrap_or(false) {
        return None; // Server isn't processing — not a stall
    }

    let n_decoded = slot["next_token"][0]["n_decoded"].as_u64().unwrap_or(0);

    // HEURISTIC: see doc comment above for derivation and error margin.
    if n_decoded > 500 && client_chunks < 10 {
        return Some(StallInfo { n_decoded });
    }

    None
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
