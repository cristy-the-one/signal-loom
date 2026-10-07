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
    #[serde(default)]
    pub math: Vec<MathChannel>,
    #[serde(default)]
    pub triggers: Vec<ThresholdTrigger>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub cursor_a_us: Option<u64>,
    #[serde(default)]
    pub cursor_b_us: Option<u64>,
    #[serde(default)]
    pub compare_path: Option<String>,
    #[serde(default)]
    pub compare_offset_us: i64,
    /// Message timeout in cycle times. Absent in older projects: 2.5.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_factor: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MathChannel {
    pub name: String,
    pub unit: String,
    pub expr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdTrigger {
    pub id: String,
    pub signal: String,
    pub op: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub t_us: u64,
    pub body: String,
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
        for channel in &project.math {
            if channel.name.trim().is_empty() {
                return Err(Error::msg("a math channel is missing a name"));
            }
            crate::analyze::compile(&channel.expr)?;
        }
        for note in &mut project.notes {
            note.body = note.body.trim().chars().take(2_000).collect();
        }
        project.notes.retain(|note| !note.body.is_empty());
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
    write_as(path, "loom", &text)
}

/// Write `text` to `path`, which must end in `.{extension}`. The app only
/// writes its own file types, so a webview cannot clobber anything else.
pub fn write_as(path: &Path, extension: &str, text: &str) -> Result<()> {
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
    {
        return Err(Error::msg(format!(
            "this file is saved only as a .{extension} file"
        )));
    }
    std::fs::write(path, text).map_err(|err| Error::write(path, err))
}

/// `\\server\share\…` or `//server/…`. On Windows, even checking such a path
/// makes the PC authenticate to that server, so a path read from a project
/// file is never followed there. The user can still open one with Open.
pub fn is_network_path(stored: &str) -> bool {
    let stored = stored.trim();
    stored.starts_with("\\\\") || stored.starts_with("//")
}

/// Read a text file the user pointed at, refusing one over `cap` bytes.
pub fn read_text_capped(path: &Path, cap: u64, what: &str) -> Result<String> {
    let len = std::fs::metadata(path)
        .map_err(|err| Error::read(path, err))?
        .len();
    if len > cap {
        return Err(Error::msg(format!(
            "{} is {} MB; a {what} is at most {} MB",
            path.display(),
            len / (1024 * 1024),
            cap / (1024 * 1024)
        )));
    }
    std::fs::read_to_string(path).map_err(|err| Error::read(path, err))
}

pub fn resolve_existing(base: &Path, stored: &str) -> Option<PathBuf> {
    let stored_path = Path::new(stored);
    if stored.is_empty() || is_network_path(stored) {
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
