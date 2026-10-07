use crate::analyze::compile;
use crate::dto::{
    EventDto, FrameDto, MapMatch, PointDto, ProjectOpen, Query, SeriesDto, SignalDto, StepDir,
    Summary, ValueDto, WindowStats,
};
use crate::error::{Error, Result};
use crate::index::{IndexControl, IndexedLog, QueryWindow, Series};
use crate::map::SignalMap;
use crate::project::{self, MathChannel, ProjectFile, ThresholdTrigger};
use crate::scan::LogFormat;
use std::collections::{BTreeSet, HashMap};
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
    log: Option<IndexedLog>,
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

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_sample(&mut self) -> Result<Summary> {
        if let Some(path) = find_up("fixtures/hypercar_lap.slog") {
            return self.open_path(&path);
        }
        self.reset_deck();
        let mut map = SignalMap::parse(SAMPLE_MAP)?;
        map.timeout_factor = self.timeout_factor();
        self.map = Some(map);
        self.map_path = find_up(&format!("fixtures/{SAMPLE_MAP_NAME}"))
            .or_else(|| Some(PathBuf::from(format!("fixtures/{SAMPLE_MAP_NAME}"))));
        self.log_label = SAMPLE_LOG_NAME.to_string();
        if let Some(path) = find_up(&format!("fixtures/{SAMPLE_LOG_NAME}")) {
            self.log_path = Some(path.clone());
            self.log = Some(IndexedLog::open_path(&path, self.map.as_ref())?);
        } else {
            self.log_path = Some(PathBuf::from(format!("fixtures/{SAMPLE_LOG_NAME}")));
            self.log = Some(IndexedLog::open_bytes(
                SAMPLE_SLOG.as_bytes().to_vec(),
                self.map.as_ref(),
            )?);
        }
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
        self.log = Some(indexed);
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
        let set_aside = !self.carried_map_fits(&indexed);
        if set_aside {
            let note = self.set_aside_note();
            if let Some(bytes) = indexed.shared_bytes() {
                indexed = IndexedLog::open_shared(bytes, None)?;
            }
            self.map = None;
            self.map_path = None;
            self.reset_deck();
            self.map_notes = vec![note];
        } else {
            self.reset_deck();
        }
        self.log_label = label.to_string();
        self.log_path = None;
        self.log = Some(indexed);
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

    pub fn query(&self, query: &Query) -> Result<Vec<SeriesDto>> {
        let log = self.log()?;
        let mut physical = Vec::new();
        let mut derived = Vec::new();
        for name in &query.signals {
            if let Some(channel) = self.math.iter().find(|channel| &channel.name == name) {
                derived.push(channel.clone());
                for dep in compile(&channel.expr)?.dependencies() {
                    if !physical.contains(&dep) && self.math.iter().all(|item| item.name != dep) {
                        physical.push(dep);
                    }
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
        for channel in &derived {
            series.push(eval_channel(channel, &series)?);
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
            compile(&channel.expr)?;
        }
        self.math = channels;
        self.summary()
    }

    pub fn set_triggers(&mut self, triggers: Vec<ThresholdTrigger>) -> Result<Summary> {
        for trigger in &triggers {
            if !matches!(
                trigger.op.as_str(),
                ">" | "<" | ">=" | "<=" | "gt" | "lt" | "ge" | "le"
            ) {
                return Err(Error::msg("trigger comparison must be >, <, >=, or <="));
            }
            if !trigger.value.is_finite() {
                return Err(Error::msg("trigger level must be finite"));
            }
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
        if let Some(channel) = self.math.iter().find(|channel| channel.name == name) {
            let deps = compile(&channel.expr)?.dependencies();
            let series = eval_channel(channel, &self.log()?.samples(&deps, t0_us, t1_us)?)?;
            return stats_of_points(name, &series);
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
            events: self.merged_events(log)?,
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
        let project = ProjectFile::parse(text)?;
        let base = base
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let mut warnings = Vec::new();

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

        self.map_notes = map
            .as_ref()
            .map(|map| map.warnings.clone())
            .unwrap_or_default();
        self.timeout_factor = timeout_factor;
        self.map = map;
        self.map_path = map_path;
        self.log_label = log_label;
        self.log_path = Some(log_path);
        self.log = Some(log);
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

    fn merged_events(&self, log: &IndexedLog) -> Result<Vec<EventDto>> {
        let mut events: Vec<EventDto> = log
            .events()
            .iter()
            .map(|(t_us, label)| EventDto {
                t_us: *t_us,
                label: label.clone(),
            })
            .collect();
        for trigger in &self.triggers {
            let Ok(hits) = log.crossings(&trigger.signal, &trigger.op, trigger.value) else {
                continue;
            };
            for (t_us, label) in hits {
                events.push(EventDto { t_us, label });
            }
        }
        events.sort_by_key(|event| event.t_us);
        events.truncate(5_000);
        Ok(events)
    }

    pub fn write_project(&self, path: &Path, project: &ProjectFile) -> Result<()> {
        project::write_project(path, project)
    }

    fn log(&self) -> Result<&IndexedLog> {
        self.log
            .as_ref()
            .ok_or_else(|| Error::msg("no log is open"))
    }

    fn reindex_controlled(&mut self, control: Option<&IndexControl>) -> Result<()> {
        let path = self
            .log
            .as_ref()
            .and_then(|log| log.path().map(Path::to_path_buf));
        let bytes = if path.is_none() {
            self.log.as_ref().and_then(|log| log.shared_bytes())
        } else {
            None
        };
        if path.is_none() && bytes.is_none() {
            return Ok(());
        }
        let rebuilt = if let Some(path) = path {
            IndexedLog::open_path_controlled(&path, self.map.as_ref(), control)?
        } else if let Some(bytes) = bytes {
            IndexedLog::open_shared(bytes, self.map.as_ref())?
        } else {
            return Ok(());
        };
        self.log = Some(rebuilt);
        Ok(())
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

fn eval_channel(channel: &MathChannel, base: &[Series]) -> Result<Series> {
    let compiled = compile(&channel.expr)?;
    let deps = compiled.dependencies();
    let mut times = BTreeSet::new();
    for dep in &deps {
        let series = base
            .iter()
            .find(|series| series.name == *dep)
            .ok_or_else(|| Error::msg(format!("math channel {} needs {dep}", channel.name)))?;
        for (t, _) in &series.points {
            times.insert(*t);
        }
    }
    let mut cursors = HashMap::<String, usize>::new();
    let mut last = HashMap::<String, f64>::new();
    let mut vars = HashMap::<String, f64>::new();
    let mut lp_state = Vec::new();
    let mut points = Vec::new();
    for t in times {
        vars.clear();
        let mut ready = true;
        for dep in &deps {
            let series = base.iter().find(|series| series.name == *dep).unwrap();
            let mut cursor = cursors.get(dep).copied().unwrap_or(0);
            while cursor < series.points.len() && series.points[cursor].0 <= t {
                last.insert(dep.clone(), series.points[cursor].1);
                cursor += 1;
            }
            cursors.insert(dep.clone(), cursor);
            match last.get(dep).copied() {
                Some(value) => {
                    vars.insert(dep.clone(), value);
                }
                None => ready = false,
            }
        }
        if ready {
            if let Some(value) = compiled.eval(&vars, &mut lp_state)? {
                points.push((t, value));
            }
        }
    }
    Ok(Series {
        name: channel.name.clone(),
        unit: channel.unit.clone(),
        points,
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
            .math
            .iter()
            .find(|channel| &channel.name == name)
            .ok_or_else(|| Error::msg(format!("no math channel named {name}")))?;
        let deps = compile(&channel.expr)?.dependencies();
        let series = eval_channel(channel, &session.log()?.samples(&deps, t0_us, t1_us)?)?;
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
