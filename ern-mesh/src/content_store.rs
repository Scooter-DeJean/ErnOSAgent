// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Content-addressable storage — local content store with hash-based addressing.
//!
//! ErnMesh uses content-addressed storage for decentralised file sharing:
//!
//! - **Hash-based addressing**: Content is identified by its SHA-256 hash (CID).
//!   Two identical files always produce the same CID regardless of origin.
//! - **Pinning**: Users can choose to pin (persist) content they want to keep
//!   available on the network. Pinning earns ErnPoints.
//! - **Chunk-based**: Large files are split into configurable chunks
//!   (size from `config.economy.chunk_size_bytes`).
//! - **Deduplication**: Content is stored once, referenced by hash.
//!
//! This module provides the local store. Content retrieval from remote
//! peers uses Kademlia content routing (integrated in future PRs).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A content identifier — SHA-256 hash of the content.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContentId(String);

impl ContentId {
    /// Compute the CID from raw content bytes.
    pub fn from_content(data: &[u8]) -> Self {
        let hash = Sha256::digest(data);
        Self(hex::encode(hash))
    }

    /// Create a CID from a known hex hash.
    pub fn from_hex(hex: &str) -> Self {
        Self(hex.to_string())
    }

    /// Returns the CID as a hex string.
    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Metadata about a stored content item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentMeta {
    /// Content identifier (SHA-256 hash).
    pub cid: ContentId,
    /// Size in bytes.
    pub size_bytes: u64,
    /// MIME type (if known).
    pub mime_type: Option<String>,
    /// Whether this content is pinned (persisted).
    pub pinned: bool,
    /// When this content was first stored.
    pub stored_at: std::time::SystemTime,
    /// Original filename (if provided).
    pub filename: Option<String>,
}

/// Local content-addressable store.
///
/// Stores content on disk in `data/mesh/content/<CID>`.
/// Metadata is tracked in memory.
#[derive(Debug)]
pub struct ContentStore {
    /// Base directory for content files.
    base_dir: PathBuf,
    /// In-memory metadata index.
    index: HashMap<ContentId, ContentMeta>,
}

impl ContentStore {
    /// Open or create a content store at the given directory.
    pub fn open(base_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(base_dir)
            .context("Failed to create content store directory")?;

        tracing::info!(
            dir = %base_dir.display(),
            "Content store opened"
        );

        Ok(Self {
            base_dir: base_dir.to_path_buf(),
            index: HashMap::new(),
        })
    }

    /// Store content and return its CID.
    ///
    /// If the content already exists (same CID), this is a no-op.
    pub fn store(
        &mut self,
        data: &[u8],
        mime_type: Option<String>,
        filename: Option<String>,
    ) -> Result<ContentId> {
        let cid = ContentId::from_content(data);

        // Deduplication: skip if already stored
        if self.index.contains_key(&cid) {
            tracing::debug!(cid = %cid, "Content already stored — deduplication hit");
            return Ok(cid);
        }

        // Write to disk
        let path = self.content_path(&cid);
        std::fs::write(&path, data)
            .context(format!("Failed to write content {}", cid))?;

        // Track metadata
        self.index.insert(cid.clone(), ContentMeta {
            cid: cid.clone(),
            size_bytes: data.len() as u64,
            mime_type,
            pinned: false,
            stored_at: std::time::SystemTime::now(),
            filename,
        });

        tracing::info!(
            cid = %cid,
            size = data.len(),
            "Content stored"
        );

        Ok(cid)
    }

    /// Retrieve content by CID.
    pub fn get(&self, cid: &ContentId) -> Result<Vec<u8>> {
        let path = self.content_path(cid);
        std::fs::read(&path)
            .context(format!("Content not found: {}", cid))
    }

    /// Check if content exists in the store.
    pub fn contains(&self, cid: &ContentId) -> bool {
        self.index.contains_key(cid)
    }

    /// Get metadata for a content item.
    pub fn metadata(&self, cid: &ContentId) -> Option<&ContentMeta> {
        self.index.get(cid)
    }

    /// Pin content (mark for persistent storage).
    pub fn pin(&mut self, cid: &ContentId) -> bool {
        if let Some(meta) = self.index.get_mut(cid) {
            meta.pinned = true;
            tracing::info!(cid = %cid, "Content pinned");
            true
        } else {
            false
        }
    }

    /// Unpin content (eligible for garbage collection).
    pub fn unpin(&mut self, cid: &ContentId) -> bool {
        if let Some(meta) = self.index.get_mut(cid) {
            meta.pinned = false;
            tracing::info!(cid = %cid, "Content unpinned");
            true
        } else {
            false
        }
    }

    /// Remove content from the store.
    ///
    /// Refuses to remove pinned content — unpin first.
    pub fn remove(&mut self, cid: &ContentId) -> Result<()> {
        if let Some(meta) = self.index.get(cid) {
            if meta.pinned {
                anyhow::bail!("Cannot remove pinned content: {}", cid);
            }
        }

        let path = self.content_path(cid);
        if path.exists() {
            std::fs::remove_file(&path)
                .context(format!("Failed to remove content file: {}", cid))?;
        }

        self.index.remove(cid);

        tracing::info!(cid = %cid, "Content removed");
        Ok(())
    }

    /// List all stored content CIDs.
    pub fn list_cids(&self) -> Vec<&ContentId> {
        self.index.keys().collect()
    }

    /// List only pinned content.
    pub fn list_pinned(&self) -> Vec<&ContentMeta> {
        self.index.values().filter(|m| m.pinned).collect()
    }

    /// Total number of stored items.
    pub fn count(&self) -> usize {
        self.index.len()
    }

    /// Total storage used in bytes.
    pub fn total_bytes(&self) -> u64 {
        self.index.values().map(|m| m.size_bytes).sum()
    }

    /// File path for a given CID.
    fn content_path(&self, cid: &ContentId) -> PathBuf {
        self.base_dir.join(cid.as_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cid_from_content() {
        let cid = ContentId::from_content(b"hello world");
        assert_eq!(cid.as_hex().len(), 64, "SHA-256 hex must be 64 chars");
    }

    #[test]
    fn test_cid_deterministic() {
        let cid1 = ContentId::from_content(b"same data");
        let cid2 = ContentId::from_content(b"same data");
        assert_eq!(cid1, cid2, "Same content must produce same CID");
    }

    #[test]
    fn test_cid_different_content() {
        let cid1 = ContentId::from_content(b"data1");
        let cid2 = ContentId::from_content(b"data2");
        assert_ne!(cid1, cid2, "Different content must produce different CIDs");
    }

    #[test]
    fn test_store_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let data = b"hello mesh";
        let cid = store.store(data, None, None).unwrap();

        let retrieved = store.get(&cid).unwrap();
        assert_eq!(retrieved, data);
    }

    #[test]
    fn test_store_deduplication() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let data = b"duplicate content";
        let cid1 = store.store(data, None, None).unwrap();
        let cid2 = store.store(data, None, None).unwrap();

        assert_eq!(cid1, cid2);
        assert_eq!(store.count(), 1, "Duplicate must not create second entry");
    }

    #[test]
    fn test_store_with_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let cid = store.store(
            b"image data",
            Some("image/png".into()),
            Some("photo.png".into()),
        ).unwrap();

        let meta = store.metadata(&cid).unwrap();
        assert_eq!(meta.mime_type.as_deref(), Some("image/png"));
        assert_eq!(meta.filename.as_deref(), Some("photo.png"));
        assert_eq!(meta.size_bytes, 10);
        assert!(!meta.pinned);
    }

    #[test]
    fn test_contains() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let cid = store.store(b"data", None, None).unwrap();
        assert!(store.contains(&cid));
        assert!(!store.contains(&ContentId::from_hex("nonexistent")));
    }

    #[test]
    fn test_pin_and_unpin() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let cid = store.store(b"pin me", None, None).unwrap();
        assert!(!store.metadata(&cid).unwrap().pinned);

        assert!(store.pin(&cid));
        assert!(store.metadata(&cid).unwrap().pinned);

        assert!(store.unpin(&cid));
        assert!(!store.metadata(&cid).unwrap().pinned);
    }

    #[test]
    fn test_pin_unknown_cid_returns_false() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();
        assert!(!store.pin(&ContentId::from_hex("unknown")));
    }

    #[test]
    fn test_remove_unpinned() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let cid = store.store(b"temp data", None, None).unwrap();
        store.remove(&cid).unwrap();

        assert!(!store.contains(&cid));
        assert_eq!(store.count(), 0);
    }

    #[test]
    fn test_remove_pinned_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let cid = store.store(b"important", None, None).unwrap();
        store.pin(&cid);

        let result = store.remove(&cid);
        assert!(result.is_err(), "Must refuse to remove pinned content");
        assert!(store.contains(&cid), "Pinned content must survive removal attempt");
    }

    #[test]
    fn test_list_pinned() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        let cid1 = store.store(b"data1", None, None).unwrap();
        let _cid2 = store.store(b"data2", None, None).unwrap();
        store.pin(&cid1);

        let pinned = store.list_pinned();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].cid, cid1);
    }

    #[test]
    fn test_total_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = ContentStore::open(tmp.path()).unwrap();

        store.store(b"12345", None, None).unwrap();
        store.store(b"1234567890", None, None).unwrap();

        assert_eq!(store.total_bytes(), 15);
    }

    #[test]
    fn test_cid_serialize_roundtrip() {
        let cid = ContentId::from_content(b"test");
        let json = serde_json::to_string(&cid).unwrap();
        let recovered: ContentId = serde_json::from_str(&json).unwrap();
        assert_eq!(cid, recovered);
    }
}
