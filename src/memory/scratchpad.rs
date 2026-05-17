// Ern-OS — Tier 5: Scratchpad — pinned key-value notes

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScratchpadEntry {
    pub key: String,
    pub value: String,
    #[serde(default)]
    pub pinned: bool,
    /// Project ID for scoping entries to a writing project (Story Bible).
    #[serde(default)]
    pub project_id: Option<String>,
    /// Category within a project: character, world, timeline, theme, style.
    #[serde(default)]
    pub category: Option<String>,
}

pub struct ScratchpadStore {
    entries: Vec<ScratchpadEntry>,
    file_path: Option<PathBuf>,
}

impl ScratchpadStore {
    pub fn new() -> Self { Self { entries: Vec::new(), file_path: None } }

    pub fn open(path: &Path) -> Result<Self> {
        tracing::info!(module = "scratchpad", fn_name = "open", "scratchpad::open called");
        let mut store = Self { entries: Vec::new(), file_path: Some(path.to_path_buf()) };
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read scratchpad: {}", path.display()))?;
            store.entries = serde_json::from_str(&content)?;
        }
        Ok(store)
    }

    fn persist(&self) -> Result<()> {
        if let Some(ref path) = self.file_path {
            if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
            std::fs::write(path, serde_json::to_string_pretty(&self.entries)?)?;
        }
        Ok(())
    }

    pub fn pin(&mut self, key: &str, value: &str) -> Result<()> {
        tracing::info!(module = "scratchpad", fn_name = "pin", "scratchpad::pin called");
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.value = value.to_string();
            entry.pinned = true;
        } else {
            self.entries.push(ScratchpadEntry {
                key: key.to_string(), value: value.to_string(), pinned: true,
                project_id: None, category: None,
            });
        }
        self.persist()
    }

    /// Pin an entry scoped to a writing project with an optional category.
    pub fn pin_with_project(&mut self, key: &str, value: &str, project_id: &str, category: Option<&str>) -> Result<()> {
        let match_key = |e: &&mut ScratchpadEntry| e.key == key && e.project_id.as_deref() == Some(project_id);
        if let Some(entry) = self.entries.iter_mut().find(match_key) {
            entry.value = value.to_string();
            if let Some(cat) = category { entry.category = Some(cat.to_string()); }
        } else {
            self.entries.push(ScratchpadEntry {
                key: key.to_string(), value: value.to_string(), pinned: true,
                project_id: Some(project_id.to_string()),
                category: category.map(|c| c.to_string()),
            });
        }
        self.persist()
    }

    pub fn unpin(&mut self, key: &str) -> Result<()> {
        tracing::info!(module = "scratchpad", fn_name = "unpin", "scratchpad::unpin called");
        self.entries.retain(|e| e.key != key);
        self.persist()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|e| e.key == key).map(|e| e.value.as_str())
    }

    pub fn all(&self) -> &[ScratchpadEntry] { &self.entries }
    pub fn count(&self) -> usize { self.entries.len() }

    /// Filter entries belonging to a specific project.
    pub fn by_project(&self, project_id: &str) -> Vec<&ScratchpadEntry> {
        self.entries.iter().filter(|e| e.project_id.as_deref() == Some(project_id)).collect()
    }

    /// Filter entries by project AND category (e.g. all characters in a novel).
    pub fn by_project_and_category(&self, project_id: &str, category: &str) -> Vec<&ScratchpadEntry> {
        self.entries.iter()
            .filter(|e| e.project_id.as_deref() == Some(project_id) && e.category.as_deref() == Some(category))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_pin_get() {
        let mut store = ScratchpadStore::new();
        store.pin("lang", "Rust").unwrap();
        assert_eq!(store.get("lang"), Some("Rust"));
    }

    #[test]
    fn test_unpin() {
        let mut store = ScratchpadStore::new();
        store.pin("k", "v").unwrap();
        store.unpin("k").unwrap();
        assert!(store.get("k").is_none());
    }

    #[test]
    fn test_persist_reload() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("scratchpad.json");
        { let mut s = ScratchpadStore::open(&path).unwrap(); s.pin("a", "b").unwrap(); }
        { let s = ScratchpadStore::open(&path).unwrap(); assert_eq!(s.count(), 1); }
    }

    #[test]
    fn test_pin_with_project() {
        let mut store = ScratchpadStore::new();
        store.pin_with_project("Elena", "28, journalist", "novel-1", Some("character")).unwrap();
        assert_eq!(store.count(), 1);
        let entry = &store.all()[0];
        assert_eq!(entry.project_id.as_deref(), Some("novel-1"));
        assert_eq!(entry.category.as_deref(), Some("character"));
    }

    #[test]
    fn test_by_project() {
        let mut store = ScratchpadStore::new();
        store.pin_with_project("Elena", "journalist", "novel-1", Some("character")).unwrap();
        store.pin_with_project("The Archive", "server farm", "novel-1", Some("world")).unwrap();
        store.pin("unrelated", "global note").unwrap();
        assert_eq!(store.by_project("novel-1").len(), 2);
        assert_eq!(store.by_project("novel-2").len(), 0);
    }

    #[test]
    fn test_by_project_and_category() {
        let mut store = ScratchpadStore::new();
        store.pin_with_project("Elena", "journalist", "novel-1", Some("character")).unwrap();
        store.pin_with_project("Marcus", "editor", "novel-1", Some("character")).unwrap();
        store.pin_with_project("The Archive", "server farm", "novel-1", Some("world")).unwrap();
        assert_eq!(store.by_project_and_category("novel-1", "character").len(), 2);
        assert_eq!(store.by_project_and_category("novel-1", "world").len(), 1);
        assert_eq!(store.by_project_and_category("novel-1", "timeline").len(), 0);
    }

    #[test]
    fn test_pin_backward_compat_no_project() {
        // Entries created with pin() have None for project_id and category
        let mut store = ScratchpadStore::new();
        store.pin("old_note", "old_value").unwrap();
        let entry = &store.all()[0];
        assert!(entry.project_id.is_none());
        assert!(entry.category.is_none());
    }
}
