use crate::analyze::{compile, Compiled};
use crate::error::{Error, Result};
use crate::index::IndexedLog;
use crate::map::TimeoutFactor;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
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
    /// Cluster slot (`speed`, `lamp1`, …) to the signal shown in it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub cluster: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MathChannel {
    pub name: String,
    pub unit: String,
    pub expr: String,
}

impl MathChannel {
    /// Whether this channel can join `channels`: a name of 1 to 64 characters
    /// that the compare suffix cannot be confused with, and an expression that
    /// compiles over signals from the log.
    pub(crate) fn validate(&self, channels: &[MathChannel]) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() || name.len() > 64 {
            return Err(Error::invalid(
                "math channel name must be 1 to 64 characters",
            ));
        }
        if name.ends_with(" · B") {
            return Err(Error::invalid(
                "math channel names cannot end with the compare suffix",
            ));
        }
        compile_math(channels, self).map(drop)
    }
}

/// Compile a math channel. A math channel can use only signals from the log:
/// naming another math channel is an error here, wherever it is evaluated, not
/// a dependency that is quietly dropped on one path and fails on another.
pub(crate) fn compile_math(channels: &[MathChannel], channel: &MathChannel) -> Result<Compiled> {
    let compiled = compile(&channel.expr)?;
    if let Some(dep) = compiled
        .dependencies()
        .iter()
        .find(|dep| channels.iter().any(|other| other.name == **dep))
    {
        return Err(Error::invalid(format!(
            "math channel {} uses math channel {dep}. A math channel can only use signals from the log, so write the expression of {dep} into it",
            channel.name
        )));
    }
    Ok(compiled)
}

/// How a trigger compares a sample to its level. The wire form is the symbol;
/// the word forms are older spellings that still load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerOp {
    #[serde(rename = ">", alias = "gt")]
    Gt,
    #[serde(rename = "<", alias = "lt")]
    Lt,
    #[serde(rename = ">=", alias = "ge")]
    Ge,
    #[serde(rename = "<=", alias = "le")]
    Le,
}

impl TriggerOp {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Gt => ">",
            Self::Lt => "<",
            Self::Ge => ">=",
            Self::Le => "<=",
        }
    }

    pub fn holds(self, value: f64, level: f64) -> bool {
        match self {
            Self::Gt => value > level,
            Self::Lt => value < level,
            Self::Ge => value >= level,
            Self::Le => value <= level,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThresholdTrigger {
    pub id: String,
    pub signal: String,
    pub op: TriggerOp,
    pub value: f64,
}

impl ThresholdTrigger {
    /// A finite level on a decoded signal. Math channels are not decoded
    /// signals, so a trigger cannot target one. Without a `log` the signal
    /// cannot be looked up and only the level is checked.
    pub(crate) fn validate(&self, log: Option<&IndexedLog>, math: &[MathChannel]) -> Result<()> {
        if !self.value.is_finite() {
            return Err(Error::invalid("trigger level must be finite"));
        }
        if log.is_some_and(|log| !log.has_signal(&self.signal)) {
            let why = if math.iter().any(|channel| channel.name == self.signal) {
                "triggers work on logged signals, not math channels"
            } else {
                "no such signal in this log"
            };
            return Err(Error::invalid(format!(
                "trigger signal {}: {why}",
                self.signal
            )));
        }
        Ok(())
    }
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

/// The part of a project a problem is about.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Item {
    Project,
    Math(usize),
    Trigger(usize),
    Timeout,
}

/// One reason a project would not load cleanly.
#[derive(Debug)]
pub(crate) struct Problem {
    item: Item,
    label: String,
    reason: String,
}

impl Problem {
    fn new(item: Item, label: impl Into<String>, reason: impl fmt::Display) -> Self {
        Self {
            item,
            label: label.into(),
            reason: reason.to_string(),
        }
    }

    /// The note shown when a project opens without the part.
    fn warning(&self) -> String {
        match self.item {
            Item::Timeout => format!(
                "{} was not used: {}. Using the default, {}.",
                self.label,
                self.reason,
                TimeoutFactor::default().get()
            ),
            _ => format!("{} was not loaded: {}", self.label, self.reason),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.label, self.reason)
    }
}

/// A cluster has a dozen slots; keep only short, non-empty assignments.
fn keeps_cluster_slot(slot: &str, signal: &str) -> bool {
    slot.len() <= 16 && !signal.trim().is_empty() && signal.len() <= 128
}

impl ProjectFile {
    /// Read a project as a user opens one. A math channel or trigger that does
    /// not parse or does not validate is dropped and reported, so one bad entry
    /// does not fail the whole project; an out-of-range timeout is reported and
    /// the default applies. The caller checks the triggers against its log with
    /// `drop_invalid` once it has one.
    pub(crate) fn read(text: &str) -> Result<(Self, Vec<String>)> {
        let (text, mut warnings) = drop_unreadable(text);
        let mut project = Self::parse(&text)?;
        warnings.extend(project.drop_invalid(None));
        Ok((project, warnings))
    }

    /// Parse the file and tidy it. Whether its math channels, triggers and
    /// timeout are usable is `validate`'s question.
    pub fn parse(text: &str) -> Result<Self> {
        let mut project: Self = serde_json::from_str(text)
            .map_err(|err| Error::invalid(format!("project file is not valid JSON: {err}")))?;
        project
            .cluster
            .retain(|slot, signal| keeps_cluster_slot(slot, signal));
        if let Some(problem) = project.shape_problems().into_iter().next() {
            return Err(Error::invalid(problem.reason));
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
        for note in &mut project.notes {
            note.body = note.body.trim().chars().take(2_000).collect();
        }
        project.notes.retain(|note| !note.body.is_empty());
        Ok(project)
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|err| Error::internal(format!("could not encode project: {err}")))
    }

    /// The timeout to index with: the project's, or the default when it has
    /// none or `drop_invalid` removed it.
    pub(crate) fn timeout(&self) -> TimeoutFactor {
        self.timeout_factor
            .and_then(|factor| TimeoutFactor::new(factor).ok())
            .unwrap_or_default()
    }

    /// Everything that keeps this project from loading cleanly. The rules are
    /// the ones the session applies when the same things are set one by one.
    /// `log`, when there is one, is what the triggers must fit.
    pub(crate) fn validate(&self, log: Option<&IndexedLog>) -> Vec<Problem> {
        let mut problems = self.shape_problems();
        problems.extend(self.item_problems(log));
        problems
    }

    /// Problems with the file as a whole; a project with one cannot load.
    fn shape_problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        if self.format != PROJECT_FORMAT {
            problems.push(Problem::new(
                Item::Project,
                "Format",
                format!("not a Signal Loom project (format is '{}')", self.format),
            ));
        } else if self.version != 1 {
            problems.push(Problem::new(
                Item::Project,
                "Version",
                format!("project version {} is not supported", self.version),
            ));
        }
        let slots = self
            .cluster
            .iter()
            .filter(|(slot, signal)| keeps_cluster_slot(slot, signal))
            .count();
        if slots > 32 {
            problems.push(Problem::new(
                Item::Project,
                "Cluster",
                "project assigns more than 32 cluster slots",
            ));
        }
        problems
    }

    /// Problems with one math channel, trigger or the timeout: each can be
    /// dropped and the rest of the project still loads.
    fn item_problems(&self, log: Option<&IndexedLog>) -> Vec<Problem> {
        let mut problems = Vec::new();
        for (at, channel) in self.math.iter().enumerate() {
            if let Err(err) = channel.validate(&self.math) {
                let name = channel.name.trim();
                let label = if name.is_empty() {
                    format!("Math channel #{}", at + 1)
                } else {
                    format!("Math channel {name}")
                };
                problems.push(Problem::new(Item::Math(at), label, err));
            }
        }
        for (at, trigger) in self.triggers.iter().enumerate() {
            if let Err(err) = trigger.validate(log, &self.math) {
                problems.push(Problem::new(
                    Item::Trigger(at),
                    format!("Trigger {}", trigger.id),
                    err,
                ));
            }
        }
        if let Some(factor) = self.timeout_factor {
            if let Err(err) = TimeoutFactor::new(factor) {
                problems.push(Problem::new(
                    Item::Timeout,
                    format!("Timeout {factor}"),
                    err,
                ));
            }
        }
        problems
    }

    /// Remove the math channels and triggers that fail `validate`, and an
    /// out-of-range timeout. Returns one warning for each.
    pub(crate) fn drop_invalid(&mut self, log: Option<&IndexedLog>) -> Vec<String> {
        let problems = self.item_problems(log);
        let flagged = |item: Item| problems.iter().any(|problem| problem.item == item);
        let mut at = 0;
        self.math.retain(|_| {
            at += 1;
            !flagged(Item::Math(at - 1))
        });
        let mut at = 0;
        self.triggers.retain(|_| {
            at += 1;
            !flagged(Item::Trigger(at - 1))
        });
        if flagged(Item::Timeout) {
            self.timeout_factor = None;
        }
        problems.iter().map(Problem::warning).collect()
    }
}

/// Remove math channels and triggers that do not parse (an unknown comparison,
/// a missing field), so one bad entry does not fail the whole project. Each
/// one is reported.
fn drop_unreadable(text: &str) -> (Cow<'_, str>, Vec<String>) {
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(text) else {
        return (Cow::Borrowed(text), Vec::new());
    };
    let mut warnings = Vec::new();
    drop_unreadable_in::<MathChannel>(&mut root, "math", "Math channel", "name", &mut warnings);
    drop_unreadable_in::<ThresholdTrigger>(&mut root, "triggers", "Trigger", "id", &mut warnings);
    if warnings.is_empty() {
        return (Cow::Borrowed(text), warnings);
    }
    (Cow::Owned(root.to_string()), warnings)
}

fn drop_unreadable_in<T: DeserializeOwned>(
    root: &mut serde_json::Value,
    key: &str,
    label: &str,
    name_field: &str,
    warnings: &mut Vec<String>,
) {
    let Some(items) = root.get_mut(key).and_then(serde_json::Value::as_array_mut) else {
        return;
    };
    for (at, item) in std::mem::take(items).into_iter().enumerate() {
        match serde_json::from_value::<T>(item.clone()) {
            Ok(_) => items.push(item),
            Err(err) => {
                let name = item
                    .get(name_field)
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| format!("#{}", at + 1), str::to_string);
                warnings.push(format!("{label} {name} was not loaded: {err}"));
            }
        }
    }
}

/// Write a project to a `.loom` file, refusing one that would not load
/// cleanly. `log` is the open log the triggers must fit, when there is one.
pub fn write_project(path: &Path, project: &ProjectFile, log: Option<&IndexedLog>) -> Result<()> {
    let problems = project.validate(log);
    if !problems.is_empty() {
        let list: Vec<String> = problems.iter().map(Problem::to_string).collect();
        return Err(Error::invalid(format!(
            "this project would not load cleanly, so it was not saved. {}",
            list.join("; ")
        )));
    }
    let text = project.to_json()?;
    write_as(path, "loom", &text)
}

/// Write `text` to `path`, which must end in `.{extension}`. The app only
/// writes its own file types, so a webview cannot clobber anything else. The
/// text goes to a temporary file beside `path` first and is renamed over it, so
/// an interrupted save leaves the old file, not half of a new one.
pub fn write_as(path: &Path, extension: &str, text: &str) -> Result<()> {
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case(extension))
    {
        return Err(Error::invalid(format!(
            "this file is saved only as a .{extension} file"
        )));
    }
    let name = path
        .file_name()
        .ok_or_else(|| Error::invalid("this file has no name"))?;
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        std::process::id()
    ));
    let written = std::fs::File::create(&temp)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temp, path));
    written.map_err(|err| {
        let _ = std::fs::remove_file(&temp);
        Error::write(path, err)
    })
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
        return Err(Error::invalid(format!(
            "{} is {} MB; a {what} is at most {} MB",
            path.display(),
            len / (1024 * 1024),
            cap / (1024 * 1024)
        )));
    }
    std::fs::read_to_string(path).map_err(|err| Error::read(path, err))
}

/// Where a path stored in a project points.
pub(crate) enum Located {
    Found(PathBuf),
    /// Nothing is there, or it is a network path, which is never followed.
    Missing,
    /// A relative path and no project folder to resolve it against. The
    /// engine's working directory is not a guess worth making.
    NoBase,
}

/// Find the file a project stored `stored` for. A relative path is looked for
/// in `base`, the project's folder, and above it.
pub(crate) fn locate(base: Option<&Path>, stored: &str) -> Located {
    let stored_path = Path::new(stored);
    if stored.is_empty() || is_network_path(stored) {
        return Located::Missing;
    }
    let mut candidates = Vec::new();
    if stored_path.is_absolute() {
        candidates.push(stored_path.to_path_buf());
    } else if let Some(base) = base {
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
    } else {
        return Located::NoBase;
    }
    match candidates.into_iter().find(|path| path.is_file()) {
        Some(path) => Located::Found(path),
        None => Located::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> ProjectFile {
        ProjectFile::parse(
            r#"{"format":"signal-loom","version":1,"logPath":"drive.slog",
            "view":{"playheadUs":0,"spanUs":1000000,"plotted":[]}}"#,
        )
        .unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loom-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_saved_project_replaces_the_old_one_and_leaves_no_temporary_file() {
        let dir = scratch("atomic");
        let target = dir.join("drive.loom");
        std::fs::write(&target, "old").unwrap();
        let mut project = minimal();
        project.timeout_factor = Some(4.0);
        write_project(&target, &project, None).unwrap();
        assert_eq!(names(&dir), ["drive.loom"]);
        let written = std::fs::read_to_string(&target).unwrap();
        assert_eq!(
            ProjectFile::parse(&written).unwrap().timeout_factor,
            Some(4.0)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_save_removes_its_temporary_file_and_reports_the_target() {
        let dir = scratch("failed");
        // A directory cannot be replaced by a file, so the rename fails.
        let target = dir.join("drive.loom");
        std::fs::create_dir(&target).unwrap();
        let err = write_project(&target, &minimal(), None).unwrap_err();
        assert!(
            err.to_string()
                .starts_with(&format!("could not write {}: ", target.display())),
            "{err}"
        );
        assert_eq!(names(&dir), ["drive.loom"]);
        assert!(target.is_dir());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_project_the_loader_would_not_accept_is_refused_before_anything_is_written() {
        let dir = scratch("refuse");
        let target = dir.join("drive.loom");
        let mut project = minimal();
        project.timeout_factor = Some(f64::NAN);
        project.triggers = vec![ThresholdTrigger {
            id: "t1".into(),
            signal: "Speed".into(),
            op: TriggerOp::Gt,
            value: f64::INFINITY,
        }];
        project.math = vec![MathChannel {
            name: "a".repeat(65),
            unit: String::new(),
            expr: "Speed".into(),
        }];
        for at in 0..33 {
            project.cluster.insert(format!("slot{at}"), "Speed".into());
        }
        project.version = 2;
        let err = write_project(&target, &project, None)
            .unwrap_err()
            .to_string();
        let long = "a".repeat(65);
        let expected = [
            "Version: project version 2 is not supported".to_string(),
            "Cluster: project assigns more than 32 cluster slots".to_string(),
            format!("Math channel {long}: math channel name must be 1 to 64 characters"),
            "Trigger t1: trigger level must be finite".to_string(),
            "Timeout NaN: timeout must be between 1 and 100 cycle times".to_string(),
        ];
        assert_eq!(
            err,
            format!(
                "this project would not load cleanly, so it was not saved. {}",
                expected.join("; ")
            )
        );
        assert!(names(&dir).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn dropping_invalid_items_keeps_the_valid_ones_in_order() {
        let mut project = minimal();
        project.math = ["One", "Two ·B", "Three · B", "Four"]
            .iter()
            .map(|name| MathChannel {
                name: name.to_string(),
                unit: String::new(),
                expr: "Speed".into(),
            })
            .collect();
        project.timeout_factor = Some(101.0);
        let warnings = project.drop_invalid(None);
        let kept: Vec<&str> = project.math.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(kept, ["One", "Two ·B", "Four"]);
        assert_eq!(project.timeout_factor, None);
        assert_eq!(
            warnings,
            [
                "Math channel Three · B was not loaded: math channel names cannot end with the compare suffix",
                "Timeout 101 was not used: timeout must be between 1 and 100 cycle times. Using the default, 2.5."
            ]
        );
        assert!(
            project.drop_invalid(None).is_empty(),
            "a second pass finds nothing"
        );
    }

    #[test]
    fn the_timeout_range_is_one_closed_interval() {
        assert_eq!(TimeoutFactor::new(1.0).unwrap().get(), 1.0);
        assert_eq!(TimeoutFactor::new(100.0).unwrap().get(), 100.0);
        for bad in [0.99, 100.01, f64::NAN, f64::INFINITY, -2.5] {
            assert!(TimeoutFactor::new(bad).is_err(), "{bad}");
        }
        assert_eq!(TimeoutFactor::default().get(), 2.5);
    }
}
