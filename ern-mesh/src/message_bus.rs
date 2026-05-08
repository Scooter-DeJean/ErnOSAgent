// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Message bus — wires Gossipsub into ErnChat and ErnieBook.
//!
//! The message bus is the integration layer between the raw Gossipsub
//! protocol events and the application-layer modules (chat, erniebook).
//!
//! ## Architecture
//!
//! ```text
//! Gossipsub Event → MessageBus::handle_incoming()
//!                        │
//!                        ├─ ernmesh/chat/*    → ChatLog::append()
//!                        ├─ ernmesh/erniebook → Feed::add()
//!                        └─ unknown topic     → logged + dropped
//!
//! ChatMessage/Post → MessageBus::publish_chat() / publish_post()
//!                        │
//!                        └─ Serialise → return bytes for Gossipsub::publish()
//! ```
//!
//! The bus does NOT hold a reference to the Swarm. Instead, it returns
//! serialised bytes that the swarm event loop publishes. This keeps
//! the bus testable without a live network.

use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::chat::{ChatLog, ChatMessage};
use crate::erniebook::{Feed, Post, ERNIEBOOK_TOPIC};

/// Incoming message from Gossipsub — pre-parsed topic and raw data.
#[derive(Debug)]
pub struct IncomingMessage {
    /// The Gossipsub topic string.
    pub topic: String,
    /// Raw message bytes (JSON-serialised ChatMessage or Post).
    pub data: Vec<u8>,
    /// PeerId of the propagation source.
    pub source: String,
}

/// Outgoing message — serialised bytes ready for Gossipsub::publish().
#[derive(Debug)]
pub struct OutgoingMessage {
    /// The Gossipsub topic string.
    pub topic: String,
    /// Serialised message bytes.
    pub data: Vec<u8>,
}

/// The message bus — routes Gossipsub events to application modules.
pub struct MessageBus {
    /// Chat log for storing received messages.
    chat_log: Arc<RwLock<ChatLog>>,
    /// ErnieBook feed for storing received posts.
    feed: Arc<RwLock<Feed>>,
}

impl MessageBus {
    /// Create a new message bus with shared state references.
    pub fn new(
        chat_log: Arc<RwLock<ChatLog>>,
        feed: Arc<RwLock<Feed>>,
    ) -> Self {
        tracing::info!("Message bus initialised");
        Self { chat_log, feed }
    }

    /// Handle an incoming Gossipsub message.
    ///
    /// Routes to the appropriate application module based on topic prefix:
    /// - `ernmesh/chat/*` → ChatLog
    /// - `ernmesh/erniebook` → Feed
    /// - Everything else → logged and dropped
    ///
    /// Returns `Ok(true)` if the message was processed, `Ok(false)` if
    /// it was for an unknown topic.
    pub async fn handle_incoming(&self, msg: IncomingMessage) -> Result<bool> {
        if msg.topic.starts_with("ernmesh/chat/") || msg.topic.starts_with("ernmesh/dm/") {
            self.handle_chat_message(&msg).await?;
            Ok(true)
        } else if msg.topic == ERNIEBOOK_TOPIC {
            self.handle_erniebook_post(&msg).await?;
            Ok(true)
        } else {
            tracing::debug!(
                topic = %msg.topic,
                source = %msg.source,
                size = msg.data.len(),
                "Message bus: unknown topic — dropped"
            );
            Ok(false)
        }
    }

    /// Prepare a chat message for Gossipsub publishing.
    ///
    /// Serialises the message and returns the topic + bytes.
    /// The caller publishes via `Swarm::behaviour_mut().gossipsub.publish()`.
    pub fn prepare_chat(&self, msg: &ChatMessage) -> Result<OutgoingMessage> {
        let data = serde_json::to_vec(msg)
            .context("Failed to serialise chat message")?;

        tracing::debug!(
            topic = %msg.topic,
            sender = %msg.sender,
            size = data.len(),
            "Message bus: chat message prepared for publish"
        );

        Ok(OutgoingMessage {
            topic: msg.topic.as_str().to_string(),
            data,
        })
    }

    /// Prepare an ErnieBook post for Gossipsub publishing.
    pub fn prepare_post(&self, post: &Post) -> Result<OutgoingMessage> {
        let data = serde_json::to_vec(post)
            .context("Failed to serialise ErnieBook post")?;

        tracing::debug!(
            author = %post.author,
            size = data.len(),
            "Message bus: ErnieBook post prepared for publish"
        );

        Ok(OutgoingMessage {
            topic: ERNIEBOOK_TOPIC.to_string(),
            data,
        })
    }

    /// Handle a chat message — deserialise and store in ChatLog.
    async fn handle_chat_message(&self, msg: &IncomingMessage) -> Result<()> {
        let chat_msg: ChatMessage = serde_json::from_slice(&msg.data)
            .context("Failed to deserialise chat message")?;

        tracing::info!(
            topic = %chat_msg.topic,
            sender = %chat_msg.sender,
            encrypted = chat_msg.encrypted,
            "Message bus: chat message received"
        );

        self.chat_log.write().await.append(chat_msg);
        Ok(())
    }

    /// Handle an ErnieBook post — deserialise and store in Feed.
    async fn handle_erniebook_post(&self, msg: &IncomingMessage) -> Result<()> {
        let post: Post = serde_json::from_slice(&msg.data)
            .context("Failed to deserialise ErnieBook post")?;

        tracing::info!(
            author = %post.author,
            content_len = post.content.len(),
            "Message bus: ErnieBook post received"
        );

        self.feed.write().await.add(post);
        Ok(())
    }

    /// Access the shared chat log.
    pub fn chat_log(&self) -> &Arc<RwLock<ChatLog>> {
        &self.chat_log
    }

    /// Access the shared ErnieBook feed.
    pub fn feed(&self) -> &Arc<RwLock<Feed>> {
        &self.feed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::TopicId;

    fn make_bus() -> MessageBus {
        MessageBus::new(
            Arc::new(RwLock::new(ChatLog::new(100))),
            Arc::new(RwLock::new(Feed::new(100))),
        )
    }

    fn make_chat_msg(topic: &str, sender: &str, content: &str) -> IncomingMessage {
        let msg = ChatMessage::new_public(
            TopicId::new(topic.strip_prefix("ernmesh/chat/").unwrap_or(topic)),
            sender,
            content,
            None,
        );
        IncomingMessage {
            topic: topic.to_string(),
            data: serde_json::to_vec(&msg).unwrap(),
            source: sender.to_string(),
        }
    }

    fn make_post_msg(author: &str, content: &str) -> IncomingMessage {
        let post = Post::new(author, content, None, vec![]);
        IncomingMessage {
            topic: ERNIEBOOK_TOPIC.to_string(),
            data: serde_json::to_vec(&post).unwrap(),
            source: author.to_string(),
        }
    }

    #[tokio::test]
    async fn test_handle_chat_message() {
        let bus = make_bus();
        let msg = make_chat_msg("ernmesh/chat/general", "peer_a", "hello");

        let result = bus.handle_incoming(msg).await;
        assert!(result.is_ok());
        assert!(result.unwrap());

        let log = bus.chat_log().read().await;
        assert_eq!(log.total_messages(), 1);
    }

    #[tokio::test]
    async fn test_handle_dm_message() {
        let bus = make_bus();
        let dm_msg = ChatMessage::new_public(
            TopicId::direct("peer_a", "peer_b"),
            "peer_a",
            "secret",
            None,
        );
        let msg = IncomingMessage {
            topic: dm_msg.topic.as_str().to_string(),
            data: serde_json::to_vec(&dm_msg).unwrap(),
            source: "peer_a".to_string(),
        };

        let result = bus.handle_incoming(msg).await;
        assert!(result.unwrap());
    }

    #[tokio::test]
    async fn test_handle_erniebook_post() {
        let bus = make_bus();
        let msg = make_post_msg("peer_a", "My first post!");

        let result = bus.handle_incoming(msg).await;
        assert!(result.is_ok());
        assert!(result.unwrap());

        let feed = bus.feed().read().await;
        assert_eq!(feed.count(), 1);
    }

    #[tokio::test]
    async fn test_handle_unknown_topic() {
        let bus = make_bus();
        let msg = IncomingMessage {
            topic: "some/other/topic".to_string(),
            data: b"irrelevant".to_vec(),
            source: "peer_x".to_string(),
        };

        let result = bus.handle_incoming(msg).await;
        assert!(result.is_ok());
        assert!(!result.unwrap(), "Unknown topic must return false");
    }

    #[tokio::test]
    async fn test_handle_invalid_json_fails() {
        let bus = make_bus();
        let msg = IncomingMessage {
            topic: "ernmesh/chat/general".to_string(),
            data: b"not valid json".to_vec(),
            source: "peer_a".to_string(),
        };

        let result = bus.handle_incoming(msg).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_prepare_chat_message() {
        let bus = make_bus();
        let msg = ChatMessage::new_public(
            TopicId::new("general"),
            "peer_a",
            "hello",
            None,
        );

        let outgoing = bus.prepare_chat(&msg).unwrap();
        assert_eq!(outgoing.topic, "ernmesh/chat/general");
        assert!(!outgoing.data.is_empty());

        // Verify roundtrip
        let recovered: ChatMessage = serde_json::from_slice(&outgoing.data).unwrap();
        assert_eq!(recovered.content, "hello");
    }

    #[test]
    fn test_prepare_erniebook_post() {
        let bus = make_bus();
        let post = Post::new("peer_a", "My post!", None, vec!["intro".into()]);

        let outgoing = bus.prepare_post(&post).unwrap();
        assert_eq!(outgoing.topic, ERNIEBOOK_TOPIC);
        assert!(!outgoing.data.is_empty());

        let recovered: Post = serde_json::from_slice(&outgoing.data).unwrap();
        assert_eq!(recovered.content, "My post!");
    }

    #[tokio::test]
    async fn test_multiple_messages_accumulate() {
        let bus = make_bus();

        bus.handle_incoming(make_chat_msg("ernmesh/chat/general", "a", "msg1")).await.unwrap();
        bus.handle_incoming(make_chat_msg("ernmesh/chat/general", "b", "msg2")).await.unwrap();
        bus.handle_incoming(make_chat_msg("ernmesh/chat/dev", "c", "msg3")).await.unwrap();
        bus.handle_incoming(make_post_msg("d", "post1")).await.unwrap();

        let log = bus.chat_log().read().await;
        assert_eq!(log.total_messages(), 3);

        let feed = bus.feed().read().await;
        assert_eq!(feed.count(), 1);
    }
}
