// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Unit tests for `ws_l1::dispatch_leading_tool_calls`. Sibling to ws_l1.rs
//! per §1.1 split. Uses the [`DispatchSink`] / [`ToolExecutor`] trait pair
//! defined in ws_l1.rs to assert the full side-effect contract
//! (WS events, message-history appends, executor invocations) without
//! requiring a real WebSocket or AppState.

use super::*;

// ============================================================================
// Test doubles
// ============================================================================

/// Captures every WS event and every pushed message for assertion.
struct CapturingDispatchSink {
    events: Vec<(String, serde_json::Value)>,
    messages_pushed: Vec<Message>,
}

impl CapturingDispatchSink {
    fn new() -> Self {
        Self { events: Vec::new(), messages_pushed: Vec::new() }
    }
}

#[async_trait::async_trait]
impl DispatchSink for CapturingDispatchSink {
    async fn send_event(&mut self, msg_type: &str, payload: serde_json::Value) {
        self.events.push((msg_type.to_string(), payload));
    }
    fn push_message(&mut self, msg: Message) {
        self.messages_pushed.push(msg);
    }
}

/// Returns a canned successful tool result on every call. Records each
/// `ToolCall` seen for assertion.
struct MockToolExecutor {
    canned: schema::ToolResult,
    calls_seen: std::sync::Mutex<Vec<schema::ToolCall>>,
}

impl MockToolExecutor {
    fn new() -> Self {
        Self {
            canned: schema::ToolResult {
                tool_call_id: "stub".to_string(),
                name: "stub".to_string(),
                output: "stub-output".to_string(),
                success: true,
                images: Vec::new(),
            },
            calls_seen: std::sync::Mutex::new(Vec::new()),
        }
    }
    fn call_count(&self) -> usize {
        self.calls_seen.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl ToolExecutor for MockToolExecutor {
    async fn execute(&self, tc: &schema::ToolCall) -> schema::ToolResult {
        self.calls_seen.lock().unwrap().push(tc.clone());
        self.canned.clone()
    }
}

// ============================================================================
// Tests covering the §3.1 review requirements for
// `dispatch_leading_tool_calls`:
//   - Empty Vec input  → returns None, no side effects
//   - Single-element   → returns Some(elem), no leading dispatch
//   - Multi-element(3) → dispatches N-1 = 2 leading calls, returns the 3rd
// ============================================================================

#[tokio::test]
async fn test_dispatch_empty_returns_none_and_no_side_effects() {
    let mut sink = CapturingDispatchSink::new();
    let executor = MockToolExecutor::new();

    let result = dispatch_leading_tool_calls(&mut sink, &executor, Vec::new()).await;

    assert!(result.is_none(), "Empty Vec must return None");
    assert!(sink.events.is_empty(), "Empty Vec must NOT fire any WS events");
    assert!(sink.messages_pushed.is_empty(), "Empty Vec must NOT push any messages");
    assert_eq!(executor.call_count(), 0, "Empty Vec must NOT invoke the tool executor");
}

#[tokio::test]
async fn test_dispatch_single_returns_some_with_no_leading_dispatch() {
    let mut sink = CapturingDispatchSink::new();
    let executor = MockToolExecutor::new();
    let calls = vec![("id1".to_string(), "search".to_string(), "{}".to_string())];

    let result = dispatch_leading_tool_calls(&mut sink, &executor, calls).await;

    let last = result.expect("Single-element input must return Some");
    assert_eq!(last.id, "id1");
    assert_eq!(last.name, "search");
    assert_eq!(last.arguments, "{}");

    // The single element is the trailing call — no leading calls to dispatch.
    assert!(sink.events.is_empty(), "Single-element input must NOT fire any leading WS events");
    assert!(sink.messages_pushed.is_empty(), "Single-element input must NOT push any leading messages");
    assert_eq!(executor.call_count(), 0, "Single-element input must NOT invoke the tool executor");
}

#[tokio::test]
async fn test_dispatch_multi_dispatches_n_minus_1_leading_returns_last() {
    let mut sink = CapturingDispatchSink::new();
    let executor = MockToolExecutor::new();
    let calls = vec![
        ("id1".to_string(), "search".to_string(), r#"{"q":"a"}"#.to_string()),
        ("id2".to_string(), "fetch".to_string(),  r#"{"url":"b"}"#.to_string()),
        ("id3".to_string(), "parse".to_string(),  r#"{"text":"c"}"#.to_string()),
    ];

    let result = dispatch_leading_tool_calls(&mut sink, &executor, calls).await;

    // Return-value contract: the LAST call is returned to the caller.
    let last = result.expect("Multi-element input must return Some");
    assert_eq!(last.id, "id3", "Returned ToolCall must be the trailing element");
    assert_eq!(last.name, "parse");
    assert_eq!(last.arguments, r#"{"text":"c"}"#);

    // Executor contract: invoked once per LEADING call (N-1 = 2 for N=3).
    assert_eq!(executor.call_count(), 2, "N=3 input must invoke executor exactly N-1=2 times");
    let seen = executor.calls_seen.lock().unwrap();
    assert_eq!(seen[0].id, "id1", "First executor call must be the first leading element");
    assert_eq!(seen[0].name, "search");
    assert_eq!(seen[1].id, "id2", "Second executor call must be the second leading element");
    assert_eq!(seen[1].name, "fetch");

    // WS-event contract: 2 leading dispatches × (tool_executing + tool_completed) = 4 events in order.
    assert_eq!(sink.events.len(), 4, "N=3 input must fire 4 WS events (2 leading × 2 events each)");
    assert_eq!(sink.events[0].0, "tool_executing");
    assert_eq!(sink.events[0].1["id"], "id1");
    assert_eq!(sink.events[0].1["name"], "search");
    assert_eq!(sink.events[1].0, "tool_completed");
    assert_eq!(sink.events[1].1["id"], "id1");
    assert_eq!(sink.events[1].1["success"], true);
    assert_eq!(sink.events[2].0, "tool_executing");
    assert_eq!(sink.events[2].1["id"], "id2");
    assert_eq!(sink.events[2].1["name"], "fetch");
    assert_eq!(sink.events[3].0, "tool_completed");
    assert_eq!(sink.events[3].1["id"], "id2");

    // Message contract: each leading dispatch pushes 2 messages
    // (assistant_tool_call + tool_result). N-1=2 leading → 4 pushes.
    assert_eq!(sink.messages_pushed.len(), 4, "N=3 input must push 4 messages (2 leading × 2 each)");
}
