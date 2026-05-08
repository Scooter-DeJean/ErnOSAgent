// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! MeshMail — decentralised email-like messaging.
//!
//! MeshMail provides asynchronous, store-and-forward messaging:
//!
//! - **Peer-addressed**: Messages are sent to a PeerId (base58).
//! - **Store-and-forward**: If the recipient is offline, their messages
//!   are stored by pinning nodes until they come online.
//! - **E2E encrypted**: All mail is encrypted with the recipient's
//!   session key. Only the recipient can read it.
//! - **Threaded**: Messages can form threads via `in_reply_to`.
//! - **Attachments**: File references (CIDs) can be attached.
//!
//! Mail is published to the `ernmesh/mail/{recipient_peer_id}` topic.
//! The recipient subscribes to their own mail topic.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::SystemTime;

/// A single mail message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailMessage {
    /// Unique message ID.
    pub message_id: String,
    /// Sender's PeerId.
    pub from: String,
    /// Recipient's PeerId.
    pub to: String,
    /// Subject line.
    pub subject: String,
    /// Body text.
    pub body: String,
    /// Optional thread parent message ID.
    pub in_reply_to: Option<String>,
    /// Attached file CIDs.
    pub attachments: Vec<String>,
    /// Timestamp.
    pub sent_at: SystemTime,
    /// Whether the body is encrypted.
    pub encrypted: bool,
    /// Whether this message has been read.
    #[serde(default)]
    pub read: bool,
}

impl MailMessage {
    /// Create a new plaintext mail message.
    pub fn new(from: &str, to: &str, subject: &str, body: &str) -> Self {
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let suffix: u32 = rand::random();

        Self {
            message_id: format!("{}-{}-{:08x}", from, ts, suffix),
            from: from.to_string(),
            to: to.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
            in_reply_to: None,
            attachments: Vec::new(),
            sent_at: SystemTime::now(),
            encrypted: false,
            read: false,
        }
    }

    /// Create a reply to an existing message.
    pub fn reply(original: &MailMessage, from: &str, body: &str) -> Self {
        let mut reply = Self::new(from, &original.from, &format!("Re: {}", original.subject), body);
        reply.in_reply_to = Some(original.message_id.clone());
        reply
    }

    /// Add an attachment CID.
    pub fn attach(&mut self, cid: &str) {
        self.attachments.push(cid.to_string());
    }

    /// Returns the Gossipsub topic for this mail's delivery.
    pub fn delivery_topic(&self) -> String {
        format!("ernmesh/mail/{}", self.to)
    }

    /// Mark this message as read.
    pub fn mark_read(&mut self) {
        self.read = true;
    }
}

/// Local mailbox — stores received messages.
#[derive(Debug)]
pub struct Mailbox {
    /// Inbox messages (newest first when iterated in reverse).
    inbox: VecDeque<MailMessage>,
    /// Sent messages.
    sent: VecDeque<MailMessage>,
    /// Maximum messages per folder.
    capacity: usize,
}

impl Mailbox {
    /// Create a new mailbox with the given capacity per folder.
    pub fn new(capacity: usize) -> Self {
        Self {
            inbox: VecDeque::with_capacity(capacity),
            sent: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Receive a message into the inbox.
    pub fn receive(&mut self, msg: MailMessage) {
        if self.inbox.len() >= self.capacity {
            self.inbox.pop_front();
        }

        tracing::info!(
            from = %msg.from,
            subject = %msg.subject,
            "MeshMail: message received"
        );

        self.inbox.push_back(msg);
    }

    /// Record a sent message.
    pub fn record_sent(&mut self, msg: MailMessage) {
        if self.sent.len() >= self.capacity {
            self.sent.pop_front();
        }
        self.sent.push_back(msg);
    }

    /// Get inbox messages.
    pub fn inbox(&self) -> &VecDeque<MailMessage> {
        &self.inbox
    }

    /// Get sent messages.
    pub fn sent(&self) -> &VecDeque<MailMessage> {
        &self.sent
    }

    /// Count of unread inbox messages.
    pub fn unread_count(&self) -> usize {
        self.inbox.iter().filter(|m| !m.read).count()
    }

    /// Count of total inbox messages.
    pub fn inbox_count(&self) -> usize {
        self.inbox.len()
    }

    /// Count of sent messages.
    pub fn sent_count(&self) -> usize {
        self.sent.len()
    }

    /// Get a message by ID from inbox.
    pub fn get_message(&self, message_id: &str) -> Option<&MailMessage> {
        self.inbox.iter().find(|m| m.message_id == message_id)
    }

    /// Get a mutable message by ID from inbox (for marking read).
    pub fn get_message_mut(&mut self, message_id: &str) -> Option<&mut MailMessage> {
        self.inbox.iter_mut().find(|m| m.message_id == message_id)
    }

    /// Delete a message from inbox by ID.
    pub fn delete_message(&mut self, message_id: &str) -> bool {
        if let Some(pos) = self.inbox.iter().position(|m| m.message_id == message_id) {
            self.inbox.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get all messages in a thread (by in_reply_to chain).
    pub fn thread(&self, root_message_id: &str) -> Vec<&MailMessage> {
        let mut thread = Vec::new();

        // Add the root message
        if let Some(root) = self.inbox.iter().find(|m| m.message_id == root_message_id) {
            thread.push(root);
        }

        // Add replies
        for msg in &self.inbox {
            if msg.in_reply_to.as_deref() == Some(root_message_id) {
                thread.push(msg);
            }
        }

        thread
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_message() {
        let msg = MailMessage::new("alice", "bob", "Hello", "Hi Bob!");
        assert_eq!(msg.from, "alice");
        assert_eq!(msg.to, "bob");
        assert_eq!(msg.subject, "Hello");
        assert_eq!(msg.body, "Hi Bob!");
        assert!(!msg.encrypted);
        assert!(!msg.read);
        assert!(msg.attachments.is_empty());
    }

    #[test]
    fn test_reply() {
        let original = MailMessage::new("alice", "bob", "Hello", "Hi!");
        let reply = MailMessage::reply(&original, "bob", "Hello back!");

        assert_eq!(reply.from, "bob");
        assert_eq!(reply.to, "alice");
        assert_eq!(reply.subject, "Re: Hello");
        assert_eq!(reply.in_reply_to, Some(original.message_id.clone()));
    }

    #[test]
    fn test_attach() {
        let mut msg = MailMessage::new("a", "b", "Files", "See attached");
        msg.attach("QmHash1");
        msg.attach("QmHash2");

        assert_eq!(msg.attachments.len(), 2);
    }

    #[test]
    fn test_delivery_topic() {
        let msg = MailMessage::new("alice", "bob", "Hi", "body");
        assert_eq!(msg.delivery_topic(), "ernmesh/mail/bob");
    }

    #[test]
    fn test_mailbox_receive() {
        let mut mb = Mailbox::new(100);
        mb.receive(MailMessage::new("alice", "me", "Hi", "Hello!"));

        assert_eq!(mb.inbox_count(), 1);
        assert_eq!(mb.unread_count(), 1);
    }

    #[test]
    fn test_mailbox_mark_read() {
        let mut mb = Mailbox::new(100);
        let msg = MailMessage::new("alice", "me", "Hi", "Hello!");
        let mid = msg.message_id.clone();
        mb.receive(msg);

        assert_eq!(mb.unread_count(), 1);

        mb.get_message_mut(&mid).unwrap().mark_read();
        assert_eq!(mb.unread_count(), 0);
    }

    #[test]
    fn test_mailbox_evicts_oldest() {
        let mut mb = Mailbox::new(3);

        for i in 0..5 {
            mb.receive(MailMessage::new("a", "me", &format!("msg{}", i), "body"));
        }

        assert_eq!(mb.inbox_count(), 3);
    }

    #[test]
    fn test_mailbox_delete_message() {
        let mut mb = Mailbox::new(100);
        let msg = MailMessage::new("a", "me", "Delete me", "body");
        let mid = msg.message_id.clone();
        mb.receive(msg);

        assert!(mb.delete_message(&mid));
        assert_eq!(mb.inbox_count(), 0);
    }

    #[test]
    fn test_mailbox_thread() {
        let mut mb = Mailbox::new(100);
        let original = MailMessage::new("alice", "me", "Thread", "Start");
        let oid = original.message_id.clone();
        mb.receive(original);

        let reply = {
            let orig = mb.get_message(&oid).unwrap();
            MailMessage::reply(orig, "me", "Reply!")
        };
        mb.receive(reply);

        let thread = mb.thread(&oid);
        assert_eq!(thread.len(), 2);
    }

    #[test]
    fn test_mailbox_sent() {
        let mut mb = Mailbox::new(100);
        mb.record_sent(MailMessage::new("me", "bob", "Out", "Outgoing"));

        assert_eq!(mb.sent_count(), 1);
        assert_eq!(mb.inbox_count(), 0);
    }

    #[test]
    fn test_message_ids_unique() {
        let m1 = MailMessage::new("a", "b", "s", "body");
        let m2 = MailMessage::new("a", "b", "s", "body");
        assert_ne!(m1.message_id, m2.message_id);
    }

    #[test]
    fn test_message_serialize_roundtrip() {
        let mut msg = MailMessage::new("alice", "bob", "Test", "Body");
        msg.attach("QmCid");

        let json = serde_json::to_string(&msg).unwrap();
        let recovered: MailMessage = serde_json::from_str(&json).unwrap();

        assert_eq!(recovered.from, "alice");
        assert_eq!(recovered.attachments.len(), 1);
    }
}
