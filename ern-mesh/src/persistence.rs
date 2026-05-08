// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! State persistence — save and restore mesh state to disk.
//!
//! Provides durable persistence for mesh components:
//!
//! - **Economy ledger**: ErnPoints balance and transaction history.
//! - **Peer reputation**: Trust scores for known peers.
//! - **Community list**: Forum communities and their rules.
//! - **Mailbox**: Received and sent messages.
//! - **Session state**: Active E2E session metadata (not keys).
//! - **Site registry**: Hosted mesh site manifests.
//!
//! State is stored as JSON files in `{data_dir}/mesh/state/`.
//! Each component is saved independently to minimise I/O.
//!
//! ## Governance Note
//!
//! Session keys are NEVER persisted to disk. Only session metadata
//! (peer_id, state, counters) is saved. Keys must be re-established
//! on restart to maintain forward secrecy.

use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::path::{Path, PathBuf};

/// The state directory — manages persistence for all mesh components.
#[derive(Debug)]
pub struct StateDir {
    /// Root state directory.
    root: PathBuf,
}

impl StateDir {
    /// Open or create the state directory.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let root = data_dir.join("mesh").join("state");

        if !root.exists() {
            std::fs::create_dir_all(&root)
                .with_context(|| format!("Failed to create state directory: {}", root.display()))?;
        }

        tracing::info!(path = %root.display(), "State directory opened");

        Ok(Self { root })
    }

    /// Save a serializable value to a named state file.
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        let path = self.root.join(format!("{}.json", name));
        let json = serde_json::to_string_pretty(value)
            .with_context(|| format!("Failed to serialise state '{}'", name))?;

        std::fs::write(&path, &json)
            .with_context(|| format!("Failed to write state file: {}", path.display()))?;

        tracing::debug!(
            name = name,
            path = %path.display(),
            size = json.len(),
            "State saved"
        );

        Ok(())
    }

    /// Load a value from a named state file.
    ///
    /// Returns `Ok(None)` if the file does not exist (fresh install).
    /// Returns `Err` if the file exists but is corrupt.
    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        let path = self.root.join(format!("{}.json", name));

        if !path.exists() {
            tracing::debug!(name = name, "State file not found — fresh start");
            return Ok(None);
        }

        let json = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read state file: {}", path.display()))?;

        let value: T = serde_json::from_str(&json)
            .with_context(|| format!("Failed to deserialise state '{}' from {}", name, path.display()))?;

        tracing::debug!(
            name = name,
            path = %path.display(),
            "State loaded"
        );

        Ok(Some(value))
    }

    /// Delete a named state file.
    pub fn delete(&self, name: &str) -> Result<bool> {
        let path = self.root.join(format!("{}.json", name));

        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("Failed to delete state file: {}", path.display()))?;

            tracing::info!(name = name, "State file deleted");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// List all saved state file names (without .json extension).
    pub fn list(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();

        for entry in std::fs::read_dir(&self.root)
            .with_context(|| format!("Failed to list state directory: {}", self.root.display()))?
        {
            let entry = entry
                .with_context(|| "Failed to read directory entry")?;
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            }
        }

        Ok(names)
    }

    /// Check if a named state file exists.
    pub fn exists(&self, name: &str) -> bool {
        self.root.join(format!("{}.json", name)).exists()
    }

    /// Get the path to the state directory.
    pub fn path(&self) -> &Path {
        &self.root
    }
}

/// Snapshot of mesh state — used for dashboard and diagnostics.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MeshStateSnapshot {
    /// When the snapshot was taken.
    pub timestamp: std::time::SystemTime,
    /// ErnPoints balance.
    pub balance: f64,
    /// Transaction count.
    pub transaction_count: usize,
    /// Known peer count.
    pub known_peers: usize,
    /// Active E2E sessions.
    pub active_sessions: usize,
    /// Hosted mesh sites.
    pub hosted_sites: usize,
    /// Stored content items.
    pub content_items: usize,
    /// Unread mail count.
    pub unread_mail: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_dir_open() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        assert!(state.path().exists());
        assert!(state.path().ends_with("mesh/state"));
    }

    #[test]
    fn test_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        let data = vec!["hello", "world"];
        state.save("test_data", &data).unwrap();

        let loaded: Option<Vec<String>> = state.load("test_data").unwrap();
        assert_eq!(loaded.unwrap(), vec!["hello", "world"]);
    }

    #[test]
    fn test_load_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        let loaded: Option<Vec<String>> = state.load("nonexistent").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_save_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        state.save("counter", &42u64).unwrap();
        state.save("counter", &99u64).unwrap();

        let loaded: Option<u64> = state.load("counter").unwrap();
        assert_eq!(loaded.unwrap(), 99);
    }

    #[test]
    fn test_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        state.save("deleteme", &"data").unwrap();
        assert!(state.exists("deleteme"));

        let deleted = state.delete("deleteme").unwrap();
        assert!(deleted);
        assert!(!state.exists("deleteme"));
    }

    #[test]
    fn test_delete_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        let deleted = state.delete("nope").unwrap();
        assert!(!deleted);
    }

    #[test]
    fn test_list() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        state.save("alpha", &1u32).unwrap();
        state.save("beta", &2u32).unwrap();
        state.save("gamma", &3u32).unwrap();

        let mut names = state.list().unwrap();
        names.sort();

        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        assert!(!state.exists("test"));
        state.save("test", &true).unwrap();
        assert!(state.exists("test"));
    }

    #[test]
    fn test_load_corrupt_file_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        let path = state.path().join("bad.json");
        std::fs::write(&path, "{{invalid json}}").unwrap();

        let result: Result<Option<Vec<String>>> = state.load("bad");
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot_serializable() {
        let snap = MeshStateSnapshot {
            timestamp: std::time::SystemTime::now(),
            balance: 42.5,
            transaction_count: 100,
            known_peers: 15,
            active_sessions: 3,
            hosted_sites: 2,
            content_items: 50,
            unread_mail: 7,
        };

        let json = serde_json::to_string(&snap).unwrap();
        let recovered: MeshStateSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(recovered.balance, 42.5);
    }

    #[test]
    fn test_complex_struct_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let state = StateDir::open(tmp.path()).unwrap();

        let snap = MeshStateSnapshot {
            timestamp: std::time::SystemTime::now(),
            balance: 1234.5,
            transaction_count: 999,
            known_peers: 42,
            active_sessions: 5,
            hosted_sites: 3,
            content_items: 100,
            unread_mail: 0,
        };

        state.save("snapshot", &snap).unwrap();
        let loaded: Option<MeshStateSnapshot> = state.load("snapshot").unwrap();
        let loaded = loaded.unwrap();

        assert_eq!(loaded.balance, 1234.5);
        assert_eq!(loaded.transaction_count, 999);
        assert_eq!(loaded.known_peers, 42);
    }
}
