use crate::analyze::{compile, Compiled};
use crate::dto::{
    EventDto, FrameDto, MapMatch, PointDto, ProjectOpen, Query, SeriesDto, SignalDto, StepDir,
    Summary, ValueDto, WindowStats,
};
use crate::error::{Error, Result};
use crate::index::{decimate_points, IndexControl, IndexedLog, QueryWindow, Series};
use crate::map::SignalMap;
use crate::project::{self, MathChannel, ProjectFile, ThresholdTrigger};
use crate::scan::LogFormat;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// A DBC or JSON map larger than this is not a signal map.
const MAX_MAP_BYTES: u64 = 32 * 1024 * 1024;
/// A `.loom` holds paths, bookmarks and notes: far below this.
const MAX_PROJECT_BYTES: u64 = 4 * 1024 * 1024;
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
    /// Set by the user or a project; `None` is the default.
    timeout_factor: Option<f64>,
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
        let mut map = SignalMap::parse(SAMPLE_MAP)?;
        map.timeout_factor = self.timeout_factor();
        let map_path = find_up(&format!("fixtures/{SAMPLE_MAP_NAME}"))
            .or_else(|| Some(PathBuf::from(format!("fixtures/{SAMPLE_MAP_NAME}"))));
        let (log, log_path) = if let Some(path) = find_up(&format!("fixtures/{SAMPLE_LOG_NAME}")) {
            (IndexedLog::open_path(&path, Some(&map))?, path)
        } else {
            (
                IndexedLog::open_bytes(SAMPLE_SLOG.as_bytes().to_vec(), Some(&map))?,
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
            map.timeout_factor = self.timeout_factor();
            incoming_map = Some(map);
            incoming_path = Some(sibling);
        }
        let map_for_index = incoming_map.as_ref().or(self.map.as_ref());
        let mut indexed = IndexedLog::open_path_controlled(path, map_for_index, control)?;
        let mut kept_notes = if incoming_map.is_none() {
            self.map_notes.clone()
        } else {
            Vec::new()
        };
        let set_aside = incoming_map.is_none() && !self.carried_map_fits(&indexed);
        if set_aside {
            indexed = IndexedLog::open_path_controlled(path, None, control)?;
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
        let mut indexed = IndexedLog::open_bytes(bytes, self.map.as_ref())?;
        let mut kept_notes = self.map_notes.clone();
        let set_aside = !self.carried_map_fits(&indexed);
        if set_aside {
            if let Some(bytes) = indexed.shared_bytes() {
                indexed = IndexedLog::open_shared(bytes, None)?;
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
        map.timeout_factor = self.timeout_factor();
        self.map = Some(map);
        if !append || self.map_path.is_none() {
            self.map_path = path;
        }
    }

    /// Physical series are bucketed by the index. A math channel is evaluated on
    /// the raw samples of its signals in the window, the same ones `stats` and
    /// the CSV export use, and the result is then bucketed the same way, so its
    /// values do not depend on `max_points`. When the raw window is too large
    /// to read (see `IndexedLog::samples`), the channel is evaluated on the
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
        for (channel, compiled) in &derived {
            let deps = compiled.dependencies();
            let result = match log.samples(deps, query.t0_us, query.t1_us) {
                Ok(raw) => {
                    let mut result = eval_channel(channel, compiled, &raw)?;
                    result.points =
                        decimate_points(result.points, query.t0_us, query.t1_us, query.max_points);
                    result
                }
                Err(_) => {
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
            let name = channel.name.trim();
            if name.is_empty() || name.len() > 64 {
                return Err(Error::msg("math channel name must be 1 to 64 characters"));
            }
            if name.ends_with(" · B") {
                return Err(Error::msg(
                    "math channel names cannot end with the compare suffix",
                ));
            }
            compile_math(&channels, channel)?;
        }
        self.math = channels;
        self.summary()
    }

    pub fn set_triggers(&mut self, triggers: Vec<ThresholdTrigger>) -> Result<Summary> {
        let log = self.log()?;
        for trigger in &triggers {
            check_trigger(log, &self.math, trigger)?;
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
        self.compare = Some(IndexedLog::open_path(path, self.map.as_ref())?);
        self.summary()
    }

    pub fn open_compare_bytes(&mut self, bytes: Vec<u8>) -> Result<Summary> {
        self.compare = Some(IndexedLog::open_bytes(bytes, self.map.as_ref())?);
        self.summary()
    }

    pub fn set_compare_offset(&mut self, offset_us: i64) {
        self.compare_offset_us = offset_us;
    }

    pub fn clear_compare(&mut self) {
        self.compare = None;
        self.compare_offset_us = 0;
    }

    /// A newly opened recording starts without the previous deck setup.
    pub fn timeout_factor(&self) -> f64 {
        self.timeout_factor
            .unwrap_or(crate::map::DEFAULT_TIMEOUT_FACTOR)
    }

    /// Mark a message late after `factor` cycle times without a frame, and
    /// reindex so the event lane follows. A failed reindex keeps the old factor.
    pub fn set_timeout_factor(&mut self, factor: f64) -> Result<Summary> {
        if !factor.is_finite() || !(1.0..=100.0).contains(&factor) {
            return Err(Error::msg("timeout must be between 1 and 100 cycle times"));
        }
        let before = self.timeout_factor;
        self.timeout_factor = Some(factor);
        if let Some(map) = &mut self.map {
            map.timeout_factor = factor;
        }
        if let Err(err) = self.reindex_controlled(None) {
            self.timeout_factor = before;
            let restored = self.timeout_factor();
            if let Some(map) = &mut self.map {
                map.timeout_factor = restored;
            }
            return Err(err);
        }
        self.summary()
    }

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
        let (text, mut warnings) = drop_unreadable_triggers(text);
        let mut project = ProjectFile::parse(&text)?;
        let base = base
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

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
        let (map, map_path) = match resolve_map(&base, project.signal_map_path.as_deref()) {
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
            MapLoad::None => (None, None),
        };
        // The project's timeout applies to the index built for it.
        let timeout_factor = project
            .timeout_factor
            .filter(|factor| factor.is_finite() && (1.0..=100.0).contains(factor));
        let mut map = map;
        if let Some(map) = &mut map {
            map.timeout_factor = timeout_factor.unwrap_or(crate::map::DEFAULT_TIMEOUT_FACTOR);
        }

        let (log, log_label, log_path) = match resolve_log(&base, &project.log_path) {
            LogLoad::File(path) => {
                let label = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(SAMPLE_LOG_NAME)
                    .to_string();
                (IndexedLog::open_path(&path, map.as_ref())?, label, path)
            }
            LogLoad::Embedded => {
                let log = IndexedLog::open_bytes(SAMPLE_SLOG.as_bytes().to_vec(), map.as_ref())?;
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
        };

        project.triggers.retain(|trigger| {
            let fit = check_trigger(&log, &project.math, trigger);
            if let Err(err) = &fit {
                warnings.push(format!("Trigger {} was not loaded: {err}", trigger.id));
            }
            fit.is_ok()
        });

        self.map_notes = map
            .as_ref()
            .map(|map| map.warnings.clone())
            .unwrap_or_default();
        self.timeout_factor = timeout_factor;
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
            if let Some(path) = project::resolve_existing(&base, stored) {
                match IndexedLog::open_path(&path, self.map.as_ref()) {
                    Ok(log) => self.compare = Some(log),
                    Err(err) => warnings.push(format!("Compare log did not open: {err}")),
                }
            } else if !project::is_network_path(stored) {
                warnings.push(format!("Compare log not found ({stored})."));
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
        project::write_project(path, project)
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
        let main = rebuild(self.log.as_ref(), self.map.as_ref(), control)?;
        let compare = rebuild(self.compare.as_ref(), self.map.as_ref(), control)?;
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
    control: Option<&IndexControl>,
) -> Result<Option<IndexedLog>> {
    let Some(log) = log else {
        return Ok(None);
    };
    if let Some(path) = log.path() {
        return IndexedLog::open_path_controlled(path, map, control).map(Some);
    }
    match log.shared_bytes() {
        Some(bytes) => IndexedLog::open_shared(bytes, map).map(Some),
        None => Ok(None),
    }
}

/// A trigger the open log can evaluate: a finite level on a decoded signal.
/// Math channels are not decoded signals, so a trigger cannot target one.
fn check_trigger(log: &IndexedLog, math: &[MathChannel], trigger: &ThresholdTrigger) -> Result<()> {
    if !trigger.value.is_finite() {
        return Err(Error::msg("trigger level must be finite"));
    }
    if !log.has_signal(&trigger.signal) {
        let why = if math.iter().any(|channel| channel.name == trigger.signal) {
            "triggers work on logged signals, not math channels"
        } else {
            "no such signal in this log"
        };
        return Err(Error::msg(format!(
            "trigger signal {}: {why}",
            trigger.signal
        )));
    }
    Ok(())
}

/// Remove triggers that do not parse (an unknown comparison, a missing field)
/// so one bad entry does not fail the whole project. Each one is reported.
fn drop_unreadable_triggers(text: &str) -> (Cow<'_, str>, Vec<String>) {
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(text) else {
        return (Cow::Borrowed(text), Vec::new());
    };
    let Some(items) = root
        .get_mut("triggers")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return (Cow::Borrowed(text), Vec::new());
    };
    let mut warnings = Vec::new();
    for (at, item) in std::mem::take(items).into_iter().enumerate() {
        match serde_json::from_value::<ThresholdTrigger>(item.clone()) {
            Ok(_) => items.push(item),
            Err(err) => {
                let id = item
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map_or_else(|| format!("#{}", at + 1), str::to_string);
                warnings.push(format!("Trigger {id} was not loaded: {err}"));
            }
        }
    }
    if warnings.is_empty() {
        return (Cow::Borrowed(text), warnings);
    }
    (Cow::Owned(root.to_string()), warnings)
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
    None,
}

enum LogLoad {
    File(PathBuf),
    Embedded,
    Missing(String),
}

fn resolve_map(base: &Path, stored: Option<&str>) -> MapLoad {
    let Some(stored) = stored.map(str::trim).filter(|s| !s.is_empty()) else {
        return MapLoad::None;
    };
    if let Some(path) = project::resolve_existing(base, stored) {
        return MapLoad::File(path);
    }
    if Path::new(stored).file_name().and_then(|n| n.to_str()) == Some(SAMPLE_MAP_NAME) {
        return MapLoad::Embedded;
    }
    MapLoad::Missing(stored.to_string())
}

fn resolve_log(base: &Path, stored: &str) -> LogLoad {
    if let Some(path) = project::resolve_existing(base, stored) {
        return LogLoad::File(path);
    }
    if Path::new(stored).file_name().and_then(|n| n.to_str()) == Some(SAMPLE_LOG_NAME) {
        return LogLoad::Embedded;
    }
    LogLoad::Missing(stored.to_string())
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

/// Compile a math channel. A math channel can use only signals from the log:
/// naming another math channel is an error here, wherever it is evaluated, not
/// a dependency that is quietly dropped on one path and fails on another.
fn compile_math(channels: &[MathChannel], channel: &MathChannel) -> Result<Compiled> {
    let compiled = compile(&channel.expr)?;
    if let Some(dep) = compiled
        .dependencies()
        .iter()
        .find(|dep| channels.iter().any(|other| other.name == **dep))
    {
        return Err(Error::msg(format!(
            "math channel {} uses math channel {dep}. A math channel can only use signals from the log, so write the expression of {dep} into it",
            channel.name
        )));
    }
    Ok(compiled)
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
}
