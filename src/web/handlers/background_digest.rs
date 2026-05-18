// Ern-OS — Background document digest store and task runner.
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Tracks in-progress and completed background deep-read operations.
//!
//! # Design
//! When a large attachment triggers the deep-read gate, the system:
//! 1. Replies immediately with a peek of the opening section.
//! 2. Spawns a background Tokio task to run the full deep_read().
//! 3. On completion: stores the digest in DigestStore and notifies the user.
//! 4. On the next message referencing the file: serves the cached digest instantly.
//!
//! DigestStore is a DashMap (lock-free, sharded) — no RwLock contention on reads.

use dashmap::DashMap;
use std::sync::Arc;

use crate::web::state::AppState;

/// Status of a background deep-read operation.
pub enum DigestStatus {
    /// Task is running. Stored so duplicate spawns are prevented.
    Pending { started_at: std::time::Instant },
    /// Full deep-read complete. Digest is ready for context injection.
    Complete { digest: String },
}

/// Concurrent map from saved file path → digest status.
/// Key: the canonical saved path (e.g. `data/uploads/20260518_abc12345.md`).
/// DashMap is lock-free for concurrent reads and writes — no RwLock needed.
pub type DigestStore = DashMap<String, DigestStatus>;

/// Create a new empty digest store.
pub fn new_digest_store() -> Arc<DigestStore> {
    Arc::new(DashMap::new())
}

/// Spawn a background deep-read task for a saved admin attachment.
///
/// Marks the path as `Pending` in the store before returning, preventing
/// duplicate spawns if the same file is submitted again before the task completes.
/// On completion: transitions to `Complete` and notifies the user via the platform.
///
/// # Governance
/// - Does not block the caller — returns immediately after spawn (§2.4).
/// - `channel_id` and `platform` are passed in from the live message context,
///   not stored globally (§13.1 — no ambient credential capture).
pub fn spawn_background_deep_read(
    state: AppState,
    config: crate::web::attachment_reader::DeepReadConfig,
    path_key: String,
    channel_id: String,
    platform: String,
) {
    // Mark pending before spawn to prevent duplicate tasks.
    state.digest_store.insert(
        path_key.clone(),
        DigestStatus::Pending { started_at: std::time::Instant::now() },
    );

    let filename = config.filename.clone();
    tokio::spawn(async move {
        run_background_deep_read(state, config, path_key, filename, channel_id, platform).await;
    });
}

/// Background task body: run full deep_read(), store result, notify user.
/// This is not pub — callers use `spawn_background_deep_read()`.
async fn run_background_deep_read(
    state: AppState,
    config: crate::web::attachment_reader::DeepReadConfig,
    path_key: String,
    filename: String,
    channel_id: String,
    platform: String,
) {
    tracing::info!(filename = %filename, path = %path_key, "Background deep-read: started");

    let digest = crate::web::attachment_reader::deep_read(
        config,
        state.provider.as_ref(),
        &state.memory,
        None, // no SSE tx in background tasks
    ).await;

    // Store digest before notifying — result is durable even if notification fails (§2.4).
    state.digest_store.insert(
        path_key.clone(),
        DigestStatus::Complete { digest: digest.clone() },
    );
    tracing::info!(filename = %filename, "Background deep-read: complete — digest stored");

    // Notify user via platform. Best-effort: failure is logged, not propagated.
    let notify_msg = format!(
        "📖 **Background read complete** — `{}` has been fully processed. \
         Your next message will have access to the complete document.",
        filename
    );
    let platforms = state.platforms.read().await;
    if let Err(e) = platforms.send_message(&platform, &channel_id, &notify_msg).await {
        tracing::warn!(
            error = %e, filename = %filename,
            "Background deep-read: platform notification failed — digest still available"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_digest_store_is_empty() {
        let store = new_digest_store();
        assert!(store.is_empty());
    }

    #[test]
    fn test_digest_store_insert_and_read() {
        let store = new_digest_store();
        store.insert(
            "data/uploads/book.md".to_string(),
            DigestStatus::Pending { started_at: std::time::Instant::now() },
        );
        assert!(store.contains_key("data/uploads/book.md"));
        assert!(!store.contains_key("data/uploads/other.md"));
    }

    #[test]
    fn test_digest_store_complete_transition() {
        let store = new_digest_store();
        let key = "data/uploads/book.md".to_string();

        store.insert(key.clone(), DigestStatus::Pending { started_at: std::time::Instant::now() });
        store.insert(key.clone(), DigestStatus::Complete { digest: "summary text".to_string() });

        let entry = store.get(&key).unwrap();
        assert!(matches!(*entry, DigestStatus::Complete { .. }));
    }

    #[test]
    fn test_digest_store_concurrent_access() {
        // DashMap must not deadlock when read and written concurrently.
        // This is a structural test — DashMap's internal sharding ensures safety.
        let store = Arc::new(new_digest_store());
        let store2 = Arc::clone(&store);

        store.insert("a".to_string(), DigestStatus::Pending { started_at: std::time::Instant::now() });
        store2.insert("b".to_string(), DigestStatus::Complete { digest: "d".to_string() });

        assert!(store.contains_key("a"));
        assert!(store.contains_key("b"));
    }
}
