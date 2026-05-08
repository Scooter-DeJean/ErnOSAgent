// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! File transfer — peer-to-peer file sharing protocol.
//!
//! ErnMesh file transfers are:
//!
//! - **Direct**: Files are sent peer-to-peer, not through a central server.
//! - **Chunked**: Large files are split into chunks (size from config).
//! - **Resumable**: Each chunk is content-addressed. A broken transfer
//!   can resume from the last successfully received chunk.
//! - **Capability-gated**: The sender must have `FileTransfer` capability
//!   for the receiving peer (enforced by `CapabilityStore`).
//! - **Encrypted**: File data is encrypted with the E2E session key.
//!
//! ## Protocol
//!
//! 1. **Offer**: Sender sends a `FileOffer` with metadata (name, size, chunks).
//! 2. **Accept/Reject**: Receiver responds with `FileResponse`.
//! 3. **Transfer**: Sender sends chunks sequentially. Each chunk is verified
//!    by its content hash (CID).
//! 4. **Complete**: Receiver confirms all chunks received.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata about a file being offered for transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileOffer {
    /// Unique transfer ID.
    pub transfer_id: String,
    /// The sender's PeerId.
    pub from: String,
    /// The recipient's PeerId.
    pub to: String,
    /// Original filename.
    pub filename: String,
    /// Total file size in bytes.
    pub size_bytes: u64,
    /// MIME type (if known).
    pub mime_type: Option<String>,
    /// Number of chunks the file is split into.
    pub chunk_count: u32,
    /// Size of each chunk in bytes (last chunk may be smaller).
    pub chunk_size: u64,
    /// SHA-256 hash of the complete file.
    pub file_hash: String,
}

/// Response to a file offer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileResponse {
    /// The file transfer is accepted.
    Accepted { transfer_id: String },
    /// The file transfer is rejected.
    Rejected { transfer_id: String, reason: String },
}

/// A single file chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChunk {
    /// The transfer this chunk belongs to.
    pub transfer_id: String,
    /// Chunk index (0-based).
    pub index: u32,
    /// SHA-256 hash of this chunk's data.
    pub chunk_hash: String,
    /// The chunk data (raw bytes, base64-encoded for serialisation).
    pub data: Vec<u8>,
}

/// State of a file transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferState {
    /// Offer sent, awaiting response.
    Offered,
    /// Transfer accepted, sending/receiving chunks.
    InProgress,
    /// All chunks sent/received successfully.
    Complete,
    /// Transfer was rejected by the recipient.
    Rejected,
    /// Transfer failed (timeout, corruption, etc.).
    Failed,
}

/// Tracks a file transfer in progress.
#[derive(Debug)]
pub struct Transfer {
    /// The file offer metadata.
    pub offer: FileOffer,
    /// Current state.
    pub state: TransferState,
    /// Which chunks have been received/sent (indexed by chunk index).
    pub chunks_completed: Vec<bool>,
    /// Whether we are the sender (true) or receiver (false).
    pub is_sender: bool,
}

impl Transfer {
    /// Create a new transfer from an offer.
    pub fn new(offer: FileOffer, is_sender: bool) -> Self {
        let chunk_count = offer.chunk_count as usize;
        Self {
            offer,
            state: TransferState::Offered,
            chunks_completed: vec![false; chunk_count],
            is_sender,
        }
    }

    /// Mark a chunk as completed.
    pub fn complete_chunk(&mut self, index: u32) -> bool {
        if let Some(slot) = self.chunks_completed.get_mut(index as usize) {
            *slot = true;

            // Check if all chunks are done
            if self.chunks_completed.iter().all(|c| *c) {
                self.state = TransferState::Complete;

                tracing::info!(
                    transfer_id = %self.offer.transfer_id,
                    filename = %self.offer.filename,
                    "File transfer complete"
                );
            }

            true
        } else {
            false
        }
    }

    /// Number of chunks completed.
    pub fn completed_count(&self) -> usize {
        self.chunks_completed.iter().filter(|c| **c).count()
    }

    /// Percentage of transfer completion.
    pub fn progress_percent(&self) -> f64 {
        if self.chunks_completed.is_empty() {
            return 0.0;
        }
        (self.completed_count() as f64 / self.chunks_completed.len() as f64) * 100.0
    }

    /// Returns `true` if the transfer is fully complete.
    pub fn is_complete(&self) -> bool {
        self.state == TransferState::Complete
    }

    /// Mark the transfer as accepted (in progress).
    pub fn accept(&mut self) {
        self.state = TransferState::InProgress;
    }

    /// Mark the transfer as rejected.
    pub fn reject(&mut self) {
        self.state = TransferState::Rejected;
    }

    /// Mark the transfer as failed.
    pub fn fail(&mut self) {
        self.state = TransferState::Failed;
    }
}

/// Transfer manager — tracks all active file transfers.
#[derive(Debug, Default)]
pub struct TransferManager {
    /// Active transfers indexed by transfer_id.
    transfers: HashMap<String, Transfer>,
}

impl TransferManager {
    /// Create a new transfer manager.
    pub fn new() -> Self {
        Self {
            transfers: HashMap::new(),
        }
    }

    /// Create a file offer and register the transfer.
    pub fn create_offer(
        &mut self,
        from: &str,
        to: &str,
        filename: &str,
        size_bytes: u64,
        file_hash: &str,
        chunk_size: u64,
    ) -> FileOffer {
        let chunk_count = if size_bytes == 0 {
            1
        } else {
            ((size_bytes + chunk_size - 1) / chunk_size) as u32
        };

        let suffix: u32 = rand::random();
        let transfer_id = format!("{}-{}-{:08x}", from, filename, suffix);

        let offer = FileOffer {
            transfer_id: transfer_id.clone(),
            from: from.to_string(),
            to: to.to_string(),
            filename: filename.to_string(),
            size_bytes,
            mime_type: None,
            chunk_count,
            chunk_size,
            file_hash: file_hash.to_string(),
        };

        self.transfers.insert(
            transfer_id,
            Transfer::new(offer.clone(), true),
        );

        tracing::info!(
            filename = filename,
            size = size_bytes,
            chunks = chunk_count,
            "File transfer offer created"
        );

        offer
    }

    /// Register an incoming transfer offer (we are the receiver).
    pub fn register_incoming(&mut self, offer: FileOffer) {
        let tid = offer.transfer_id.clone();
        self.transfers.insert(tid, Transfer::new(offer, false));
    }

    /// Get a transfer by ID.
    pub fn get_transfer(&self, transfer_id: &str) -> Option<&Transfer> {
        self.transfers.get(transfer_id)
    }

    /// Get a mutable transfer by ID.
    pub fn get_transfer_mut(&mut self, transfer_id: &str) -> Option<&mut Transfer> {
        self.transfers.get_mut(transfer_id)
    }

    /// Remove a completed or failed transfer.
    pub fn remove_transfer(&mut self, transfer_id: &str) -> bool {
        self.transfers.remove(transfer_id).is_some()
    }

    /// Count of active (in-progress) transfers.
    pub fn active_count(&self) -> usize {
        self.transfers.values()
            .filter(|t| t.state == TransferState::InProgress)
            .count()
    }

    /// Total tracked transfers.
    pub fn total_count(&self) -> usize {
        self.transfers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_offer() {
        let mut mgr = TransferManager::new();
        let offer = mgr.create_offer(
            "alice", "bob", "photo.jpg", 1048576, "abc123", 262144,
        );

        assert_eq!(offer.from, "alice");
        assert_eq!(offer.to, "bob");
        assert_eq!(offer.filename, "photo.jpg");
        assert_eq!(offer.size_bytes, 1048576);
        assert_eq!(offer.chunk_count, 4); // 1MB / 256KB = 4
    }

    #[test]
    fn test_create_offer_chunk_count_rounding() {
        let mut mgr = TransferManager::new();
        let offer = mgr.create_offer("a", "b", "f", 300000, "h", 262144);
        assert_eq!(offer.chunk_count, 2); // ceil(300000/262144) = 2
    }

    #[test]
    fn test_create_offer_empty_file() {
        let mut mgr = TransferManager::new();
        let offer = mgr.create_offer("a", "b", "empty.txt", 0, "h", 262144);
        assert_eq!(offer.chunk_count, 1); // at least 1 chunk
    }

    #[test]
    fn test_transfer_accept() {
        let offer = FileOffer {
            transfer_id: "test-1".into(),
            from: "alice".into(),
            to: "bob".into(),
            filename: "file.txt".into(),
            size_bytes: 100,
            mime_type: None,
            chunk_count: 1,
            chunk_size: 262144,
            file_hash: "hash".into(),
        };

        let mut transfer = Transfer::new(offer, false);
        assert_eq!(transfer.state, TransferState::Offered);

        transfer.accept();
        assert_eq!(transfer.state, TransferState::InProgress);
    }

    #[test]
    fn test_transfer_complete_chunks() {
        let offer = FileOffer {
            transfer_id: "test-1".into(),
            from: "a".into(), to: "b".into(),
            filename: "f".into(), size_bytes: 100,
            mime_type: None, chunk_count: 3,
            chunk_size: 50, file_hash: "h".into(),
        };

        let mut transfer = Transfer::new(offer, false);
        transfer.accept();

        assert!(!transfer.is_complete());
        assert_eq!(transfer.progress_percent(), 0.0);

        transfer.complete_chunk(0);
        assert_eq!(transfer.completed_count(), 1);
        assert!((transfer.progress_percent() - 33.33).abs() < 1.0);

        transfer.complete_chunk(1);
        transfer.complete_chunk(2);

        assert!(transfer.is_complete());
        assert_eq!(transfer.progress_percent(), 100.0);
    }

    #[test]
    fn test_transfer_reject() {
        let offer = FileOffer {
            transfer_id: "t".into(), from: "a".into(), to: "b".into(),
            filename: "f".into(), size_bytes: 100, mime_type: None,
            chunk_count: 1, chunk_size: 100, file_hash: "h".into(),
        };

        let mut transfer = Transfer::new(offer, false);
        transfer.reject();
        assert_eq!(transfer.state, TransferState::Rejected);
    }

    #[test]
    fn test_transfer_fail() {
        let offer = FileOffer {
            transfer_id: "t".into(), from: "a".into(), to: "b".into(),
            filename: "f".into(), size_bytes: 100, mime_type: None,
            chunk_count: 1, chunk_size: 100, file_hash: "h".into(),
        };

        let mut transfer = Transfer::new(offer, false);
        transfer.fail();
        assert_eq!(transfer.state, TransferState::Failed);
    }

    #[test]
    fn test_transfer_manager_active_count() {
        let mut mgr = TransferManager::new();
        mgr.create_offer("a", "b", "f1", 100, "h", 100);
        mgr.create_offer("a", "c", "f2", 200, "h", 100);

        assert_eq!(mgr.total_count(), 2);
        assert_eq!(mgr.active_count(), 0); // still in "Offered" state

        // Accept the first transfer
        let id = mgr.transfers.keys().next().unwrap().clone();
        mgr.get_transfer_mut(&id).unwrap().accept();

        assert_eq!(mgr.active_count(), 1);
    }

    #[test]
    fn test_register_incoming() {
        let mut mgr = TransferManager::new();
        let offer = FileOffer {
            transfer_id: "incoming-1".into(),
            from: "alice".into(), to: "bob".into(),
            filename: "doc.pdf".into(), size_bytes: 5000,
            mime_type: Some("application/pdf".into()),
            chunk_count: 1, chunk_size: 262144,
            file_hash: "hash".into(),
        };

        mgr.register_incoming(offer);
        let t = mgr.get_transfer("incoming-1").unwrap();
        assert!(!t.is_sender);
        assert_eq!(t.state, TransferState::Offered);
    }

    #[test]
    fn test_remove_transfer() {
        let mut mgr = TransferManager::new();
        mgr.create_offer("a", "b", "f", 100, "h", 100);
        let id = mgr.transfers.keys().next().unwrap().clone();

        assert!(mgr.remove_transfer(&id));
        assert_eq!(mgr.total_count(), 0);
    }

    #[test]
    fn test_file_offer_serializable() {
        let offer = FileOffer {
            transfer_id: "t1".into(),
            from: "alice".into(), to: "bob".into(),
            filename: "test.txt".into(), size_bytes: 1024,
            mime_type: Some("text/plain".into()),
            chunk_count: 1, chunk_size: 262144,
            file_hash: "abc".into(),
        };

        let json = serde_json::to_string(&offer).unwrap();
        let recovered: FileOffer = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.filename, "test.txt");
    }
}
