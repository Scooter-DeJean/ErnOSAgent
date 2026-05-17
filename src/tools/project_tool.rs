// Ern-OS — Project tool for long-form writing projects
//! Manages writing projects (novels, journalism, research).
//! Delegates to ScratchpadStore for Story Bible, DocumentStore for manuscript chunks.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Persistent project metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub id: String,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Create a new project directory and metadata file. Returns the project ID.
pub fn create_project(data_dir: &Path, name: &str) -> Result<ProjectMeta> {
    let id = uuid::Uuid::new_v4().to_string();
    let project_dir = data_dir.join("projects").join(&id);
    std::fs::create_dir_all(&project_dir)
        .with_context(|| format!("Failed to create project dir: {}", project_dir.display()))?;

    let meta = ProjectMeta {
        id: id.clone(),
        name: name.to_string(),
        created_at: chrono::Utc::now(),
    };
    let meta_path = project_dir.join("meta.json");
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)?)?;
    tracing::info!(project_id = %id, name = %name, "Project created");
    Ok(meta)
}

/// Load project metadata from disk.
pub fn load_project(data_dir: &Path, project_id: &str) -> Result<Option<ProjectMeta>> {
    let meta_path = data_dir.join("projects").join(project_id).join("meta.json");
    if !meta_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&meta_path)?;
    let meta: ProjectMeta = serde_json::from_str(&content)?;
    Ok(Some(meta))
}

/// List all projects from the data directory.
pub fn list_projects(data_dir: &Path) -> Result<Vec<ProjectMeta>> {
    let projects_dir = data_dir.join("projects");
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }
    let mut projects = Vec::new();
    for entry in std::fs::read_dir(&projects_dir)? {
        let path = entry?.path();
        let meta_path = path.join("meta.json");
        if meta_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<ProjectMeta>(&content) {
                    projects.push(meta);
                }
            }
        }
    }
    projects.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(projects)
}

/// Get the project directory path for a given project ID.
pub fn project_dir(data_dir: &Path, project_id: &str) -> PathBuf {
    data_dir.join("projects").join(project_id)
}

/// Execute the project tool action. Returns a formatted result string.
pub fn execute_bible(
    scratchpad: &mut crate::memory::scratchpad::ScratchpadStore,
    project_id: &str,
    action: &str,
    key: Option<&str>,
    value: Option<&str>,
    category: Option<&str>,
    query: Option<&str>,
) -> Result<String> {
    match action {
        "add" => {
            let k = key.context("bible add requires 'key'")?;
            let v = value.context("bible add requires 'value'")?;
            scratchpad.pin_with_project(k, v, project_id, category)?;
            let cat_label = category.unwrap_or("uncategorized");
            Ok(format!("Added to bible [{}]: '{}' = '{}'", cat_label, k, v))
        }
        "list" => {
            let entries = if let Some(cat) = category {
                scratchpad.by_project_and_category(project_id, cat)
            } else {
                scratchpad.by_project(project_id)
            };
            if entries.is_empty() {
                return Ok("Story bible is empty for this project.".to_string());
            }
            let mut out = format!("Story Bible ({} entries):\n", entries.len());
            for e in &entries {
                let cat = e.category.as_deref().unwrap_or("—");
                out.push_str(&format!("  [{}] {}: {}\n", cat, e.key, e.value));
            }
            Ok(out)
        }
        "search" => {
            let q = query.unwrap_or("").to_lowercase();
            let entries = scratchpad.by_project(project_id);
            let matches: Vec<_> = entries.iter()
                .filter(|e| e.key.to_lowercase().contains(&q) || e.value.to_lowercase().contains(&q))
                .collect();
            if matches.is_empty() {
                return Ok(format!("No bible entries matching '{}'", q));
            }
            let mut out = format!("Bible search '{}' ({} matches):\n", q, matches.len());
            for e in &matches {
                let cat = e.category.as_deref().unwrap_or("—");
                out.push_str(&format!("  [{}] {}: {}\n", cat, e.key, e.value));
            }
            Ok(out)
        }
        "remove" => {
            let k = key.context("bible remove requires 'key'")?;
            scratchpad.unpin(k)?;
            Ok(format!("Removed '{}' from bible", k))
        }
        other => Ok(format!("Unknown bible action: {}", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_project() {
        let tmp = TempDir::new().unwrap();
        let meta = create_project(tmp.path(), "My Novel").unwrap();
        assert_eq!(meta.name, "My Novel");
        assert!(tmp.path().join("projects").join(&meta.id).join("meta.json").exists());
    }

    #[test]
    fn test_load_project() {
        let tmp = TempDir::new().unwrap();
        let meta = create_project(tmp.path(), "Test").unwrap();
        let loaded = load_project(tmp.path(), &meta.id).unwrap().unwrap();
        assert_eq!(loaded.name, "Test");
    }

    #[test]
    fn test_load_missing_project() {
        let tmp = TempDir::new().unwrap();
        let loaded = load_project(tmp.path(), "nonexistent").unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn test_list_projects() {
        let tmp = TempDir::new().unwrap();
        create_project(tmp.path(), "Novel A").unwrap();
        create_project(tmp.path(), "Novel B").unwrap();
        let list = list_projects(tmp.path()).unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_bible_add_list() {
        let mut sp = crate::memory::scratchpad::ScratchpadStore::new();
        let result = execute_bible(&mut sp, "p1", "add", Some("Elena"), Some("journalist"), Some("character"), None).unwrap();
        assert!(result.contains("Added to bible"));

        let list = execute_bible(&mut sp, "p1", "list", None, None, None, None).unwrap();
        assert!(list.contains("Elena"));
        assert!(list.contains("character"));
    }

    #[test]
    fn test_bible_search() {
        let mut sp = crate::memory::scratchpad::ScratchpadStore::new();
        execute_bible(&mut sp, "p1", "add", Some("Elena"), Some("journalist"), Some("character"), None).unwrap();
        execute_bible(&mut sp, "p1", "add", Some("The Archive"), Some("server farm"), Some("world"), None).unwrap();

        let result = execute_bible(&mut sp, "p1", "search", None, None, None, Some("elena")).unwrap();
        assert!(result.contains("Elena"));
        assert!(!result.contains("Archive"));
    }

    #[test]
    fn test_bible_list_by_category() {
        let mut sp = crate::memory::scratchpad::ScratchpadStore::new();
        execute_bible(&mut sp, "p1", "add", Some("Elena"), Some("journalist"), Some("character"), None).unwrap();
        execute_bible(&mut sp, "p1", "add", Some("Archive"), Some("server farm"), Some("world"), None).unwrap();

        let chars = execute_bible(&mut sp, "p1", "list", None, None, Some("character"), None).unwrap();
        assert!(chars.contains("Elena"));
        assert!(!chars.contains("Archive"));
    }
}
