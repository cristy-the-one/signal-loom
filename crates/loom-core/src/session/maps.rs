//! Signal maps: reading one, installing it, putting the old one back, and
//! finding the one that sits beside a log.

use super::Session;
use crate::dto::{MapMatch, Summary};
use crate::error::Result;
use crate::index::{IndexControl, IndexedLog};
use crate::map::SignalMap;
use crate::project;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A DBC or JSON map larger than this is not a signal map.
const MAX_MAP_BYTES: u64 = 32 * 1024 * 1024;

/// The signal map in use, where it came from, and what reading it had to say.
#[derive(Clone, Default)]
pub(super) struct MapSet {
    map: Option<SignalMap>,
    path: Option<PathBuf>,
    notes: Vec<String>,
}

impl MapSet {
    pub(super) fn new(map: Option<SignalMap>, path: Option<PathBuf>) -> Self {
        let notes = map
            .as_ref()
            .map(|map| map.warnings.clone())
            .unwrap_or_default();
        Self { map, path, notes }
    }

    pub(super) fn map(&self) -> Option<&SignalMap> {
        self.map.as_ref()
    }

    pub(super) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(super) fn notes(&self) -> &[String] {
        &self.notes
    }

    #[cfg(test)]
    pub(super) fn set_notes(&mut self, notes: Vec<String>) {
        self.notes = notes;
    }

    /// A map that came with the previous log fits a new one only if at least
    /// one of its messages appears there. A DBC for another bus would list
    /// all its signals with no data, and hide that it does not apply.
    pub(super) fn fits(&self, log: &IndexedLog) -> bool {
        match (&self.map, map_match(self.map.as_ref(), log)) {
            (Some(_), Some(fit)) => fit.total == 0 || fit.matched > 0,
            _ => true,
        }
    }

    /// Drop a map that does not fit the new log and say why.
    pub(super) fn set_aside(&mut self) {
        let label = self
            .map
            .as_ref()
            .map(|map| map.name.clone())
            .unwrap_or_else(|| "signal map".to_string());
        self.notes = vec![format!(
            "{label} matches no message in this log, so it was set aside. Load this log's DBC with Map."
        )];
        self.map = None;
        self.path = None;
    }

    /// Install `map`, appended to the one in use or replacing it. Signals
    /// without a channel are given `channel` when it is not 0.
    fn install(&mut self, mut map: SignalMap, path: Option<PathBuf>, append: bool, channel: u8) {
        if append {
            if let Some(existing) = &mut self.map {
                let notes = map.warnings.clone();
                existing.append(map, channel);
                extend_notes(&mut self.notes, &notes);
                return;
            }
        }
        if channel != 0 {
            for signal in &mut map.signals {
                if signal.channel == 0 {
                    signal.channel = channel;
                }
            }
        }
        self.notes = map.warnings.clone();
        self.map = Some(map);
        if !append || self.path.is_none() {
            self.path = path;
        }
    }
}

impl Session {
    pub fn open_map_path(&mut self, path: &Path) -> Result<Summary> {
        self.open_map_path_controlled(path, None)
    }

    pub fn open_map_path_controlled(
        &mut self,
        path: &Path,
        control: Option<&IndexControl>,
    ) -> Result<Summary> {
        let map = read_map_file(path)?;
        self.apply_map(map, Some(path.to_path_buf()), false, 0, control)
    }

    /// Append a DBC or map. Signals are limited to `channel` when it is not 0.
    pub fn add_map_path(&mut self, path: &Path, channel: u8) -> Result<Summary> {
        self.add_map_path_controlled(path, channel, None)
    }

    pub fn add_map_path_controlled(
        &mut self,
        path: &Path,
        channel: u8,
        control: Option<&IndexControl>,
    ) -> Result<Summary> {
        let map = read_map_file(path)?;
        self.apply_map(map, Some(path.to_path_buf()), true, channel, control)
    }

    pub fn open_map_json(&mut self, json: &str) -> Result<Summary> {
        let map = parse_map_text(json)?;
        self.apply_map(map, None, false, 0, None)
    }

    pub fn add_map_json(&mut self, json: &str, channel: u8) -> Result<Summary> {
        let map = parse_map_text(json)?;
        self.apply_map(map, None, true, channel, None)
    }

    /// Install a map and reindex. A failed or cancelled reindex puts the
    /// previous map back, so a retry does not stack the same DBC twice.
    fn apply_map(
        &mut self,
        map: SignalMap,
        path: Option<PathBuf>,
        append: bool,
        channel: u8,
        control: Option<&IndexControl>,
    ) -> Result<Summary> {
        let before = self.maps.clone();
        self.maps.install(map, path, append, channel);
        if let Err(err) = self.reindex_controlled(control) {
            self.maps = before;
            return Err(err);
        }
        self.summary()
    }
}

/// Read a DBC or JSON map from disk, naming a DBC after its file.
pub(super) fn read_map_file(path: &Path) -> Result<SignalMap> {
    let text = project::read_text_capped(path, MAX_MAP_BYTES, "signal map")?;
    let mut map = parse_map_text(&text)?;
    name_dbc_from_path(&mut map, path);
    Ok(map)
}

fn parse_map_text(text: &str) -> Result<SignalMap> {
    if crate::dbc::looks_like(text) {
        crate::dbc::parse(text)
    } else {
        SignalMap::parse(text)
    }
}

fn name_dbc_from_path(map: &mut SignalMap, path: &Path) {
    if !map.name.starts_with("DBC import") {
        return;
    }
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return;
    };
    map.name = format!("{stem} — {}", map.name);
}

/// Prefer a sibling `.dbc`, then a sibling `.map.json`.
pub(super) fn sibling_map(log: &Path) -> Option<PathBuf> {
    for extension in ["dbc", "map.json"] {
        let mut candidate = log.to_path_buf();
        candidate.set_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn extend_notes(notes: &mut Vec<String>, extra: &[String]) {
    for warning in extra {
        if notes.len() >= 32 {
            break;
        }
        if !notes.iter().any(|have| have == warning) {
            notes.push(warning.clone());
        }
    }
}

/// How many of the map's message ids carry at least one frame in the log.
pub(super) fn map_match(map: Option<&SignalMap>, log: &IndexedLog) -> Option<MapMatch> {
    let map = map?;
    if log.frame_count() == 0 {
        return None;
    }
    let ids: BTreeSet<u32> = map.messages.iter().map(|message| message.id).collect();
    let matched = ids.iter().filter(|id| log.carries_id(**id)).count();
    Some(MapMatch {
        matched: matched as u32,
        total: ids.len() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{compare_values, fixtures};
    use super::*;
    use crate::dto::Query;

    #[test]
    fn a_cancelled_map_change_leaves_both_logs_and_the_map_as_they_were() {
        let root = fixtures();
        let log = root.join("cluster_drive.slog");
        let mut session = Session::new();
        session.open_path(&log).unwrap();
        session
            .open_map_path(&root.join("cluster.map.json"))
            .unwrap();
        session.open_compare_path(&log).unwrap();
        let query = Query {
            t0_us: 12_000_000,
            t1_us: 12_500_000,
            signals: vec!["VehicleSpeed".to_string()],
            max_points: 50,
            include_compare: true,
        };
        let before = compare_values(&session, &query);
        assert!(!before.is_empty());
        let label = session.summary().unwrap().map_label;

        let cancelled = IndexControl::default();
        cancelled.request_cancel();
        assert!(session
            .add_map_path_controlled(&root.join("hypercar_lap.dbc"), 0, Some(&cancelled))
            .is_err());
        assert_eq!(compare_values(&session, &query), before);
        assert_eq!(session.summary().unwrap().map_label, label);
    }
}
