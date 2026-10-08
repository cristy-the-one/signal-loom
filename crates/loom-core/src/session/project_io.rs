//! Projects: opening a `.loom` file with its paths resolved, and saving one.
//!
//! The session owns the deck of a saved project: math, triggers, timeout, the
//! map and compare paths and the compare offset. The UI hands over only what
//! it owns (`ProjectView`) and the session fills in the rest on save.

use super::compare::Compare;
use super::maps::{read_map_file, MapSet};
use super::{sample, uses_map, Session};
use crate::dto::ProjectOpen;
use crate::error::{Error, Result};
use crate::index::IndexedLog;
use crate::project::{self, Bookmark, Located, Note, ProjectFile, ViewState, PROJECT_FORMAT};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A `.loom` holds paths, bookmarks and notes: far below this.
const MAX_PROJECT_BYTES: u64 = 4 * 1024 * 1024;

/// The part of a project the UI owns: where it looked, what it marked, how it
/// bound the cluster. Saving adds the deck from the session.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub view: ViewState,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default)]
    pub notes: Vec<Note>,
    #[serde(default)]
    pub cursor_a_us: Option<u64>,
    #[serde(default)]
    pub cursor_b_us: Option<u64>,
    /// Cluster slot to the signal shown in it.
    #[serde(default)]
    pub cluster: BTreeMap<String, String>,
}

enum MapLoad {
    File(PathBuf),
    Embedded,
    Missing(String),
    /// A relative path in a project that has no folder.
    NoBase(String),
    None,
}

enum LogLoad {
    File(PathBuf),
    Embedded,
    Missing(String),
    /// A relative path in a project that has no folder.
    NoBase(String),
}

impl Session {
    pub fn load_project_file(&mut self, path: &Path) -> Result<ProjectOpen> {
        let text = project::read_text_capped(path, MAX_PROJECT_BYTES, "project")?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        self.load_project_json(&text, Some(base))
    }

    pub fn load_project_json(&mut self, text: &str, base: Option<&Path>) -> Result<ProjectOpen> {
        let (mut project, mut warnings) = ProjectFile::read(text)?;

        if project::is_network_path(&project.log_path) {
            return Err(Error::msg(format!(
                "this project names a network path for its log ({}). Projects do not \
                 follow network paths; open the log with Open, then save the project again.",
                project.log_path.trim()
            )));
        }
        for (what, stored) in [
            ("signal map", project.signal_map_path.as_deref()),
            ("compare log", project.compare_path.as_deref()),
        ] {
            if let Some(stored) = stored.filter(|stored| project::is_network_path(stored)) {
                warnings.push(format!(
                    "The {what} is on a network path ({}), which projects do not follow. Open it with Open.",
                    stored.trim()
                ));
            }
        }

        // Open the map and log into locals first. A project that fails to load
        // leaves the deck that was open untouched.
        let timeout = project.timeout();
        let (map, map_path) = match resolve_map(base, project.signal_map_path.as_deref()) {
            MapLoad::File(path) => (Some(read_map_file(&path)?), Some(path)),
            MapLoad::Embedded => (Some(sample::map()?), Some(sample::embedded_map_path())),
            MapLoad::Missing(stored) => {
                if !project::is_network_path(&stored) {
                    warnings.push(format!(
                        "Signal map not found ({stored}). Frames will load without decode."
                    ));
                }
                (None, None)
            }
            MapLoad::NoBase(stored) => {
                warnings.push(no_base_note("signal map", &stored));
                (None, None)
            }
            MapLoad::None => (None, None),
        };

        let (log, log_label, log_path) = match resolve_log(base, &project.log_path) {
            LogLoad::File(path) => {
                let label = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(sample::LOG_NAME)
                    .to_string();
                (
                    IndexedLog::open_path_timed(&path, map.as_ref(), timeout, None)?,
                    label,
                    path,
                )
            }
            LogLoad::Embedded => {
                let log = IndexedLog::open_bytes_timed(sample::log_bytes(), map.as_ref(), timeout)?;
                warnings.push(
                    "Opened the built-in cluster sample because the project log path was not on disk."
                        .into(),
                );
                (
                    log,
                    sample::LOG_NAME.to_string(),
                    sample::embedded_log_path(),
                )
            }
            LogLoad::Missing(stored) => {
                return Err(Error::msg(format!(
                    "project log not found: {stored}. Open the log, then save the project again."
                )));
            }
            LogLoad::NoBase(stored) => {
                return Err(Error::msg(format!(
                    "project log {stored} is a relative path and this project has no folder to resolve it against. Open the log, then save the project again."
                )));
            }
        };

        warnings.extend(project.drop_invalid(Some(&log)));

        self.timeout = timeout;
        self.maps = MapSet::new(map, map_path);
        self.log_label = log_label;
        self.log_path = Some(log_path);
        self.log.set(Some(log));
        self.deck
            .load(project.math.clone(), project.triggers.clone());
        let stored = project
            .compare_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        self.compare = Compare::named(stored.map(str::to_string), project.compare_offset_us);
        if let Some(stored) = stored {
            match project::locate(base, stored) {
                Located::Found(path) => {
                    match IndexedLog::open_path_timed(&path, self.maps.map(), timeout, None) {
                        Ok(log) => self.compare.replace_log(log),
                        Err(err) => warnings.push(format!("Compare log did not open: {err}")),
                    }
                }
                Located::Missing if !project::is_network_path(stored) => {
                    warnings.push(format!("Compare log not found ({stored})."));
                }
                Located::Missing => {}
                Located::NoBase => warnings.push(no_base_note("compare log", stored)),
            }
        }

        Ok(ProjectOpen {
            project,
            summary: self.summary()?,
            warnings,
        })
    }

    /// Write the open deck and the UI's `view` to a `.loom` file, refusing a
    /// project that would not load cleanly.
    pub fn write_project(&self, path: &Path, view: &ProjectView) -> Result<()> {
        let project = self.compose(view)?;
        project::write_project(path, &project, self.log.as_ref())
    }

    /// The same project as JSON text, for the browser preview, which downloads
    /// it instead of writing a file.
    pub fn project_json(&self, view: &ProjectView) -> Result<String> {
        let project = self.compose(view)?;
        let problems = project.validate(self.log.as_ref());
        if !problems.is_empty() {
            let list: Vec<String> = problems.iter().map(ToString::to_string).collect();
            return Err(Error::msg(format!(
                "this project would not load cleanly, so it was not saved. {}",
                list.join("; ")
            )));
        }
        project.to_json()
    }

    /// The project to save: the deck from the session, the rest from `view`.
    fn compose(&self, view: &ProjectView) -> Result<ProjectFile> {
        let log = self.log()?;
        Ok(ProjectFile {
            format: PROJECT_FORMAT.to_string(),
            version: 1,
            log_path: self
                .log_path
                .as_ref()
                .map_or_else(|| self.log_label.clone(), |path| path.display().to_string()),
            signal_map_path: uses_map(log)
                .then(|| self.maps.path())
                .flatten()
                .map(|path| path.display().to_string()),
            bookmarks: view.bookmarks.clone(),
            view: view.view.clone(),
            math: self.deck.math().to_vec(),
            triggers: self.deck.triggers().to_vec(),
            notes: view.notes.clone(),
            cursor_a_us: view.cursor_a_us,
            cursor_b_us: view.cursor_b_us,
            compare_path: self.compare.path().map(str::to_string),
            compare_offset_us: self.compare.offset_us(),
            timeout_factor: Some(self.timeout.get()),
            cluster: view.cluster.clone(),
        })
    }
}

/// A project with no folder cannot say where a relative path points, and the
/// engine's working directory is not where its author kept their files.
fn no_base_note(what: &str, stored: &str) -> String {
    format!(
        "The {what} ({}) is a relative path and this project has no folder to resolve it against, so it was not opened. Open it with Open.",
        stored.trim()
    )
}

fn resolve_map(base: Option<&Path>, stored: Option<&str>) -> MapLoad {
    let Some(stored) = stored.map(str::trim).filter(|s| !s.is_empty()) else {
        return MapLoad::None;
    };
    let located = project::locate(base, stored);
    if let Located::Found(path) = located {
        return MapLoad::File(path);
    }
    if Path::new(stored).file_name().and_then(|n| n.to_str()) == Some(sample::MAP_NAME) {
        return MapLoad::Embedded;
    }
    match located {
        Located::NoBase => MapLoad::NoBase(stored.to_string()),
        _ => MapLoad::Missing(stored.to_string()),
    }
}

fn resolve_log(base: Option<&Path>, stored: &str) -> LogLoad {
    let located = project::locate(base, stored);
    if let Located::Found(path) = located {
        return LogLoad::File(path);
    }
    if Path::new(stored).file_name().and_then(|n| n.to_str()) == Some(sample::LOG_NAME) {
        return LogLoad::Embedded;
    }
    match located {
        Located::NoBase => LogLoad::NoBase(stored.to_string()),
        _ => LogLoad::Missing(stored.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{cluster_project, fixtures, math};
    use super::*;
    use crate::project::{ThresholdTrigger, TriggerOp};

    /// Write a project as it stands, checked against the session's open log.
    fn write(session: &Session, path: &Path, project: &ProjectFile) -> Result<()> {
        project::write_project(path, project, session.log.as_ref())
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("loom-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cluster_session() -> Session {
        let mut session = Session::new();
        session
            .open_path(&fixtures().join("cluster_drive.slog"))
            .unwrap();
        session
            .open_map_path(&fixtures().join("cluster.map.json"))
            .unwrap();
        session
    }

    fn fast_trigger() -> ThresholdTrigger {
        ThresholdTrigger {
            id: "fast".into(),
            signal: "VehicleSpeed".into(),
            op: TriggerOp::Gt,
            value: 50.0,
        }
    }

    fn empty_view() -> ProjectView {
        serde_json::from_str(
            r#"{"view":{"playheadUs":1000000,"spanUs":2000000,"plotted":["VehicleSpeed"]}}"#,
        )
        .unwrap()
    }

    #[test]
    fn a_project_saved_by_the_session_carries_its_deck_and_reopens_to_it() {
        let dir = scratch("deck");
        let log = fixtures().join("cluster_drive.slog");
        let mut session = cluster_session();
        session
            .set_math(vec![math("Half", "VehicleSpeed / 2")])
            .unwrap();
        session.set_triggers(vec![fast_trigger()]).unwrap();
        session.set_timeout_factor(4.0).unwrap();
        session.open_compare_path(&log).unwrap();
        session.set_compare_offset(250_000);

        // The UI part holds no deck. Even a payload that still says the deck is
        // empty, as the old UI sent, does not take the session's deck away.
        let stale: ProjectView = serde_json::from_str(
            r#"{"view":{"playheadUs":1000000,"spanUs":2000000,"plotted":["VehicleSpeed"]},
                "cursorAUs":5,"math":[],"triggers":[],"timeoutFactor":2.5,
                "comparePath":null,"compareOffsetUs":0,"signalMapPath":null,"logPath":"x"}"#,
        )
        .unwrap();
        let target = dir.join("deck.loom");
        session.write_project(&target, &stale).unwrap();

        let saved = ProjectFile::parse(&std::fs::read_to_string(&target).unwrap()).unwrap();
        let names: Vec<&str> = saved.math.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["Half"]);
        assert_eq!(saved.math[0].expr, "VehicleSpeed / 2");
        assert_eq!(saved.triggers, [fast_trigger()]);
        assert_eq!(saved.timeout_factor, Some(4.0));
        assert_eq!(saved.compare_path, Some(log.display().to_string()));
        assert_eq!(saved.compare_offset_us, 250_000);
        assert_eq!(saved.log_path, log.display().to_string());
        assert_eq!(
            saved.signal_map_path,
            Some(fixtures().join("cluster.map.json").display().to_string())
        );
        assert_eq!(saved.cursor_a_us, Some(5));
        assert_eq!(saved.view.playhead_us, 1_000_000);
        assert_eq!(saved.view.plotted, ["VehicleSpeed"]);

        let mut reopened = Session::new();
        let opened = reopened.load_project_file(&target).unwrap();
        assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
        assert_eq!(opened.summary.timeout_factor, 4.0);
        assert!(opened
            .summary
            .events
            .iter()
            .any(|event| event.label == "Trigger VehicleSpeed > 50"));
        assert_eq!(reopened.deck.math().len(), 1);
        assert_eq!(reopened.deck.triggers(), [fast_trigger()]);
        assert_eq!(
            reopened.compare.path(),
            Some(log.display().to_string().as_str())
        );
        assert_eq!(reopened.compare.offset_us(), 250_000);
        assert!(reopened.compare.log().is_some());

        let again = dir.join("again.loom");
        reopened.write_project(&again, &stale).unwrap();
        assert_eq!(
            std::fs::read_to_string(&again).unwrap(),
            std::fs::read_to_string(&target).unwrap()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_saved_project_keeps_the_loom_file_shape() {
        let session = cluster_session();
        let text = session.project_json(&empty_view()).unwrap();
        let saved: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut keys: Vec<&str> = saved
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "bookmarks",
                "compareOffsetUs",
                "comparePath",
                "cursorAUs",
                "cursorBUs",
                "format",
                "logPath",
                "math",
                "notes",
                "signalMapPath",
                "timeoutFactor",
                "triggers",
                "version",
                "view",
            ]
        );
        assert_eq!(saved["format"], "signal-loom");
        assert_eq!(saved["version"], 1);
        assert_eq!(saved["comparePath"], serde_json::Value::Null);
        assert_eq!(saved["compareOffsetUs"], 0);
        assert_eq!(saved["timeoutFactor"], 2.5);
        assert_eq!(
            saved["view"],
            serde_json::json!({ "playheadUs": 1_000_000, "spanUs": 2_000_000, "plotted": ["VehicleSpeed"] })
        );
    }

    #[test]
    fn nothing_is_saved_without_an_open_log() {
        let dir = scratch("nolog");
        let session = Session::new();
        let err = session
            .write_project(&dir.join("none.loom"), &empty_view())
            .unwrap_err();
        assert_eq!(err.to_string(), "no log is open");
        assert!(session.project_json(&empty_view()).is_err());
        assert!(!dir.join("none.loom").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_ui_part_the_loader_would_not_accept_is_refused() {
        let dir = scratch("view");
        let session = cluster_session();
        let mut view = empty_view();
        for at in 0..33 {
            view.cluster
                .insert(format!("slot{at}"), "VehicleSpeed".into());
        }
        let expected = "this project would not load cleanly, so it was not saved. \
                        Cluster: project assigns more than 32 cluster slots";
        let err = session
            .write_project(&dir.join("v.loom"), &view)
            .unwrap_err();
        assert_eq!(err.to_string(), expected);
        assert_eq!(
            session.project_json(&view).unwrap_err().to_string(),
            expected
        );
        assert!(!dir.join("v.loom").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_project_with_a_bad_trigger_loads_without_it() {
        let root = fixtures();
        let project = serde_json::json!({
            "format": "signal-loom",
            "version": 1,
            "logPath": root.join("cluster_drive.slog"),
            "signalMapPath": root.join("cluster.map.json"),
            "view": { "playheadUs": 0, "spanUs": 1_000_000, "plotted": [] },
            "triggers": [
                { "id": "fast", "signal": "VehicleSpeed", "op": ">", "value": 50.0 },
                { "id": "odd-op", "signal": "VehicleSpeed", "op": "~", "value": 50.0 },
                { "id": "ghost", "signal": "NoSuchSignal", "op": "<", "value": 1.0 }
            ]
        })
        .to_string();
        let mut session = Session::new();
        let opened = session.load_project_json(&project, None).unwrap();
        let kept: Vec<&str> = opened
            .project
            .triggers
            .iter()
            .map(|trigger| trigger.id.as_str())
            .collect();
        assert_eq!(kept, ["fast"]);
        assert_eq!(session.deck.triggers().len(), 1);
        assert_eq!(opened.warnings.len(), 2, "{:?}", opened.warnings);
        assert!(opened.warnings[0].starts_with("Trigger odd-op was not loaded: "));
        assert!(opened.warnings[0].contains("unknown variant `~`"));
        assert_eq!(
            opened.warnings[1],
            "Trigger ghost was not loaded: trigger signal NoSuchSignal: no such signal in this log"
        );
        assert!(opened
            .summary
            .events
            .iter()
            .any(|event| event.label == "Trigger VehicleSpeed > 50"));
    }

    #[test]
    fn a_project_with_a_bad_math_channel_loads_the_good_one_and_warns() {
        let project = cluster_project(serde_json::json!({
            "math": [
                { "name": "Half", "unit": "km/h", "expr": "VehicleSpeed / 2" },
                { "name": "Broken", "unit": "", "expr": "VehicleSpeed +" },
                { "name": "Chain", "unit": "", "expr": "Half * 2" },
                { "name": "Cut", "unit": "" }
            ]
        }));
        let mut session = Session::new();
        let opened = session.load_project_json(&project, None).unwrap();
        let kept: Vec<&str> = opened
            .project
            .math
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(kept, ["Half"]);
        assert_eq!(session.deck.math().len(), 1);
        assert!(opened
            .summary
            .signals
            .iter()
            .any(|signal| signal.name == "Half"));
        assert_eq!(opened.warnings.len(), 3, "{:?}", opened.warnings);
        assert!(opened.warnings[0].starts_with("Math channel Cut was not loaded: "));
        assert!(opened.warnings[0].contains("missing field `expr`"));
        assert_eq!(
            opened.warnings[1],
            "Math channel Broken was not loaded: math expression ended early"
        );
        assert!(
            opened.warnings[2].starts_with(
                "Math channel Chain was not loaded: math channel Chain uses math channel Half."
            ),
            "{}",
            opened.warnings[2]
        );
    }

    #[test]
    fn load_and_set_math_apply_the_same_rules() {
        let long = "x".repeat(65);
        let cases = [
            ("   ", "VehicleSpeed"),
            (long.as_str(), "VehicleSpeed"),
            ("Speed · B", "VehicleSpeed"),
            ("Bad", "VehicleSpeed +"),
            ("Self", "Self + 1"),
        ];
        for (name, expr) in cases {
            let mut session = Session::new();
            session
                .open_path(&fixtures().join("cluster_drive.slog"))
                .unwrap();
            let refused = session.set_math(vec![math(name, expr)]);
            assert!(refused.is_err(), "set_math accepted {name:?} = {expr}");

            let project = cluster_project(serde_json::json!({
                "math": [{ "name": name, "unit": "", "expr": expr }]
            }));
            let opened = Session::new().load_project_json(&project, None).unwrap();
            assert!(
                opened.project.math.is_empty(),
                "load kept {name:?} = {expr}"
            );
            assert_eq!(opened.warnings.len(), 1, "{:?}", opened.warnings);
            assert!(
                opened.warnings[0].ends_with(&refused.unwrap_err().to_string()),
                "{:?}",
                opened.warnings
            );
        }
        let kept = Session::new()
            .load_project_json(
                &cluster_project(serde_json::json!({
                    "math": [{ "name": "x".repeat(64), "unit": "", "expr": "VehicleSpeed" }]
                })),
                None,
            )
            .unwrap();
        assert_eq!(kept.project.math.len(), 1, "64 characters is allowed");
    }

    #[test]
    fn an_out_of_range_timeout_is_reported_and_the_default_applies() {
        for bad in [0.5, 100.5, 500.0] {
            let project = cluster_project(serde_json::json!({ "timeoutFactor": bad }));
            let mut session = Session::new();
            let opened = session.load_project_json(&project, None).unwrap();
            assert_eq!(
                opened.warnings,
                [format!(
                    "Timeout {bad} was not used: timeout must be between 1 and 100 cycle times. Using the default, 2.5."
                )]
            );
            assert_eq!(opened.summary.timeout_factor, 2.5);
            assert_eq!(opened.project.timeout_factor, None);
        }
        let project = cluster_project(serde_json::json!({ "timeoutFactor": 100.0 }));
        let opened = Session::new().load_project_json(&project, None).unwrap();
        assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
        assert_eq!(opened.summary.timeout_factor, 100.0);
    }

    #[test]
    fn write_project_refuses_a_project_that_would_not_load_and_keeps_the_old_file() {
        let dir = std::env::temp_dir().join(format!("loom-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("drive.loom");
        let session = Session::new();
        let mut project = ProjectFile::parse(&cluster_project(serde_json::json!({}))).unwrap();
        write(&session, &target, &project).unwrap();
        let saved = std::fs::read(&target).unwrap();

        project.math = vec![
            math("Half", "VehicleSpeed / 2"),
            math("Bad", "VehicleSpeed +"),
        ];
        project.timeout_factor = Some(0.0);
        let err = write(&session, &target, &project).unwrap_err();
        assert_eq!(
            err.to_string(),
            "this project would not load cleanly, so it was not saved. \
             Math channel Bad: math expression ended early; \
             Timeout 0: timeout must be between 1 and 100 cycle times"
        );
        assert_eq!(std::fs::read(&target).unwrap(), saved);

        project.math.truncate(1);
        project.timeout_factor = Some(4.0);
        write(&session, &target, &project).unwrap();
        let reopened = Session::new().load_project_file(&target).unwrap();
        assert!(reopened.warnings.is_empty(), "{:?}", reopened.warnings);
        assert_eq!(reopened.project.math.len(), 1);
        assert_eq!(reopened.summary.timeout_factor, 4.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_trigger_that_does_not_fit_the_open_log_is_not_saved() {
        let dir = std::env::temp_dir().join(format!("loom-trigger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session = cluster_session();
        let mut project = ProjectFile::parse(&cluster_project(serde_json::json!({}))).unwrap();
        project.triggers = vec![ThresholdTrigger {
            id: "ghost".into(),
            signal: "NoSuchSignal".into(),
            op: TriggerOp::Gt,
            value: 1.0,
        }];
        let err = write(&session, &dir.join("t.loom"), &project).unwrap_err();
        assert_eq!(
            err.to_string(),
            "this project would not load cleanly, so it was not saved. \
             Trigger ghost: trigger signal NoSuchSignal: no such signal in this log"
        );
        assert!(!dir.join("t.loom").exists());
        project.triggers[0].signal = "VehicleSpeed".into();
        write(&session, &dir.join("t.loom"), &project).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn without_a_project_folder_a_relative_path_is_reported_and_not_opened() {
        // These exist relative to this crate's directory, where the tests run.
        let relative_map = "../../fixtures/hypercar_lap.dbc";
        let relative_compare = "../../fixtures/cluster_drive.slog";
        assert!(Path::new(relative_map).is_file() && Path::new(relative_compare).is_file());
        let project = cluster_project(serde_json::json!({
            "signalMapPath": relative_map,
            "comparePath": relative_compare,
        }));

        let mut session = Session::new();
        let opened = session.load_project_json(&project, None).unwrap();
        assert!(opened.summary.map_label.is_none());
        assert!(session.maps.map().is_none());
        assert!(session.compare.log().is_none());
        assert_eq!(
            opened.warnings,
            [
                "The signal map (../../fixtures/hypercar_lap.dbc) is a relative path and this project has no folder to resolve it against, so it was not opened. Open it with Open.",
                "The compare log (../../fixtures/cluster_drive.slog) is a relative path and this project has no folder to resolve it against, so it was not opened. Open it with Open."
            ]
        );

        let mut session = Session::new();
        let opened = session
            .load_project_json(&project, Some(&fixtures()))
            .unwrap();
        assert!(opened.summary.map_label.is_some());
        assert!(session.compare.log().is_some());
        assert!(
            opened
                .warnings
                .iter()
                .all(|warning| !warning.contains("relative path")),
            "{:?}",
            opened.warnings
        );
    }

    #[test]
    fn without_a_project_folder_a_relative_log_is_refused() {
        let relative_log = "../../fixtures/hypercar_lap.slog";
        assert!(Path::new(relative_log).is_file());
        let project = serde_json::json!({
            "format": "signal-loom",
            "version": 1,
            "logPath": relative_log,
            "view": { "playheadUs": 0, "spanUs": 1_000_000, "plotted": [] },
        })
        .to_string();
        let mut session = Session::new();
        let err = session.load_project_json(&project, None).unwrap_err();
        assert_eq!(
            err.to_string(),
            "project log ../../fixtures/hypercar_lap.slog is a relative path and this project has no folder to resolve it against. Open the log, then save the project again."
        );
        assert!(session.summary().is_err(), "nothing was opened");
        session
            .load_project_json(&project, Some(&fixtures()))
            .unwrap();
    }
}
