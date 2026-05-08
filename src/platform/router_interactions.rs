//! Router interaction handlers — processes button clicks from Discord.
//!
//! Handles feedback (👍👎), regeneration (🔄), TTS (🔊), and plan actions (✅✏️❌)
//! by calling the hub's REST API.

use crate::platform::adapter::{PlatformInteraction, PlatformMessage};
use crate::platform::discord_interaction;
use crate::platform::registry::PlatformRegistry;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Route interaction events (button clicks) from a platform adapter.
pub async fn route_interactions(
    platform: String,
    mut rx: tokio::sync::mpsc::Receiver<PlatformInteraction>,
    hub_port: u16,
    registry: Arc<RwLock<PlatformRegistry>>,
) {
    tracing::info!(platform = %platform, "Interaction router started");

    while let Some(interaction) = rx.recv().await {
        tracing::info!(
            action = %interaction.action,
            session = %interaction.session_id,
            "Processing interaction"
        );
        handle_interaction(&registry, &interaction, hub_port).await;
    }

    tracing::info!(platform = %platform, "Interaction router stopped");
}

/// Dispatch a single interaction to the appropriate handler.
async fn handle_interaction(
    registry: &Arc<RwLock<PlatformRegistry>>,
    interaction: &PlatformInteraction,
    hub_port: u16,
) {
    match interaction.action.as_str() {
        "thumbsup" => handle_feedback(hub_port, interaction, "up").await,
        "thumbsdown" => handle_feedback(hub_port, interaction, "down").await,
        "regenerate" => {
            handle_regenerate(registry, interaction, hub_port).await;
        }
        "speak" => {
            handle_speak(registry, interaction, hub_port).await;
        }
        "plan_approve" | "plan_revise" | "plan_cancel" => {
            handle_plan_action(registry, hub_port, interaction).await;
        }
        other => {
            tracing::warn!(action = %other, "Unknown interaction action");
        }
    }
}

/// Handle thumbs up/down feedback by calling the session react API.
async fn handle_feedback(hub_port: u16, interaction: &PlatformInteraction, reaction: &str) {
    if let Err(e) = discord_interaction::call_feedback_api(
        hub_port, &interaction.session_id, interaction.message_index, reaction,
    ).await {
        tracing::warn!(error = %e, "Feedback API call failed");
    }
}

/// Handle Redo: create new thinking thread, re-infer, deliver with buttons.
async fn handle_regenerate(
    registry: &Arc<RwLock<PlatformRegistry>>,
    interaction: &PlatformInteraction,
    hub_port: u16,
) {
    let result = discord_interaction::call_regenerate_api(
        hub_port, interaction, "/regenerate",
    ).await;

    match result {
        Ok(body) => {
            let hub_resp = super::router::parse_hub_response(body);

            // Create new thinking thread for the regenerated response
            let thread = super::router::create_thinking_thread(
                registry, &interaction.platform, &interaction.channel_id,
                &interaction.interaction_id, "User",
            ).await;

            // Post tool/audit to thread if available
            if let Some(ref tid) = thread {
                super::router::post_tool_events(
                    registry, &interaction.platform, tid, &hub_resp.tool_events,
                ).await;
                super::router::post_audit_summary(
                    registry, &interaction.platform, tid, &hub_resp.audit,
                ).await;
            }

            // Deliver the new response with buttons
            super::router::deliver_response(
                registry, &interaction.platform, &interaction.channel_id,
                &interaction.interaction_id, &thread, hub_resp,
            ).await;
        }
        Err(e) => {
            tracing::error!(error = %e, "Regeneration failed");
        }
    }
}

/// Handle plan approve/revise/cancel via the hub API.
/// Uses the SSE streaming path for live thinking updates and full response delivery.
async fn handle_plan_action(
    registry: &Arc<RwLock<PlatformRegistry>>,
    hub_port: u16,
    interaction: &PlatformInteraction,
) {
    let action = interaction.action.replace("plan_", "");
    tracing::info!(
        action = %action,
        session = %interaction.session_id,
        "Plan action: building synthetic message for streaming execution"
    );

    // Build a synthetic PlatformMessage from the interaction
    let msg = PlatformMessage {
        platform: interaction.platform.clone(),
        channel_id: interaction.channel_id.clone(),
        user_id: interaction.user_id.clone(),
        user_name: "User".into(),
        content: format!("/plan_{}", action),
        attachments: vec![],
        message_id: interaction.interaction_id.clone(),
        is_admin: true,
    };

    // Create thinking thread for plan execution
    let thread = super::router::create_thinking_thread(
        registry, &interaction.platform, &interaction.channel_id,
        &interaction.interaction_id, "User",
    ).await;

    // Use the streaming path — live thinking updates during execution
    match super::router_stream::forward_to_hub_streaming(
        &msg, hub_port, registry, &interaction.platform, &thread,
    ).await {
        Ok(hub_resp) => {
            super::router::deliver_response(
                registry, &interaction.platform, &interaction.channel_id,
                &interaction.interaction_id, &thread, hub_resp,
            ).await;
        }
        Err(e) => {
            tracing::error!(error = %e, action = %action, "Plan execution failed");
            if let Some(ref tid) = thread {
                let reg = registry.read().await;
                let _ = reg.send_to_thread(
                    &interaction.platform, tid,
                    &format!("❌ Plan execution failed: {}", e),
                ).await;
                let _ = reg.archive_thread(&interaction.platform, tid).await;
            }
        }
    }
}

/// Handle Speak: call TTS API, send WAV as a Discord file attachment.
async fn handle_speak(
    registry: &Arc<RwLock<PlatformRegistry>>,
    interaction: &PlatformInteraction,
    hub_port: u16,
) {
    // Get the response text from the session to synthesize
    let text = fetch_message_text(hub_port, interaction).await;
    let Some(text) = text else {
        tracing::warn!("No message text found for TTS");
        return;
    };

    match discord_interaction::call_tts_api(hub_port, &text).await {
        Ok(wav_bytes) => {
            send_audio_attachment(registry, interaction, wav_bytes).await;
        }
        Err(e) => {
            tracing::warn!(error = %e, "TTS generation failed");
        }
    }
}

/// Fetch a message's text from the session for TTS synthesis.
/// When `message_index > 0`, looks up the specific message at that position.
/// Falls back to the last assistant message when index is 0 or out of range.
async fn fetch_message_text(
    hub_port: u16,
    interaction: &PlatformInteraction,
) -> Option<String> {
    let url = format!(
        "http://127.0.0.1:{}/api/sessions/{}",
        hub_port, interaction.session_id,
    );
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    let messages = body["messages"].as_array()?;

    // Try the specific index first (from the button's custom_id)
    if interaction.message_index > 0 && interaction.message_index <= messages.len() {
        // message_index is 1-based (count at time of creation); convert to 0-based
        let idx = interaction.message_index - 1;
        if let Some(msg) = messages.get(idx) {
            if msg["role"].as_str() == Some("assistant") {
                return msg["content"].as_str().map(|s| s.to_string());
            }
        }
    }

    // Fallback: last assistant message
    messages.iter().rev()
        .find(|m| m["role"].as_str() == Some("assistant"))
        .and_then(|m| m["content"].as_str().map(|s| s.to_string()))
}

/// Send WAV audio bytes as a Discord file attachment.
async fn send_audio_attachment(
    registry: &Arc<RwLock<PlatformRegistry>>,
    interaction: &PlatformInteraction,
    wav_bytes: Vec<u8>,
) {
    let reg = registry.read().await;
    // Use send_message with a note; the actual file sending requires the adapter
    if let Err(e) = reg.send_audio_file(
        &interaction.platform, &interaction.channel_id, wav_bytes, "response.wav",
    ).await {
        tracing::warn!(error = %e, "Failed to send TTS audio file");
    }
}
