// Ern-OS — Discord platform adapter
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Concrete `PlatformAdapter` implementation for Discord via serenity.
//! Connects to the Discord gateway, ingests messages, and delivers responses.
//! Per governance §6.3: this is a standalone client that speaks the WebUI API.

use crate::config::DiscordConfig;
use crate::platform::adapter::{
    PlatformAdapter, PlatformMessage, PlatformStatus, PlatformInteraction, MessageComponent,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serenity::all::{GatewayIntents, Http};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

/// Discord adapter — bridges Discord gateway events to the Ern-OS hub.
pub struct DiscordAdapter {
    config: DiscordConfig,
    http: Option<Arc<Http>>,
    tx: mpsc::Sender<PlatformMessage>,
    rx: Option<mpsc::Receiver<PlatformMessage>>,
    interaction_tx: mpsc::Sender<PlatformInteraction>,
    interaction_rx: Option<mpsc::Receiver<PlatformInteraction>>,
    connected: Arc<AtomicBool>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    hub_port: u16,
}

impl DiscordAdapter {
    /// Create a new Discord adapter from config.
    pub fn new(config: DiscordConfig, hub_port: u16) -> Self {
        let (tx, rx) = mpsc::channel(64);
        let (itx, irx) = mpsc::channel(64);
        Self {
            config,
            http: None,
            tx,
            rx: Some(rx),
            interaction_tx: itx,
            interaction_rx: Some(irx),
            connected: Arc::new(AtomicBool::new(false)),
            shutdown: None,
            hub_port,
        }
    }
}

#[async_trait]
impl PlatformAdapter for DiscordAdapter {
    fn name(&self) -> &str {
        "Discord"
    }

    fn is_configured(&self) -> bool {
        self.config.is_configured()
    }

    async fn connect(&mut self) -> Result<()> {
        let token = self.config.resolve_token()
            .context("Discord token not configured — set in ern-os.toml or DISCORD_TOKEN env")?;

        let handler = super::discord_handler::Handler::new(
            self.tx.clone(),
            self.interaction_tx.clone(),
            self.config.clone(),
        );

        let http = Arc::new(Http::new(&token));
        self.http = Some(http.clone());

        let connected = self.connected.clone();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown = Some(shutdown_tx);

        spawn_discord_client(token, handler, self.hub_port, self.config.admin_ids.clone(), connected, shutdown_rx);

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.connected.store(false, Ordering::SeqCst);
        tracing::info!("Discord adapter disconnected");
        Ok(())
    }

    async fn send_message(&self, channel_id: &str, content: &str) -> Result<()> {
        let http = self.http.as_ref()
            .context("Discord not connected")?;
        let channel = channel_id.parse::<u64>()
            .context("Invalid Discord channel ID")?;

        let chunks = chunk_message(content, 2000);
        for chunk in chunks {
            serenity::all::ChannelId::new(channel)
                .say(http.as_ref(), &chunk)
                .await
                .context("Failed to send Discord message")?;
        }
        Ok(())
    }

    async fn reply_to_message(
        &self, channel_id: &str, message_id: &str, content: &str,
    ) -> Result<()> {
        let http = self.http.as_ref()
            .context("Discord not connected")?;
        let channel = channel_id.parse::<u64>()
            .context("Invalid Discord channel ID")?;
        let msg_id = message_id.parse::<u64>()
            .context("Invalid Discord message ID")?;

        let chunks = chunk_message(content, 2000);
        for (i, chunk) in chunks.iter().enumerate() {
            let ch = serenity::all::ChannelId::new(channel);
            if i == 0 {
                // First chunk: reply to the original message
                let result = ch.send_message(http.as_ref(), serenity::builder::CreateMessage::new()
                    .content(chunk)
                    .reference_message(serenity::all::MessageReference::from((
                        ch,
                        serenity::all::MessageId::new(msg_id),
                    )))
                ).await;

                // Fallback: if reply fails (stale reference), send as channel message
                if result.is_err() {
                    tracing::warn!("Discord reply failed, sending as channel message");
                    ch.say(http.as_ref(), chunk).await
                        .context("Failed to send Discord fallback message")?;
                }
            } else {
                ch.say(http.as_ref(), chunk).await
                    .context("Failed to send Discord chunk")?;
            }
        }
        Ok(())
    }

    async fn start_typing(&self, channel_id: &str) -> Result<()> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let channel = channel_id.parse::<u64>().context("Invalid channel ID")?;
        let _ = serenity::all::ChannelId::new(channel)
            .broadcast_typing(http.as_ref()).await;
        Ok(())
    }

    async fn create_thinking_thread(&self, channel_id: &str, message_id: &str, title: &str) -> Result<String> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let channel = channel_id.parse::<u64>().context("Invalid channel ID")?;
        let msg_id = message_id.parse::<u64>().context("Invalid message ID")?;
        let ch = serenity::all::ChannelId::new(channel);

        let thread = ch.create_thread_from_message(
            http.as_ref(),
            serenity::all::MessageId::new(msg_id),
            serenity::builder::CreateThread::new(title)
                .auto_archive_duration(serenity::all::AutoArchiveDuration::OneHour),
        ).await.context("Failed to create thinking thread")?;

        Ok(thread.id.get().to_string())
    }

    async fn send_to_thread(&self, thread_id: &str, content: &str) -> Result<()> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let tid = thread_id.parse::<u64>().context("Invalid thread ID")?;
        let ch = serenity::all::ChannelId::new(tid);

        // Chunk for Discord's 2000 char limit
        let chunks = chunk_message(content, 2000);
        for chunk in chunks {
            ch.say(http.as_ref(), &chunk).await
                .context("Failed to send to thinking thread")?;
        }
        Ok(())
    }

    async fn archive_thread(&self, thread_id: &str) -> Result<()> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let tid = thread_id.parse::<u64>().context("Invalid thread ID")?;
        let builder = serenity::builder::EditThread::new().archived(true);
        serenity::all::ChannelId::new(tid)
            .edit_thread(http.as_ref(), builder).await
            .context("Failed to archive thinking thread")?;
        Ok(())
    }

    async fn reply_with_components(
        &self, channel_id: &str, message_id: &str, content: &str,
        components: &[MessageComponent],
    ) -> Result<String> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let channel = channel_id.parse::<u64>().context("Invalid channel ID")?;
        let msg_id = message_id.parse::<u64>().context("Invalid message ID")?;
        let ch = serenity::all::ChannelId::new(channel);

        let action_rows = super::discord_interaction::to_serenity_action_rows(components);
        let chunks = chunk_message(content, 2000);

        let mut sent_id = String::new();
        for (i, chunk) in chunks.iter().enumerate() {
            let is_last = i == chunks.len() - 1;
            let mut msg_builder = serenity::builder::CreateMessage::new().content(chunk);

            // Attach buttons only to the last chunk
            if is_last {
                msg_builder = msg_builder.components(action_rows.clone());
            }
            // Reply-reference only on first chunk
            if i == 0 {
                msg_builder = msg_builder.reference_message(serenity::all::MessageReference::from((
                    ch, serenity::all::MessageId::new(msg_id),
                )));
            }

            match ch.send_message(http.as_ref(), msg_builder).await {
                Ok(sent) => { sent_id = sent.id.get().to_string(); }
                Err(e) => {
                    tracing::warn!(error = %e, "Discord reply_with_components failed, fallback");
                    let fallback = serenity::builder::CreateMessage::new()
                        .content(chunk)
                        .components(if is_last { action_rows.clone() } else { vec![] });
                    if let Ok(sent) = ch.send_message(http.as_ref(), fallback).await {
                        sent_id = sent.id.get().to_string();
                    }
                }
            }
        }
        Ok(sent_id)
    }

    async fn delete_message(&self, channel_id: &str, message_id: &str) -> Result<()> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let channel = channel_id.parse::<u64>().context("Invalid channel ID")?;
        let msg_id = message_id.parse::<u64>().context("Invalid message ID")?;
        serenity::all::ChannelId::new(channel)
            .delete_message(http.as_ref(), serenity::all::MessageId::new(msg_id)).await
            .context("Failed to delete Discord message")?;
        Ok(())
    }

    fn take_interaction_receiver(&mut self) -> Option<mpsc::Receiver<PlatformInteraction>> {
        self.interaction_rx.take()
    }

    async fn send_audio_file(
        &self, channel_id: &str, audio_bytes: Vec<u8>, filename: &str,
    ) -> Result<()> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let channel = channel_id.parse::<u64>().context("Invalid channel ID")?;
        let ch = serenity::all::ChannelId::new(channel);

        let attachment = serenity::builder::CreateAttachment::bytes(audio_bytes, filename);
        let msg = serenity::builder::CreateMessage::new()
            .content("🔊 *Voice response:*")
            .add_file(attachment);

        ch.send_message(http.as_ref(), msg).await
            .context("Failed to send audio attachment")?;
        Ok(())
    }

    async fn send_image_file(
        &self, channel_id: &str, message_id: &str, image_bytes: Vec<u8>,
        filename: &str, caption: &str,
    ) -> Result<()> {
        let http = self.http.as_ref().context("Discord not connected")?;
        let channel = channel_id.parse::<u64>().context("Invalid channel ID")?;
        let ch = serenity::all::ChannelId::new(channel);

        let attachment = serenity::builder::CreateAttachment::bytes(image_bytes, filename);
        let content = if caption.is_empty() { "🎨".to_string() } else { caption.to_string() };
        let mut msg_builder = serenity::builder::CreateMessage::new()
            .content(&content)
            .add_file(attachment);

        // Reply to the original message if we have a valid ID
        if let Ok(msg_id) = message_id.parse::<u64>() {
            msg_builder = msg_builder.reference_message(serenity::all::MessageReference::from((
                ch,
                serenity::all::MessageId::new(msg_id),
            )));
        }

        match ch.send_message(http.as_ref(), msg_builder).await {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::warn!(error = %e, "Discord image reply failed, sending without reference");
                let _ = ch.say(http.as_ref(), &format!("{} *(image upload failed)*", content)).await;
                Ok(())
            }
        }
    }

    fn take_message_receiver(&mut self) -> Option<mpsc::Receiver<PlatformMessage>> {
        self.rx.take()
    }

    fn status(&self) -> PlatformStatus {
        PlatformStatus {
            name: "Discord".to_string(),
            connected: self.connected.load(Ordering::SeqCst),
            error: None,
        }
    }
}

/// Spawn the serenity Discord client in a background task with automatic reconnect.
/// On disconnect (internet drop, gateway error, etc.), retries with jittered
/// exponential backoff: 1s → 2s → 4s → … → 60s cap, ±25% jitter.
/// Only exits when `shutdown_rx` fires.
fn spawn_discord_client(
    token: String,
    handler: super::discord_handler::Handler,
    hub_port: u16,
    admin_ids: Vec<String>,
    connected: Arc<AtomicBool>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let intents = GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::DIRECT_MESSAGES
        | GatewayIntents::MESSAGE_CONTENT;

    tokio::spawn(async move {
        let base_backoff = std::time::Duration::from_secs(1);
        let max_backoff = std::time::Duration::from_secs(60);
        let mut backoff = base_backoff;

        loop {
            let client = build_discord_client(
                &token, handler.clone(), hub_port, admin_ids.clone(), intents,
            ).await;

            let mut client = match client {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!(error = %e, "Failed to create Discord client");
                    // Wait before retrying client creation
                    if wait_or_shutdown(&mut shutdown_rx, backoff).await {
                        return; // Shutdown signaled
                    }
                    backoff = grow_backoff(backoff, max_backoff);
                    continue;
                }
            };

            connected.store(true, Ordering::SeqCst);
            tracing::info!("Discord adapter connected");
            backoff = base_backoff; // Reset on successful connect

            tokio::select! {
                result = client.start() => {
                    connected.store(false, Ordering::SeqCst);
                    match result {
                        Err(e) => tracing::error!(error = %e, "Discord client disconnected"),
                        Ok(()) => tracing::warn!("Discord client exited cleanly"),
                    }
                }
                _ = &mut shutdown_rx => {
                    tracing::info!("Discord adapter shutting down");
                    client.shard_manager.shutdown_all().await;
                    connected.store(false, Ordering::SeqCst);
                    return; // Clean shutdown — no retry
                }
            }

            // Disconnected — wait with backoff before reconnecting
            tracing::warn!(
                delay_secs = backoff.as_secs(),
                "Discord disconnected — reconnecting after backoff"
            );
            if wait_or_shutdown(&mut shutdown_rx, backoff).await {
                return; // Shutdown signaled during backoff
            }
            backoff = grow_backoff(backoff, max_backoff);
        }
    });
}

/// Build a serenity Client with the given handler and type_map entries.
async fn build_discord_client(
    token: &str,
    handler: super::discord_handler::Handler,
    hub_port: u16,
    admin_ids: Vec<String>,
    intents: GatewayIntents,
) -> Result<serenity::Client, serenity::Error> {
    serenity::Client::builder(token, intents)
        .event_handler(handler)
        .type_map_insert::<super::discord_cmd_handlers::HubPortKey>(hub_port)
        .type_map_insert::<super::discord_cmd_handlers::AdminIdsKey>(admin_ids)
        .await
}

/// Wait for `duration` or until shutdown is signaled.
/// Returns `true` if shutdown was signaled, `false` if the timer expired.
async fn wait_or_shutdown(
    shutdown_rx: &mut tokio::sync::oneshot::Receiver<()>,
    duration: std::time::Duration,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(duration) => false,
        _ = shutdown_rx => {
            tracing::info!("Discord adapter shutdown during reconnect backoff");
            true
        }
    }
}

/// Grow backoff with ±25% jitter, capped at `max`.
fn grow_backoff(current: std::time::Duration, max: std::time::Duration) -> std::time::Duration {
    let doubled = (current * 2).min(max);
    // Simple jitter: ±25% using the current time's nanoseconds as entropy.
    // No external RNG dependency needed.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let jitter_pct = (nanos % 50) as f64 / 100.0 - 0.25; // -0.25 to +0.24
    let jittered_ms = doubled.as_millis() as f64 * (1.0 + jitter_pct);
    std::time::Duration::from_millis(jittered_ms.max(500.0) as u64)
}

/// Split a message into chunks that respect Discord's character limit.
/// Splits at newlines when possible, otherwise at the hard limit.
/// Safe for multi-byte UTF-8 — always slices at char boundaries.
fn chunk_message(content: &str, max_len: usize) -> Vec<String> {
    if content.len() <= max_len {
        return vec![content.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = content;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        // Find the nearest char boundary at or before max_len.
        // This prevents panicking when max_len falls inside a
        // multi-byte UTF-8 character (e.g. emoji like 🌀).
        let safe_max = (0..=max_len)
            .rev()
            .find(|&i| remaining.is_char_boundary(i))
            .unwrap_or(0);

        // Find a good split point (prefer newline)
        let split_at = remaining[..safe_max]
            .rfind('\n')
            .unwrap_or(safe_max);

        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk.to_string());
        remaining = rest.trim_start_matches('\n');
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_short_message() {
        let chunks = chunk_message("hello", 2000);
        assert_eq!(chunks, vec!["hello"]);
    }

    #[test]
    fn test_chunk_exact_limit() {
        let msg = "a".repeat(2000);
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn test_chunk_over_limit() {
        let msg = "a".repeat(4500);
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.len() <= 2000));
    }

    #[test]
    fn test_chunk_prefers_newline_split() {
        let mut msg = "a".repeat(1500);
        msg.push('\n');
        msg.push_str(&"b".repeat(1500));
        let chunks = chunk_message(&msg, 2000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with('a'));
        assert!(chunks[1].starts_with('b'));
    }

    #[test]
    fn test_chunk_emoji_at_boundary() {
        // Regression test: 🌀 is 4 bytes (U+1F300). If it straddles byte 2000,
        // the old code panicked with "byte index 2000 is not a char boundary".
        let mut msg = "a".repeat(1997);
        msg.push('🌀'); // bytes 1997-2001, crosses the 2000 boundary
        msg.push_str(&"b".repeat(500));
        let chunks = chunk_message(&msg, 2000);
        assert!(chunks.len() >= 2, "Should split into multiple chunks");
        assert!(chunks.iter().all(|c| c.len() <= 2000), "No chunk should exceed limit");
        // Verify no data was lost
        let rejoined: String = chunks.join("");
        assert_eq!(rejoined, msg, "Chunks should rejoin to original");
    }

    #[test]
    fn test_adapter_name() {
        let config = DiscordConfig::default();
        let adapter = DiscordAdapter::new(config, 3000);
        assert_eq!(adapter.name(), "Discord");
    }

    #[test]
    fn test_not_configured_without_token() {
        let config = DiscordConfig::default();
        let adapter = DiscordAdapter::new(config, 3000);
        assert!(!adapter.is_configured());
    }

    #[test]
    fn test_status_disconnected() {
        let config = DiscordConfig::default();
        let adapter = DiscordAdapter::new(config, 3000);
        let status = adapter.status();
        assert!(!status.connected);
        assert_eq!(status.name, "Discord");
    }

    #[test]
    fn test_backoff_growth() {
        let d1 = std::time::Duration::from_secs(1);
        let max = std::time::Duration::from_secs(60);
        let d2 = grow_backoff(d1, max);
        // Should be approximately 2s ±25%
        assert!(d2.as_millis() >= 1500, "Backoff should grow: got {}ms", d2.as_millis());
        assert!(d2.as_millis() <= 2500, "Backoff should not overshoot: got {}ms", d2.as_millis());
    }

    #[test]
    fn test_backoff_cap() {
        let max = std::time::Duration::from_secs(60);
        let d = grow_backoff(std::time::Duration::from_secs(60), max);
        // At the cap, doubled=60 (capped), jitter ±25% → 45-75s, but capped input stays ~60
        assert!(d.as_secs() <= 75, "Backoff should cap near max: got {}s", d.as_secs());
    }

    #[test]
    fn test_backoff_minimum() {
        let min = std::time::Duration::from_millis(100);
        let max = std::time::Duration::from_secs(60);
        let d = grow_backoff(min, max);
        // Even with negative jitter, should stay >= 500ms floor
        assert!(d.as_millis() >= 500, "Backoff should have 500ms floor: got {}ms", d.as_millis());
    }
}
