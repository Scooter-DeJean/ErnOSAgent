// Ern-OS — Tier 2: Consolidation engine
// Ported from ErnOSAgent with structural improvements

use crate::provider::Message;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationRecord {
    pub id: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub messages_consolidated: usize,
    pub summary: String,
    pub original_token_estimate: usize,
    pub summary_token_estimate: usize,
}

pub struct ConsolidationEngine {
    records: Vec<ConsolidationRecord>,
    file_path: Option<PathBuf>,
}

impl ConsolidationEngine {
    pub fn new() -> Self {
        Self { records: Vec::new(), file_path: None }
    }

    pub fn open(path: &Path) -> Result<Self> {
        tracing::info!(module = "consolidation", fn_name = "open", "consolidation::open called");
        let mut engine = Self { records: Vec::new(), file_path: Some(path.to_path_buf()) };
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read consolidation: {}", path.display()))?;
            engine.records = serde_json::from_str(&content)
                .with_context(|| "Failed to parse consolidation file")?;
        }
        Ok(engine)
    }

    fn persist(&self) -> Result<()> {
        if let Some(ref path) = self.file_path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let content = serde_json::to_string_pretty(&self.records)?;
            std::fs::write(path, content)?;
        }
        Ok(())
    }

    pub fn needs_consolidation(&self, usage_pct: f32, threshold: f32) -> bool {
        tracing::info!(module = "consolidation", fn_name = "needs_consolidation", "consolidation::needs_consolidation called");
        usage_pct >= threshold
    }

    /// Split session history for consolidation using the configured split ratio.
    /// Returns (old_messages_to_consolidate, recent_messages_to_keep).
    pub fn split_for_consolidation(
        &self, messages: &[Message], split_ratio: f64,
    ) -> (Vec<Message>, Vec<Message>) {
        if messages.len() <= 2 {
            return (Vec::new(), messages.to_vec());
        }
        let split = ((messages.len() as f64) * split_ratio) as usize;
        let split = split.max(1);
        (messages[..split].to_vec(), messages[split..].to_vec())
    }

    pub fn record_consolidation(
        &mut self, count: usize, summary: &str, original_chars: usize,
    ) -> Result<()> {
        tracing::info!(module = "consolidation", fn_name = "record_consolidation", "consolidation::record_consolidation called");
        self.records.push(ConsolidationRecord {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: chrono::Utc::now(),
            messages_consolidated: count,
            summary: summary.to_string(),
            // NOTE: these are char-based estimates stored for audit trail only.
            // They are not used for any decisions — thresholds use count_tokens.
            original_token_estimate: original_chars / 4,
            summary_token_estimate: summary.len() / 4,
        });
        self.persist()
    }

    pub fn summary_message(summary: &str) -> Message {
        Message::text("system", &format!(
            "[Memory — Consolidated Context]\n\
             The following is a compressed summary of earlier conversation:\n\n{}",
            summary
        ))
    }

    pub fn consolidation_count(&self) -> usize { self.records.len() }
}

/// Build the system prompt for the memory sorting inference pass.
/// Injected into digest_provider (slot 1) before consolidation fires.
/// The model reads the old messages and sorts everything into permanent tiers.
pub fn pre_consolidation_sort_prompt(message_count: usize) -> String {
    format!(
        "You are performing a MEMORY CONSOLIDATION SORT. \
         You have been given {message_count} messages that are about to be \
         compressed and their verbatim content will be permanently lost. \
         Your job is to read every message and use your tools to store \
         everything that matters before it is discarded.\n\n\
         REQUIRED ACTIONS — you MUST call tools to store:\n\
         • synaptic(action='store') — every person, place, entity, fact, \
           preference, and relationship mentioned. Include ALL personal data: \
           names, relationships (fiancé, partner, family), pets and their names, \
           preferences, dates, locations, project names.\n\
         • synaptic(action='store_relationship') — every relationship between entities.\n\
         • scratchpad(action='pin') — any important standing fact, user preference, \
           or recurring context that should be immediately visible in future sessions.\n\
         • lessons(action='add') — any rule, pattern, or lesson about how to \
           work with this user or handle similar situations.\n\
         • self_skills — any reusable workflow or procedure that was discovered.\n\n\
         Work systematically through all {message_count} messages. When you have stored \
         everything important, call reply_request with a brief summary of what you stored. \
         Do NOT output text before finishing all tool calls. Sort first, then reply.",
        message_count = message_count
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_consolidation() {
        let engine = ConsolidationEngine::new();
        assert!(engine.needs_consolidation(0.85, 0.80));
        assert!(!engine.needs_consolidation(0.75, 0.80));
    }

    #[test]
    fn test_record_and_count() {
        let mut engine = ConsolidationEngine::new();
        engine.record_consolidation(5, "Summary", 2000).unwrap();
        assert_eq!(engine.consolidation_count(), 1);
    }
}
