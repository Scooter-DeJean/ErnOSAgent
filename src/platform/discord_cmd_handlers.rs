//! Discord slash command execution handlers.
//!
//! Each handler: defers the response → calls the hub REST API → edits the
//! deferred response with the result. All hub communication is via HTTP REST
//! per governance §6.3.

use serenity::all::{
    CommandInteraction, CommandDataOptionValue, Context,
    CreateInteractionResponse, CreateInteractionResponseMessage,
    EditInteractionResponse,
};


/// TypeMap key for storing the hub API port in serenity's shared data.
pub struct HubPortKey;

impl serenity::prelude::TypeMapKey for HubPortKey {
    type Value = u16;
}

/// TypeMap key for storing admin user IDs.
pub struct AdminIdsKey;

impl serenity::prelude::TypeMapKey for AdminIdsKey {
    type Value = Vec<String>;
}

/// Dispatch a slash command to the appropriate handler.
pub async fn dispatch(ctx: &Context, cmd: &CommandInteraction) {
    let (hub_port, admin_ids) = {
        let data = ctx.data.read().await;
        let port = *data.get::<HubPortKey>().unwrap_or(&3000);
        let ids = data.get::<AdminIdsKey>().cloned().unwrap_or_default();
        (port, ids)
    };
    let session_id = format!(
        "discord_{}_{}",
        cmd.user.id.get(), cmd.channel_id.get(),
    );
    let user_id_str = cmd.user.id.get().to_string();
    let is_admin = admin_ids.iter().any(|a| a == &user_id_str);

    match cmd.data.name.as_str() {
        "new" => handle_new(ctx, cmd, hub_port, &session_id).await,
        "regenerate" => handle_regenerate(ctx, cmd, hub_port, &session_id).await,
        "speak" => handle_speak(ctx, cmd, hub_port, &session_id).await,
        "fork" => handle_fork(ctx, cmd, hub_port, &session_id).await,
        "status" => handle_status(ctx, cmd, hub_port).await,
        "sessions" => handle_sessions(ctx, cmd, hub_port).await,
        "export" => handle_export(ctx, cmd, hub_port, &session_id).await,
        "stop" => handle_stop(ctx, cmd, hub_port).await,
        "shutdown" => handle_shutdown(ctx, cmd, is_admin).await,
        "reboot" => handle_reboot(ctx, cmd, is_admin).await,
        other => {
            tracing::warn!(cmd = %other, "Unknown slash command");
            let _ = ack_ephemeral(ctx, cmd, "Unknown command").await;
        }
    }
}

/// /new — Start a fresh chat session.
async fn handle_new(
    ctx: &Context, cmd: &CommandInteraction, hub_port: u16, session_id: &str,
) {
    let _ = defer_response(ctx, cmd).await;

    // Delete the existing Discord session to force a fresh start
    let delete_url = format!("/api/sessions/{}", session_id);
    let _ = delete_hub_api(hub_port, &delete_url).await;

    let msg = "✨ **New session started.** Your next message will begin a fresh conversation.";
    let _ = edit_deferred(ctx, cmd, msg).await;
}

/// /regenerate — Redo the last response.
async fn handle_regenerate(
    ctx: &Context, cmd: &CommandInteraction, hub_port: u16, _session_id: &str,
) {
    let _ = defer_response(ctx, cmd).await;
    let payload = serde_json::json!({
        "platform": "discord",
        "channel_id": cmd.channel_id.get().to_string(),
        "user_id": cmd.user.id.get().to_string(),
        "user_name": cmd.user.name,
        "content": "/regenerate",
        "attachments": [],
        "message_id": cmd.id.get().to_string(),
        "is_admin": true,
    });

    match post_hub_api(hub_port, "/api/chat/platform", &payload).await {
        Ok(body) => {
            let reply = body["response"].as_str().unwrap_or("No response");
            let _ = edit_deferred(ctx, cmd, reply).await;
        }
        Err(e) => {
            let _ = edit_deferred(ctx, cmd, &format!("❌ Regeneration failed: {e}")).await;
        }
    }
}

/// /speak — Read the last response aloud.
async fn handle_speak(
    ctx: &Context, cmd: &CommandInteraction, hub_port: u16, session_id: &str,
) {
    let _ = defer_response(ctx, cmd).await;
    let voice = get_string_option(cmd, "voice").unwrap_or("am_michael".to_string());

    // Fetch last assistant message from the session
    let text = fetch_last_assistant_message(hub_port, session_id).await;
    let Some(text) = text else {
        let _ = edit_deferred(ctx, cmd, "❌ No previous response to speak").await;
        return;
    };

    match call_tts(hub_port, &text, &voice).await {
        Ok(wav_bytes) => {
            let attachment = serenity::builder::CreateAttachment::bytes(wav_bytes, "response.wav");
            let edit = EditInteractionResponse::new()
                .content("🔊 *Voice response:*")
                .new_attachment(attachment);
            let _ = cmd.edit_response(&ctx.http, edit).await;
        }
        Err(e) => {
            let _ = edit_deferred(ctx, cmd, &format!("❌ TTS failed: {e}")).await;
        }
    }
}

/// /fork — Branch the conversation.
async fn handle_fork(
    ctx: &Context, cmd: &CommandInteraction, hub_port: u16, session_id: &str,
) {
    let _ = defer_response(ctx, cmd).await;
    let msg_idx = get_int_option(cmd, "message_number").unwrap_or(-1);
    let idx_str = if msg_idx >= 0 { msg_idx.to_string() } else { "last".to_string() };

    let url = if msg_idx >= 0 {
        format!("/api/sessions/{}/fork/{}", session_id, msg_idx)
    } else {
        format!("/api/sessions/{}/fork/last", session_id)
    };

    match post_hub_api(hub_port, &url, &serde_json::json!({})).await {
        Ok(body) => {
            let new_id = body["session_id"].as_str().unwrap_or("unknown");
            let msg = format!("🔀 Forked from message {} → new session `{}`", idx_str, new_id);
            let _ = edit_deferred(ctx, cmd, &msg).await;
        }
        Err(e) => {
            let _ = edit_deferred(ctx, cmd, &format!("❌ Fork failed: {e}")).await;
        }
    }
}

/// /status — Show connection and service status.
async fn handle_status(ctx: &Context, cmd: &CommandInteraction, hub_port: u16) {
    let _ = defer_response(ctx, cmd).await;
    let mut lines = vec!["**Ern-OS Status**".to_string()];

    lines.push("🟢 Discord — Connected".to_string());

    // Check TTS
    match get_hub_api(hub_port, "/api/tts/status").await {
        Ok(body) => {
            let available = body["available"].as_bool().unwrap_or(false);
            let icon = if available { "🟢" } else { "🔴" };
            let port = body["port"].as_u64().unwrap_or(8880);
            lines.push(format!("{} Kokoro TTS — {} (port {})",
                icon, if available { "Available" } else { "Unavailable" }, port));
        }
        Err(_) => lines.push("🔴 Kokoro TTS — Unreachable".to_string()),
    }

    // Check sessions count
    if let Ok(body) = get_hub_api(hub_port, "/api/sessions").await {
        let count = body.as_array().map(|a| a.len()).unwrap_or(0);
        lines.push(format!("📊 Active sessions: {}", count));
    }

    let _ = edit_deferred(ctx, cmd, &lines.join("\n")).await;
}

/// /sessions — List recent sessions.
async fn handle_sessions(ctx: &Context, cmd: &CommandInteraction, hub_port: u16) {
    let _ = defer_response(ctx, cmd).await;
    let count = get_int_option(cmd, "count").unwrap_or(5) as usize;

    match get_hub_api(hub_port, "/api/sessions").await {
        Ok(body) => {
            let sessions = body.as_array().cloned().unwrap_or_default();
            let mut lines = vec!["**Recent Sessions**".to_string()];
            for (i, s) in sessions.iter().take(count).enumerate() {
                let title = s["title"].as_str().unwrap_or("Untitled");
                let id = s["id"].as_str().unwrap_or("?");
                let msgs = s["message_count"].as_u64().unwrap_or(0);
                lines.push(format!("{}. **{}** — `{}` ({} msgs)", i + 1, title, id, msgs));
            }
            if sessions.is_empty() {
                lines.push("No sessions found.".to_string());
            }
            let _ = edit_deferred(ctx, cmd, &lines.join("\n")).await;
        }
        Err(e) => {
            let _ = edit_deferred(ctx, cmd, &format!("❌ Failed to list sessions: {e}")).await;
        }
    }
}

/// /export — Export current session as markdown file.
async fn handle_export(
    ctx: &Context, cmd: &CommandInteraction, hub_port: u16, session_id: &str,
) {
    let _ = defer_response(ctx, cmd).await;
    let url = format!("/api/sessions/{}/export", session_id);

    match get_hub_api(hub_port, &url).await {
        Ok(body) => {
            let content = body["markdown"].as_str()
                .or_else(|| body.as_str())
                .unwrap_or("No export data");
            let attachment = serenity::builder::CreateAttachment::bytes(
                content.as_bytes().to_vec(), "session_export.md",
            );
            let edit = EditInteractionResponse::new()
                .content("📄 *Session export:*")
                .new_attachment(attachment);
            let _ = cmd.edit_response(&ctx.http, edit).await;
        }
        Err(e) => {
            let _ = edit_deferred(ctx, cmd, &format!("❌ Export failed: {e}")).await;
        }
    }
}

/// /stop — Emergency halt of active inference.
async fn handle_stop(ctx: &Context, cmd: &CommandInteraction, hub_port: u16) {
    let _ = defer_response(ctx, cmd).await;

    // Signal the hub to cancel any active inference
    match post_hub_api(hub_port, "/api/inference/stop", &serde_json::json!({})).await {
        Ok(_) => {
            tracing::warn!(
                user = %cmd.user.name,
                "Emergency stop triggered via /stop"
            );
            let _ = edit_deferred(ctx, cmd, "🛑 **Emergency stop** — inference halted.").await;
        }
        Err(_) => {
            // Even if the API doesn't exist yet, acknowledge the intent
            tracing::warn!(user = %cmd.user.name, "Stop requested (no active inference)");
            let _ = edit_deferred(ctx, cmd, "🛑 **Stop acknowledged** — no active inference to halt.").await;
        }
    }
}

/// /shutdown — Gracefully exit the Ern-OS process (admin only).
async fn handle_shutdown(ctx: &Context, cmd: &CommandInteraction, is_admin: bool) {
    if !is_admin {
        let _ = ack_ephemeral(ctx, cmd, "⛔ Shutdown is restricted to admins.").await;
        return;
    }
    let _ = ack_ephemeral(ctx, cmd, "🔌 **Shutting down Ern-OS…** Goodbye.").await;
    tracing::warn!(user = %cmd.user.name, "Shutdown triggered via /shutdown");

    // Give Discord a moment to deliver the acknowledgement
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    std::process::exit(0);
}

/// /reboot — Restart the Ern-OS process by re-exec'ing the current binary.
/// Admin only. The process replaces itself with a fresh instance.
async fn handle_reboot(ctx: &Context, cmd: &CommandInteraction, is_admin: bool) {
    if !is_admin {
        let _ = ack_ephemeral(ctx, cmd, "⛔ Reboot is restricted to admins.").await;
        return;
    }
    let _ = ack_ephemeral(ctx, cmd, "⚡ **Rebooting Ern-OS…** Back shortly.").await;
    tracing::warn!(user = %cmd.user.name, "Reboot triggered via /reboot");

    // Give Discord a moment to deliver the acknowledgement
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    reboot_process();
}

/// Re-exec the current binary, replacing this process with a fresh instance.
/// On Unix, uses `exec()` for atomic process replacement.
/// On non-Unix, falls back to spawning a new process and exiting.
fn reboot_process() -> ! {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            tracing::error!(error = %e, "Failed to determine executable path for reboot");
            std::process::exit(1);
        }
    };
    let args: Vec<String> = std::env::args().collect();

    tracing::info!(exe = %exe.display(), args = ?args, "Re-exec'ing process");

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&exe)
            .args(&args[1..])
            .exec();
        // exec() only returns on error
        tracing::error!(error = %err, "exec() failed — falling back to exit");
        std::process::exit(1);
    }

    #[cfg(not(unix))]
    {
        // Non-Unix: spawn new process then exit current
        match std::process::Command::new(&exe).args(&args[1..]).spawn() {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                tracing::error!(error = %e, "Failed to spawn new process for reboot");
                std::process::exit(1);
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────

/// Defer the slash command response (shows "thinking…" to the user).
async fn defer_response(ctx: &Context, cmd: &CommandInteraction) -> Result<(), serenity::Error> {
    cmd.create_response(
        &ctx.http,
        CreateInteractionResponse::Defer(
            CreateInteractionResponseMessage::new(),
        ),
    ).await
}

/// Send an ephemeral acknowledgement message.
async fn ack_ephemeral(ctx: &Context, cmd: &CommandInteraction, msg: &str) -> Result<(), serenity::Error> {
    cmd.create_response(
        &ctx.http,
        CreateInteractionResponse::Message(
            CreateInteractionResponseMessage::new().content(msg).ephemeral(true),
        ),
    ).await
}

/// Edit the deferred response with final content.
async fn edit_deferred(ctx: &Context, cmd: &CommandInteraction, content: &str) -> Result<(), serenity::Error> {
    let edit = EditInteractionResponse::new().content(content);
    cmd.edit_response(&ctx.http, edit).await.map(|_| ())
}

/// Extract a string option from the command.
fn get_string_option(cmd: &CommandInteraction, name: &str) -> Option<String> {
    cmd.data.options.iter()
        .find(|o| o.name == name)
        .and_then(|o| match &o.value {
            CommandDataOptionValue::String(s) => Some(s.clone()),
            _ => None,
        })
}

/// Extract an integer option from the command.
fn get_int_option(cmd: &CommandInteraction, name: &str) -> Option<i64> {
    cmd.data.options.iter()
        .find(|o| o.name == name)
        .and_then(|o| match o.value {
            CommandDataOptionValue::Integer(i) => Some(i),
            _ => None,
        })
}

/// POST to a hub REST API endpoint.
async fn post_hub_api(
    port: u16, path: &str, payload: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let resp = client.post(&url).json(payload).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Hub returned {}", resp.status());
    }
    resp.json().await.map_err(Into::into)
}

/// GET from a hub REST API endpoint.
async fn get_hub_api(port: u16, path: &str) -> anyhow::Result<serde_json::Value> {
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Hub returned {}", resp.status());
    }
    resp.json().await.map_err(Into::into)
}

/// DELETE a hub REST API endpoint.
async fn delete_hub_api(port: u16, path: &str) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{}{}", port, path);
    let client = reqwest::Client::new();
    let resp = client.delete(&url).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("Hub returned {}", resp.status());
    }
    Ok(())
}

/// Call the TTS API and return WAV bytes.
async fn call_tts(port: u16, text: &str, voice: &str) -> anyhow::Result<Vec<u8>> {
    let url = format!("http://127.0.0.1:{}/api/tts", port);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let payload = serde_json::json!({
        "text": text,
        "voice": voice,
        "speed": 1.0,
    });
    let resp = client.post(&url).json(&payload).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("TTS API returned {}", resp.status());
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Fetch the last assistant message text from a session.
async fn fetch_last_assistant_message(port: u16, session_id: &str) -> Option<String> {
    let url = format!("http://127.0.0.1:{}/api/sessions/{}", port, session_id);
    let client = reqwest::Client::new();
    let resp = client.get(&url).send().await.ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    let messages = body["messages"].as_array()?;
    messages.iter().rev()
        .find(|m| m["role"].as_str() == Some("assistant"))
        .and_then(|m| m["content"].as_str().map(|s| s.to_string()))
}
