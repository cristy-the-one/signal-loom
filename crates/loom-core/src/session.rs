use crate::analyze::Compiled;
use crate::dto::{
    EventDto, FrameDto, MapMatch, PointDto, ProjectOpen, Query, SeriesDto, SignalDto, StepDir,
    Summary, ValueDto, WindowStats,
};
use crate::error::{Error, Result};
use crate::index::{decimate_points, IndexControl, IndexedLog, QueryWindow, Series};
use crate::map::{SignalMap, TimeoutFactor};
use crate::project::{self, compile_math, Located, MathChannel, ProjectFile, ThresholdTrigger};
use crate::scan::LogFormat;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A DBC or JSON map larger than this is not a signal map.
const MAX_MAP_BYTES: u64 = 32 * 1024 * 1024;
/// A `.loom` holds paths, bookmarks and notes: far below this.
const MAX_PROJECT_BYTES: u64 = 4 * 1024 * 1024;
/// `query` evaluates a math channel on raw samples only when the window replays
/// at most this many frames, so a refresh stays quick on a huge log. The
/// hypercar lap fixture (about 235,000 frames, 0.2 s to replay in a release
/// build) is below it.
const MATH_RAW_RECORDS: u64 = 500_000;
const SAMPLE_SLOG: &str = include_str!("../../../fixtures/cluster_drive.slog");
const SAMPLE_MAP: &str = include_str!("../../../fixtures/cluster.map.json");
const SAMPLE_LOG_NAME: &str = "cluster_drive.slog";
const SAMPLE_MAP_NAME: &str = "cluster.map.json";

/// One open recording. The UI asks for windows; the index stays here.
#[derive(Default)]
pub struct Session {
    log: LogSlot,
    map: Option<SignalMap>,
    log_label: String,
    log_path: Option<PathBuf>,
    map_path: Option<PathBuf>,
    math: Vec<MathChannel>,
    triggers: Vec<ThresholdTrigger>,
    compare: Option<IndexedLog>,
    compare_offset_us: i64,
    map_notes: Vec<String>,
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
        if let Some(path) = find_up("fixtures/hypercar_lap.slog") {
            return self.open_path(&path);
        }
        let map = SignalMap::parse(SAMPLE_MAP)?;
        let map_path = find_up(&format!("fixtures/{SAMPLE_MAP_NAME}"))
            .or_else(|| Some(PathBuf::from(format!("fixtures/{SAMPLE_MAP_NAME}"))));
        let (log, log_path) = if let Some(path) = find_up(&format!("fixtures/{SAMPLE_LOG_NAME}")) {
            (
                IndexedLog::open_path_timed(&path, Some(&map), self.timeout, None)?,
                path,
            )
        } else {
            (
                IndexedLog::open_bytes_timed(
                    SAMPLE_SLOG.as_bytes().to_vec(),
                    Some(&map),
                    self.timeout,
                )?,
                PathBuf::from(format!("fixtures/{SAMPLE_LOG_NAME}")),
            )
        };
        self.reset_deck();
        self.map = Some(map);
        self.map_path = map_path;
        self.log_label = SAMPLE_LOG_NAME.to_string();
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
            return Err(Error::msg(format!("log not found: {}", path.display())));
        }
        let mut incoming_map = None;
        let mut incoming_path = None;
        let mut incoming_notes = Vec::new();
        if let Some(sibling) = sibling_map(path) {
            let text = project::read_text_capped(&sibling, MAX_MAP_BYTES, "signal map")?;
            let mut map = parse_map_text(&text)?;
            name_dbc_from_path(&mut map, &sibling);
            incoming_notes = map.warnings.clone();
            incoming_map = Some(map);
            incoming_path = Some(sibling);
        }
        let map_for_index = incoming_map.as_ref().or(self.map.as_ref());
        let mut indexed = IndexedLog::open_path_timed(path, map_for_index, self.timeout, control)?;
        let mut kept_notes = if incoming_map.is_none() {
            self.map_notes.clone()
        } else {
            Vec::new()
        };
        let set_aside = incoming_map.is_none() && !self.carried_map_fits(&indexed);
        if set_aside {
            indexed = IndexedLog::open_path_timed(path, None, self.timeout, control)?;
            kept_notes = vec![self.set_aside_note()];
        }
        self.reset_deck();
        self.log_label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("log")
            .to_string();
        self.log_path = Some(path.to_path_buf());
        if let Some(map) = incoming_map {
            self.map = Some(map);
            self.map_path = incoming_path;
            self.map_notes = incoming_notes;
        } else {
            if set_aside {
                self.map = None;
                self.map_path = None;
            }
            self.map_notes = kept_notes;
        }
        self.log.set(Some(indexed));
        self.summary()
    }

    /// A map that came with the previous log fits a new one only if at least
    /// one of its messages appears there. A DBC for another bus would list
    /// all its signals with no data, and hide that it does not apply.
    fn carried_map_fits(&self, log: &IndexedLog) -> bool {
        match (&self.map, map_match(self.map.as_ref(), log)) {
            (Some(_), Some(fit)) => fit.total == 0 || fit.matched > 0,
            _ => true,
        }
    }

    fn set_aside_note(&self) -> String {
        let label = self
            .map
            .as_ref()
            .map(|map| map.name.clone())
            .unwrap_or_else(|| "signal map".to_string());
        format!(
            "{label} matches no message in this log, so it was set aside. Load this log's DBC with Map."
        )
    }

    pub fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Summary> {
        let label = Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload");
        let mut indexed = IndexedLog::open_bytes_timed(bytes, self.map.as_ref(), self.timeout)?;
        let mut kept_notes = self.map_notes.clone();
        let set_aside = !self.carried_map_fits(&indexed);
        if set_aside {
            if let Some(bytes) = indexed.shared_bytes() {
                indexed = IndexedLog::open_shared(bytes, None, self.timeout)?;
            }
            kept_notes = vec![self.set_aside_note()];
        }
        self.reset_deck();
        if set_aside {
            self.map = None;
            self.map_path = None;
        }
        self.map_notes = kept_notes;
        self.log_label = label.to_string();
        self.log_path = None;
        self.log.set(Some(indexed));
        self.summary()
    }

    pub fn open_map_path(&mut self, path: &Path) -> Result<Summary> {
        self.open_map_path_controlled(path, None)
    }

    pub fn open_map_path_controlled(
        &mut self,
        path: &Path,
        control: Option<&IndexControl>,
    ) -> Result<Summary> {
        let text = project::read_text_capped(path, MAX_MAP_BYTES, "signal map")?;
        let mut map = parse_map_text(&text)?;
        name_dbc_from_path(&mut map, path);
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
        let text = project::read_text_capped(path, MAX_MAP_BYTES, "signal map")?;
        let mut map = parse_map_text(&text)?;
        name_dbc_from_path(&mut map, path);
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
        let before = (
            self.map.clone(),
            self.map_path.clone(),
            self.map_notes.clone(),
        );
        self.install_map(map, path, append, channel);
        if let Err(err) = self.reindex_controlled(control) {
            (self.map, self.map_path, self.map_notes) = before;
            return Err(err);
        }
        self.summary()
    }

    fn install_map(
        &mut self,
        mut map: SignalMap,
        path: Option<PathBuf>,
        append: bool,
        channel: u8,
    ) {
        if append {
            if let Some(existing) = &mut self.map {
                let notes = map.warnings.clone();
                existing.append(map, channel);
                extend_notes(&mut self.map_notes, &notes);
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
        self.map_notes = map.warnings.clone();
        self.map = Some(map);
        if !append || self.map_path.is_none() {
            self.map_path = path;
        }
    }

    /// Physical series are bucketed by the index. A math channel is evaluated on
    /// the raw samples of its signals in the window, the same ones `stats` and
    /// the CSV export use, and the result is then bucketed the same way, so its
    /// values do not depend on `max_points`. When replaying the window would
    /// take more than `MATH_RAW_RECORDS` frames, or its raw samples cannot be
    /// read (see `IndexedLog::samples`), the channel is evaluated on the
    /// bucketed series of its signals instead, as that is all the plot can
    /// afford; its values then depend on the zoom.
    pub fn query(&self, query: &Query) -> Result<Vec<SeriesDto>> {
        let log = self.log()?;
        let mut physical = Vec::new();
        let mut derived: Vec<(&MathChannel, Compiled)> = Vec::new();
        for name in &query.signals {
            if let Some(channel) = self.math_channel(name) {
                if derived.iter().all(|(item, _)| item.name != *name) {
                    derived.push((channel, compile_math(&self.math, channel)?));
                }
            } else if !physical.contains(name) {
                physical.push(name.clone());
            }
        }
        let mut series = if physical.is_empty() {
            Vec::new()
        } else {
            log.query(&QueryWindow {
                t0_us: query.t0_us,
                t1_us: query.t1_us,
                signals: physical.clone(),
                max_points: query.max_points,
            })?
        };
        if query.include_compare {
            if let Some(compare) = &self.compare {
                series.extend(compare_series(
                    compare,
                    &physical,
                    query,
                    self.compare_offset_us,
                )?);
            }
        }
        let replay_is_cheap = log.records_in_window(query.t0_us, query.t1_us) <= MATH_RAW_RECORDS;
        for (channel, compiled) in &derived {
            let deps = compiled.dependencies();
            let raw = replay_is_cheap
                .then(|| log.samples(deps, query.t0_us, query.t1_us))
                .and_then(Result::ok);
            let result = match raw {
                Some(raw) => {
                    let mut result = eval_channel(channel, compiled, &raw)?;
                    result.points =
                        decimate_points(result.points, query.t0_us, query.t1_us, query.max_points);
                    result
                }
                None => {
                    let bucketed = log.query(&QueryWindow {
                        t0_us: query.t0_us,
                        t1_us: query.t1_us,
                        signals: deps.to_vec(),
                        max_points: query.max_points,
                    })?;
                    eval_channel(channel, compiled, &bucketed)?
                }
            };
            series.push(result);
        }
        let wanted: Vec<&str> = query.signals.iter().map(String::as_str).collect();
        Ok(series
            .into_iter()
            .filter(|series| {
                wanted.iter().any(|name| series.name == *name)
                    || (query.include_compare && series.name.ends_with(" · B"))
            })
            .map(series_dto)
            .collect())
    }

    fn math_channel(&self, name: &str) -> Option<&MathChannel> {
        self.math.iter().find(|channel| channel.name == name)
    }

    /// A math channel over the raw samples of its signals in the window.
    fn math_series(&self, channel: &MathChannel, t0_us: u64, t1_us: u64) -> Result<Series> {
        let compiled = compile_math(&self.math, channel)?;
        let raw = self.log()?.samples(compiled.dependencies(), t0_us, t1_us)?;
        eval_channel(channel, &compiled, &raw)
    }

    pub fn set_math(&mut self, channels: Vec<MathChannel>) -> Result<Summary> {
        for channel in &channels {
            channel.validate(&channels)?;
        }
        self.math = channels;
        self.summary()
    }

    pub fn set_triggers(&mut self, triggers: Vec<ThresholdTrigger>) -> Result<Summary> {
        let log = self.log()?;
        for trigger in &triggers {
            trigger.validate(Some(log), &self.math)?;
        }
        self.triggers = triggers;
        self.summary()
    }

    pub fn open_compare_path(&mut self, path: &Path) -> Result<Summary> {
        if !path.is_file() {
            return Err(Error::msg(format!(
                "compare log not found: {}",
                path.display()
            )));
        }
        self.compare = Some(IndexedLog::open_path_timed(
            path,
            self.map.as_ref(),
            self.timeout,
            None,
        )?);
        self.summary()
    }

    pub fn open_compare_bytes(&mut self, bytes: Vec<u8>) -> Result<Summary> {
        self.compare = Some(IndexedLog::open_bytes_timed(
            bytes,
            self.map.as_ref(),
            self.timeout,
        )?);
        self.summary()
    }

    pub fn set_compare_offset(&mut self, offset_us: i64) {
        self.compare_offset_us = offset_us;
    }

    pub fn clear_compare(&mut self) {
        self.compare = None;
        self.compare_offset_us = 0;
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
        self.math.clear();
        self.triggers.clear();
        self.compare = None;
        self.compare_offset_us = 0;
        self.map_notes.clear();
    }

    pub fn stats(&self, name: &str, t0_us: u64, t1_us: u64) -> Result<WindowStats> {
        if let Some(channel) = self.math_channel(name) {
            return stats_of_points(name, &self.math_series(channel, t0_us, t1_us)?);
        }
        self.log()?.stats(name, t0_us, t1_us)
    }

    pub fn export_csv(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<String> {
        let (math, physical): (Vec<String>, Vec<String>) = names
            .iter()
            .cloned()
            .partition(|name| self.math.iter().any(|channel| &channel.name == name));
        match (math.is_empty(), physical.is_empty()) {
            (true, _) => self.log()?.export_csv(&physical, t0_us, t1_us),
            (false, true) => export_math_csv(self, &math, t0_us, t1_us),
            (false, false) => Err(Error::msg(
                "export the math channel on its own, or export physical signals on their own",
            )),
        }
    }

    pub fn export_slog(&self, t0_us: u64, t1_us: u64) -> Result<String> {
        self.log()?.export_slog(t0_us, t1_us)
    }

    /// Export to a `.csv` file on disk. Returns the bytes written.
    pub fn save_csv(&self, path: &Path, names: &[String], t0_us: u64, t1_us: u64) -> Result<u64> {
        let text = self.export_csv(names, t0_us, t1_us)?;
        project::write_as(path, "csv", &text)?;
        Ok(text.len() as u64)
    }

    /// Trim the log to a `.slog` file on disk. Returns the bytes written.
    pub fn save_slog(&self, path: &Path, t0_us: u64, t1_us: u64) -> Result<u64> {
        let text = self.export_slog(t0_us, t1_us)?;
        project::write_as(path, "slog", &text)?;
        Ok(text.len() as u64)
    }

    pub fn bus_load(&self, t0_us: u64, t1_us: u64) -> Result<crate::dto::BusLoad> {
        self.log()?.bus_load(t0_us, t1_us)
    }

    /// Read frames from a SocketCAN interface. This does not transmit.
    pub fn capture_socketcan(&mut self, iface: &str, duration_ms: u64) -> Result<Summary> {
        let text = crate::socketcan::capture_slog(iface, duration_ms)?;
        self.open_bytes(&format!("{iface}.slog"), text.into_bytes())
    }

    pub fn values_at(&self, t_us: u64) -> Result<Vec<ValueDto>> {
        let log = self.log()?;
        Ok(log
            .values_at(t_us)?
            .into_iter()
            .map(|value| ValueDto {
                name: value.name,
                unit: value.unit,
                value: value.value,
                label: value.label,
            })
            .collect())
    }

    pub fn frame_at(&self, t_us: u64) -> Result<Option<FrameDto>> {
        self.step(t_us.saturating_add(1), StepDir::Prev)
    }

    pub fn step(&self, t_us: u64, dir: StepDir) -> Result<Option<FrameDto>> {
        let log = self.log()?;
        let Some(hit) = log.step_frame(t_us, matches!(dir, StepDir::Next))? else {
            return Ok(None);
        };
        let values = log
            .values_at(hit.t_us)?
            .into_iter()
            .map(|value| ValueDto {
                name: value.name,
                unit: value.unit,
                value: value.value,
                label: value.label,
            })
            .collect();
        Ok(Some(FrameDto {
            t_us: hit.t_us,
            ordinal: hit.ordinal,
            message_id: hit.message_id,
            message_name: hit.message_name,
            extended: hit.extended,
            dlc: hit.dlc,
            data_hex: hit.data_hex,
            values,
        }))
    }

    pub fn summary(&self) -> Result<Summary> {
        let log = self.log()?;
        let uses_map = log.format() != LogFormat::DecodedCsv;
        let (events, trigger_warnings) = self.merged_events(log);
        Ok(Summary {
            timeout_factor: self.timeout_factor(),
            map_match: map_match(self.map.as_ref(), log),
            log_label: self.log_label.clone(),
            log_path: self
                .log_path
                .as_ref()
                .map(|path| path.display().to_string()),
            map_label: uses_map
                .then(|| self.map.as_ref().map(|map| map.name.clone()))
                .flatten(),
            map_path: uses_map
                .then(|| {
                    self.map_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                })
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
                for channel in &self.math {
                    signals.push(SignalDto {
                        name: channel.name.clone(),
                        unit: channel.unit.clone(),
                        message_name: "Math".to_string(),
                        message_id: None,
                        min: None,
                        max: None,
                        step: None,
                        from_map: false,
                    });
                }
                signals
            },
            events,
            skipped_records: log.skipped(),
            warnings: {
                let mut warnings = Vec::new();
                if let Some(fit) = map_match(self.map.as_ref(), log)
                    .filter(|fit| fit.matched == 0 && fit.total > 0)
                {
                    warnings.push(format!(
                        "None of the {} messages in the signal map appear in this log, so nothing decodes. Is it this log's DBC?",
                        fit.total
                    ));
                }
                warnings.extend(self.map_notes.iter().cloned());
                warnings.extend(log.warnings().iter().cloned());
                warnings.extend(trigger_warnings);
                warnings
            },
        })
    }

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
            MapLoad::File(path) => {
                let text = project::read_text_capped(&path, MAX_MAP_BYTES, "signal map")?;
                let mut map = parse_map_text(&text)?;
                name_dbc_from_path(&mut map, &path);
                (Some(map), Some(path))
            }
            MapLoad::Embedded => (
                Some(SignalMap::parse(SAMPLE_MAP)?),
                Some(PathBuf::from(format!("fixtures/{SAMPLE_MAP_NAME}"))),
            ),
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
                    .unwrap_or(SAMPLE_LOG_NAME)
                    .to_string();
                (
                    IndexedLog::open_path_timed(&path, map.as_ref(), timeout, None)?,
                    label,
                    path,
                )
            }
            LogLoad::Embedded => {
                let log = IndexedLog::open_bytes_timed(
                    SAMPLE_SLOG.as_bytes().to_vec(),
                    map.as_ref(),
                    timeout,
                )?;
                warnings.push(
                    "Opened the built-in cluster sample because the project log path was not on disk."
                        .into(),
                );
                (
                    log,
                    SAMPLE_LOG_NAME.to_string(),
                    PathBuf::from(format!("fixtures/{SAMPLE_LOG_NAME}")),
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

        self.map_notes = map
            .as_ref()
            .map(|map| map.warnings.clone())
            .unwrap_or_default();
        self.timeout = timeout;
        self.map = map;
        self.map_path = map_path;
        self.log_label = log_label;
        self.log_path = Some(log_path);
        self.log.set(Some(log));
        self.math = project.math.clone();
        self.triggers = project.triggers.clone();
        self.compare_offset_us = project.compare_offset_us;
        self.compare = None;
        if let Some(stored) = project
            .compare_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match project::locate(base, stored) {
                Located::Found(path) => {
                    match IndexedLog::open_path_timed(&path, self.map.as_ref(), timeout, None) {
                        Ok(log) => self.compare = Some(log),
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

    /// The event lane and any trigger that could not be evaluated. Cached per
    /// log and trigger set; a read failure is reported but not cached, so a
    /// retry scans again.
    fn merged_events(&self, log: &IndexedLog) -> (Vec<EventDto>, Vec<String>) {
        let mut cache = self.log.trigger_events.borrow_mut();
        if let Some(hit) = cache.as_ref().filter(|hit| hit.triggers == self.triggers) {
            return (hit.events.clone(), hit.warnings.clone());
        }
        let mut events: Vec<EventDto> = log
            .events()
            .iter()
            .map(|(t_us, label)| EventDto {
                t_us: *t_us,
                label: label.clone(),
            })
            .collect();
        let mut warnings = Vec::new();
        let mut cacheable = true;
        for trigger in &self.triggers {
            match log.crossings(&trigger.signal, trigger.op, trigger.value) {
                Ok(hits) => events.extend(
                    hits.into_iter()
                        .map(|(t_us, label)| EventDto { t_us, label }),
                ),
                Err(err) => {
                    warnings.push(format!(
                        "Trigger {} {} {} could not be evaluated: {err}",
                        trigger.signal,
                        trigger.op.symbol(),
                        trigger.value
                    ));
                    cacheable &= !log.has_signal(&trigger.signal);
                }
            }
        }
        events.sort_by_key(|event| event.t_us);
        events.truncate(5_000);
        if cacheable {
            *cache = Some(TriggerEvents {
                triggers: self.triggers.clone(),
                events: events.clone(),
                warnings: warnings.clone(),
            });
        }
        (events, warnings)
    }

    pub fn write_project(&self, path: &Path, project: &ProjectFile) -> Result<()> {
        project::write_project(path, project, self.log.as_ref())
    }

    fn log(&self) -> Result<&IndexedLog> {
        self.log
            .as_ref()
            .ok_or_else(|| Error::msg("no log is open"))
    }

    /// Rebuild every log that decodes through the map: the main log and the
    /// compare log. Both are rebuilt before either is replaced, so a failed or
    /// cancelled rebuild leaves both as they were.
    fn reindex_controlled(&mut self, control: Option<&IndexControl>) -> Result<()> {
        let main = rebuild(self.log.as_ref(), self.map.as_ref(), self.timeout, control)?;
        let compare = rebuild(
            self.compare.as_ref(),
            self.map.as_ref(),
            self.timeout,
            control,
        )?;
        if main.is_some() {
            self.log.set(main);
        }
        if compare.is_some() {
            self.compare = compare;
        }
        Ok(())
    }
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
    if Path::new(stored).file_name().and_then(|n| n.to_str()) == Some(SAMPLE_MAP_NAME) {
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
    if Path::new(stored).file_name().and_then(|n| n.to_str()) == Some(SAMPLE_LOG_NAME) {
        return LogLoad::Embedded;
    }
    match located {
        Located::NoBase => LogLoad::NoBase(stored.to_string()),
        _ => LogLoad::Missing(stored.to_string()),
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

fn series_dto(series: Series) -> SeriesDto {
    SeriesDto {
        name: series.name,
        unit: series.unit,
        points: series
            .points
            .into_iter()
            .map(|(t, v)| PointDto { t, v })
            .collect(),
    }
}

fn compare_series(
    compare: &IndexedLog,
    signals: &[String],
    query: &Query,
    offset_us: i64,
) -> Result<Vec<Series>> {
    let start = query.t0_us as i64 - offset_us;
    let end = query.t1_us as i64 - offset_us;
    if end < 0 {
        return Ok(Vec::new());
    }
    let series = compare.query(&QueryWindow {
        t0_us: start.max(0) as u64,
        t1_us: end.max(0) as u64,
        signals: signals.to_vec(),
        max_points: query.max_points,
    })?;
    Ok(series
        .into_iter()
        .map(|mut series| {
            series.name = format!("{} · B", series.name);
            for (t, _) in &mut series.points {
                let shifted = *t as i64 + offset_us;
                *t = shifted.max(0) as u64;
            }
            series
        })
        .collect())
}

/// Evaluate a compiled channel over `base`, which holds a series for each of
/// its signals. Names are matched to the plan's variables once.
fn eval_channel(channel: &MathChannel, compiled: &Compiled, base: &[Series]) -> Result<Series> {
    let inputs = compiled
        .dependencies()
        .iter()
        .map(|dep| {
            base.iter()
                .find(|series| series.name == *dep)
                .map(|series| series.points.as_slice())
                .ok_or_else(|| Error::msg(format!("math channel {} needs {dep}", channel.name)))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Series {
        name: channel.name.clone(),
        unit: channel.unit.clone(),
        points: compiled.eval_series(&inputs),
    })
}

fn stats_of_points(name: &str, series: &Series) -> Result<WindowStats> {
    if series.points.is_empty() {
        return Err(Error::msg(format!("no samples of {name} in that window")));
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for (_, value) in &series.points {
        min = min.min(*value);
        max = max.max(*value);
        sum += value;
    }
    let count = series.points.len() as u64;
    Ok(WindowStats {
        count,
        min,
        max,
        avg: sum / count as f64,
        first: series.points[0].1,
        last: series.points[series.points.len() - 1].1,
    })
}

fn export_math_csv(session: &Session, names: &[String], t0_us: u64, t1_us: u64) -> Result<String> {
    let mut header = String::from("t_us");
    let mut columns = Vec::new();
    for name in names {
        let channel = session
            .math_channel(name)
            .ok_or_else(|| Error::msg(format!("no math channel named {name}")))?;
        let series = session.math_series(channel, t0_us, t1_us)?;
        header.push(',');
        header.push_str(name);
        columns.push(series);
    }
    let mut times = BTreeSet::new();
    for series in &columns {
        for (t, _) in &series.points {
            times.insert(*t);
        }
    }
    let mut out = header;
    out.push('\n');
    let mut cursors = vec![0usize; columns.len()];
    for t in times {
        out.push_str(&t.to_string());
        for (series, cursor) in columns.iter().zip(cursors.iter_mut()) {
            out.push(',');
            while *cursor < series.points.len() && series.points[*cursor].0 < t {
                *cursor += 1;
            }
            if let Some((_, value)) = series.points.get(*cursor).filter(|(stamp, _)| *stamp == t) {
                out.push_str(&format!("{value:.6}"));
            }
        }
        out.push('\n');
    }
    Ok(out)
}

/// How many of the map's message ids carry at least one frame in the log.
fn map_match(map: Option<&SignalMap>, log: &IndexedLog) -> Option<MapMatch> {
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

fn parse_map_text(text: &str) -> Result<SignalMap> {
    if crate::dbc::looks_like(text) {
        crate::dbc::parse(text)
    } else {
        SignalMap::parse(text)
    }
}

/// Prefer a sibling `.dbc`, then a sibling `.map.json`.
fn sibling_map(log: &Path) -> Option<PathBuf> {
    for extension in ["dbc", "map.json"] {
        let mut candidate = log.to_path_buf();
        candidate.set_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_up(relative: &str) -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for start in starts {
        let mut dir = Some(start.as_path());
        while let Some(current) = dir {
            let candidate = current.join(relative);
            if candidate.is_file() {
                return Some(candidate);
            }
            dir = current.parent();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::TriggerOp;

    const RPM_LOG: &str = "SLOGv1\nF 0 1A0 800C881378640000\nF 10000 1A0 800C881378640000\n";
    const RPM_MAP: &str = r#"{"name":"rpm","version":1,"messages":[{"id":"0x1A0","name":"Powertrain",
        "signals":[{"name":"EngineRPM","startBit":0,"bitLength":16,"factor":0.25,"unit":"rpm"}]}]}"#;

    fn fixtures() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
    }

    fn rpm_query() -> Query {
        Query {
            t0_us: 0,
            t1_us: 20_000,
            signals: vec!["EngineRPM".to_string()],
            max_points: 100,
            include_compare: true,
        }
    }

    fn compare_values(session: &Session, query: &Query) -> Vec<f64> {
        session
            .query(query)
            .unwrap()
            .into_iter()
            .filter(|series| series.name.ends_with(" · B"))
            .flat_map(|series| series.points.into_iter().map(|point| point.v))
            .collect()
    }

    #[test]
    fn a_map_applied_later_decodes_the_compare_log_too() {
        let mut session = Session::new();
        session
            .open_bytes("main.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session
            .open_compare_bytes(RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        assert_eq!(compare_values(&session, &rpm_query()), [800.0, 800.0]);
    }

    #[test]
    fn a_timeout_change_rebuilds_the_compare_log_with_the_same_map() {
        let mut session = Session::new();
        session
            .open_bytes("main.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session
            .open_compare_bytes(RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.set_timeout_factor(4.0).unwrap();
        assert_eq!(compare_values(&session, &rpm_query()), [800.0, 800.0]);
        assert_eq!(
            session.compare.as_ref().unwrap().frame_count(),
            session.log.as_ref().unwrap().frame_count()
        );
    }

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

    #[test]
    fn a_failed_open_bytes_keeps_the_open_deck() {
        let root = fixtures();
        let mut session = Session::new();
        session.open_path(&root.join("cluster_drive.slog")).unwrap();
        session
            .open_map_path(&root.join("cluster.map.json"))
            .unwrap();
        session.map_notes = vec!["kept note".to_string()];
        session
            .set_math(vec![MathChannel {
                name: "double".to_string(),
                expr: "VehicleSpeed * 2".to_string(),
                unit: String::new(),
            }])
            .unwrap();
        let frames = session.log.as_ref().unwrap().frame_count();

        assert!(session
            .open_bytes("junk.bin", b"\x00\x01 not a log".to_vec())
            .is_err());

        assert_eq!(session.log_label, "cluster_drive.slog");
        assert_eq!(session.log_path, Some(root.join("cluster_drive.slog")));
        assert_eq!(session.map_path, Some(root.join("cluster.map.json")));
        assert_eq!(session.map.as_ref().unwrap().name, "Instrument cluster");
        assert_eq!(session.map_notes, ["kept note"]);
        assert_eq!(session.math.len(), 1);
        assert_eq!(session.log.as_ref().unwrap().frame_count(), frames);
    }

    #[test]
    fn open_bytes_keeps_the_notes_of_the_map_it_carries() {
        let mut session = Session::new();
        session
            .open_bytes("first.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session.map_notes = vec!["kept note".to_string()];
        let summary = session
            .open_bytes("second.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        assert!(summary.warnings.iter().any(|note| note == "kept note"));
        assert_eq!(session.map_notes, ["kept note"]);
    }

    /// A rises 10, 12, 11, 30, 13, 12, 14, 11, 12, 13 at every 1000 µs from 0.
    /// B is 1, 2, 3, 4, 40, 6, 7, 8, 9, 10 at every 1000 µs from 500.
    fn math_session(channels: &[(&str, &str)]) -> Session {
        let a = [10, 12, 11, 30, 13, 12, 14, 11, 12, 13];
        let b = [1, 2, 3, 4, 40, 6, 7, 8, 9, 10];
        let mut text = String::from(
            "t_us,signal,value,unit
",
        );
        for i in 0..10 {
            text.push_str(&format!(
                "{},A,{},
",
                i * 1000,
                a[i]
            ));
            text.push_str(&format!(
                "{},B,{},
",
                i * 1000 + 500,
                b[i]
            ));
        }
        let mut session = Session::new();
        session.open_bytes("math.csv", text.into_bytes()).unwrap();
        session
            .set_math(
                channels
                    .iter()
                    .map(|(name, expr)| MathChannel {
                        name: name.to_string(),
                        expr: expr.to_string(),
                        unit: String::new(),
                    })
                    .collect(),
            )
            .unwrap();
        session
    }

    fn math_points(
        session: &Session,
        name: &str,
        t0_us: u64,
        max_points: usize,
    ) -> Vec<(u64, f64)> {
        let series = session
            .query(&Query {
                t0_us,
                t1_us: 9_000,
                signals: vec![name.to_string()],
                max_points,
                include_compare: false,
            })
            .unwrap();
        assert_eq!(series.len(), 1);
        series[0].points.iter().map(|p| (p.t, p.v)).collect()
    }

    #[test]
    fn a_plotted_difference_has_the_stats_extremes_at_any_zoom() {
        let session = math_session(&[("Diff", "A - B")]);
        let stats = session.stats("Diff", 0, 9_000).unwrap();
        assert_eq!((stats.count, stats.min, stats.max), (18, -28.0, 27.0));

        let zoomed_out = math_points(&session, "Diff", 0, 4);
        assert_eq!(
            zoomed_out,
            [(2500, 8.0), (3000, 27.0), (5000, -28.0), (6000, 8.0)]
        );
        let detailed = math_points(&session, "Diff", 0, 1000);
        assert_eq!(detailed.len(), 18);
        for points in [zoomed_out, detailed] {
            let min = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let max = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            assert_eq!((min, max), (stats.min, stats.max));
        }
    }

    #[test]
    fn a_low_pass_channel_has_the_same_value_at_every_zoom() {
        let session = math_session(&[("Smooth", "lp(A, 0.5)")]);
        let zoomed_out = math_points(&session, "Smooth", 0, 6);
        assert_eq!(
            zoomed_out,
            [
                (0, 10.0),
                (1000, 11.0),
                (3000, 20.5),
                (5000, 14.375),
                (8000, 12.296875)
            ]
        );
        let detailed = math_points(&session, "Smooth", 0, 1000);
        assert_eq!(detailed.len(), 10);
        assert_eq!(detailed[3], (3000, 20.5));
        assert_eq!(detailed[8], (8000, 12.296875));
        let stats = session.stats("Smooth", 0, 9_000).unwrap();
        assert_eq!((stats.min, stats.max), (10.0, 20.5));
    }

    #[test]
    fn a_math_channel_is_bucketed_like_a_physical_one_from_a_mid_log_start() {
        let session = math_session(&[("SameA", "A + 0")]);
        let physical = math_points(&session, "A", 2250, 4);
        assert_eq!(physical, [(2250, 11.0), (3000, 30.0), (7000, 11.0)]);
        assert_eq!(math_points(&session, "SameA", 2250, 4), physical);
    }

    #[test]
    fn exported_math_values_are_the_plotted_values() {
        let session = math_session(&[("Diff", "A - B"), ("Smooth", "lp(A, 0.5)")]);
        let csv = session.export_csv(&["Diff".to_string()], 0, 9_000).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 19);
        assert_eq!(&lines[..3], ["t_us,Diff", "500,9.000000", "1000,11.000000"]);
        assert_eq!(lines[9], "4500,-27.000000");
        for (t, v) in math_points(&session, "Diff", 0, 1000) {
            assert!(
                csv.contains(&format!(
                    "
{t},{v:.6}
"
                )),
                "{t}"
            );
        }

        let csv = session
            .export_csv(&["Smooth".to_string()], 0, 9_000)
            .unwrap();
        assert!(csv.contains(
            "
3000,20.500000
"
        ));
        assert!(csv.contains(
            "
8000,12.296875
"
        ));
    }

    #[test]
    fn a_math_channel_cannot_use_another_math_channel() {
        let mut session = math_session(&[("Diff", "A - B")]);
        let err = session
            .set_math(vec![
                MathChannel {
                    name: "Diff".to_string(),
                    expr: "A - B".to_string(),
                    unit: String::new(),
                },
                MathChannel {
                    name: "Twice".to_string(),
                    expr: "Diff * 2".to_string(),
                    unit: String::new(),
                },
            ])
            .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("math channel Twice uses math channel Diff."),
            "{err}"
        );
        assert_eq!(session.math.len(), 1);
    }

    const SWING_LOG: &str = "SLOGv1
F 0 1A0 800C000000000000
F 10000 1A0 401F000000000000
F 20000 1A0 800C000000000000
F 30000 1A0 401F000000000000
";
    const SWING_MAP_HALF: &str = r#"{"name":"rpm","version":1,"messages":[{"id":"0x1A0","name":"Powertrain",
        "signals":[{"name":"EngineRPM","startBit":0,"bitLength":16,"factor":0.5,"unit":"rpm"}]}]}"#;
    const SWING_MAP_RENAMED: &str = r#"{"name":"rpm","version":1,"messages":[{"id":"0x1A0","name":"Powertrain",
        "signals":[{"name":"Revs","startBit":0,"bitLength":16,"factor":0.25,"unit":"rpm"}]}]}"#;

    fn rpm_trigger(op: TriggerOp, value: f64) -> ThresholdTrigger {
        ThresholdTrigger {
            id: "rev".into(),
            signal: "EngineRPM".into(),
            op,
            value,
        }
    }

    fn trigger_events(summary: &Summary) -> Vec<(u64, &str)> {
        summary
            .events
            .iter()
            .map(|event| (event.t_us, event.label.as_str()))
            .collect()
    }

    fn swing_session() -> Session {
        let mut session = Session::new();
        session
            .open_bytes("swing.slog", SWING_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session
    }

    #[test]
    fn trigger_ops_keep_their_wire_strings() {
        for (op, wire) in [
            (TriggerOp::Gt, ">"),
            (TriggerOp::Lt, "<"),
            (TriggerOp::Ge, ">="),
            (TriggerOp::Le, "<="),
        ] {
            let json = serde_json::to_string(&op).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(serde_json::from_str::<TriggerOp>(&json).unwrap(), op);
        }
        for (word, op) in [
            ("gt", TriggerOp::Gt),
            ("lt", TriggerOp::Lt),
            ("ge", TriggerOp::Ge),
            ("le", TriggerOp::Le),
        ] {
            let json = format!("\"{word}\"");
            assert_eq!(serde_json::from_str::<TriggerOp>(&json).unwrap(), op);
        }
        assert!(serde_json::from_str::<TriggerOp>("\"=\"").is_err());
        let trigger = rpm_trigger(TriggerOp::Ge, 1500.0);
        assert_eq!(
            serde_json::to_string(&trigger).unwrap(),
            r#"{"id":"rev","signal":"EngineRPM","op":">=","value":1500.0}"#
        );
    }

    #[test]
    fn a_trigger_on_an_unknown_signal_is_refused_by_name() {
        let mut session = swing_session();
        let mut ghost = rpm_trigger(TriggerOp::Gt, 1500.0);
        ghost.signal = "EngineRMP".into();
        let err = session.set_triggers(vec![ghost]).unwrap_err().to_string();
        assert!(err.contains("EngineRMP"), "{err}");
        assert!(session.triggers.is_empty());
    }

    #[test]
    fn a_trigger_on_a_math_channel_is_refused() {
        let mut session = swing_session();
        session
            .set_math(vec![MathChannel {
                name: "Half".into(),
                unit: "rpm".into(),
                expr: "EngineRPM / 2".into(),
            }])
            .unwrap();
        let mut on_math = rpm_trigger(TriggerOp::Gt, 100.0);
        on_math.signal = "Half".into();
        let err = session.set_triggers(vec![on_math]).unwrap_err().to_string();
        assert!(err.contains("Half") && err.contains("math"), "{err}");
    }

    #[test]
    fn trigger_events_follow_the_triggers_and_the_log() {
        let mut session = swing_session();
        let summary = session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        assert_eq!(
            trigger_events(&summary),
            [
                (10_000, "Trigger EngineRPM > 1500"),
                (30_000, "Trigger EngineRPM > 1500")
            ]
        );
        assert!(session.log.trigger_events.borrow().is_some());

        let summary = session
            .set_triggers(vec![rpm_trigger(TriggerOp::Le, 800.0)])
            .unwrap();
        assert_eq!(
            trigger_events(&summary),
            [
                (0, "Trigger EngineRPM <= 800"),
                (20_000, "Trigger EngineRPM <= 800")
            ]
        );

        session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        session.reindex_controlled(None).unwrap();
        assert!(session.log.trigger_events.borrow().is_none());

        // Doubling the factor puts 800 rpm at 1600: hot from the first frame.
        let summary = session.open_map_json(SWING_MAP_HALF).unwrap();
        assert_eq!(trigger_events(&summary), [(0, "Trigger EngineRPM > 1500")]);
    }

    #[test]
    fn a_trigger_whose_signal_leaves_the_log_is_reported_not_dropped_silently() {
        let mut session = swing_session();
        session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        let summary = session.open_map_json(SWING_MAP_RENAMED).unwrap();
        assert!(trigger_events(&summary).is_empty());
        assert_eq!(
            summary.warnings,
            ["Trigger EngineRPM > 1500 could not be evaluated: no signal named EngineRPM"]
        );
    }

    #[test]
    fn opening_a_log_clears_the_triggers_and_their_events() {
        let mut session = swing_session();
        session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        let summary = session
            .open_bytes("again.slog", SWING_LOG.as_bytes().to_vec())
            .unwrap();
        assert!(trigger_events(&summary).is_empty());
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
        assert_eq!(session.triggers.len(), 1);
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

    fn cluster_project(extra: serde_json::Value) -> String {
        let root = fixtures();
        let mut project = serde_json::json!({
            "format": "signal-loom",
            "version": 1,
            "logPath": root.join("cluster_drive.slog"),
            "signalMapPath": root.join("cluster.map.json"),
            "view": { "playheadUs": 0, "spanUs": 1_000_000, "plotted": [] },
        });
        project
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        project.to_string()
    }

    fn math(name: &str, expr: &str) -> MathChannel {
        MathChannel {
            name: name.to_string(),
            unit: String::new(),
            expr: expr.to_string(),
        }
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
        assert_eq!(session.math.len(), 1);
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

    #[test]
    fn write_project_refuses_a_project_that_would_not_load_and_keeps_the_old_file() {
        let dir = std::env::temp_dir().join(format!("loom-refuse-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("drive.loom");
        let session = Session::new();
        let mut project = ProjectFile::parse(&cluster_project(serde_json::json!({}))).unwrap();
        session.write_project(&target, &project).unwrap();
        let saved = std::fs::read(&target).unwrap();

        project.math = vec![
            math("Half", "VehicleSpeed / 2"),
            math("Bad", "VehicleSpeed +"),
        ];
        project.timeout_factor = Some(0.0);
        let err = session.write_project(&target, &project).unwrap_err();
        assert_eq!(
            err.to_string(),
            "this project would not load cleanly, so it was not saved. \
             Math channel Bad: math expression ended early; \
             Timeout 0: timeout must be between 1 and 100 cycle times"
        );
        assert_eq!(std::fs::read(&target).unwrap(), saved);

        project.math.truncate(1);
        project.timeout_factor = Some(4.0);
        session.write_project(&target, &project).unwrap();
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
        let mut session = Session::new();
        session
            .open_path(&fixtures().join("cluster_drive.slog"))
            .unwrap();
        session
            .open_map_path(&fixtures().join("cluster.map.json"))
            .unwrap();
        let mut project = ProjectFile::parse(&cluster_project(serde_json::json!({}))).unwrap();
        project.triggers = vec![ThresholdTrigger {
            id: "ghost".into(),
            signal: "NoSuchSignal".into(),
            op: TriggerOp::Gt,
            value: 1.0,
        }];
        let err = session
            .write_project(&dir.join("t.loom"), &project)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "this project would not load cleanly, so it was not saved. \
             Trigger ghost: trigger signal NoSuchSignal: no such signal in this log"
        );
        assert!(!dir.join("t.loom").exists());
        project.triggers[0].signal = "VehicleSpeed".into();
        session
            .write_project(&dir.join("t.loom"), &project)
            .unwrap();
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
        assert!(session.map.is_none());
        assert!(session.compare.is_none());
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
        assert!(session.compare.is_some());
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
