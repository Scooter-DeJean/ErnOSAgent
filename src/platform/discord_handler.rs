// Ern-OS — Discord event handler
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Serenity event handler — translates Discord gateway events into PlatformMessages.

use crate::config::DiscordConfig;
use crate::platform::adapter::{PlatformMessage, PlatformInteraction};
use serenity::all::{Context, EventHandler, Interaction, Message, Ready};
use async_trait::async_trait;
use tokio::sync::mpsc;

/// Serenity event handler that forwards messages to the adapter's channel.
#[derive(Clone)]
pub struct Handler {
    tx: mpsc::Sender<PlatformMessage>,
    interaction_tx: mpsc::Sender<PlatformInteraction>,
    config: DiscordConfig,
}

impl Handler {
    /// Create a new handler with the given message sender and config.
    pub fn new(
        tx: mpsc::Sender<PlatformMessage>,
        interaction_tx: mpsc::Sender<PlatformInteraction>,
        config: DiscordConfig,
    ) -> Self {
        Self { tx, interaction_tx, config }
    }

    /// Check if a channel is in the listen list (or list is empty = all channels).
    fn is_allowed_channel(&self, channel_id: u64) -> bool {
        if self.config.listen_channels.is_empty() {
            return true;
        }
        let id_str = channel_id.to_string();
        self.config.listen_channels.iter().any(|c| c == &id_str)
    }

    /// Check if a user ID is in the admin list.
    fn is_admin(&self, user_id: u64) -> bool {
        let id_str = user_id.to_string();
        self.config.admin_ids.iter().any(|a| a == &id_str)
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        tracing::info!(
            author = %msg.author.name,
            author_bot = msg.author.bot,
            channel = %msg.channel_id,
            content_len = msg.content.len(),
            listen_channels = ?self.config.listen_channels,
            "Discord message event fired"
        );

        if msg.author.bot {
            return;
        }

        let user_is_admin = self.is_admin(msg.author.id.get());
        if !self.check_message_allowed(&ctx, &msg, user_is_admin).await {
            return;
        }

        tracing::debug!(
            user = %msg.author.name,
            user_id = msg.author.id.get(),
            channel = %msg.channel_id,
            content_len = msg.content.len(),
            is_admin = user_is_admin,
            is_dm = msg.guild_id.is_none(),
            "Discord message received"
        );

        let _ = msg.channel_id.broadcast_typing(&ctx.http).await;

        let platform_msg = build_platform_message(&msg, user_is_admin);
        if let Err(e) = self.tx.send(platform_msg).await {
            tracing::error!(error = %e, "Failed to forward Discord message to adapter");
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        tracing::info!(
            user = %ready.user.name,
            guilds = ready.guilds.len(),
            "Discord bot ready"
        );

        // Register slash commands with each guild (instant availability)
        let guild_ids: Vec<serenity::all::GuildId> = ready.guilds.iter()
            .map(|g| g.id)
            .collect();
        super::discord_commands::register_commands(&ctx.http, &guild_ids).await;
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        match interaction {
            Interaction::Component(ci) => {
                self.handle_component_interaction(&ctx, ci).await;
            }
            Interaction::Command(ci) => {
                super::discord_cmd_handlers::dispatch(&ctx, &ci).await;
            }
            _ => {}
        }
    }
}

impl Handler {
    /// Handle a button click component interaction.
    async fn handle_component_interaction(
        &self, ctx: &Context, ci: serenity::all::ComponentInteraction,
    ) {

        let custom_id = ci.data.custom_id.clone();
        tracing::info!(custom_id = %custom_id, user = %ci.user.name, "Discord button interaction");

        let parsed = match super::discord_interaction::parse_custom_id(&custom_id) {
            Some(p) => p,
            None => {
                tracing::warn!(custom_id = %custom_id, "Unknown interaction custom_id");
                return;
            }
        };

        // Acknowledge the interaction with an ephemeral message
        let ack_text = match parsed.action.as_str() {
            "thumbsup" => "👍 Feedback recorded",
            "thumbsdown" => "👎 Feedback recorded",
            "regenerate" => "🔄 Regenerating response…",
            "speak" => "🔊 Generating audio…",
            "plan_approve" => "✅ Plan approved — executing…",
            "plan_revise" => "✏️ Revise requested",
            "plan_cancel" => "❌ Plan cancelled",
            _ => "Acknowledged",
        };
        let response = serenity::builder::CreateInteractionResponse::Message(
            serenity::builder::CreateInteractionResponseMessage::new()
                .content(ack_text)
                .ephemeral(true),
        );
        let _ = ci.create_response(&ctx.http, response).await;

        // Forward to interaction handler
        let platform_interaction = PlatformInteraction {
            platform: "discord".to_string(),
            action: parsed.action,
            session_id: parsed.session_id,
            message_index: parsed.message_index,
            user_id: ci.user.id.get().to_string(),
            channel_id: ci.channel_id.get().to_string(),
            interaction_token: ci.token.clone(),
            interaction_id: ci.id.get().to_string(),
        };

        if let Err(e) = self.interaction_tx.send(platform_interaction).await {
            tracing::error!(error = %e, "Failed to forward Discord interaction");
        }
    }

    /// Check DM and channel gating — returns true if the message should be processed.
    async fn check_message_allowed(
        &self, ctx: &Context, msg: &Message, user_is_admin: bool,
    ) -> bool {
        let is_dm = msg.guild_id.is_none();

        if is_dm {
            if !user_is_admin {
                tracing::info!(
                    user = %msg.author.name,
                    user_id = msg.author.id.get(),
                    "DM rejected — non-admin user"
                );
                let _ = msg.reply(
                    &ctx.http,
                    "I only accept DMs from admins. Please message me in a server channel.",
                ).await;
                return false;
            }
            tracing::info!(
                user = %msg.author.name,
                user_id = msg.author.id.get(),
                "Admin DM accepted"
            );
            return true;
        }

        // Guild channel filtering
        if !self.is_allowed_channel(msg.channel_id.get()) {
            tracing::warn!(
                channel = %msg.channel_id,
                listen_channels = ?self.config.listen_channels,
                "Message rejected by channel filter"
            );
            return false;
        }

        true
    }
}

/// Build a PlatformMessage from a serenity Message.
fn build_platform_message(msg: &Message, is_admin: bool) -> PlatformMessage {
    PlatformMessage {
        platform: "discord".to_string(),
        channel_id: msg.channel_id.get().to_string(),
        user_id: msg.author.id.get().to_string(),
        user_name: msg.author.name.clone(),
        content: msg.content.clone(),
        attachments: msg.attachments.iter()
            .map(|a| a.url.clone())
            .collect(),
        message_id: msg.id.get().to_string(),
        is_admin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_handler() -> Handler {
        let (tx, _rx) = mpsc::channel(16);
        let (itx, _irx) = mpsc::channel(16);
        let config = DiscordConfig {
            token: None,
            admin_ids: vec!["123456".to_string(), "789012".to_string()],
            listen_channels: vec!["channel_1".to_string()],
            enabled: true,
        };
        Handler::new(tx, itx, config)
    }

    #[test]
    fn test_is_admin_true() {
        let handler = test_handler();
        assert!(handler.is_admin(123456));
    }

    #[test]
    fn test_is_admin_false() {
        let handler = test_handler();
        assert!(!handler.is_admin(999));
    }

    #[test]
    fn test_allowed_channel() {
        let handler = test_handler();
        // "channel_1" is a string, not a u64 — but the check compares string representations
        // In practice, Discord channel IDs are u64. This tests the filtering logic.
        assert!(!handler.is_allowed_channel(999));
    }

    #[test]
    fn test_empty_channels_allows_all() {
        let (tx, _rx) = mpsc::channel(16);
        let (itx, _irx) = mpsc::channel(16);
        let config = DiscordConfig {
            token: None,
            admin_ids: vec![],
            listen_channels: vec![], // empty = all channels allowed
            enabled: true,
        };
        let handler = Handler::new(tx, itx, config);
        assert!(handler.is_allowed_channel(12345));
        assert!(handler.is_allowed_channel(99999));
    }
}
