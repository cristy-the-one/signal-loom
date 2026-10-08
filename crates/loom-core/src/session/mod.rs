//! One open recording and everything set up around it.
//!
//! `Session` is the facade the adapters call. It owns the log and the reindex
//! that keeps every decoded log in step with the map and the timeout; the rest
//! lives in focused modules: maps, the deck (math and triggers), the compare
//! drive, reads, exports and project files.

mod compare;
mod deck;
mod export;
mod maps;
mod project_io;
mod reads;
mod sample;
#[cfg(test)]
mod test_support;

use crate::dto::{EventDto, SignalDto, Summary};
use crate::error::{Error, Result};
use crate::index::{IndexControl, IndexedLog};
use crate::map::{SignalMap, TimeoutFactor};
use crate::project::ThresholdTrigger;
use crate::scan::LogFormat;
use compare::Compare;
use deck::Deck;
use maps::{map_match, read_map_file, sibling_map, MapSet};
use std::cell::RefCell;
use std::ops::Deref;
use std::path::{Path, PathBuf};

pub use project_io::ProjectView;

/// One open recording. The UI asks for windows; the index stays here.
#[derive(Default)]
pub struct Session {
    log: LogSlot,
    maps: MapSet,
    log_label: String,
    log_path: Option<PathBuf>,
    deck: Deck,
    compare: Compare,
    /// Set by the user or a project. The one copy: every index is built from it.
    timeout: TimeoutFactor,
}

/// The open log and what is derived from it. The log is replaced only through
/// `set`, which drops the derived data, so a new log cannot show old triggers.
#[derive(Default)]
struct LogSlot {
    log: Option<IndexedLog>,
    trigger_events: RefCell<Option<TriggerEvents>>,
}

/// Event lane for one trigger set on one log: the log's own events merged with
/// the trigger crossings, capped. Valid only while `triggers` is the live set.
struct TriggerEvents {
    triggers: Vec<ThresholdTrigger>,
    events: Vec<EventDto>,
    warnings: Vec<String>,
}

impl LogSlot {
    fn set(&mut self, log: Option<IndexedLog>) {
        self.log = log;
        self.trigger_events.get_mut().take();
    }

    fn get(&self) -> Result<&IndexedLog> {
        self.log
            .as_ref()
            .ok_or_else(|| Error::not_found("no log is open"))
    }

    /// The events cached for exactly this trigger set, with their warnings.
    fn cached_events(&self, triggers: &[ThresholdTrigger]) -> Option<(Vec<EventDto>, Vec<String>)> {
        let cache = self.trigger_events.borrow();
        cache
            .as_ref()
            .filter(|hit| hit.triggers == triggers)
            .map(|hit| (hit.events.clone(), hit.warnings.clone()))
    }

    fn cache_events(
        &self,
        triggers: &[ThresholdTrigger],
        events: &[EventDto],
        warnings: &[String],
    ) {
        *self.trigger_events.borrow_mut() = Some(TriggerEvents {
            triggers: triggers.to_vec(),
            events: events.to_vec(),
            warnings: warnings.to_vec(),
        });
    }

    #[cfg(test)]
    fn has_cached_events(&self) -> bool {
        self.trigger_events.borrow().is_some()
    }
}

impl Deref for LogSlot {
    type Target = Option<IndexedLog>;

    fn deref(&self) -> &Option<IndexedLog> {
        &self.log
    }
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_sample(&mut self) -> Result<Summary> {
        if let Some(path) = sample::hypercar_log() {
            return self.open_path(&path);
        }
        let map = sample::map()?;
        let (log, log_path) = match sample::log_file() {
            Some(path) => (
                IndexedLog::open_path_timed(&path, Some(&map), self.timeout, None)?,
                path,
            ),
            None => (
                IndexedLog::open_bytes_timed(sample::log_bytes(), Some(&map), self.timeout)?,
                sample::embedded_log_path(),
            ),
        };
        self.reset_deck();
        self.maps = MapSet::new(Some(map), Some(sample::map_path()));
        self.log_label = sample::LOG_NAME.to_string();
        self.log_path = Some(log_path);
        self.log.set(Some(log));
        self.summary()
    }

    pub fn open_path(&mut self, path: &Path) -> Result<Summary> {
        self.open_path_controlled(path, None)
    }

    /// Index `path` without replacing the open session until the scan finishes.
    /// `control` publishes progress and can cancel the scan.
    pub fn open_path_controlled(
        &mut self,
        path: &Path,
        control: Option<&IndexControl>,
    ) -> Result<Summary> {
        if !path.is_file() {
            return Err(Error::not_found(format!(
                "log not found: {}",
                path.display()
            )));
        }
        let incoming = match sibling_map(path) {
            Some(sibling) => Some((read_map_file(&sibling)?, sibling)),
            None => None,
        };
        let map_for_index = incoming.as_ref().map(|(map, _)| map).or(self.maps.map());
        let mut indexed = IndexedLog::open_path_timed(path, map_for_index, self.timeout, control)?;
        let set_aside = incoming.is_none() && !self.maps.fits(&indexed);
        if set_aside {
            indexed = IndexedLog::open_path_timed(path, None, self.timeout, control)?;
        }
        self.reset_deck();
        self.log_label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("log")
            .to_string();
        self.log_path = Some(path.to_path_buf());
        match incoming {
            Some((map, sibling)) => self.maps = MapSet::new(Some(map), Some(sibling)),
            None if set_aside => self.maps.set_aside(),
            None => {}
        }
        self.log.set(Some(indexed));
        self.summary()
    }

    pub fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Summary> {
        let label = Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload");
        let mut indexed = IndexedLog::open_bytes_timed(bytes, self.maps.map(), self.timeout)?;
        let set_aside = !self.maps.fits(&indexed);
        if set_aside {
            if let Some(bytes) = indexed.shared_bytes() {
                indexed = IndexedLog::open_shared(bytes, None, self.timeout)?;
            }
        }
        self.reset_deck();
        if set_aside {
            self.maps.set_aside();
        }
        self.log_label = label.to_string();
        self.log_path = None;
        self.log.set(Some(indexed));
        self.summary()
    }

    /// Open the SLOG text a SocketCAN capture on `iface` produced.
    pub fn open_capture(&mut self, iface: &str, text: String) -> Result<Summary> {
        self.open_bytes(&format!("{iface}.slog"), text.into_bytes())
    }

    /// Message timeout in cycle times.
    pub fn timeout_factor(&self) -> f64 {
        self.timeout.get()
    }

    /// Mark a message late after `factor` cycle times without a frame, and
    /// reindex so the event lane follows. A failed reindex keeps the old factor.
    pub fn set_timeout_factor(&mut self, factor: f64) -> Result<Summary> {
        let before = self.timeout;
        self.timeout = TimeoutFactor::new(factor)?;
        if let Err(err) = self.reindex_controlled(None) {
            self.timeout = before;
            return Err(err);
        }
        self.summary()
    }

    /// A newly opened recording starts without the previous deck setup.
    fn reset_deck(&mut self) {
        self.deck.clear();
        self.compare.clear();
    }

    pub fn summary(&self) -> Result<Summary> {
        let log = self.log()?;
        let uses_map = uses_map(log);
        let (events, trigger_warnings) = self.deck.events(&self.log, log);
        Ok(Summary {
            timeout_factor: self.timeout_factor(),
            map_match: map_match(self.maps.map(), log),
            log_label: self.log_label.clone(),
            log_path: self
                .log_path
                .as_ref()
                .map(|path| path.display().to_string()),
            map_label: uses_map
                .then(|| self.maps.map().map(|map| map.name.clone()))
                .flatten(),
            map_path: uses_map
                .then(|| self.maps.path().map(|path| path.display().to_string()))
                .flatten(),
            format: log.format().label().to_string(),
            frame_count: log.frame_count(),
            event_count: log.event_count(),
            events_truncated: log.events_truncated(),
            checkpoint_count: log.checkpoint_count() as u64,
            t_start_us: log.t_start_us(),
            t_end_us: log.t_end_us(),
            bytes: log.byte_len(),
            signals: {
                let mut signals: Vec<SignalDto> = log
                    .signals()
                    .map(|signal| SignalDto {
                        name: signal.name,
                        unit: signal.unit,
                        message_name: signal.message_name,
                        message_id: signal.message_id,
                        min: signal.min,
                        max: signal.max,
                        step: signal.step,
                        from_map: signal.from_map,
                    })
                    .collect();
                signals.extend(self.deck.signals());
                signals
            },
            events,
            skipped_records: log.skipped(),
            warnings: {
                let mut warnings = Vec::new();
                if let Some(fit) =
                    map_match(self.maps.map(), log).filter(|fit| fit.matched == 0 && fit.total > 0)
                {
                    warnings.push(format!(
                        "None of the {} messages in the signal map appear in this log, so nothing decodes. Is it this log's DBC?",
                        fit.total
                    ));
                }
                warnings.extend(self.maps.notes().iter().cloned());
                warnings.extend(log.warnings().iter().cloned());
                warnings.extend(trigger_warnings);
                warnings
            },
        })
    }

    fn log(&self) -> Result<&IndexedLog> {
        self.log.get()
    }

    /// Rebuild every log that decodes through the map: the main log and the
    /// compare log. Both are rebuilt before either is replaced, so a failed or
    /// cancelled rebuild leaves both as they were.
    fn reindex_controlled(&mut self, control: Option<&IndexControl>) -> Result<()> {
        let main = rebuild(self.log.as_ref(), self.maps.map(), self.timeout, control)?;
        let compare = rebuild(self.compare.log(), self.maps.map(), self.timeout, control)?;
        if main.is_some() {
            self.log.set(main);
        }
        if let Some(compare) = compare {
            self.compare.replace_log(compare);
        }
        Ok(())
    }
}

/// A decoded CSV carries names, not a map, so a map is not shown for it.
fn uses_map(log: &IndexedLog) -> bool {
    log.format() != LogFormat::DecodedCsv
}

/// Index `log`'s source again with `map`. `None` when there is nothing to
/// rebuild from.
fn rebuild(
    log: Option<&IndexedLog>,
    map: Option<&SignalMap>,
    timeout: TimeoutFactor,
    control: Option<&IndexControl>,
) -> Result<Option<IndexedLog>> {
    let Some(log) = log else {
        return Ok(None);
    };
    if let Some(path) = log.path() {
        return IndexedLog::open_path_timed(path, map, timeout, control).map(Some);
    }
    match log.shared_bytes() {
        Some(bytes) => IndexedLog::open_shared(bytes, map, timeout).map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{fixtures, math, RPM_LOG, RPM_MAP};
    use super::*;

    #[test]
    fn a_failed_open_bytes_keeps_the_open_deck() {
        let root = fixtures();
        let mut session = Session::new();
        session.open_path(&root.join("cluster_drive.slog")).unwrap();
        session
            .open_map_path(&root.join("cluster.map.json"))
            .unwrap();
        session.maps.set_notes(vec!["kept note".to_string()]);
        session
            .set_math(vec![math("double", "VehicleSpeed * 2")])
            .unwrap();
        let frames = session.log.as_ref().unwrap().frame_count();

        assert!(session
            .open_bytes("junk.bin", b"\x00\x01 not a log".to_vec())
            .is_err());

        assert_eq!(session.log_label, "cluster_drive.slog");
        assert_eq!(session.log_path, Some(root.join("cluster_drive.slog")));
        assert_eq!(
            session.maps.path(),
            Some(root.join("cluster.map.json").as_path())
        );
        assert_eq!(session.maps.map().unwrap().name, "Instrument cluster");
        assert_eq!(session.maps.notes(), ["kept note"]);
        assert_eq!(session.deck.math().len(), 1);
        assert_eq!(session.log.as_ref().unwrap().frame_count(), frames);
    }

    #[test]
    fn open_bytes_keeps_the_notes_of_the_map_it_carries() {
        let mut session = Session::new();
        session
            .open_bytes("first.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session.maps.set_notes(vec!["kept note".to_string()]);
        let summary = session
            .open_bytes("second.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        assert!(summary.warnings.iter().any(|note| note == "kept note"));
        assert_eq!(session.maps.notes(), ["kept note"]);
    }

    #[test]
    fn a_timeout_set_before_any_map_applies_to_the_map_that_follows() {
        // A 100 ms message with gaps of 260 ms and 290 ms.
        let log = "SLOGv1\nF 0 120 00\nF 100000 120 00\nF 200000 120 00\nF 460000 120 00\nF 750000 120 00\n";
        let map = r#"{"name":"bus","version":1,"messages":[
            {"id":"0x120","name":"Leds","cycleUs":100000,"signals":[
                {"name":"Lamp","startBit":0,"bitLength":8}]}]}"#;
        let late = |summary: &Summary| {
            summary
                .events
                .iter()
                .filter(|event| event.label.starts_with("Timeout"))
                .count()
        };
        let mut session = Session::new();
        session
            .open_bytes("leds.slog", log.as_bytes().to_vec())
            .unwrap();
        session.set_timeout_factor(3.0).unwrap();
        assert_eq!(late(&session.open_map_json(map).unwrap()), 0);
        assert_eq!(session.timeout_factor(), 3.0);
        assert!(session.set_timeout_factor(f64::NAN).is_err());
        assert_eq!(late(&session.set_timeout_factor(2.5).unwrap()), 2);
    }
}
