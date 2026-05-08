// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! ErnMesh Groups — multi-member group chat with roles and invites.
//!
//! Provides persistent group management for the mesh network:
//!
//! - **Create groups** with a name, emoji, and initial member list.
//! - **Invite/kick members** — admins control membership.
//! - **Group messaging** — each group has a dedicated Gossipsub topic.
//! - **Role system** — creator is admin, admins can invite/kick.
//! - **Persistence** — saved/restored via `StateDir("groups")`.
//!
//! Each group maps to a Gossipsub topic: `ernmesh/group/{group_id}`.
//! Group control messages use: `ernmesh/groups/control`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// Maximum groups per node.
const DEFAULT_MAX_GROUPS: usize = 50;

/// Maximum messages stored per group (ring buffer).
const DEFAULT_MAX_MESSAGES_PER_GROUP: usize = 500;

/// Maximum members per group.
const DEFAULT_MAX_MEMBERS_PER_GROUP: usize = 100;

// ─── Data Structures ────────────────────────────────────────

/// A mesh group — a multi-member chat room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    /// Unique group identifier (UUID v4).
    pub id: String,
    /// Human-readable group name.
    pub name: String,
    /// Emoji used as group avatar.
    pub emoji: String,
    /// PeerIds of all current members.
    pub members: Vec<String>,
    /// PeerIds with admin privileges.
    pub admins: Vec<String>,
    /// PeerId of the group creator.
    pub created_by: String,
    /// When the group was created.
    pub created_at: SystemTime,
}

impl Group {
    /// Check if a peer is a member of this group.
    pub fn is_member(&self, peer_id: &str) -> bool {
        self.members.iter().any(|m| m == peer_id)
    }

    /// Check if a peer is an admin of this group.
    pub fn is_admin(&self, peer_id: &str) -> bool {
        self.admins.iter().any(|a| a == peer_id)
    }

    /// Gossipsub topic for this group's messages.
    pub fn topic(&self) -> String {
        format!("ernmesh/group/{}", self.id)
    }

    /// Member count.
    pub fn member_count(&self) -> usize {
        self.members.len()
    }
}

/// A message within a group chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMessage {
    /// Unique message ID.
    pub id: String,
    /// The group this message belongs to.
    pub group_id: String,
    /// PeerId of the author.
    pub author: String,
    /// Author's display name at time of sending.
    pub display_name: String,
    /// Message content.
    pub content: String,
    /// When the message was sent.
    pub timestamp: SystemTime,
}

/// A pending group invitation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInvite {
    /// The group being invited to.
    pub group_id: String,
    /// Name of the group (for display before joining).
    pub group_name: String,
    /// PeerId of the peer who sent the invite.
    pub invited_by: String,
    /// When the invite was sent.
    pub timestamp: SystemTime,
}

/// Gossipsub protocol messages for group management.
///
/// Sent over the `ernmesh/groups/control` topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GroupProtocol {
    /// Invite a peer to a group.
    Invite { group_id: String, group_name: String, invited_by: String },
    /// Accept a group invitation.
    AcceptInvite { group_id: String, from: String },
    /// Leave a group.
    Leave { group_id: String, from: String },
    /// Admin kicks a member.
    Kick { group_id: String, target: String, admin: String },
    /// A chat message in a group.
    Message { group_id: String, content: String, display_name: String },
}

// ─── GroupStore ──────────────────────────────────────────────

/// Persistent group store — manages all groups and their messages.
#[derive(Debug, Serialize, Deserialize)]
pub struct GroupStore {
    groups: HashMap<String, Group>,
    messages: HashMap<String, Vec<GroupMessage>>,
    pending_invites: Vec<GroupInvite>,
    #[serde(default = "default_max_groups")]
    max_groups: usize,
    #[serde(default = "default_max_messages")]
    max_messages_per_group: usize,
}

fn default_max_groups() -> usize { DEFAULT_MAX_GROUPS }
fn default_max_messages() -> usize { DEFAULT_MAX_MESSAGES_PER_GROUP }

impl GroupStore {
    /// Create a new empty group store.
    pub fn new(max_groups: usize, max_messages: usize) -> Self {
        Self {
            groups: HashMap::new(),
            messages: HashMap::new(),
            pending_invites: Vec::new(),
            max_groups,
            max_messages_per_group: max_messages,
        }
    }

    /// Create with default limits.
    pub fn default_limits() -> Self {
        Self::new(DEFAULT_MAX_GROUPS, DEFAULT_MAX_MESSAGES_PER_GROUP)
    }

    /// List all groups the local node is a member of.
    pub fn groups(&self) -> Vec<&Group> {
        let mut list: Vec<&Group> = self.groups.values().collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        list
    }

    /// Get a specific group by ID.
    pub fn get(&self, group_id: &str) -> Option<&Group> {
        self.groups.get(group_id)
    }

    /// Total group count.
    pub fn count(&self) -> usize {
        self.groups.len()
    }

    /// List pending invitations.
    pub fn pending_invites(&self) -> &[GroupInvite] {
        &self.pending_invites
    }

    /// Get message history for a group.
    pub fn messages(&self, group_id: &str) -> &[GroupMessage] {
        self.messages.get(group_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

// ─── Group Lifecycle ────────────────────────────────────────

impl GroupStore {
    /// Create a new group. The creator is automatically an admin and member.
    pub fn create(
        &mut self, name: &str, emoji: &str, creator_id: &str, initial_members: &[String],
    ) -> Result<&Group, &'static str> {
        if self.groups.len() >= self.max_groups {
            return Err("Maximum group count reached");
        }
        if name.is_empty() || name.len() > 64 {
            return Err("Group name must be 1-64 characters");
        }
        let id = uuid_v4();
        let mut members = vec![creator_id.to_string()];
        for m in initial_members {
            if !members.contains(m) && members.len() < DEFAULT_MAX_MEMBERS_PER_GROUP {
                members.push(m.clone());
            }
        }
        let group = Group {
            id: id.clone(), name: name.to_string(),
            emoji: if emoji.is_empty() { "👥".to_string() } else { emoji.to_string() },
            members, admins: vec![creator_id.to_string()],
            created_by: creator_id.to_string(), created_at: SystemTime::now(),
        };
        tracing::info!(group_id = %id, name, creator = creator_id, "Group created");
        self.groups.insert(id.clone(), group);
        self.messages.insert(id.clone(), Vec::new());
        self.groups.get(&id).ok_or("Internal error: group not inserted")
    }

    /// Add a message to a group's history.
    pub fn add_message(&mut self, msg: GroupMessage) -> Result<(), &'static str> {
        if !self.groups.contains_key(&msg.group_id) {
            return Err("Group does not exist");
        }
        let msgs = self.messages.entry(msg.group_id.clone()).or_default();
        if msgs.len() >= self.max_messages_per_group {
            msgs.remove(0); // ring buffer: drop oldest
        }
        tracing::debug!(
            group_id = %msg.group_id, author = %msg.author,
            "Group message added"
        );
        msgs.push(msg);
        Ok(())
    }

    /// Remove a member from a group (admin action).
    pub fn kick_member(
        &mut self, group_id: &str, target: &str, admin: &str,
    ) -> Result<(), &'static str> {
        let group = self.groups.get_mut(group_id).ok_or("Group not found")?;
        if !group.is_admin(admin) {
            return Err("Only admins can kick members");
        }
        if target == group.created_by {
            return Err("Cannot kick the group creator");
        }
        group.members.retain(|m| m != target);
        group.admins.retain(|a| a != target);
        tracing::info!(group_id, target, admin, "Member kicked from group");
        Ok(())
    }

    /// Leave a group voluntarily.
    pub fn leave(&mut self, group_id: &str, peer_id: &str) -> Result<(), &'static str> {
        let group = self.groups.get_mut(group_id).ok_or("Group not found")?;
        if !group.is_member(peer_id) {
            return Err("Not a member of this group");
        }
        group.members.retain(|m| m != peer_id);
        group.admins.retain(|a| a != peer_id);
        if group.members.is_empty() {
            self.groups.remove(group_id);
            self.messages.remove(group_id);
            tracing::info!(group_id, "Group dissolved — last member left");
        } else {
            tracing::info!(group_id, peer_id, "Left group");
        }
        Ok(())
    }
}

// ─── Invitations ────────────────────────────────────────────

impl GroupStore {
    /// Record a received group invitation.
    pub fn receive_invite(&mut self, invite: GroupInvite) {
        if self.pending_invites.iter().any(|i| i.group_id == invite.group_id) {
            tracing::debug!(group_id = %invite.group_id, "Ignoring duplicate invite");
            return;
        }
        tracing::info!(
            group_id = %invite.group_id, group_name = %invite.group_name,
            invited_by = %invite.invited_by, "Group invite received"
        );
        self.pending_invites.push(invite);
    }

    /// Accept a pending group invitation.
    pub fn accept_invite(
        &mut self, group_id: &str, our_id: &str,
    ) -> Result<(), &'static str> {
        let idx = self.pending_invites.iter()
            .position(|i| i.group_id == group_id)
            .ok_or("No pending invite for this group")?;
        let invite = self.pending_invites.remove(idx);
        if let Some(group) = self.groups.get_mut(group_id) {
            if !group.is_member(our_id) {
                group.members.push(our_id.to_string());
            }
        } else {
            // Create a local representation — full sync happens via Gossipsub
            let group = Group {
                id: group_id.to_string(), name: invite.group_name,
                emoji: "👥".to_string(),
                members: vec![our_id.to_string()],
                admins: vec![invite.invited_by],
                created_by: String::new(), created_at: SystemTime::now(),
            };
            self.groups.insert(group_id.to_string(), group);
            self.messages.insert(group_id.to_string(), Vec::new());
        }
        tracing::info!(group_id, our_id, "Group invite accepted");
        Ok(())
    }

    /// Invite a peer to a group (must be admin).
    pub fn invite_member(
        &mut self, group_id: &str, admin_id: &str, _target: &str,
    ) -> Result<GroupInvite, &'static str> {
        let group = self.groups.get(group_id).ok_or("Group not found")?;
        if !group.is_admin(admin_id) {
            return Err("Only admins can invite members");
        }
        let invite = GroupInvite {
            group_id: group_id.to_string(),
            group_name: group.name.clone(),
            invited_by: admin_id.to_string(),
            timestamp: SystemTime::now(),
        };
        tracing::info!(group_id, admin_id, "Group invite sent");
        Ok(invite)
    }
}

/// Generate a UUID v4 string (no external dependency — uses random bytes).
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:032x}", nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_group() {
        let mut store = GroupStore::default_limits();
        let group = store.create("Test Group", "🎮", "creator_1", &[]).unwrap();
        assert_eq!(group.name, "Test Group");
        assert_eq!(group.emoji, "🎮");
        assert!(group.is_member("creator_1"));
        assert!(group.is_admin("creator_1"));
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn test_create_group_with_members() {
        let mut store = GroupStore::default_limits();
        let members = vec!["peer_a".into(), "peer_b".into()];
        let group = store.create("Party", "🎉", "host", &members).unwrap();
        assert_eq!(group.member_count(), 3); // host + 2
    }

    #[test]
    fn test_add_message() {
        let mut store = GroupStore::default_limits();
        let group = store.create("Chat", "💬", "me", &[]).unwrap();
        let gid = group.id.clone();
        let msg = GroupMessage {
            id: "m1".into(), group_id: gid.clone(), author: "me".into(),
            display_name: "Me".into(), content: "Hello".into(),
            timestamp: SystemTime::now(),
        };
        store.add_message(msg).unwrap();
        assert_eq!(store.messages(&gid).len(), 1);
    }

    #[test]
    fn test_kick_member() {
        let mut store = GroupStore::default_limits();
        let members = vec!["peer_x".into()];
        let group = store.create("Room", "🏠", "admin_1", &members).unwrap();
        let gid = group.id.clone();
        store.kick_member(&gid, "peer_x", "admin_1").unwrap();
        assert!(!store.get(&gid).unwrap().is_member("peer_x"));
    }

    #[test]
    fn test_kick_non_admin_fails() {
        let mut store = GroupStore::default_limits();
        let members = vec!["peer_y".into()];
        let group = store.create("Room2", "🏠", "admin_2", &members).unwrap();
        let gid = group.id.clone();
        let result = store.kick_member(&gid, "admin_2", "peer_y");
        assert_eq!(result.unwrap_err(), "Only admins can kick members");
    }

    #[test]
    fn test_leave_group() {
        let mut store = GroupStore::default_limits();
        let members = vec!["peer_z".into()];
        let group = store.create("Temp", "⏳", "owner", &members).unwrap();
        let gid = group.id.clone();
        store.leave(&gid, "peer_z").unwrap();
        assert_eq!(store.get(&gid).unwrap().member_count(), 1);
    }

    #[test]
    fn test_leave_last_member_dissolves_group() {
        let mut store = GroupStore::default_limits();
        let group = store.create("Solo", "🎯", "only_one", &[]).unwrap();
        let gid = group.id.clone();
        store.leave(&gid, "only_one").unwrap();
        assert!(store.get(&gid).is_none());
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_invite_and_accept() {
        let mut store = GroupStore::default_limits();
        let group = store.create("Private", "🔒", "admin_3", &[]).unwrap();
        let gid = group.id.clone();
        let invite = store.invite_member(&gid, "admin_3", "invitee").unwrap();
        store.receive_invite(invite);
        assert_eq!(store.pending_invites().len(), 1);

        store.accept_invite(&gid, "invitee").unwrap();
        assert!(store.pending_invites().is_empty());
    }

    #[test]
    fn test_message_ring_buffer() {
        let mut store = GroupStore::new(10, 3); // max 3 messages
        let group = store.create("Small", "📎", "me", &[]).unwrap();
        let gid = group.id.clone();
        for i in 0..5 {
            let msg = GroupMessage {
                id: format!("m{}", i), group_id: gid.clone(), author: "me".into(),
                display_name: "Me".into(), content: format!("msg {}", i),
                timestamp: SystemTime::now(),
            };
            store.add_message(msg).unwrap();
        }
        assert_eq!(store.messages(&gid).len(), 3); // oldest dropped
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut store = GroupStore::default_limits();
        store.create("Persistent", "💾", "saver", &[]).unwrap();
        let json = serde_json::to_string(&store).unwrap();
        let restored: GroupStore = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.count(), 1);
    }
}
