// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! ErnMesh Offline Queue — store-and-forward messaging for offline peers.
//!
//! When a peer is not currently connected to the swarm, messages destined
//! for them are queued locally. On reconnection (swarm `ConnectionEstablished`
//! event), queued messages are flushed and delivered via Gossipsub.
//!
//! - **Per-peer queue**: Each target peer gets an independent FIFO queue.
//! - **Size limits**: Max messages per peer and max payload size enforced.
//! - **TTL expiry**: Messages older than 7 days are automatically pruned.
//! - **Persistence**: Saved/restored via `StateDir("offline_queue")`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// Maximum queued messages per target peer.
const DEFAULT_MAX_PER_PEER: usize = 1000;

/// Maximum payload size per message (10 KB).
const DEFAULT_MAX_PAYLOAD_BYTES: usize = 10_240;

/// Time-to-live for queued messages (7 days).
const DEFAULT_TTL_SECS: u64 = 7 * 24 * 60 * 60;

// ─── Data Structures ────────────────────────────────────────

/// A message waiting for its target peer to come online.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// Target peer's PeerId.
    pub target_peer: String,
    /// Gossipsub topic to publish this message to on delivery.
    pub topic: String,
    /// Serialised message payload (opaque bytes).
    pub payload: Vec<u8>,
    /// When this message was queued.
    pub queued_at: SystemTime,
}

/// Statistics for a single peer's queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStats {
    /// Number of messages queued for this peer.
    pub message_count: usize,
    /// Total payload bytes queued for this peer.
    pub total_bytes: usize,
    /// Oldest message timestamp.
    pub oldest: Option<SystemTime>,
}

// ─── OfflineQueue ───────────────────────────────────────────

/// Store-and-forward queue for offline peer message delivery.
#[derive(Debug, Serialize, Deserialize)]
pub struct OfflineQueue {
    queued: HashMap<String, Vec<QueuedMessage>>,
    #[serde(default = "default_max_per_peer")]
    max_per_peer: usize,
    #[serde(default = "default_max_payload")]
    max_payload_bytes: usize,
    #[serde(default = "default_ttl")]
    ttl_secs: u64,
}

fn default_max_per_peer() -> usize { DEFAULT_MAX_PER_PEER }
fn default_max_payload() -> usize { DEFAULT_MAX_PAYLOAD_BYTES }
fn default_ttl() -> u64 { DEFAULT_TTL_SECS }

impl OfflineQueue {
    /// Create a new empty offline queue with the given limits.
    pub fn new(max_per_peer: usize, max_payload_bytes: usize, ttl_secs: u64) -> Self {
        Self {
            queued: HashMap::new(),
            max_per_peer,
            max_payload_bytes,
            ttl_secs,
        }
    }

    /// Create with default limits.
    pub fn default_limits() -> Self {
        Self::new(DEFAULT_MAX_PER_PEER, DEFAULT_MAX_PAYLOAD_BYTES, DEFAULT_TTL_SECS)
    }

    /// Enqueue a message for an offline peer.
    ///
    /// Returns `Err` if the payload exceeds the size limit or the queue is full.
    pub fn enqueue(
        &mut self, target_peer: &str, topic: &str, payload: Vec<u8>,
    ) -> Result<usize, &'static str> {
        if payload.len() > self.max_payload_bytes {
            return Err("Payload exceeds maximum size limit");
        }
        let queue = self.queued.entry(target_peer.to_string()).or_default();
        if queue.len() >= self.max_per_peer {
            return Err("Queue full for this peer — oldest messages may need pruning");
        }
        let msg = QueuedMessage {
            target_peer: target_peer.to_string(),
            topic: topic.to_string(),
            payload,
            queued_at: SystemTime::now(),
        };
        queue.push(msg);
        let count = queue.len();
        tracing::debug!(
            target_peer, topic, queued = count,
            "Message queued for offline peer"
        );
        Ok(count)
    }

    /// Flush all queued messages for a peer (called on reconnection).
    ///
    /// Returns the messages to be delivered. They are removed from the queue.
    pub fn flush(&mut self, peer_id: &str) -> Vec<QueuedMessage> {
        let messages = self.queued.remove(peer_id).unwrap_or_default();
        if !messages.is_empty() {
            tracing::info!(
                peer_id, count = messages.len(),
                "Flushing offline queue — peer reconnected"
            );
        }
        messages
    }

    /// Peek at queued messages for a peer without removing them.
    pub fn peek(&self, peer_id: &str) -> &[QueuedMessage] {
        self.queued.get(peer_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Get the number of queued messages for a specific peer.
    pub fn count_for(&self, peer_id: &str) -> usize {
        self.queued.get(peer_id).map_or(0, |v| v.len())
    }

    /// Get the total number of queued messages across all peers.
    pub fn total_count(&self) -> usize {
        self.queued.values().map(|v| v.len()).sum()
    }

    /// Get the number of peers with queued messages.
    pub fn peer_count(&self) -> usize {
        self.queued.len()
    }

    /// Get statistics per peer.
    pub fn stats(&self) -> Vec<(String, QueueStats)> {
        self.queued.iter().map(|(peer, msgs)| {
            let stats = QueueStats {
                message_count: msgs.len(),
                total_bytes: msgs.iter().map(|m| m.payload.len()).sum(),
                oldest: msgs.first().map(|m| m.queued_at),
            };
            (peer.clone(), stats)
        }).collect()
    }

    /// Prune expired messages (older than TTL).
    ///
    /// Returns the number of messages pruned.
    pub fn prune_expired(&mut self) -> usize {
        let ttl = Duration::from_secs(self.ttl_secs);
        let now = SystemTime::now();
        let mut pruned = 0;
        self.queued.retain(|peer_id, msgs| {
            let before = msgs.len();
            msgs.retain(|m| {
                now.duration_since(m.queued_at).unwrap_or_default() < ttl
            });
            let removed = before - msgs.len();
            if removed > 0 {
                tracing::debug!(
                    peer_id, removed,
                    "Pruned expired messages from offline queue"
                );
            }
            pruned += removed;
            !msgs.is_empty()
        });
        if pruned > 0 {
            tracing::info!(pruned, "Offline queue TTL pruning complete");
        }
        pruned
    }

    /// Clear the entire queue for a specific peer.
    pub fn clear_peer(&mut self, peer_id: &str) -> usize {
        let count = self.queued.remove(peer_id)
            .map_or(0, |v| v.len());
        if count > 0 {
            tracing::info!(peer_id, cleared = count, "Offline queue cleared for peer");
        }
        count
    }

    /// Clear the entire queue.
    pub fn clear_all(&mut self) -> usize {
        let total = self.total_count();
        self.queued.clear();
        if total > 0 {
            tracing::info!(total, "Entire offline queue cleared");
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_queue_is_empty() {
        let q = OfflineQueue::default_limits();
        assert_eq!(q.total_count(), 0);
        assert_eq!(q.peer_count(), 0);
    }

    #[test]
    fn test_enqueue_and_count() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("peer_a", "topic/test", b"hello".to_vec()).unwrap();
        assert_eq!(q.total_count(), 1);
        assert_eq!(q.count_for("peer_a"), 1);
        assert_eq!(q.peer_count(), 1);
    }

    #[test]
    fn test_enqueue_multiple_peers() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("peer_a", "t1", b"a".to_vec()).unwrap();
        q.enqueue("peer_b", "t2", b"b".to_vec()).unwrap();
        assert_eq!(q.total_count(), 2);
        assert_eq!(q.peer_count(), 2);
    }

    #[test]
    fn test_flush_returns_and_removes() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("peer_c", "t", b"msg1".to_vec()).unwrap();
        q.enqueue("peer_c", "t", b"msg2".to_vec()).unwrap();

        let flushed = q.flush("peer_c");
        assert_eq!(flushed.len(), 2);
        assert_eq!(q.count_for("peer_c"), 0);
        assert_eq!(q.total_count(), 0);
    }

    #[test]
    fn test_flush_empty_peer() {
        let mut q = OfflineQueue::default_limits();
        let flushed = q.flush("nonexistent");
        assert!(flushed.is_empty());
    }

    #[test]
    fn test_payload_size_limit() {
        let mut q = OfflineQueue::new(100, 5, 86400); // max 5 bytes
        let result = q.enqueue("peer_d", "t", b"too_big_payload".to_vec());
        assert_eq!(result.unwrap_err(), "Payload exceeds maximum size limit");
        assert_eq!(q.total_count(), 0);
    }

    #[test]
    fn test_queue_full_limit() {
        let mut q = OfflineQueue::new(2, 1024, 86400); // max 2 per peer
        q.enqueue("peer_e", "t", b"1".to_vec()).unwrap();
        q.enqueue("peer_e", "t", b"2".to_vec()).unwrap();
        let result = q.enqueue("peer_e", "t", b"3".to_vec());
        assert!(result.is_err());
    }

    #[test]
    fn test_peek_does_not_remove() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("peer_f", "t", b"peek_me".to_vec()).unwrap();
        assert_eq!(q.peek("peer_f").len(), 1);
        assert_eq!(q.count_for("peer_f"), 1); // still there
    }

    #[test]
    fn test_clear_peer() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("peer_g", "t", b"1".to_vec()).unwrap();
        q.enqueue("peer_g", "t", b"2".to_vec()).unwrap();
        let cleared = q.clear_peer("peer_g");
        assert_eq!(cleared, 2);
        assert_eq!(q.total_count(), 0);
    }

    #[test]
    fn test_clear_all() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("p1", "t", b"a".to_vec()).unwrap();
        q.enqueue("p2", "t", b"b".to_vec()).unwrap();
        let cleared = q.clear_all();
        assert_eq!(cleared, 2);
        assert_eq!(q.total_count(), 0);
        assert_eq!(q.peer_count(), 0);
    }

    #[test]
    fn test_stats() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("peer_h", "t", b"hello".to_vec()).unwrap();
        let stats = q.stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].1.message_count, 1);
        assert_eq!(stats[0].1.total_bytes, 5);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut q = OfflineQueue::default_limits();
        q.enqueue("peer_i", "topic/x", b"persist".to_vec()).unwrap();
        let json = serde_json::to_string(&q).unwrap();
        let restored: OfflineQueue = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.total_count(), 1);
        assert_eq!(restored.count_for("peer_i"), 1);
    }
}
