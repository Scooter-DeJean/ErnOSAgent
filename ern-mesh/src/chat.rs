// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! ErnChat — encrypted topic-based messaging over Gossipsub.
//!
//! ErnChat provides decentralised group and direct messaging:
//!
//! - **Topic channels**: Named Gossipsub topics (e.g., `ernmesh/chat/general`).
//!   Anyone can subscribe to public topics.
//! - **Direct messages**: Peer-to-peer encrypted channels using the E2E
//!   encryption primitives from `encryption.rs`.
//! - **Message format**: Each chat message is a signed [`ChatMessage`] wrapped
//!   in the standard [`SignedEnvelope`](crate::protocol::SignedEnvelope).
//! - **History**: Local-only message log per topic (no server storage).
//!
//! Messages are broadcast via Gossipsub. For DMs, the payload is encrypted
//! with the recipient's session key before broadcast — only the intended
//! recipient can decrypt it.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// A chat topic identifier — maps to a Gossipsub topic hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TopicId(String);

impl TopicId {
    /// Create a new topic from a name.
    ///
    /// The name is prefixed with `ernmesh/chat/` to namespace ErnChat topics.
    pub fn new(name: &str) -> Self {
        Self(format!("ernmesh/chat/{}", name))
    }

    /// Create a direct message topic between two peers.
    ///
    /// The topic name is deterministic: both peers produce the same topic
    /// by sorting their PeerIds lexicographically.
    pub fn direct(peer_a: &str, peer_b: &str) -> Self {
        let (first, second) = if peer_a < peer_b {
            (peer_a, peer_b)
        } else {
            (peer_b, peer_a)
        };
        Self(format!("ernmesh/dm/{}/{}", first, second))
    }

    /// Returns the full Gossipsub topic string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns `true` if this is a direct message topic.
    pub fn is_dm(&self) -> bool {
        self.0.starts_with("ernmesh/dm/")
    }
}

impl std::fmt::Display for TopicId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A single chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// The topic this message belongs to.
    pub topic: TopicId,
    /// Sender's PeerId (base58).
    pub sender: String,
    /// Display name (optional, user-set).
    pub display_name: Option<String>,
    /// Message content (plaintext for public topics, encrypted for DMs).
    pub content: String,
    /// Timestamp of the message.
    pub timestamp: SystemTime,
    /// Unique message ID (sender-generated).
    pub message_id: String,
    /// Whether this message is encrypted (DM).
    pub encrypted: bool,
}

impl ChatMessage {
    /// Create a new plaintext chat message for a public topic.
    pub fn new_public(
        topic: TopicId,
        sender: &str,
        content: &str,
        display_name: Option<String>,
    ) -> Self {
        Self {
            topic,
            sender: sender.to_string(),
            display_name,
            content: content.to_string(),
            timestamp: SystemTime::now(),
            message_id: generate_message_id(sender),
            encrypted: false,
        }
    }

    /// Create an encrypted chat message for a direct message.
    pub fn new_encrypted(
        topic: TopicId,
        sender: &str,
        encrypted_content: String,
        display_name: Option<String>,
    ) -> Self {
        Self {
            topic,
            sender: sender.to_string(),
            display_name,
            content: encrypted_content,
            timestamp: SystemTime::now(),
            message_id: generate_message_id(sender),
            encrypted: true,
        }
    }
}

/// Generate a unique message ID from sender + timestamp + random suffix.
fn generate_message_id(sender: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let suffix: u32 = rand::random();
    format!("{}-{}-{:08x}", sender, ts, suffix)
}

/// Local chat log — stores messages per topic.
///
/// Not replicated — each node keeps its own history.
/// Thread-safety: wrap in `RwLock` if shared across tasks.
#[derive(Debug, Default)]
pub struct ChatLog {
    /// Messages per topic, ordered by insertion.
    topics: HashMap<TopicId, Vec<ChatMessage>>,
    /// Maximum messages retained per topic.
    max_per_topic: usize,
}

impl ChatLog {
    /// Create a new chat log with a per-topic message limit.
    pub fn new(max_per_topic: usize) -> Self {
        Self {
            topics: HashMap::new(),
            max_per_topic,
        }
    }

    /// Append a message to the log for its topic.
    ///
    /// If the topic exceeds `max_per_topic`, the oldest message is evicted.
    pub fn append(&mut self, msg: ChatMessage) {
        let topic = msg.topic.clone();
        let messages = self.topics.entry(topic).or_default();

        messages.push(msg);

        // Evict oldest if over capacity
        if messages.len() > self.max_per_topic {
            let excess = messages.len() - self.max_per_topic;
            messages.drain(..excess);
        }
    }

    /// Get messages for a topic.
    pub fn messages(&self, topic: &TopicId) -> &[ChatMessage] {
        self.topics
            .get(topic)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Get the most recent N messages for a topic.
    pub fn recent_messages(&self, topic: &TopicId, n: usize) -> &[ChatMessage] {
        let msgs = self.messages(topic);
        let start = msgs.len().saturating_sub(n);
        &msgs[start..]
    }

    /// Returns all topics with at least one message.
    pub fn active_topics(&self) -> Vec<&TopicId> {
        self.topics
            .iter()
            .filter(|(_, msgs)| !msgs.is_empty())
            .map(|(topic, _)| topic)
            .collect()
    }

    /// Total messages across all topics.
    pub fn total_messages(&self) -> usize {
        self.topics.values().map(|v| v.len()).sum()
    }

    /// Clear all messages for a topic.
    pub fn clear_topic(&mut self, topic: &TopicId) {
        self.topics.remove(topic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_new() {
        let topic = TopicId::new("general");
        assert_eq!(topic.as_str(), "ernmesh/chat/general");
        assert!(!topic.is_dm());
    }

    #[test]
    fn test_topic_dm_deterministic() {
        let dm1 = TopicId::direct("peer_a", "peer_b");
        let dm2 = TopicId::direct("peer_b", "peer_a");
        assert_eq!(dm1, dm2, "DM topic must be deterministic regardless of order");
        assert!(dm1.is_dm());
    }

    #[test]
    fn test_topic_display() {
        let topic = TopicId::new("lobby");
        assert_eq!(format!("{}", topic), "ernmesh/chat/lobby");
    }

    #[test]
    fn test_chat_message_public() {
        let topic = TopicId::new("general");
        let msg = ChatMessage::new_public(
            topic.clone(),
            "peer_a",
            "Hello, mesh!",
            Some("Alice".into()),
        );

        assert_eq!(msg.topic, topic);
        assert_eq!(msg.sender, "peer_a");
        assert_eq!(msg.content, "Hello, mesh!");
        assert_eq!(msg.display_name.as_deref(), Some("Alice"));
        assert!(!msg.encrypted);
        assert!(!msg.message_id.is_empty());
    }

    #[test]
    fn test_chat_message_encrypted() {
        let topic = TopicId::direct("peer_a", "peer_b");
        let msg = ChatMessage::new_encrypted(
            topic,
            "peer_a",
            "BASE64_ENCRYPTED_DATA".into(),
            None,
        );

        assert!(msg.encrypted);
        assert_eq!(msg.content, "BASE64_ENCRYPTED_DATA");
    }

    #[test]
    fn test_message_ids_unique() {
        let id1 = generate_message_id("peer_a");
        let id2 = generate_message_id("peer_a");
        assert_ne!(id1, id2, "Message IDs must be unique");
    }

    #[test]
    fn test_chat_log_new_empty() {
        let log = ChatLog::new(100);
        assert_eq!(log.total_messages(), 0);
        assert!(log.active_topics().is_empty());
    }

    #[test]
    fn test_chat_log_append_and_retrieve() {
        let mut log = ChatLog::new(100);
        let topic = TopicId::new("general");

        log.append(ChatMessage::new_public(
            topic.clone(), "peer_a", "msg1", None,
        ));
        log.append(ChatMessage::new_public(
            topic.clone(), "peer_b", "msg2", None,
        ));

        assert_eq!(log.messages(&topic).len(), 2);
        assert_eq!(log.total_messages(), 2);
        assert_eq!(log.active_topics().len(), 1);
    }

    #[test]
    fn test_chat_log_evicts_oldest() {
        let mut log = ChatLog::new(3); // max 3 per topic
        let topic = TopicId::new("test");

        for i in 0..5 {
            log.append(ChatMessage::new_public(
                topic.clone(),
                "peer_a",
                &format!("msg{}", i),
                None,
            ));
        }

        let msgs = log.messages(&topic);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].content, "msg2"); // oldest surviving
        assert_eq!(msgs[2].content, "msg4"); // newest
    }

    #[test]
    fn test_chat_log_recent_messages() {
        let mut log = ChatLog::new(100);
        let topic = TopicId::new("general");

        for i in 0..10 {
            log.append(ChatMessage::new_public(
                topic.clone(), "peer_a", &format!("msg{}", i), None,
            ));
        }

        let recent = log.recent_messages(&topic, 3);
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "msg7");
        assert_eq!(recent[2].content, "msg9");
    }

    #[test]
    fn test_chat_log_multiple_topics() {
        let mut log = ChatLog::new(100);
        let t1 = TopicId::new("general");
        let t2 = TopicId::new("dev");

        log.append(ChatMessage::new_public(t1.clone(), "a", "hi", None));
        log.append(ChatMessage::new_public(t2.clone(), "b", "hello", None));

        assert_eq!(log.messages(&t1).len(), 1);
        assert_eq!(log.messages(&t2).len(), 1);
        assert_eq!(log.total_messages(), 2);
        assert_eq!(log.active_topics().len(), 2);
    }

    #[test]
    fn test_chat_log_unknown_topic_empty() {
        let log = ChatLog::new(100);
        let topic = TopicId::new("nonexistent");
        assert!(log.messages(&topic).is_empty());
    }

    #[test]
    fn test_chat_log_clear_topic() {
        let mut log = ChatLog::new(100);
        let topic = TopicId::new("general");

        log.append(ChatMessage::new_public(topic.clone(), "a", "hi", None));
        assert_eq!(log.total_messages(), 1);

        log.clear_topic(&topic);
        assert_eq!(log.total_messages(), 0);
        assert!(log.messages(&topic).is_empty());
    }

    #[test]
    fn test_chat_message_serialize_roundtrip() {
        let msg = ChatMessage::new_public(
            TopicId::new("test"),
            "peer_a",
            "Hello!",
            Some("Alice".into()),
        );

        let json = serde_json::to_string(&msg).unwrap();
        let recovered: ChatMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(recovered.sender, "peer_a");
        assert_eq!(recovered.content, "Hello!");
        assert_eq!(recovered.display_name.as_deref(), Some("Alice"));
    }
}
