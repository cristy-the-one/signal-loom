use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub log_label: String,
    pub log_path: Option<String>,
    pub map_label: Option<String>,
    pub map_path: Option<String>,
    pub format: String,
    pub frame_count: u64,
    pub event_count: u64,
    pub events_truncated: bool,
    pub checkpoint_count: u64,
    pub t_start_us: u64,
    pub t_end_us: u64,
    pub bytes: u64,
    pub signals: Vec<SignalDto>,
    pub events: Vec<EventDto>,
    pub skipped_records: u64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalDto {
    pub name: String,
    pub unit: String,
    pub message_name: String,
    pub message_id: Option<u32>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub from_map: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventDto {
    pub t_us: u64,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesDto {
    pub name: String,
    pub unit: String,
    pub points: Vec<PointDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PointDto {
    pub t: u64,
    pub v: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BusLoad {
    pub frames: u64,
    /// Frames per second in the window.
    pub rate: f64,
    /// Share of a 500 kbit/s classic CAN bus. 1.0 is full.
    pub load: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowStats {
    pub count: u64,
    pub min: f64,
    pub max: f64,
    pub avg: f64,
    pub first: f64,
    pub last: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueDto {
    pub name: String,
    pub unit: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameDto {
    pub t_us: u64,
    pub ordinal: u64,
    pub message_id: Option<u32>,
    pub message_name: String,
    pub extended: bool,
    pub dlc: u8,
    pub data_hex: String,
    pub values: Vec<ValueDto>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Query {
    #[serde(deserialize_with = "deserialize_us")]
    pub t0_us: u64,
    #[serde(deserialize_with = "deserialize_us")]
    pub t1_us: u64,
    pub signals: Vec<String>,
    pub max_points: usize,
    /// When set, each plotted signal is also returned from the compare log.
    #[serde(default)]
    pub include_compare: bool,
}

/// Timestamps are integer microseconds. A scrubber can still send a fractional
/// sample; round it instead of failing the request.
pub fn deserialize_us<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visit;
    impl serde::de::Visitor<'_> for Visit {
        type Value = u64;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a timestamp in microseconds")
        }
        fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<u64, E> {
            Ok(value)
        }
        fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<u64, E> {
            if value < 0 {
                return Err(E::custom("timestamp is negative"));
            }
            Ok(value as u64)
        }
        fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<u64, E> {
            if !value.is_finite() || value < 0.0 {
                return Err(E::custom("timestamp is not a usable number"));
            }
            Ok(value.round() as u64)
        }
    }
    deserializer.deserialize_any(Visit)
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepDir {
    Next,
    Prev,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOpen {
    pub project: crate::project::ProjectFile,
    pub summary: Summary,
    pub warnings: Vec<String>,
}

/// Polled while a log or DBC is indexed on a background thread.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub running: bool,
    pub done: bool,
    pub idle: bool,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub frames: u64,
    pub skipped: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
