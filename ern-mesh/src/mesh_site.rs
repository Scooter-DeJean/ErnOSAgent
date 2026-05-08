// Ern-OS — High-performance, model-neutral Rust AI agent engine
// Created by @mettamazza (github.com/mettamazza)
// License: MIT
//! Mesh sites — decentralised web page hosting on ErnMesh.
//!
//! Mesh sites are the ErnMesh equivalent of websites:
//!
//! - **Hosted by peers**: Any node can host a mesh site by publishing
//!   its content to the content-addressable store and registering a
//!   human-readable name via the DHT.
//! - **Content-addressed**: Site assets (HTML, CSS, JS, images) are
//!   stored as content-addressable blobs. The site manifest links
//!   CIDs to filenames.
//! - **Replicated**: Popular sites are replicated across multiple
//!   pinning nodes for availability. Replication earns ErnPoints.
//! - **Versioned**: Each site publish creates a new version. The DHT
//!   always points to the latest version.
//!
//! ## Addressing
//!
//! Sites are addressed as `ern://{site_name}` in the ErnMesh browser.
//! The site name is a human-readable identifier registered via DHT.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A mesh site manifest — maps filenames to content CIDs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteManifest {
    /// The site's human-readable name (e.g., "my-blog").
    pub name: String,
    /// The PeerId of the site owner/publisher.
    pub owner: String,
    /// Site version (monotonically increasing).
    pub version: u64,
    /// Description of the site.
    pub description: String,
    /// Mapping of file paths → content CIDs.
    pub files: HashMap<String, String>,
    /// The default entry point (e.g., "index.html").
    pub index_file: String,
    /// When this version was published.
    pub published_at: std::time::SystemTime,
}

impl SiteManifest {
    /// Create a new site manifest.
    pub fn new(name: &str, owner: &str, description: &str) -> Self {
        Self {
            name: name.to_string(),
            owner: owner.to_string(),
            version: 1,
            description: description.to_string(),
            files: HashMap::new(),
            index_file: "index.html".to_string(),
            published_at: std::time::SystemTime::now(),
        }
    }

    /// Add a file to the manifest.
    pub fn add_file(&mut self, path: &str, cid: &str) {
        self.files.insert(path.to_string(), cid.to_string());
    }

    /// Set the index/entry file.
    pub fn set_index(&mut self, path: &str) {
        self.index_file = path.to_string();
    }

    /// Increment the version for a new publish.
    pub fn next_version(&mut self) {
        self.version += 1;
        self.published_at = std::time::SystemTime::now();
    }

    /// Get the CID for a file path.
    pub fn get_file_cid(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(|s| s.as_str())
    }

    /// Get the CID for the index file.
    pub fn index_cid(&self) -> Option<&str> {
        self.get_file_cid(&self.index_file)
    }

    /// Total number of files in the site.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }
}

/// Local site registry — tracks sites this node hosts.
#[derive(Debug, Default)]
pub struct SiteRegistry {
    /// Sites indexed by name.
    sites: HashMap<String, SiteManifest>,
}

impl SiteRegistry {
    /// Create a new empty site registry.
    pub fn new() -> Self {
        Self {
            sites: HashMap::new(),
        }
    }

    /// Register or update a site.
    pub fn publish(&mut self, manifest: SiteManifest) -> Result<()> {
        if manifest.name.is_empty() {
            anyhow::bail!("Site name cannot be empty");
        }
        if manifest.files.is_empty() {
            anyhow::bail!("Site must have at least one file");
        }
        if manifest.index_cid().is_none() {
            anyhow::bail!(
                "Index file '{}' not found in manifest files",
                manifest.index_file
            );
        }

        tracing::info!(
            name = %manifest.name,
            version = manifest.version,
            files = manifest.file_count(),
            "Mesh site published"
        );

        self.sites.insert(manifest.name.clone(), manifest);
        Ok(())
    }

    /// Get a site by name.
    pub fn get_site(&self, name: &str) -> Option<&SiteManifest> {
        self.sites.get(name)
    }

    /// Remove a site.
    pub fn remove_site(&mut self, name: &str) -> bool {
        if self.sites.remove(name).is_some() {
            tracing::info!(name = name, "Mesh site removed");
            true
        } else {
            false
        }
    }

    /// List all hosted site names.
    pub fn list_sites(&self) -> Vec<&str> {
        self.sites.keys().map(|s| s.as_str()).collect()
    }

    /// Total number of hosted sites.
    pub fn count(&self) -> usize {
        self.sites.len()
    }
}

/// Resolve a mesh site URL to its components.
///
/// Format: `ern://site-name/path/to/file`
/// Returns `(site_name, file_path)`.
pub fn resolve_mesh_url(url: &str) -> Result<(String, String)> {
    let stripped = url
        .strip_prefix("ern://")
        .ok_or_else(|| anyhow::anyhow!("Invalid mesh URL — must start with 'ern://': {}", url))?;

    let mut parts = stripped.splitn(2, '/');
    let site_name = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing site name in URL: {}", url))?
        .to_string();

    let file_path = parts.next().unwrap_or("index.html").to_string();

    if site_name.is_empty() {
        anyhow::bail!("Empty site name in URL: {}", url);
    }

    Ok((site_name, file_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_new() {
        let manifest = SiteManifest::new("my-blog", "peer_a", "A cool blog");
        assert_eq!(manifest.name, "my-blog");
        assert_eq!(manifest.owner, "peer_a");
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.index_file, "index.html");
        assert_eq!(manifest.file_count(), 0);
    }

    #[test]
    fn test_manifest_add_files() {
        let mut manifest = SiteManifest::new("site", "owner", "desc");
        manifest.add_file("index.html", "QmHash1");
        manifest.add_file("style.css", "QmHash2");
        manifest.add_file("app.js", "QmHash3");

        assert_eq!(manifest.file_count(), 3);
        assert_eq!(manifest.get_file_cid("index.html"), Some("QmHash1"));
        assert_eq!(manifest.index_cid(), Some("QmHash1"));
    }

    #[test]
    fn test_manifest_versioning() {
        let mut manifest = SiteManifest::new("site", "owner", "desc");
        assert_eq!(manifest.version, 1);

        manifest.next_version();
        assert_eq!(manifest.version, 2);

        manifest.next_version();
        assert_eq!(manifest.version, 3);
    }

    #[test]
    fn test_manifest_set_index() {
        let mut manifest = SiteManifest::new("site", "owner", "desc");
        manifest.add_file("home.html", "QmHome");
        manifest.set_index("home.html");
        assert_eq!(manifest.index_cid(), Some("QmHome"));
    }

    #[test]
    fn test_registry_publish() {
        let mut registry = SiteRegistry::new();
        let mut manifest = SiteManifest::new("my-site", "peer_a", "desc");
        manifest.add_file("index.html", "QmHash1");

        assert!(registry.publish(manifest).is_ok());
        assert_eq!(registry.count(), 1);
    }

    #[test]
    fn test_registry_publish_empty_name_fails() {
        let mut registry = SiteRegistry::new();
        let mut manifest = SiteManifest::new("", "peer_a", "desc");
        manifest.add_file("index.html", "QmHash1");

        assert!(registry.publish(manifest).is_err());
    }

    #[test]
    fn test_registry_publish_no_files_fails() {
        let mut registry = SiteRegistry::new();
        let manifest = SiteManifest::new("site", "peer_a", "desc");

        assert!(registry.publish(manifest).is_err());
    }

    #[test]
    fn test_registry_publish_missing_index_fails() {
        let mut registry = SiteRegistry::new();
        let mut manifest = SiteManifest::new("site", "peer_a", "desc");
        manifest.add_file("other.html", "QmHash");
        // index_file is "index.html" but that file isn't in the manifest

        assert!(registry.publish(manifest).is_err());
    }

    #[test]
    fn test_registry_get_site() {
        let mut registry = SiteRegistry::new();
        let mut manifest = SiteManifest::new("my-site", "peer_a", "desc");
        manifest.add_file("index.html", "QmHash1");
        registry.publish(manifest).unwrap();

        let site = registry.get_site("my-site").unwrap();
        assert_eq!(site.owner, "peer_a");
    }

    #[test]
    fn test_registry_remove_site() {
        let mut registry = SiteRegistry::new();
        let mut manifest = SiteManifest::new("my-site", "peer_a", "desc");
        manifest.add_file("index.html", "QmHash1");
        registry.publish(manifest).unwrap();

        assert!(registry.remove_site("my-site"));
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_resolve_mesh_url_basic() {
        let (name, path) = resolve_mesh_url("ern://my-blog/index.html").unwrap();
        assert_eq!(name, "my-blog");
        assert_eq!(path, "index.html");
    }

    #[test]
    fn test_resolve_mesh_url_default_index() {
        let (name, path) = resolve_mesh_url("ern://my-blog").unwrap();
        assert_eq!(name, "my-blog");
        assert_eq!(path, "index.html");
    }

    #[test]
    fn test_resolve_mesh_url_nested_path() {
        let (name, path) = resolve_mesh_url("ern://my-blog/assets/style.css").unwrap();
        assert_eq!(name, "my-blog");
        assert_eq!(path, "assets/style.css");
    }

    #[test]
    fn test_resolve_mesh_url_invalid_prefix() {
        let result = resolve_mesh_url("https://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_mesh_url_empty_name() {
        let result = resolve_mesh_url("ern:///path");
        assert!(result.is_err());
    }

    #[test]
    fn test_manifest_serialize_roundtrip() {
        let mut manifest = SiteManifest::new("test", "peer_a", "A test site");
        manifest.add_file("index.html", "QmHash");

        let json = serde_json::to_string(&manifest).unwrap();
        let recovered: SiteManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(recovered.name, "test");
        assert_eq!(recovered.file_count(), 1);
    }
}
