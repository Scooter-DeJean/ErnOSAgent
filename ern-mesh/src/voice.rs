// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Voice channels — signalling, room management, and SFU coordination.
//!
//! ErnMesh voice is fully decentralised:
//!
//! - **Small rooms (≤4)**: Direct peer-to-peer WebRTC connections.
//!   Each participant sends audio directly to every other participant.
//! - **Large rooms (>4)**: A Super-Node SFU (Selective Forwarding Unit)
//!   is elected from volunteers. Participants send audio to the SFU,
//!   which forwards it to all others. SFU operators earn elevated ErnPoints.
//! - **E2E encrypted**: Audio frames are encrypted with a per-room
//!   group key derived from the room creator's session keys.
//! - **Capability-gated**: Joining a voice channel requires the
//!   `PublishTopics` capability from the room host.
//!
//! This module handles signalling (join/leave/mute) and room state.
//! Actual audio transport uses the platform's WebRTC stack.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

/// A voice channel room.
#[derive(Debug, Clone)]
pub struct VoiceRoom {
    /// Unique room ID.
    pub room_id: String,
    /// The PeerId that created the room.
    pub creator: String,
    /// Human-readable room name.
    pub name: String,
    /// Connected participants.
    participants: HashSet<String>,
    /// Muted participants.
    muted: HashSet<String>,
    /// Deafened participants (not receiving audio).
    deafened: HashSet<String>,
    /// Maximum participants (from config).
    pub max_participants: u32,
    /// When the room was created.
    pub created_at: SystemTime,
    /// Threshold for switching to SFU mode.
    pub sfu_threshold: u32,
    /// PeerId of the active SFU (if in SFU mode).
    pub active_sfu: Option<String>,
}

impl VoiceRoom {
    /// Create a new voice room.
    pub fn new(
        room_id: &str,
        creator: &str,
        name: &str,
        max_participants: u32,
        sfu_threshold: u32,
    ) -> Self {
        tracing::info!(
            room_id = room_id,
            creator = creator,
            name = name,
            max = max_participants,
            "Voice room created"
        );

        Self {
            room_id: room_id.to_string(),
            creator: creator.to_string(),
            name: name.to_string(),
            participants: HashSet::new(),
            muted: HashSet::new(),
            deafened: HashSet::new(),
            max_participants,
            created_at: SystemTime::now(),
            sfu_threshold,
            active_sfu: None,
        }
    }

    /// Join the room. Returns `Err` if full.
    pub fn join(&mut self, peer_id: &str) -> anyhow::Result<()> {
        if self.participants.len() as u32 >= self.max_participants {
            anyhow::bail!(
                "Room '{}' is full ({}/{})",
                self.room_id,
                self.participants.len(),
                self.max_participants
            );
        }

        self.participants.insert(peer_id.to_string());

        tracing::info!(
            room = %self.room_id,
            peer = peer_id,
            count = self.participants.len(),
            "Peer joined voice room"
        );

        Ok(())
    }

    /// Leave the room.
    pub fn leave(&mut self, peer_id: &str) {
        self.participants.remove(peer_id);
        self.muted.remove(peer_id);
        self.deafened.remove(peer_id);

        // If the SFU left, clear the SFU assignment
        if self.active_sfu.as_deref() == Some(peer_id) {
            self.active_sfu = None;
        }

        tracing::info!(
            room = %self.room_id,
            peer = peer_id,
            count = self.participants.len(),
            "Peer left voice room"
        );
    }

    /// Toggle mute for a participant.
    pub fn toggle_mute(&mut self, peer_id: &str) -> bool {
        if self.muted.contains(peer_id) {
            self.muted.remove(peer_id);
            false
        } else {
            self.muted.insert(peer_id.to_string());
            true
        }
    }

    /// Toggle deafen for a participant.
    pub fn toggle_deafen(&mut self, peer_id: &str) -> bool {
        if self.deafened.contains(peer_id) {
            self.deafened.remove(peer_id);
            false
        } else {
            self.deafened.insert(peer_id.to_string());
            true
        }
    }

    /// Check if a peer is in the room.
    pub fn contains(&self, peer_id: &str) -> bool {
        self.participants.contains(peer_id)
    }

    /// Check if a peer is muted.
    pub fn is_muted(&self, peer_id: &str) -> bool {
        self.muted.contains(peer_id)
    }

    /// Check if a peer is deafened.
    pub fn is_deafened(&self, peer_id: &str) -> bool {
        self.deafened.contains(peer_id)
    }

    /// Number of participants.
    pub fn participant_count(&self) -> usize {
        self.participants.len()
    }

    /// Whether the room needs an SFU (participant count exceeds threshold).
    pub fn needs_sfu(&self) -> bool {
        self.participants.len() as u32 > self.sfu_threshold
    }

    /// Whether the room is empty.
    pub fn is_empty(&self) -> bool {
        self.participants.is_empty()
    }

    /// List all participant PeerIds.
    pub fn participants(&self) -> Vec<&str> {
        self.participants.iter().map(|s| s.as_str()).collect()
    }

    /// Assign an SFU for this room.
    pub fn assign_sfu(&mut self, sfu_peer_id: &str) {
        self.active_sfu = Some(sfu_peer_id.to_string());
        tracing::info!(
            room = %self.room_id,
            sfu = sfu_peer_id,
            "SFU assigned to voice room"
        );
    }
}

/// A voice signalling message — sent via Gossipsub.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VoiceSignal {
    /// Peer wants to join a room.
    Join { room_id: String, peer_id: String },
    /// Peer is leaving a room.
    Leave { room_id: String, peer_id: String },
    /// Peer toggled mute.
    Mute { room_id: String, peer_id: String, muted: bool },
    /// Peer toggled deafen.
    Deafen { room_id: String, peer_id: String, deafened: bool },
    /// SDP offer for WebRTC connection establishment.
    SdpOffer { room_id: String, from: String, to: String, sdp: String },
    /// SDP answer for WebRTC connection establishment.
    SdpAnswer { room_id: String, from: String, to: String, sdp: String },
    /// ICE candidate exchange.
    IceCandidate { room_id: String, from: String, to: String, candidate: String },
}

impl VoiceSignal {
    /// The Gossipsub topic for voice signalling.
    pub fn topic(room_id: &str) -> String {
        format!("ernmesh/voice/{}", room_id)
    }
}

/// Voice room manager — tracks all active voice rooms.
#[derive(Debug, Default)]
pub struct VoiceManager {
    /// Active rooms indexed by room_id.
    rooms: HashMap<String, VoiceRoom>,
}

impl VoiceManager {
    /// Create a new voice manager.
    pub fn new() -> Self {
        Self {
            rooms: HashMap::new(),
        }
    }

    /// Create a new voice room.
    pub fn create_room(
        &mut self,
        room_id: &str,
        creator: &str,
        name: &str,
        max_participants: u32,
        sfu_threshold: u32,
    ) -> &VoiceRoom {
        self.rooms.insert(
            room_id.to_string(),
            VoiceRoom::new(room_id, creator, name, max_participants, sfu_threshold),
        );
        self.rooms.get(room_id).unwrap()
    }

    /// Get a room by ID.
    pub fn get_room(&self, room_id: &str) -> Option<&VoiceRoom> {
        self.rooms.get(room_id)
    }

    /// Get a mutable room by ID.
    pub fn get_room_mut(&mut self, room_id: &str) -> Option<&mut VoiceRoom> {
        self.rooms.get_mut(room_id)
    }

    /// Remove a room (cleanup when empty).
    pub fn remove_room(&mut self, room_id: &str) -> bool {
        if self.rooms.remove(room_id).is_some() {
            tracing::info!(room = room_id, "Voice room removed");
            true
        } else {
            false
        }
    }

    /// Cleanup all empty rooms.
    pub fn cleanup_empty(&mut self) -> usize {
        let empties: Vec<String> = self.rooms.iter()
            .filter(|(_, r)| r.is_empty())
            .map(|(id, _)| id.clone())
            .collect();

        let count = empties.len();
        for id in empties {
            self.rooms.remove(&id);
        }

        if count > 0 {
            tracing::info!(removed = count, "Cleaned up empty voice rooms");
        }

        count
    }

    /// List all active room IDs.
    pub fn list_rooms(&self) -> Vec<&str> {
        self.rooms.keys().map(|s| s.as_str()).collect()
    }

    /// Total active rooms.
    pub fn room_count(&self) -> usize {
        self.rooms.len()
    }

    /// Total participants across all rooms.
    pub fn total_participants(&self) -> usize {
        self.rooms.values().map(|r| r.participant_count()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_room() {
        let room = VoiceRoom::new("room-1", "alice", "General", 10, 4);
        assert_eq!(room.room_id, "room-1");
        assert_eq!(room.creator, "alice");
        assert!(room.is_empty());
        assert_eq!(room.max_participants, 10);
    }

    #[test]
    fn test_join_leave() {
        let mut room = VoiceRoom::new("r1", "alice", "Room", 10, 4);
        room.join("alice").unwrap();
        room.join("bob").unwrap();

        assert_eq!(room.participant_count(), 2);
        assert!(room.contains("alice"));

        room.leave("alice");
        assert_eq!(room.participant_count(), 1);
        assert!(!room.contains("alice"));
    }

    #[test]
    fn test_join_full_room_fails() {
        let mut room = VoiceRoom::new("r1", "alice", "Room", 2, 4);
        room.join("alice").unwrap();
        room.join("bob").unwrap();

        let result = room.join("charlie");
        assert!(result.is_err());
    }

    #[test]
    fn test_mute_toggle() {
        let mut room = VoiceRoom::new("r1", "a", "R", 10, 4);
        room.join("alice").unwrap();

        assert!(!room.is_muted("alice"));

        let muted = room.toggle_mute("alice");
        assert!(muted);
        assert!(room.is_muted("alice"));

        let unmuted = room.toggle_mute("alice");
        assert!(!unmuted);
        assert!(!room.is_muted("alice"));
    }

    #[test]
    fn test_deafen_toggle() {
        let mut room = VoiceRoom::new("r1", "a", "R", 10, 4);
        room.join("alice").unwrap();

        let deafened = room.toggle_deafen("alice");
        assert!(deafened);
        assert!(room.is_deafened("alice"));
    }

    #[test]
    fn test_leave_clears_mute_deafen() {
        let mut room = VoiceRoom::new("r1", "a", "R", 10, 4);
        room.join("alice").unwrap();
        room.toggle_mute("alice");
        room.toggle_deafen("alice");

        room.leave("alice");
        assert!(!room.is_muted("alice"));
        assert!(!room.is_deafened("alice"));
    }

    #[test]
    fn test_needs_sfu() {
        let mut room = VoiceRoom::new("r1", "a", "R", 10, 3);

        for i in 0..3 {
            room.join(&format!("peer_{}", i)).unwrap();
        }
        assert!(!room.needs_sfu());

        room.join("peer_extra").unwrap();
        assert!(room.needs_sfu());
    }

    #[test]
    fn test_assign_sfu() {
        let mut room = VoiceRoom::new("r1", "a", "R", 10, 4);
        assert!(room.active_sfu.is_none());

        room.assign_sfu("sfu_peer");
        assert_eq!(room.active_sfu.as_deref(), Some("sfu_peer"));
    }

    #[test]
    fn test_sfu_clears_on_leave() {
        let mut room = VoiceRoom::new("r1", "a", "R", 10, 4);
        room.join("sfu_peer").unwrap();
        room.assign_sfu("sfu_peer");

        room.leave("sfu_peer");
        assert!(room.active_sfu.is_none());
    }

    #[test]
    fn test_voice_manager_create() {
        let mut mgr = VoiceManager::new();
        mgr.create_room("r1", "alice", "General", 10, 4);

        assert_eq!(mgr.room_count(), 1);
        assert!(mgr.get_room("r1").is_some());
    }

    #[test]
    fn test_voice_manager_cleanup_empty() {
        let mut mgr = VoiceManager::new();
        mgr.create_room("r1", "alice", "Empty", 10, 4);
        mgr.create_room("r2", "bob", "Active", 10, 4);
        mgr.get_room_mut("r2").unwrap().join("bob").unwrap();

        let cleaned = mgr.cleanup_empty();
        assert_eq!(cleaned, 1);
        assert_eq!(mgr.room_count(), 1);
    }

    #[test]
    fn test_voice_signal_topic() {
        assert_eq!(VoiceSignal::topic("room-1"), "ernmesh/voice/room-1");
    }

    #[test]
    fn test_voice_signal_serializable() {
        let signal = VoiceSignal::Join {
            room_id: "r1".into(),
            peer_id: "alice".into(),
        };
        let json = serde_json::to_string(&signal).unwrap();
        let recovered: VoiceSignal = serde_json::from_str(&json).unwrap();
        match recovered {
            VoiceSignal::Join { room_id, peer_id } => {
                assert_eq!(room_id, "r1");
                assert_eq!(peer_id, "alice");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_total_participants() {
        let mut mgr = VoiceManager::new();
        mgr.create_room("r1", "a", "R1", 10, 4);
        mgr.create_room("r2", "b", "R2", 10, 4);

        mgr.get_room_mut("r1").unwrap().join("a").unwrap();
        mgr.get_room_mut("r1").unwrap().join("b").unwrap();
        mgr.get_room_mut("r2").unwrap().join("c").unwrap();

        assert_eq!(mgr.total_participants(), 3);
    }
}
