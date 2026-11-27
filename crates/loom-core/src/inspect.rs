//! Recording inspection and slice helpers for CLI and agent workflows.
//!
//! `inspect_log` summarizes any supported recording (format, time bounds,
//! per-channel/id frame counts and cadence, decoded signal ranges and labels,
//! and integrity/log events) in a single streaming pass.

use crate::dto::{PointDto, SeriesDto, SignalLabelDto, WindowStats};
use crate::error::Result;
use crate::index::IndexedLog;
use crate::map::{SignalMap, TimeoutFactor};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct LogInspection {
    pub format: &'static str,
    pub file_format: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub t_start_us: u64,
    pub t_end_us: u64,
    pub duration_us: u64,
    pub frames: u64,
    pub skipped: u64,
    pub events_truncated: bool,
    pub channels: Vec<u8>,
    pub messages: Vec<InspectedMessage>,
    pub signals: Vec<InspectedSignal>,
    pub events: InspectedEvents,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectedMessage {
    pub id: u32,
    pub id_hex: String,
    pub extended: bool,
    pub channel: u8,
    pub frames: u64,
    pub min_dlc: u8,
    pub max_dlc: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_gap_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_gap_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_gap_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dbc_message: Option<String>,
    pub matched_dbc: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectedSignal {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defined_name: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<u32>,
    pub channel: u8,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<SignalLabelDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectedEvents {
    pub total: u64,
    pub timeouts: u64,
    pub checksums: u64,
    pub counters: u64,
    pub error_frames: u64,
    pub other: u64,
    pub sample: Vec<InspectedEvent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InspectedEvent {
    pub t_us: u64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NamedStats {
    pub name: String,
    pub unit: String,
    pub stats: WindowStats,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatsReport {
    pub format: &'static str,
    pub t0_us: u64,
    pub t1_us: u64,
    pub signals: Vec<NamedStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SamplesReport {
    pub format: &'static str,
    pub t0_us: u64,
    pub t1_us: u64,
    pub decimated: bool,
    pub series: Vec<SeriesDto>,
}

const INSPECT_FORMAT: &str = "loom-inspect/1";
const STATS_FORMAT: &str = "loom-stats/1";
const SAMPLES_FORMAT: &str = "loom-samples/1";
const EVENT_SAMPLE_CAP: usize = 25;

/// Parse either a Vector `.dbc` or a Signal Loom `.map.json` document.
pub fn parse_map_or_dbc(text: &str) -> Result<SignalMap> {
    if crate::dbc::looks_like(text) {
        crate::dbc::parse(text)
    } else {
        SignalMap::parse(text)
    }
}

/// Inspect `path` (with an optional `SignalMap` / DBC) and return a structured
/// `loom-inspect/1` summary in a single streaming pass over the recording.
pub fn inspect_log(
    path: &Path,
    map: Option<&SignalMap>,
    sha256: Option<String>,
) -> Result<LogInspection> {
    let timeout = TimeoutFactor::default();

    #[derive(Default)]
    struct MsgAcc {
        extended: bool,
        frames: u64,
        min_dlc: u8,
        max_dlc: u8,
        last_t: Option<u64>,
        gaps: u64,
        sum_gap: u128,
        min_gap: u64,
        max_gap: u64,
    }

    let mut by_bus_id: BTreeMap<(u8, u32), MsgAcc> = BTreeMap::new();
    let mut channels = BTreeSet::new();
    let mut tap = |t_us: u64, id: u32, channel: u8, extended: bool, dlc: u8, _: &[(usize, f64)]| {
        channels.insert(channel);
        let entry = by_bus_id.entry((channel, id)).or_insert_with(|| MsgAcc {
            extended,
            frames: 0,
            min_dlc: dlc,
            max_dlc: dlc,
            last_t: None,
            gaps: 0,
            sum_gap: 0,
            min_gap: u64::MAX,
            max_gap: 0,
        });
        entry.extended |= extended;
        entry.frames += 1;
        entry.min_dlc = entry.min_dlc.min(dlc);
        entry.max_dlc = entry.max_dlc.max(dlc);
        if let Some(last) = entry.last_t {
            let gap = t_us.saturating_sub(last);
            entry.gaps += 1;
            entry.sum_gap += u128::from(gap);
            entry.min_gap = entry.min_gap.min(gap);
            entry.max_gap = entry.max_gap.max(gap);
        }
        entry.last_t = Some(t_us);
    };

    let log = IndexedLog::open_path_tapped(path, map, timeout, &mut tap)?;

    let messages = by_bus_id
        .into_iter()
        .map(|((channel, id), acc)| {
            let dbc_msg = map.and_then(|m| {
                m.signals
                    .iter()
                    .find(|s| {
                        s.message_id == id
                            && (s.channel == 0 || channel == 0 || s.channel == channel)
                    })
                    .map(|s| s.message_name.clone())
                    .or_else(|| {
                        m.messages
                            .iter()
                            .find(|msg| msg.id == id)
                            .map(|msg| msg.name.clone())
                    })
            });
            let matched_dbc = dbc_msg.is_some();
            InspectedMessage {
                id,
                id_hex: format!("0x{id:X}"),
                extended: acc.extended,
                channel,
                frames: acc.frames,
                min_dlc: acc.min_dlc,
                max_dlc: acc.max_dlc,
                min_gap_us: (acc.gaps > 0).then_some(acc.min_gap),
                avg_gap_us: (acc.gaps > 0).then(|| (acc.sum_gap / u128::from(acc.gaps)) as u64),
                max_gap_us: (acc.gaps > 0).then_some(acc.max_gap),
                dbc_message: dbc_msg,
                matched_dbc,
            }
        })
        .collect();

    let signals = log
        .signals()
        .map(|sig| InspectedSignal {
            name: sig.name,
            defined_name: sig.defined_name,
            message: sig.message_name,
            message_id: sig.message_id,
            channel: sig.channel,
            unit: sig.unit,
            min: sig.min,
            max: sig.max,
            step: sig.step,
            labels: sig
                .table
                .into_iter()
                .map(|(value, label)| SignalLabelDto { value, label })
                .collect(),
        })
        .collect();

    let mut timeouts = 0u64;
    let mut checksums = 0u64;
    let mut counters = 0u64;
    let mut error_frames = 0u64;
    let mut other = 0u64;
    for (_, label) in log.events() {
        if label.starts_with("Timeout ") {
            timeouts += 1;
        } else if label.starts_with("Checksum ") {
            checksums += 1;
        } else if label.starts_with("Counter ") {
            counters += 1;
        } else if label == "Error frame" {
            error_frames += 1;
        } else {
            other += 1;
        }
    }
    let sample = log
        .events()
        .iter()
        .take(EVENT_SAMPLE_CAP)
        .map(|(t_us, label)| InspectedEvent {
            t_us: *t_us,
            label: label.clone(),
        })
        .collect();

    let mut warnings = Vec::new();
    if let Some(m) = map {
        warnings.extend(m.warnings.iter().cloned());
    }
    warnings.extend(log.warnings().iter().cloned());

    Ok(LogInspection {
        format: INSPECT_FORMAT,
        file_format: log.format().label().to_string(),
        bytes: log.byte_len(),
        sha256,
        t_start_us: log.t_start_us(),
        t_end_us: log.t_end_us(),
        duration_us: log.t_end_us().saturating_sub(log.t_start_us()),
        frames: log.frame_count(),
        skipped: log.skipped(),
        events_truncated: log.events_truncated(),
        channels: channels.into_iter().collect(),
        messages,
        signals,
        events: InspectedEvents {
            total: log.event_count(),
            timeouts,
            checksums,
            counters,
            error_frames,
            other,
            sample,
        },
        warnings,
    })
}

/// Compute window statistics for one or more signals over `[from_us, to_us]`
/// (defaulting to `[t_start_us, t_end_us]` when omitted).
pub fn stats_for_log(
    path: &Path,
    map: Option<&SignalMap>,
    names: &[String],
    from_us: Option<u64>,
    to_us: Option<u64>,
) -> Result<StatsReport> {
    let log = IndexedLog::open_path_timed(path, map, TimeoutFactor::default(), None)?;
    let t0_us = from_us.unwrap_or_else(|| log.t_start_us());
    let t1_us = to_us.unwrap_or_else(|| log.t_end_us());
    let unit_by_name: BTreeMap<String, String> = log.signals().map(|s| (s.name, s.unit)).collect();
    let target_names: Vec<String> = if names.is_empty() {
        log.signals()
            .filter(|s| s.min.is_some())
            .map(|s| s.name)
            .collect()
    } else {
        names.to_vec()
    };
    let mut signals = Vec::with_capacity(target_names.len());
    for name in &target_names {
        let stats = log.stats(name, t0_us, t1_us)?;
        let unit = unit_by_name.get(name).cloned().unwrap_or_default();
        signals.push(NamedStats {
            name: name.clone(),
            unit,
            stats,
        });
    }
    Ok(StatsReport {
        format: STATS_FORMAT,
        t0_us,
        t1_us,
        signals,
    })
}

/// Read raw or decimated samples for `names` over `[from_us, to_us]`.
pub fn samples_for_log(
    path: &Path,
    map: Option<&SignalMap>,
    names: &[String],
    from_us: Option<u64>,
    to_us: Option<u64>,
    max_points: Option<usize>,
) -> Result<SamplesReport> {
    if names.is_empty() {
        return Err(crate::Error::invalid(
            "--samples requires at least one signal name",
        ));
    }
    let log = IndexedLog::open_path_timed(path, map, TimeoutFactor::default(), None)?;
    for name in names {
        if !log.has_signal(name) {
            return Err(crate::Error::not_found(format!("no signal named {name}")));
        }
    }
    let t0_us = from_us.unwrap_or_else(|| log.t_start_us());
    let t1_us = to_us.unwrap_or_else(|| log.t_end_us());
    let (decimated, raw_series) = match max_points {
        Some(max_points) => (
            true,
            log.query(&crate::index::QueryWindow {
                t0_us,
                t1_us,
                signals: names.to_vec(),
                max_points,
            })?,
        ),
        None => (false, log.samples(names, t0_us, t1_us)?),
    };
    let series = raw_series
        .into_iter()
        .map(|s| SeriesDto {
            name: s.name,
            unit: s.unit,
            points: s
                .points
                .into_iter()
                .map(|(t, v)| PointDto { t, v })
                .collect(),
        })
        .collect();
    Ok(SamplesReport {
        format: SAMPLES_FORMAT,
        t0_us,
        t1_us,
        decimated,
        series,
    })
}

/// Format an inspection, stats, or samples report as pretty-printed JSON with a trailing newline.
pub fn render_json<T: Serialize>(value: &T) -> Result<String> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|err| crate::Error::internal(format!("could not serialize JSON: {err}")))?;
    text.push('\n');
    Ok(text)
}
