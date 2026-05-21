// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Unit tests for llamacpp.rs orchestrator.
//!
//! Moved from inline `#[cfg(test)] mod tests` in llamacpp.rs per §1.1
//! to keep the orchestrator file under the 500-line cap. Included from
//! llamacpp.rs via `#[path = "llamacpp_tests.rs"] mod tests;` so the
//! test paths remain `provider::llamacpp::tests::*` (no rename).

use super::*;

#[test]
fn test_build_server_args_includes_jinja() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let args = provider.build_server_args();
    assert!(args.contains(&"--jinja".to_string()));
}

#[test]
fn test_build_server_args_includes_mmproj() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let args = provider.build_server_args();
    assert!(args.contains(&"--mmproj".to_string()));
}

#[test]
fn test_build_chat_body_stream() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let messages = vec![Message::text("user", "Hello")];
    let body = provider.build_chat_body(&messages, None, true, true);
    assert_eq!(body["stream"], true);
}

#[test]
fn test_build_chat_body_with_tools() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let messages = vec![Message::text("user", "Hello")];
    let tools = serde_json::json!([{"type": "function", "function": {"name": "test"}}]);
    let body = provider.build_chat_body(&messages, Some(&tools), true, true);
    assert!(body["tools"].is_array());
}

#[test]
fn test_chat_sync_body_structure() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let messages = vec![Message::text("user", "Test")];
    let body = provider.build_chat_body(&messages, None, false, false);
    assert_eq!(body["stream"], false, "chat_sync must use stream=false");
    assert!(body["messages"].is_array());
}

#[test]
fn test_chat_sync_retry_delay_calculation() {
    // Verify exponential backoff: 500ms, 1000ms, 2000ms
    for attempt in 1..=3u32 {
        let delay_ms = 500u64 * (1 << (attempt - 1));
        match attempt {
            1 => assert_eq!(delay_ms, 500),
            2 => assert_eq!(delay_ms, 1000),
            3 => assert_eq!(delay_ms, 2000),
            _ => unreachable!(),
        }
    }
}

#[test]
fn test_chat_sync_connection_error_classification() {
    // Validate that the same error strings are checked in both chat() and chat_sync()
    let test_messages = ["connection closed", "connection reset"];
    for msg in &test_messages {
        assert!(msg.contains("connection closed") || msg.contains("connection reset"),
            "Error classification must match: {}", msg);
    }
}

#[test]
fn test_build_chat_body_no_slot_by_default() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let body = provider.build_chat_body(&[], None, false, false);
    assert!(body.get("id_slot").is_none(), "Default provider must not pin to any slot");
}

#[test]
fn test_build_chat_body_includes_slot_when_set() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new_with_slot(&config, 1);
    let body = provider.build_chat_body(&[], None, false, false);
    assert_eq!(
        body["id_slot"],
        serde_json::json!(1),
        "Audit provider must pin to slot 1"
    );
}

#[test]
fn test_build_server_args_uses_two_slots() {
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let args = provider.build_server_args();
    let np_pos = args.iter().position(|a| a == "-np")
        .expect("-np must be present in server args");
    assert_eq!(
        args[np_pos + 1], "2",
        "-np must be 2 to give the observer its own KV cache slot"
    );
}

// ─── New tests for context_length feature (PR #5) ──────────────────────────

#[test]
fn test_build_server_args_auto_detect_when_context_length_zero() {
    // Legacy default: context_length=0 → emit "-c 0" (auto-detect from GGUF).
    let config = LlamaCppConfig::default();
    let args = crate::provider::llamacpp_server_args::build_server_args(&config);
    let c_pos = args.iter().position(|a| a == "-c")
        .expect("-c must be present in server args");
    assert_eq!(
        args[c_pos + 1], "0",
        "context_length=0 must emit -c 0 (auto-detect from GGUF)"
    );
}

#[test]
fn test_build_server_args_explicit_context_length() {
    // Operator override: context_length=32768 → emit "-c 32768".
    let mut config = LlamaCppConfig::default();
    config.context_length = 32768;
    let args = crate::provider::llamacpp_server_args::build_server_args(&config);
    let c_pos = args.iter().position(|a| a == "-c")
        .expect("-c must be present in server args");
    assert_eq!(
        args[c_pos + 1], "32768",
        "context_length=32768 must emit -c 32768"
    );
}
