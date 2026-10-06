use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PROJECT_FORMAT: &str = "signal-loom";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFile {
    pub format: String,
    pub version: u32,
    pub log_path: String,
    #[serde(default)]
    pub signal_map_path: Option<String>,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    pub view: ViewState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bookmark {
    pub id: String,
    pub t_us: u64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewState {
    pub playhead_us: u64,
    pub span_us: u64,
    pub plotted: Vec<String>,
}

impl ProjectFile {
    pub fn parse(text: &str) -> Result<Self> {
        let mut project: Self = serde_json::from_str(text)
            .map_err(|err| Error::msg(format!("project file is not valid JSON: {err}")))?;
        if project.format != PROJECT_FORMAT {
            return Err(Error::msg(format!(
                "not a Signal Loom project (format is '{}')",
                project.format
            )));
        }
        if project.version != 1 {
            return Err(Error::msg(format!(
                "project version {} is not supported",
                project.version
            )));
        }
        project
            .bookmarks
            .retain(|mark| !mark.label.trim().is_empty());
        for mark in &mut project.bookmarks {
            mark.label = mark.label.trim().chars().take(80).collect();
            if mark.id.trim().is_empty() {
                mark.id = format!("mark-{}", mark.t_us);
            }
        }
        Ok(project)
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|err| Error::msg(format!("could not encode project: {err}")))
    }
}

pub fn write_project(path: &Path, project: &ProjectFile) -> Result<()> {
    if project.format != PROJECT_FORMAT || project.version != 1 {
        return Err(Error::msg("refusing to write an unsupported project"));
    }
    let text = project.to_json()?;
    std::fs::write(path, text).map_err(|err| Error::write(path, err))
}

pub fn resolve_existing(base: &Path, stored: &str) -> Option<PathBuf> {
    let stored_path = Path::new(stored);
    if stored.is_empty() {
        return None;
    }
    let mut candidates = Vec::new();
    if stored_path.is_absolute() {
        candidates.push(stored_path.to_path_buf());
    } else {
        candidates.push(base.join(stored_path));
        if let Some(name) = stored_path.file_name() {
            candidates.push(base.join(name));
        }
        candidates.push(stored_path.to_path_buf());
        let mut dir = Some(base);
        while let Some(current) = dir {
            candidates.push(current.join(stored_path));
            dir = current.parent();
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}
