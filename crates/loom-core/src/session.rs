use crate::dto::{
    EventDto, FrameDto, PointDto, ProjectOpen, Query, SeriesDto, SignalDto, StepDir, Summary,
    ValueDto,
};
use crate::error::{Error, Result};
use crate::index::{IndexedLog, QueryWindow};
use crate::map::SignalMap;
use crate::project::{self, ProjectFile};
use crate::scan::LogFormat;
use std::path::{Path, PathBuf};

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
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open_sample(&mut self) -> Result<Summary> {
        self.map = Some(SignalMap::parse(SAMPLE_MAP)?);
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
        if !path.is_file() {
            return Err(Error::msg(format!("log not found: {}", path.display())));
        }
        self.log_label = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("log")
            .to_string();
        self.log_path = Some(path.to_path_buf());
        if let Some(sibling) = sibling_map(path) {
            let text =
                std::fs::read_to_string(&sibling).map_err(|err| Error::read(&sibling, err))?;
            self.map = Some(SignalMap::parse(&text)?);
            self.map_path = Some(sibling);
        }
        let indexed = IndexedLog::open_path(path, self.map.as_ref())?;
        self.log = Some(indexed);
        self.summary()
    }

    pub fn open_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<Summary> {
        let label = Path::new(name)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload");
        self.log_label = label.to_string();
        self.log_path = None;
        self.log = Some(IndexedLog::open_bytes(bytes, self.map.as_ref())?);
        self.summary()
    }

    pub fn open_map_path(&mut self, path: &Path) -> Result<Summary> {
        let text = std::fs::read_to_string(path).map_err(|err| Error::read(path, err))?;
        self.map = Some(SignalMap::parse(&text)?);
        self.map_path = Some(path.to_path_buf());
        self.reindex()?;
        self.summary()
    }

    pub fn open_map_json(&mut self, json: &str) -> Result<Summary> {
        self.map = Some(SignalMap::parse(json)?);
        self.map_path = None;
        self.reindex()?;
        self.summary()
    }

    pub fn query(&self, query: &Query) -> Result<Vec<SeriesDto>> {
        let log = self.log()?;
        let series = log.query(&QueryWindow {
            t0_us: query.t0_us,
            t1_us: query.t1_us,
            signals: query.signals.clone(),
            max_points: query.max_points,
        })?;
        Ok(series
            .into_iter()
            .map(|series| SeriesDto {
                name: series.name,
                unit: series.unit,
                points: series
                    .points
                    .into_iter()
                    .map(|(t, v)| PointDto { t, v })
                    .collect(),
            })
            .collect())
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
            })
            .collect();
        Ok(Some(FrameDto {
            t_us: hit.t_us,
            ordinal: hit.ordinal,
            message_id: hit.message_id,
            message_name: hit.message_name,
            dlc: hit.dlc,
            data_hex: hit.data_hex,
            values,
        }))
    }

    pub fn summary(&self) -> Result<Summary> {
        let log = self.log()?;
        let uses_map = log.format() != LogFormat::DecodedCsv;
        Ok(Summary {
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
            signals: log
                .signals()
                .map(|signal| SignalDto {
                    name: signal.name,
                    unit: signal.unit,
                    message_name: signal.message_name,
                    message_id: signal.message_id,
                    min: signal.min,
                    max: signal.max,
                    from_map: signal.from_map,
                })
                .collect(),
            events: log
                .events()
                .iter()
                .map(|(t_us, label)| EventDto {
                    t_us: *t_us,
                    label: label.clone(),
                })
                .collect(),
        })
    }

    pub fn load_project_file(&mut self, path: &Path) -> Result<ProjectOpen> {
        let text = std::fs::read_to_string(path).map_err(|err| Error::read(path, err))?;
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

        match resolve_map(&base, project.signal_map_path.as_deref()) {
            MapLoad::File(path) => {
                let json = std::fs::read_to_string(&path).map_err(|err| Error::read(&path, err))?;
                self.map = Some(SignalMap::parse(&json)?);
                self.map_path = Some(path);
            }
            MapLoad::Embedded => {
                self.map = Some(SignalMap::parse(SAMPLE_MAP)?);
                self.map_path = Some(PathBuf::from(format!("fixtures/{SAMPLE_MAP_NAME}")));
            }
            MapLoad::Missing(stored) => {
                warnings.push(format!(
                    "Signal map not found ({stored}). Frames will load without decode."
                ));
                self.map = None;
                self.map_path = None;
            }
            MapLoad::None => {
                self.map = None;
                self.map_path = None;
            }
        }

        match resolve_log(&base, &project.log_path) {
            LogLoad::File(path) => {
                self.log_label = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(SAMPLE_LOG_NAME)
                    .to_string();
                self.log_path = Some(path.clone());
                self.log = Some(IndexedLog::open_path(&path, self.map.as_ref())?);
            }
            LogLoad::Embedded => {
                self.log_label = SAMPLE_LOG_NAME.to_string();
                self.log_path = Some(PathBuf::from(format!("fixtures/{SAMPLE_LOG_NAME}")));
                self.log = Some(IndexedLog::open_bytes(
                    SAMPLE_SLOG.as_bytes().to_vec(),
                    self.map.as_ref(),
                )?);
                warnings.push(
                    "Opened the built-in cluster sample because the project log path was not on disk."
                        .into(),
                );
            }
            LogLoad::Missing(stored) => {
                return Err(Error::msg(format!(
                    "project log not found: {stored}. Open the log, then save the project again."
                )));
            }
        }

        Ok(ProjectOpen {
            project,
            summary: self.summary()?,
            warnings,
        })
    }

    pub fn write_project(&self, path: &Path, project: &ProjectFile) -> Result<()> {
        project::write_project(path, project)
    }

    fn log(&self) -> Result<&IndexedLog> {
        self.log
            .as_ref()
            .ok_or_else(|| Error::msg("no log is open"))
    }

    fn reindex(&mut self) -> Result<()> {
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
            IndexedLog::open_path(&path, self.map.as_ref())?
        } else if let Some(bytes) = bytes {
            IndexedLog::open_shared(bytes, self.map.as_ref())?
        } else {
            return Ok(());
        };
        self.log = Some(rebuilt);
        Ok(())
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

fn sibling_map(log: &Path) -> Option<PathBuf> {
    let mut candidate = log.to_path_buf();
    candidate.set_extension("map.json");
    candidate.is_file().then_some(candidate)
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
