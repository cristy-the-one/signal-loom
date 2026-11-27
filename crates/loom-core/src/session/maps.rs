//! Signal maps: reading one, installing it, putting the old one back, and
//! finding the one that sits beside a log.

use super::Session;
use crate::dto::{MapMatch, Summary};
use crate::error::Result;
use crate::index::{IndexControl, IndexedLog};
use crate::judge::VerdictDbc;
use crate::map::SignalMap;
use crate::project;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A DBC or JSON map larger than this is not a signal map.
const MAX_MAP_BYTES: u64 = 32 * 1024 * 1024;

/// The signal map in use, where it came from, and what reading it had to say.
#[derive(Clone, Default)]
pub(super) struct MapSet {
    map: Option<SignalMap>,
    path: Option<PathBuf>,
    sources: Vec<VerdictDbc>,
    notes: Vec<String>,
}

impl MapSet {
    pub(super) fn new(map: Option<SignalMap>, path: Option<PathBuf>) -> Self {
        let notes = map
            .as_ref()
            .map(|map| map.warnings.clone())
            .unwrap_or_default();
        let sources = match &map {
            Some(m) => {
                let sha = path
                    .as_deref()
                    .and_then(|p| std::fs::read(p).ok())
                    .map(|bytes| sha256_hex(&bytes))
                    .unwrap_or_default();
                vec![verdict_entry(m, path.as_deref(), sha, 0)]
            }
            None => Vec::new(),
        };
        Self {
            map,
            path,
            sources,
            notes,
        }
    }

    pub(super) fn map(&self) -> Option<&SignalMap> {
        self.map.as_ref()
    }

    pub(super) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub(super) fn sources(&self) -> &[VerdictDbc] {
        &self.sources
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
        self.sources.clear();
    }

    /// Install `map`, appended to the one in use or replacing it. Signals
    /// without a channel are given `channel` when it is not 0.
    fn install(
        &mut self,
        mut map: SignalMap,
        path: Option<PathBuf>,
        sha256: String,
        append: bool,
        channel: u8,
    ) {
        let entry = verdict_entry(&map, path.as_deref(), sha256, channel);
        if append {
            if let Some(existing) = &mut self.map {
                let notes = map.warnings.clone();
                existing.append(map, channel);
                self.sources.push(entry);
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
        self.sources = vec![entry];
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
        let (map, sha256) = read_map_file_hashed(path)?;
        self.apply_map(map, Some(path.to_path_buf()), sha256, false, 0, control)
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
        let (map, sha256) = read_map_file_hashed(path)?;
        self.apply_map(
            map,
            Some(path.to_path_buf()),
            sha256,
            true,
            channel,
            control,
        )
    }

    pub fn open_map_json(&mut self, json: &str) -> Result<Summary> {
        let map = parse_map_text(json)?;
        self.apply_map(map, None, sha256_hex(json.as_bytes()), false, 0, None)
    }

    pub fn add_map_json(&mut self, json: &str, channel: u8) -> Result<Summary> {
        let map = parse_map_text(json)?;
        self.apply_map(map, None, sha256_hex(json.as_bytes()), true, channel, None)
    }

    /// Install one or more `(path, channel)` DBC or signal-map files before opening
    /// a recording so the log is indexed once with all maps active.
    pub fn preload_maps(&mut self, dbcs: &[(PathBuf, u8)]) -> Result<()> {
        let mut next = MapSet::default();
        for (index, (path, channel)) in dbcs.iter().enumerate() {
            let (map, sha256) = read_map_file_hashed(path)?;
            next.install(map, Some(path.clone()), sha256, index > 0, *channel);
        }
        self.maps = next;
        Ok(())
    }

    /// Install a map and reindex. A failed or cancelled reindex puts the
    /// previous map back, so a retry does not stack the same DBC twice.
    fn apply_map(
        &mut self,
        map: SignalMap,
        path: Option<PathBuf>,
        sha256: String,
        append: bool,
        channel: u8,
        control: Option<&IndexControl>,
    ) -> Result<Summary> {
        let before = self.maps.clone();
        self.maps.install(map, path, sha256, append, channel);
        if let Err(err) = self.reindex_controlled(control) {
            self.maps = before;
            return Err(err);
        }
        self.summary()
    }
}

/// Read a DBC or JSON map from disk, naming a DBC after its file.
pub(super) fn read_map_file(path: &Path) -> Result<SignalMap> {
    read_map_file_hashed(path).map(|(map, _)| map)
}

fn read_map_file_hashed(path: &Path) -> Result<(SignalMap, String)> {
    let text = project::read_text_capped(path, MAX_MAP_BYTES, "signal map")?;
    let sha256 = std::fs::read(path)
        .map(|bytes| sha256_hex(&bytes))
        .unwrap_or_else(|_| sha256_hex(text.as_bytes()));
    let mut map = parse_map_text(&text)?;
    name_dbc_from_path(&mut map, path);
    Ok((map, sha256))
}

fn verdict_entry(map: &SignalMap, path: Option<&Path>, sha256: String, channel: u8) -> VerdictDbc {
    let file = path
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or(&map.name)
        .to_string();
    VerdictDbc {
        file,
        sha256,
        channel,
    }
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
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
