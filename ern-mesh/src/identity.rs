// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Node identity management — Ed25519 keypair lifecycle and PeerId derivation.
//!
//! Every mesh node has a unique cryptographic identity: an Ed25519 keypair
//! generated at first boot and persisted to `data/mesh/identity.key`.
//! The public key is never transmitted directly — only the derived `PeerId`
//! (a multihash of the public key) is shared with peers.
//!
//! Private keys never leave the local filesystem. File permissions are
//! set to owner-read-only (`0600`) on Unix systems. The key is loaded
//! into memory at startup and held for the lifetime of the mesh node.

use anyhow::{Context, Result};
use libp2p_identity::{Keypair, PeerId};
use std::path::{Path, PathBuf};

/// Key file name within the mesh data directory.
const KEY_FILE_NAME: &str = "identity.key";

/// The node's cryptographic identity — Ed25519 keypair + derived PeerId.
///
/// Created via [`load_or_generate`], which handles the full lifecycle:
/// generate on first boot, persist to disk, load on subsequent boots.
#[derive(Debug)]
pub struct NodeIdentity {
    /// The Ed25519 keypair. Used for signing messages and QUIC/Noise
    /// handshakes. Never transmitted — only the derived PeerId is shared.
    keypair: Keypair,

    /// The PeerId derived from the public key. This is the node's unique
    /// identifier on the mesh network — a multihash of the Ed25519 public key.
    peer_id: PeerId,

    /// Path to the persisted key file on disk.
    key_path: PathBuf,
}

impl NodeIdentity {
    /// Returns a reference to the node's keypair.
    pub fn keypair(&self) -> &Keypair {
        &self.keypair
    }

    /// Returns the node's PeerId — its mesh network address.
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }

    /// Returns the path where the identity key is persisted.
    pub fn key_path(&self) -> &Path {
        &self.key_path
    }

    /// Returns the PeerId as a base58-encoded string (standard libp2p format).
    pub fn peer_id_base58(&self) -> String {
        self.peer_id.to_base58()
    }

    /// Sign arbitrary data with this node's private key.
    ///
    /// Returns the Ed25519 signature as a byte vector.
    /// Used for message signing (§5.1 T3, §14.3), DHT record signing,
    /// and ErnieBook post authentication.
    pub fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        self.keypair
            .sign(data)
            .map_err(|e| anyhow::anyhow!("Ed25519 signing failed: {}", e))
    }
}

/// Load an existing identity from disk or generate a new one.
///
/// - If `{mesh_data_dir}/identity.key` exists: loads and validates it.
/// - If it does not exist: generates a new Ed25519 keypair, writes it to
///   disk with restricted permissions, and returns the new identity.
///
/// This is the **only** way to create a `NodeIdentity`. The keypair is
/// never exposed outside this module for direct construction.
pub fn load_or_generate(mesh_data_dir: &Path) -> Result<NodeIdentity> {
    let key_path = mesh_data_dir.join(KEY_FILE_NAME);

    if key_path.exists() {
        load_existing(&key_path)
    } else {
        generate_new(&key_path)
    }
}

/// Load an existing keypair from the protobuf-encoded key file.
fn load_existing(key_path: &Path) -> Result<NodeIdentity> {
    tracing::info!(path = %key_path.display(), "Loading existing mesh identity");

    let key_bytes = std::fs::read(key_path)
        .with_context(|| format!(
            "Failed to read identity key file: {}. \
             If the file is corrupted, delete it and restart — a new identity will be generated.",
            key_path.display()
        ))?;

    let keypair = Keypair::from_protobuf_encoding(&key_bytes)
        .map_err(|e| anyhow::anyhow!(
            "Failed to decode identity key from {}: {}. \
             The key file may be corrupted — delete it and restart to generate a new identity.",
            key_path.display(), e
        ))?;

    validate_ed25519(&keypair)?;
    let peer_id = PeerId::from_public_key(&keypair.public());

    tracing::info!(
        peer_id = %peer_id,
        key_path = %key_path.display(),
        "Loaded mesh identity"
    );

    Ok(NodeIdentity {
        keypair,
        peer_id,
        key_path: key_path.to_path_buf(),
    })
}

/// Generate a new Ed25519 keypair and persist it to disk.
fn generate_new(key_path: &Path) -> Result<NodeIdentity> {
    tracing::info!(path = %key_path.display(), "Generating new mesh identity");

    let keypair = Keypair::generate_ed25519();
    let peer_id = PeerId::from_public_key(&keypair.public());

    let key_bytes = keypair
        .to_protobuf_encoding()
        .map_err(|e| anyhow::anyhow!("Failed to encode keypair to protobuf: {}", e))?;

    write_key_file(key_path, &key_bytes)?;

    tracing::info!(
        peer_id = %peer_id,
        key_path = %key_path.display(),
        "Generated and persisted new mesh identity"
    );

    Ok(NodeIdentity {
        keypair,
        peer_id,
        key_path: key_path.to_path_buf(),
    })
}

/// Write the key bytes to disk with restricted permissions.
///
/// On Unix: file mode `0600` (owner read/write only).
/// On non-Unix: relies on OS-level file ACLs (documented limitation).
fn write_key_file(key_path: &Path, key_bytes: &[u8]) -> Result<()> {
    std::fs::write(key_path, key_bytes)
        .with_context(|| format!(
            "Failed to write identity key to: {}",
            key_path.display()
        ))?;

    set_file_permissions(key_path)?;

    Ok(())
}

/// Set restrictive file permissions on the key file.
///
/// Unix: `0600` — owner read/write only. No group, no other access.
/// Non-Unix: logs a warning that OS-level ACLs should be set manually.
fn set_file_permissions(key_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(key_path, perms)
            .with_context(|| format!(
                "Failed to set permissions (0600) on identity key: {}",
                key_path.display()
            ))?;
        tracing::debug!(
            path = %key_path.display(),
            mode = "0600",
            "Set restrictive permissions on identity key"
        );
    }

    #[cfg(not(unix))]
    {
        tracing::warn!(
            path = %key_path.display(),
            "Non-Unix platform: cannot set file mode 0600 on identity key. \
             Ensure the key file is protected by OS-level ACLs."
        );
    }

    Ok(())
}

/// Validate that the keypair is Ed25519 — the only algorithm ErnMesh supports.
fn validate_ed25519(keypair: &Keypair) -> Result<()> {
    if keypair.key_type() != libp2p_identity::KeyType::Ed25519 {
        anyhow::bail!(
            "Identity key is not Ed25519 (found {:?}). \
             ErnMesh requires Ed25519 — delete the key file and restart.",
            keypair.key_type()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_new_creates_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path())
            .expect("Must generate new identity");

        assert!(
            identity.key_path().exists(),
            "Key file must be persisted to disk"
        );
        assert_eq!(
            identity.key_path(),
            tmp.path().join(KEY_FILE_NAME)
        );
    }

    #[test]
    fn test_generate_new_produces_valid_peer_id() {
        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path()).unwrap();

        let expected_peer_id = PeerId::from_public_key(&identity.keypair().public());
        assert_eq!(
            identity.peer_id(), &expected_peer_id,
            "PeerId must match the keypair's public key"
        );
    }

    #[test]
    fn test_load_existing_returns_same_identity() {
        let tmp = tempfile::tempdir().unwrap();

        let first = load_or_generate(tmp.path()).unwrap();
        let second = load_or_generate(tmp.path()).unwrap();

        assert_eq!(
            first.peer_id(), second.peer_id(),
            "Loading the same key file must produce the same PeerId"
        );
    }

    #[test]
    fn test_peer_id_base58_format() {
        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path()).unwrap();

        let base58 = identity.peer_id_base58();
        assert!(
            !base58.is_empty(),
            "Base58 PeerId must not be empty"
        );
        assert!(
            base58.len() > 10,
            "Base58 PeerId must be a substantial string, got: {}",
            base58
        );
    }

    #[test]
    fn test_sign_produces_verifiable_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path()).unwrap();

        let message = b"ErnMesh test message";
        let signature = identity.sign(message)
            .expect("Signing must succeed");

        assert!(
            !signature.is_empty(),
            "Signature must not be empty"
        );

        // Verify using the public key
        let public_key = identity.keypair().public();
        let ed25519_pub = public_key
            .try_into_ed25519()
            .expect("Must be Ed25519");
        assert!(
            ed25519_pub.verify(message, &signature),
            "Signature must verify against the public key"
        );
    }

    #[test]
    fn test_sign_different_messages_produce_different_signatures() {
        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path()).unwrap();

        let sig_a = identity.sign(b"message A").unwrap();
        let sig_b = identity.sign(b"message B").unwrap();

        assert_ne!(
            sig_a, sig_b,
            "Different messages must produce different signatures"
        );
    }

    #[test]
    fn test_corrupted_key_file_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join(KEY_FILE_NAME);

        // Write garbage to simulate corruption
        std::fs::write(&key_path, b"this is not a valid protobuf key").unwrap();

        let result = load_or_generate(tmp.path());
        assert!(
            result.is_err(),
            "Corrupted key file must return error, not panic"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("corrupted") || err_msg.contains("decode"),
            "Error must mention corruption/decoding: got '{}'",
            err_msg
        );
    }

    #[test]
    fn test_empty_key_file_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let key_path = tmp.path().join(KEY_FILE_NAME);

        std::fs::write(&key_path, b"").unwrap();

        let result = load_or_generate(tmp.path());
        assert!(
            result.is_err(),
            "Empty key file must return error"
        );
    }

    #[test]
    fn test_two_generates_produce_different_peer_ids() {
        let tmp_a = tempfile::tempdir().unwrap();
        let tmp_b = tempfile::tempdir().unwrap();

        let id_a = load_or_generate(tmp_a.path()).unwrap();
        let id_b = load_or_generate(tmp_b.path()).unwrap();

        assert_ne!(
            id_a.peer_id(), id_b.peer_id(),
            "Two fresh generates must produce different PeerIds"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_key_file_has_restricted_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path()).unwrap();

        let metadata = std::fs::metadata(identity.key_path()).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "Key file must have mode 0600, got {:o}",
            mode
        );
    }

    #[test]
    fn test_keypair_is_ed25519() {
        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path()).unwrap();

        assert_eq!(
            identity.keypair().key_type(),
            libp2p_identity::KeyType::Ed25519,
            "Keypair must be Ed25519"
        );
    }

    #[test]
    fn test_key_file_roundtrip_preserves_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let identity = load_or_generate(tmp.path()).unwrap();

        let key_bytes_on_disk = std::fs::read(identity.key_path()).unwrap();
        let reloaded = Keypair::from_protobuf_encoding(&key_bytes_on_disk)
            .expect("Key file bytes must roundtrip through protobuf encoding");

        let original_peer_id = PeerId::from_public_key(&identity.keypair().public());
        let reloaded_peer_id = PeerId::from_public_key(&reloaded.public());

        assert_eq!(
            original_peer_id, reloaded_peer_id,
            "Roundtripped key must produce the same PeerId"
        );
    }
}
