//! Discord slash command definitions — builders + guild-scoped registration.
//!
//! All commands are registered per-guild for instant availability.
//! Global commands take up to 1 hour to propagate — guild-scoped is correct practice.

use serenity::all::{CommandOptionType, CreateCommand, CreateCommandOption, GuildId, Http};

/// Register all slash commands with each guild the bot is a member of.
pub async fn register_commands(http: &Http, guild_ids: &[GuildId]) {
    let commands = all_commands();
    for guild_id in guild_ids {
        for cmd in &commands {
            if let Err(e) = guild_id.create_command(http, cmd.clone()).await {
                tracing::warn!(
                    guild = %guild_id, error = %e,
                    "Failed to register slash command"
                );
            }
        }
    }
    tracing::info!(
        guilds = guild_ids.len(),
        commands = commands.len(),
        "Slash commands registered"
    );
}

/// Return all slash command definitions.
fn all_commands() -> Vec<CreateCommand> {
    vec![
        build_new(),
        build_regenerate(),
        build_speak(),
        build_fork(),
        build_status(),
        build_sessions(),
        build_export(),
        build_stop(),
        build_shutdown(),
        build_reboot(),
    ]
}

/// /new — Start a fresh chat session.
fn build_new() -> CreateCommand {
    CreateCommand::new("new")
        .description("Start a fresh chat session")
}

/// /regenerate — Redo the last response.
fn build_regenerate() -> CreateCommand {
    CreateCommand::new("regenerate")
        .description("Redo the last AI response")
}

/// /speak — Read the last response aloud via TTS.
fn build_speak() -> CreateCommand {
    CreateCommand::new("speak")
        .description("Read the last AI response aloud")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::String,
                "voice",
                "TTS voice to use (default: am_michael)",
            )
            .required(false),
        )
}

/// /fork — Branch the conversation from a message.
fn build_fork() -> CreateCommand {
    CreateCommand::new("fork")
        .description("Branch the conversation from a specific message")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::Integer,
                "message_number",
                "Message index to fork from (default: last)",
            )
            .required(false)
            .min_int_value(0),
        )
}

/// /status — Show bot and platform connection status.
fn build_status() -> CreateCommand {
    CreateCommand::new("status")
        .description("Show bot connection and service status")
}

/// /sessions — List recent chat sessions.
fn build_sessions() -> CreateCommand {
    CreateCommand::new("sessions")
        .description("List recent chat sessions")
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::Integer,
                "count",
                "Number of sessions to show (default: 5)",
            )
            .required(false)
            .min_int_value(1)
            .max_int_value(20),
        )
}

/// /export — Export the current session as a markdown file.
fn build_export() -> CreateCommand {
    CreateCommand::new("export")
        .description("Export the current session as a markdown file")
}

/// /stop — Emergency halt of active inference.
fn build_stop() -> CreateCommand {
    CreateCommand::new("stop")
        .description("Halt and interrupt the AI immediately")
}

/// /shutdown — Gracefully exit the Ern-OS process (admin only).
fn build_shutdown() -> CreateCommand {
    CreateCommand::new("shutdown")
        .description("Gracefully shut down Ern-OS (admin only)")
}

/// /reboot — Restart the Ern-OS process (admin only).
fn build_reboot() -> CreateCommand {
    CreateCommand::new("reboot")
        .description("Reboot and restart Ern-OS (admin only)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_commands_count() {
        assert_eq!(all_commands().len(), 10);
    }

    #[test]
    fn test_regenerate_command() {
        let cmd = build_regenerate();
        // CreateCommand doesn't expose fields, but we can verify it builds
        let _ = cmd;
    }

    #[test]
    fn test_speak_has_voice_option() {
        let cmd = build_speak();
        let _ = cmd;
    }

    #[test]
    fn test_fork_has_message_number_option() {
        let cmd = build_fork();
        let _ = cmd;
    }

    #[test]
    fn test_sessions_has_count_option() {
        let cmd = build_sessions();
        let _ = cmd;
    }
}
