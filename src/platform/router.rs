// Ern-OS — Platform router (ported from ErnOSAgent)
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Platform router — routes incoming platform messages to the Ern-OS
//! WebSocket chat API as a client, per governance §6.3.
//! Reads the hub response and delivers it back to the originating platform.
//! Posts tool execution metadata and observer audit results to thinking threads.

use crate::platform::adapter::PlatformMessage;
use crate::platform::discord_interaction;
use crate::platform::registry::PlatformRegistry;
use futures_util::FutureExt;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Start routing messages from all connected platform adapters
/// into the Ern-OS chat pipeline.
pub async fn start_platform_router(
    registry: Arc<RwLock<PlatformRegistry>>,
    hub_port: u16,
) {
    let mut msg_receivers = Vec::new();
    let mut int_receivers = Vec::new();
    {
        let mut reg = registry.write().await;
        let adapters = reg.adapters_mut();
        for adapter in adapters.iter_mut() {
            let name = adapter.name().to_string();
            if let Some(rx) = adapter.take_message_receiver() {
                msg_receivers.push((name.clone(), rx));
            }
            if let Some(rx) = adapter.take_interaction_receiver() {
                int_receivers.push((name, rx));
            }
        }
    }

    for (name, rx) in msg_receivers {
        let reg_clone = registry.clone();
        let port = hub_port;
        tokio::spawn(async move {
            let task_name = name.clone();
            let result = std::panic::AssertUnwindSafe(
                route_platform_messages(name, rx, port, reg_clone)
            )
            .catch_unwind()
            .await;

            match result {
                Ok(()) => tracing::info!(platform = %task_name, "Platform router exited normally"),
                Err(panic) => {
                    let msg = panic.downcast_ref::<String>()
                        .map(|s| s.as_str())
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    tracing::error!(
                        platform = %task_name,
                        panic = %msg,
                        "Platform router task PANICKED — adapter channel is dead, restart required"
                    );
                }
            }
        });
    }

    for (name, rx) in int_receivers {
        let reg_clone = registry.clone();
        let port = hub_port;
        tokio::spawn(async move {
            let task_name = name.clone();
            let result = std::panic::AssertUnwindSafe(
                super::router_interactions::route_interactions(name, rx, port, reg_clone)
            )
            .catch_unwind()
            .await;

            match result {
                Ok(()) => tracing::info!(platform = %task_name, "Interaction router exited normally"),
                Err(panic) => {
                    let msg = panic.downcast_ref::<String>()
                        .map(|s| s.as_str())
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    tracing::error!(
                        platform = %task_name,
                        panic = %msg,
                        "Interaction router task PANICKED — restart required"
                    );
                }
            }
        });
    }
}

/// Route messages from a single platform adapter to the WebUI hub.
async fn route_platform_messages(
    platform: String,
    mut rx: tokio::sync::mpsc::Receiver<PlatformMessage>,
    hub_port: u16,
    registry: Arc<RwLock<PlatformRegistry>>,
) {
    tracing::info!(platform = %platform, "Platform router started");

    while let Some(msg) = rx.recv().await {
        tracing::debug!(
            platform = %msg.platform,
            user = %msg.user_name,
            content_len = msg.content.len(),
            "Routing platform message to hub"
        );

        let channel_id = msg.channel_id.clone();
        let message_id = msg.message_id.clone();
        let msg_platform = msg.platform.clone();
        let msg_user = msg.user_name.clone();

        let typing_cancel = start_typing_loop(&registry, &msg_platform, &channel_id);
        let thinking_thread_id = create_thinking_thread(
            &registry, &msg_platform, &channel_id, &message_id, &msg_user,
        ).await;

        match super::router_stream::forward_to_hub_streaming(&msg, hub_port, &registry, &msg_platform, &thinking_thread_id).await {
            Ok(hub_resp) => {
                typing_cancel.cancel();
                deliver_response(
                    &registry, &msg_platform, &channel_id, &message_id,
                    &thinking_thread_id, hub_resp,
                ).await;
            }
            Err(e) => {
                typing_cancel.cancel();
                tracing::warn!(
                    platform = %msg_platform,
                    error = %e,
                    error_debug = ?e,
                    hub_port = hub_port,
                    "Failed to forward platform message to hub — check if server is running"
                );
                finalize_thread_on_error(&registry, &msg_platform, &thinking_thread_id, &e).await;
            }
        }
    }

    tracing::info!(platform = %platform, "Platform router stopped");
}

/// Spawn a continuous typing indicator loop. Returns a cancel token.
fn start_typing_loop(
    registry: &Arc<RwLock<PlatformRegistry>>,
    platform: &str,
    channel_id: &str,
) -> tokio_util::sync::CancellationToken {
    let typing_registry = registry.clone();
    let typing_platform = platform.to_string();
    let typing_channel = channel_id.to_string();
    let cancel = tokio_util::sync::CancellationToken::new();
    let token = cancel.clone();

    tokio::spawn(async move {
        loop {
            {
                let reg = typing_registry.read().await;
                let _ = reg.start_typing(&typing_platform, &typing_channel).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(8)) => {}
                _ = token.cancelled() => break,
            }
        }
    });

    cancel
}

/// Create a thinking thread on the platform. Returns the thread ID.
pub(crate) async fn create_thinking_thread(
    registry: &Arc<RwLock<PlatformRegistry>>,
    platform: &str,
    channel_id: &str,
    message_id: &str,
    user_name: &str,
) -> Option<String> {
    let reg = registry.read().await;
    let title = format!("💭 Thinking… ({})", user_name);
    match reg.create_thinking_thread(platform, channel_id, message_id, &title).await {
        Ok(id) => {
            let _ = reg.send_to_thread(platform, &id,
                "🧠 *Processing your message…*"
            ).await;
            Some(id)
        }
        Err(e) => {
            tracing::debug!(error = %e, "Thinking thread not available");
            None
        }
    }
}

/// Deliver the hub response: post tool events + audit to thinking thread, then reply with buttons.
pub(crate) async fn deliver_response(
    registry: &Arc<RwLock<PlatformRegistry>>,
    platform: &str,
    channel_id: &str,
    message_id: &str,
    thinking_thread_id: &Option<String>,
    hub_resp: HubResponse,
) {
    // Post thinking tokens to thread
    if let (Some(tid), Some(ref thinking)) = (thinking_thread_id, &hub_resp.thinking) {
        if !thinking.is_empty() {
            let reg = registry.read().await;
            let _ = reg.send_to_thread(platform, tid, thinking).await;
        }
    }

    // Post tool events to thread
    if let Some(tid) = thinking_thread_id {
        post_tool_events(registry, platform, tid, &hub_resp.tool_events).await;
        post_audit_summary(registry, platform, tid, &hub_resp.audit).await;
    }

    // Post plan content to thinking thread for review
    if let Some(tid) = thinking_thread_id {
        if let Some(ref plan_md) = hub_resp.plan_markdown {
            post_plan_to_thread(registry, platform, tid, plan_md).await;
        }
    }

    // Deliver response with interactive buttons (after sanitization)
    if hub_resp.response.is_empty() {
        tracing::error!(
            platform = %platform, channel = %channel_id,
            "Hub returned empty response — delivering error to user"
        );
        let reg = registry.read().await;
        let _ = reg.reply_with_components(
            platform, channel_id, message_id,
            "⚠️ Inference returned no content. This usually means the context window is full \
             or the model timed out. Try starting a new session.",
            &[],
        ).await;
    } else {
        let scrub = crate::web::output_sanitizer::scrub_tool_leaks(&hub_resp.response);
        let mut final_text = if scrub.text.is_empty() && scrub.had_leak {
            tracing::warn!(platform = %platform, leak = ?scrub.leak_description, "Sanitizer stripped entire response — delivering original");
            hub_resp.response.clone()
        } else {
            scrub.text
        };

        // Extract and upload images as Discord attachments
        let images = extract_local_images(&final_text);
        for img in &images {
            if let Ok(bytes) = tokio::fs::read(&img.file_path).await {
                let reg = registry.read().await;
                let _ = reg.send_image_file(
                    platform, channel_id, message_id,
                    bytes, &img.filename, &img.alt_text,
                ).await;
            } else {
                tracing::warn!(path = %img.file_path, "Failed to read image for Discord upload");
            }
        }
        // Strip image markdown from the text (Discord renders them as attachments)
        for img in &images {
            final_text = final_text.replace(&img.full_match, "");
        }
        let final_text = final_text.trim().to_string();

        // If text is empty after stripping images, skip text delivery
        if !final_text.is_empty() {
            let buttons = build_buttons_for_response(&hub_resp);
            let reg = registry.read().await;
            let result = reg.reply_with_components(
                platform, channel_id, message_id, &final_text, &buttons,
            ).await;
            if let Err(e) = result {
                tracing::error!(platform = %platform, error = %e, "Failed to deliver response");
            }
        }
    }

    // Finalize and archive thread (preserve for audit trail)
    if let Some(tid) = thinking_thread_id {
        let reg = registry.read().await;
        let _ = reg.send_to_thread(platform, tid, "✅ *Done — response delivered.*").await;
        let _ = reg.archive_thread(platform, tid).await;
    }
}

/// A local image reference extracted from markdown.
struct LocalImage {
    full_match: String,
    alt_text: String,
    filename: String,
    file_path: String,
}

/// Extract `![alt](/api/images/filename.png)` patterns from response text.
/// Returns the image metadata needed to read the file and upload it.
fn extract_local_images(text: &str) -> Vec<LocalImage> {
    let mut images = Vec::new();
    // Match ![any alt text](/api/images/filename.ext)
    let re = regex::Regex::new(r"!\[([^\]]*)\]\(/api/images/([^)]+)\)").unwrap();
    for cap in re.captures_iter(text) {
        let full_match = cap[0].to_string();
        let alt_text = cap[1].to_string();
        let filename = cap[2].to_string();
        let file_path = format!("data/images/{}", filename);
        images.push(LocalImage { full_match, alt_text, filename, file_path });
    }
    images
}

/// Build the appropriate button set for a hub response.
fn build_buttons_for_response(
    hub_resp: &HubResponse,
) -> Vec<crate::platform::adapter::MessageComponent> {
    let session_id = hub_resp.session_id.as_deref().unwrap_or("unknown");
    if hub_resp.has_plan {
        discord_interaction::build_plan_buttons(session_id)
    } else {
        // Use the session's message count as the index so the TTS button
        // targets the correct assistant turn, not always the latest.
        discord_interaction::build_response_buttons(session_id, hub_resp.message_count)
    }
}

/// Format and post tool execution events to the thinking thread.
pub(crate) async fn post_tool_events(
    registry: &Arc<RwLock<PlatformRegistry>>,
    platform: &str,
    thread_id: &str,
    tool_events: &[serde_json::Value],
) {
    super::router_thread::post_tool_events(registry, platform, thread_id, tool_events).await;
}

/// Format and post audit summary to the thinking thread.
pub(crate) async fn post_audit_summary(
    registry: &Arc<RwLock<PlatformRegistry>>,
    platform: &str,
    thread_id: &str,
    audit: &Option<serde_json::Value>,
) {
    super::router_thread::post_audit_summary(registry, platform, thread_id, audit).await;
}

/// Post plan markdown content to the thinking thread.
async fn post_plan_to_thread(
    registry: &Arc<RwLock<PlatformRegistry>>,
    platform: &str,
    thread_id: &str,
    plan_markdown: &str,
) {
    super::router_thread::post_plan_to_thread(registry, platform, thread_id, plan_markdown).await;
}

/// Post an error message to the thinking thread on hub failure.
async fn finalize_thread_on_error(
    registry: &Arc<RwLock<PlatformRegistry>>,
    platform: &str,
    thinking_thread_id: &Option<String>,
    error: &anyhow::Error,
) {
    super::router_thread::finalize_thread_on_error(registry, platform, thinking_thread_id, error).await;
}

/// Format milliseconds into a human-readable duration (test-only wrapper).
#[cfg(test)]
fn format_elapsed(ms: u64) -> String {
    super::router_thread::format_elapsed(ms)
}

/// Split a string at char boundaries near `max_len`, preferring newline splits (test-only wrapper).
#[cfg(test)]
fn split_at_boundary(text: &str, max_len: usize) -> Vec<String> {
    super::router_thread::split_at_boundary(text, max_len)
}

/// Hub response containing the reply, thinking, tool events, audit, and plan flag.
pub(crate) struct HubResponse {
    pub(crate) response: String,
    pub(crate) thinking: Option<String>,
    pub(crate) tool_events: Vec<serde_json::Value>,
    pub(crate) audit: Option<serde_json::Value>,
    pub(crate) session_id: Option<String>,
    pub(crate) has_plan: bool,
    pub(crate) plan_markdown: Option<String>,
    /// Index of the assistant message in the session for button targeting.
    /// Used by TTS/regenerate buttons to reference the correct turn.
    pub(crate) message_count: usize,
}

/// Parse a hub JSON response body into a HubResponse.
pub(crate) fn parse_hub_response(body: serde_json::Value) -> HubResponse {
    HubResponse {
        response: body["response"].as_str().unwrap_or("").to_string(),
        thinking: body["thinking"].as_str().map(|s| s.to_string()),
        tool_events: body["tool_events"].as_array().cloned().unwrap_or_default(),
        audit: body.get("audit").cloned(),
        session_id: body["session_id"].as_str().map(|s| s.to_string()),
        has_plan: body["has_plan"].as_bool().unwrap_or(false),
        plan_markdown: body["plan_markdown"].as_str().map(|s| s.to_string()),
        message_count: body["message_count"].as_u64().unwrap_or(0) as usize,
    }
}


/// Forward a platform message to the Ern-OS hub via HTTP API (blocking fallback).
pub(crate) async fn forward_to_hub(msg: &PlatformMessage, port: u16) -> anyhow::Result<HubResponse> {
    let url = format!("http://127.0.0.1:{}/api/chat/platform", port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let payload = super::router_stream::build_platform_payload(msg);

    let resp = client.post(&url).json(&payload).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Hub rejected message: {} {}", status, body);
    }

    let body: serde_json::Value = resp.json().await?;

    Ok(parse_hub_response(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forward_payload_structure() {
        let msg = PlatformMessage {
            platform: "discord".to_string(),
            channel_id: "ch1".to_string(),
            user_id: "u1".to_string(),
            user_name: "Test".to_string(),
            content: "Hello".to_string(),
            attachments: vec![],
            message_id: "m1".to_string(),
            is_admin: true,
        };
        let payload = serde_json::json!({
            "platform": msg.platform,
            "content": msg.content,
            "is_admin": msg.is_admin,
        });
        assert_eq!(payload["platform"], "discord");
        assert_eq!(payload["is_admin"], true);
    }

    #[test]
    fn test_format_elapsed_ms() {
        assert_eq!(format_elapsed(45), "45ms");
        assert_eq!(format_elapsed(999), "999ms");
    }

    #[test]
    fn test_format_elapsed_seconds() {
        assert_eq!(format_elapsed(1000), "1.0s");
        assert_eq!(format_elapsed(2500), "2.5s");
    }

    #[test]
    fn test_hub_response_parse() {
        let body = serde_json::json!({
            "response": "Hello",
            "thinking": "Let me think",
            "tool_events": [
                {"name": "web_search", "success": true, "elapsed_ms": 1234, "output_preview": "Found results"}
            ],
            "audit": {
                "verdict": "Allowed",
                "confidence": 8.5,
                "retries": 0,
                "active_topic": "Testing"
            }
        });

        let response = body["response"].as_str().unwrap_or("").to_string();
        let thinking = body["thinking"].as_str().map(|s| s.to_string());
        let tool_events = body["tool_events"].as_array().cloned().unwrap_or_default();
        let audit = body.get("audit").cloned();

        assert_eq!(response, "Hello");
        assert_eq!(thinking.unwrap(), "Let me think");
        assert_eq!(tool_events.len(), 1);
        assert_eq!(tool_events[0]["name"], "web_search");
        assert!(audit.is_some());
        assert_eq!(audit.unwrap()["verdict"], "Allowed");
    }

    #[test]
    fn test_split_at_boundary_short() {
        let chunks = split_at_boundary("short text", 100);
        assert_eq!(chunks, vec!["short text"]);
    }

    #[test]
    fn test_split_at_boundary_long() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let chunks = split_at_boundary(text, 12);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].len() <= 12);
    }

    #[test]
    fn test_hub_response_plan_markdown() {
        let body = serde_json::json!({
            "response": "Plan ready",
            "has_plan": true,
            "plan_markdown": "## Steps\n- Do thing 1\n- Do thing 2",
        });
        let resp = parse_hub_response(body);
        assert!(resp.has_plan);
        assert_eq!(resp.plan_markdown.unwrap(), "## Steps\n- Do thing 1\n- Do thing 2");
    }

    #[test]
    fn test_extract_local_images_single() {
        let text = "Here is your image: ![a landscape](/api/images/abc123.png)";
        let images = extract_local_images(text);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].alt_text, "a landscape");
        assert_eq!(images[0].filename, "abc123.png");
        assert_eq!(images[0].file_path, "data/images/abc123.png");
        assert_eq!(images[0].full_match, "![a landscape](/api/images/abc123.png)");
    }

    #[test]
    fn test_extract_local_images_multiple() {
        let text = "![img1](/api/images/a.png) and ![img2](/api/images/b.jpg)";
        let images = extract_local_images(text);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].filename, "a.png");
        assert_eq!(images[1].filename, "b.jpg");
    }

    #[test]
    fn test_extract_local_images_none() {
        let text = "No images here, just text.";
        let images = extract_local_images(text);
        assert!(images.is_empty());
    }

    #[test]
    fn test_extract_local_images_external_url() {
        let text = "![ext](https://example.com/img.png)";
        let images = extract_local_images(text);
        assert!(images.is_empty(), "Should not match external URLs");
    }

    #[test]
    fn test_extract_strips_from_text() {
        let text = "Look: ![photo](/api/images/x.png) nice!";
        let images = extract_local_images(text);
        let stripped = text.replace(&images[0].full_match, "").trim().to_string();
        assert_eq!(stripped, "Look:  nice!");
    }
}
