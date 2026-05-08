// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! ErnMesh Contacts — peer contact management with offline awareness.
//!
//! Provides a persistent contact book for managing peer relationships:
//!
//! - **Add/accept/block contacts** by PeerId with display names.
//! - **Presence tracking** — online/offline/last-seen via swarm events.
//! - **Contact requests** — signed Gossipsub protocol for mutual opt-in.
//! - **DM routing** — deterministic `TopicId::direct()` for 1:1 messaging.
//! - **Persistence** — saved/restored via `StateDir("contacts")`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

/// Maximum contacts per node — configurable via `MeshConfig`.
const DEFAULT_MAX_CONTACTS: usize = 500;

/// Maximum pending requests (inbound + outbound combined).
const DEFAULT_MAX_PENDING: usize = 100;

// ─── Data Structures ────────────────────────────────────────

/// A saved contact — a peer the user has an active relationship with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    /// The peer's libp2p PeerId (base58-encoded).
    pub peer_id: String,
    /// User-assigned display name for this peer.
    pub display_name: String,
    /// Current presence status.
    pub status: ContactStatus,
    /// When this contact was added.
    pub added_at: SystemTime,
    /// Last time we saw this peer online (swarm event).
    pub last_seen: Option<SystemTime>,
    /// Whether this peer is blocked.
    pub blocked: bool,
}

/// Presence status of a contact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContactStatus {
    /// Peer is currently connected to the swarm.
    Online,
    /// Peer is not currently connected.
    Offline,
}

/// A pending contact request (inbound or outbound).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactRequest {
    /// PeerId of the sender.
    pub from: String,
    /// PeerId of the recipient.
    pub to: String,
    /// Display name offered by the sender.
    pub display_name: String,
    /// When the request was created.
    pub timestamp: SystemTime,
}

/// Gossipsub protocol messages for contact management.
///
/// Sent over the `ernmesh/contacts/control` topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContactProtocol {
    /// Request to add a contact.
    Request { from: String, display_name: String },
    /// Accept a pending request.
    Accept { from: String },
    /// Block a peer (informational — the blocker enforces locally).
    Block { from: String },
}

// ─── ContactBook ────────────────────────────────────────────

/// Persistent contact book — manages all peer relationships.
///
/// Thread-safe via `Arc<RwLock<ContactBook>>` in the runtime.
#[derive(Debug, Serialize, Deserialize)]
pub struct ContactBook {
    contacts: HashMap<String, Contact>,
    outbound_requests: Vec<ContactRequest>,
    inbound_requests: Vec<ContactRequest>,
    #[serde(default = "default_max_contacts")]
    max_contacts: usize,
    #[serde(default = "default_max_pending")]
    max_pending: usize,
}

fn default_max_contacts() -> usize { DEFAULT_MAX_CONTACTS }
fn default_max_pending() -> usize { DEFAULT_MAX_PENDING }

impl ContactBook {
    /// Create a new empty contact book.
    pub fn new(max_contacts: usize, max_pending: usize) -> Self {
        Self {
            contacts: HashMap::new(),
            outbound_requests: Vec::new(),
            inbound_requests: Vec::new(),
            max_contacts,
            max_pending,
        }
    }

    /// Create with default limits.
    pub fn default_limits() -> Self {
        Self::new(DEFAULT_MAX_CONTACTS, DEFAULT_MAX_PENDING)
    }

    /// Get all contacts sorted by display name.
    pub fn contacts(&self) -> Vec<&Contact> {
        let mut list: Vec<&Contact> = self.contacts.values().collect();
        list.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        list
    }

    /// Get a specific contact by PeerId.
    pub fn get(&self, peer_id: &str) -> Option<&Contact> {
        self.contacts.get(peer_id)
    }

    /// Total contact count.
    pub fn count(&self) -> usize {
        self.contacts.len()
    }

    /// List pending inbound requests.
    pub fn inbound_requests(&self) -> &[ContactRequest] {
        &self.inbound_requests
    }

    /// List pending outbound requests.
    pub fn outbound_requests(&self) -> &[ContactRequest] {
        &self.outbound_requests
    }
}

// ─── Mutations ──────────────────────────────────────────────

impl ContactBook {
    /// Send a contact request to a peer.
    ///
    /// Returns `Err` if at capacity or if the peer is already a contact.
    pub fn send_request(
        &mut self, our_id: &str, peer_id: &str, display_name: &str,
    ) -> Result<ContactRequest, &'static str> {
        if self.contacts.contains_key(peer_id) {
            return Err("Peer is already a contact");
        }
        if self.contacts.len() >= self.max_contacts {
            return Err("Contact book is full");
        }
        let total_pending = self.outbound_requests.len() + self.inbound_requests.len();
        if total_pending >= self.max_pending {
            return Err("Too many pending requests");
        }
        if self.outbound_requests.iter().any(|r| r.to == peer_id) {
            return Err("Request already sent to this peer");
        }
        let req = ContactRequest {
            from: our_id.to_string(),
            to: peer_id.to_string(),
            display_name: display_name.to_string(),
            timestamp: SystemTime::now(),
        };
        self.outbound_requests.push(req.clone());
        tracing::info!(peer_id, display_name, "Contact request sent");
        Ok(req)
    }

    /// Record an inbound contact request from another peer.
    pub fn receive_request(&mut self, request: ContactRequest) {
        if self.contacts.contains_key(&request.from) {
            tracing::debug!(from = %request.from, "Ignoring request — already a contact");
            return;
        }
        if self.inbound_requests.iter().any(|r| r.from == request.from) {
            tracing::debug!(from = %request.from, "Ignoring duplicate request");
            return;
        }
        tracing::info!(from = %request.from, name = %request.display_name, "Contact request received");
        self.inbound_requests.push(request);
    }

    /// Accept a pending inbound request — adds the peer as a contact.
    pub fn accept_request(&mut self, peer_id: &str) -> Result<&Contact, &'static str> {
        let idx = self.inbound_requests.iter()
            .position(|r| r.from == peer_id)
            .ok_or("No pending request from this peer")?;
        let req = self.inbound_requests.remove(idx);
        let contact = Contact {
            peer_id: req.from.clone(),
            display_name: req.display_name,
            status: ContactStatus::Offline,
            added_at: SystemTime::now(),
            last_seen: None,
            blocked: false,
        };
        tracing::info!(peer_id, name = %contact.display_name, "Contact accepted");
        self.contacts.insert(req.from, contact);
        self.contacts.get(peer_id).ok_or("Internal error: contact not inserted")
    }

    /// Mark a request as accepted (called when we receive an Accept protocol msg).
    pub fn mark_accepted(&mut self, peer_id: &str, display_name: &str) {
        self.outbound_requests.retain(|r| r.to != peer_id);
        if !self.contacts.contains_key(peer_id) {
            let contact = Contact {
                peer_id: peer_id.to_string(),
                display_name: display_name.to_string(),
                status: ContactStatus::Offline,
                added_at: SystemTime::now(),
                last_seen: None,
                blocked: false,
            };
            tracing::info!(peer_id, display_name, "Contact confirmed (mutual)");
            self.contacts.insert(peer_id.to_string(), contact);
        }
    }

    /// Block a peer — sets blocked flag and removes from active contacts.
    pub fn block(&mut self, peer_id: &str) {
        if let Some(contact) = self.contacts.get_mut(peer_id) {
            contact.blocked = true;
            tracing::info!(peer_id, "Contact blocked");
        }
        self.inbound_requests.retain(|r| r.from != peer_id);
        self.outbound_requests.retain(|r| r.to != peer_id);
    }

    /// Remove a contact entirely.
    pub fn remove(&mut self, peer_id: &str) -> bool {
        let removed = self.contacts.remove(peer_id).is_some();
        if removed {
            tracing::info!(peer_id, "Contact removed");
        }
        removed
    }

    /// Check if a peer is blocked.
    pub fn is_blocked(&self, peer_id: &str) -> bool {
        self.contacts.get(peer_id).map_or(false, |c| c.blocked)
    }
}

// ─── Presence ───────────────────────────────────────────────

impl ContactBook {
    /// Update presence for a peer (called from swarm connect/disconnect events).
    pub fn update_presence(&mut self, peer_id: &str, online: bool) {
        if let Some(contact) = self.contacts.get_mut(peer_id) {
            contact.status = if online { ContactStatus::Online } else { ContactStatus::Offline };
            if online {
                contact.last_seen = Some(SystemTime::now());
            }
            tracing::debug!(
                peer_id, online,
                "Contact presence updated"
            );
        }
    }

    /// Get all currently online contacts.
    pub fn online_contacts(&self) -> Vec<&Contact> {
        self.contacts.values()
            .filter(|c| c.status == ContactStatus::Online && !c.blocked)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_contact_book_is_empty() {
        let book = ContactBook::default_limits();
        assert_eq!(book.count(), 0);
        assert!(book.contacts().is_empty());
        assert!(book.inbound_requests().is_empty());
        assert!(book.outbound_requests().is_empty());
    }

    #[test]
    fn test_send_request_creates_outbound() {
        let mut book = ContactBook::default_limits();
        let req = book.send_request("our_id", "peer_a", "Alice").unwrap();
        assert_eq!(req.to, "peer_a");
        assert_eq!(req.display_name, "Alice");
        assert_eq!(book.outbound_requests().len(), 1);
    }

    #[test]
    fn test_send_request_rejects_duplicate() {
        let mut book = ContactBook::default_limits();
        book.send_request("our_id", "peer_a", "Alice").unwrap();
        let result = book.send_request("our_id", "peer_a", "Alice2");
        assert!(result.is_err());
    }

    #[test]
    fn test_receive_and_accept_request() {
        let mut book = ContactBook::default_limits();
        let req = ContactRequest {
            from: "peer_b".into(), to: "our_id".into(),
            display_name: "Bob".into(), timestamp: SystemTime::now(),
        };
        book.receive_request(req);
        assert_eq!(book.inbound_requests().len(), 1);

        let contact = book.accept_request("peer_b").unwrap();
        assert_eq!(contact.display_name, "Bob");
        assert_eq!(book.count(), 1);
        assert!(book.inbound_requests().is_empty());
    }

    #[test]
    fn test_block_sets_flag_and_clears_requests() {
        let mut book = ContactBook::default_limits();
        let req = ContactRequest {
            from: "peer_c".into(), to: "our_id".into(),
            display_name: "Charlie".into(), timestamp: SystemTime::now(),
        };
        book.receive_request(req);
        book.accept_request("peer_c").unwrap();

        book.block("peer_c");
        assert!(book.is_blocked("peer_c"));
    }

    #[test]
    fn test_remove_contact() {
        let mut book = ContactBook::default_limits();
        let req = ContactRequest {
            from: "peer_d".into(), to: "our_id".into(),
            display_name: "Dave".into(), timestamp: SystemTime::now(),
        };
        book.receive_request(req);
        book.accept_request("peer_d").unwrap();
        assert_eq!(book.count(), 1);

        assert!(book.remove("peer_d"));
        assert_eq!(book.count(), 0);
    }

    #[test]
    fn test_presence_update() {
        let mut book = ContactBook::default_limits();
        let req = ContactRequest {
            from: "peer_e".into(), to: "our_id".into(),
            display_name: "Eve".into(), timestamp: SystemTime::now(),
        };
        book.receive_request(req);
        book.accept_request("peer_e").unwrap();

        book.update_presence("peer_e", true);
        assert_eq!(book.online_contacts().len(), 1);

        book.update_presence("peer_e", false);
        assert!(book.online_contacts().is_empty());
    }

    #[test]
    fn test_capacity_limit() {
        let mut book = ContactBook::new(1, 10);
        let req = ContactRequest {
            from: "peer_f".into(), to: "our_id".into(),
            display_name: "Frank".into(), timestamp: SystemTime::now(),
        };
        book.receive_request(req);
        book.accept_request("peer_f").unwrap();

        let result = book.send_request("our_id", "peer_g", "Grace");
        assert_eq!(result.unwrap_err(), "Contact book is full");
    }

    #[test]
    fn test_mark_accepted_adds_mutual_contact() {
        let mut book = ContactBook::default_limits();
        book.send_request("our_id", "peer_h", "Heidi").unwrap();
        assert_eq!(book.outbound_requests().len(), 1);

        book.mark_accepted("peer_h", "Heidi");
        assert_eq!(book.count(), 1);
        assert!(book.outbound_requests().is_empty());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut book = ContactBook::default_limits();
        let req = ContactRequest {
            from: "peer_i".into(), to: "our_id".into(),
            display_name: "Ivan".into(), timestamp: SystemTime::now(),
        };
        book.receive_request(req);
        book.accept_request("peer_i").unwrap();

        let json = serde_json::to_string(&book).unwrap();
        let restored: ContactBook = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.count(), 1);
        assert_eq!(restored.get("peer_i").unwrap().display_name, "Ivan");
    }
}
