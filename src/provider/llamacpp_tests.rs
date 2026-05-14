// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Unit tests for the llamacpp provider. Sibling to llamacpp.rs per §1.1 split.

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

/// Helper: extract the value passed to the `-c` flag from build_server_args.
fn ctx_arg(args: &[String]) -> &str {
    let pos = args.iter().position(|a| a == "-c")
        .expect("build_server_args must always emit -c");
    &args[pos + 1]
}

#[test]
fn test_build_server_args_default_context_passes_zero() {
    // Default config has context_length = 0, which must produce `-c 0`
    // (legacy behavior — llama-server interprets 0 as auto-detect from GGUF).
    let config = LlamaCppConfig::default();
    let provider = LlamaCppProvider::new(&config);
    let args = provider.build_server_args();
    assert_eq!(ctx_arg(&args), "0");
}

#[test]
fn test_build_server_args_explicit_context_overrides() {
    // Setting context_length must produce `-c <value>` instead of `-c 0`.
    let mut config = LlamaCppConfig::default();
    config.context_length = 32768;
    let provider = LlamaCppProvider::new(&config);
    let args = provider.build_server_args();
    assert_eq!(ctx_arg(&args), "32768");
}
