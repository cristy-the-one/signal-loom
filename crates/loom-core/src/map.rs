use crate::decode::{DecodeSpec, Endian};
use crate::error::{Error, Result};
use crate::scan::id::{parse_id, IdBase};
use serde::Deserialize;
use std::collections::HashSet;

/// A JSON signal map: the subset of a DBC we actually decode.
#[derive(Debug, Clone)]
pub struct SignalMap {
    pub name: String,
    pub signals: Vec<MappedSignal>,
    pub messages: Vec<MapMessage>,
    pub warnings: Vec<String>,
}

/// A message is late after this many of its cycle times without a frame. The
/// session owns it and hands it to every index it builds; this is the one
/// place that says what a valid value is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeoutFactor(f64);

impl TimeoutFactor {
    pub const MIN: f64 = 1.0;
    pub const MAX: f64 = 100.0;

    pub fn new(factor: f64) -> Result<Self> {
        if factor.is_finite() && (Self::MIN..=Self::MAX).contains(&factor) {
            return Ok(Self(factor));
        }
        Err(Error::invalid(format!(
            "timeout must be between {} and {} cycle times",
            Self::MIN,
            Self::MAX
        )))
    }

    pub fn get(self) -> f64 {
        self.0
    }
}

/// 2.5 cycles: a 100 ms message times out after 250 ms, as receiving ECUs
/// commonly configure it. One lost frame still stays inside it.
impl Default for TimeoutFactor {
    fn default() -> Self {
        Self(2.5)
    }
}

/// One CAN message described by a map or a DBC.
#[derive(Debug, Clone)]
pub struct MapMessage {
    pub id: u32,
    pub name: String,
    pub dlc: u8,
    /// Nominal period, when the map or DBC `GenMsgCycleTime` provides one.
    pub cycle_us: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct MappedSignal {
    pub name: String,
    pub unit: String,
    pub message_name: String,
    pub message_id: u32,
    pub spec: DecodeSpec,
    /// 0 applies on every channel.
    pub channel: u8,
    pub mux_switch: bool,
    pub mux_value: Option<u32>,
    pub table: Vec<(i64, String)>,
}

impl SignalMap {
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawMap = serde_json::from_str(text)
            .map_err(|err| Error::invalid(format!("signal map is not valid JSON: {err}")))?;
        if raw.version != 1 {
            return Err(Error::invalid(format!(
                "signal map version {} is not supported (expected 1)",
                raw.version
            )));
        }
        let name = raw.name.trim();
        if name.is_empty() {
            return Err(Error::invalid("signal map is missing a name"));
        }
        let mut signals = Vec::new();
        let mut seen = HashSet::new();
        for message in &raw.messages {
            let message_name = message.name.trim().to_string();
            let message_id = message.id;
            if message_name.is_empty() {
                return Err(Error::invalid("a message in the signal map has no name"));
            }
            for signal in &message.signals {
                let signal_name = signal.name.trim().to_string();
                if signal_name.is_empty() {
                    return Err(Error::invalid(format!(
                        "message {message_name} has a signal with no name"
                    )));
                }
                if !seen.insert(signal_name.clone()) {
                    return Err(Error::invalid(format!(
                        "signal map has two signals named {signal_name}"
                    )));
                }
                let endian = match signal.endian {
                    RawEndian::Little => Endian::Little,
                    RawEndian::Big => Endian::Big,
                };
                let spec = DecodeSpec {
                    start_bit: signal.start_bit,
                    bit_length: signal.bit_length,
                    factor: signal.factor,
                    offset: signal.offset,
                    signed: signal.signed,
                    endian,
                };
                spec.validate().map_err(|message| {
                    Error::invalid(format!("signal {signal_name}: {message}"))
                })?;
                signals.push(MappedSignal {
                    name: signal_name,
                    unit: signal.unit.clone(),
                    message_name: message_name.clone(),
                    message_id,
                    spec,
                    channel: message.channel.unwrap_or(0),
                    mux_switch: signal.mux_switch,
                    mux_value: signal.mux_value,
                    table: signal
                        .table
                        .iter()
                        .map(|row| (row.value, row.label.clone()))
                        .collect(),
                });
            }
        }
        if signals.is_empty() {
            return Err(Error::invalid("signal map has no signals"));
        }
        let messages = raw
            .messages
            .iter()
            .map(|message| MapMessage {
                id: message.id,
                name: message.name.trim().to_string(),
                dlc: message.dlc.unwrap_or(8).min(64),
                cycle_us: message.cycle_us,
            })
            .collect();
        Ok(Self {
            name: name.to_string(),
            signals,
            messages,
            warnings: Vec::new(),
        })
    }

    /// Add another DBC or map. A colliding signal name becomes `Name@channel`.
    /// Channel 0 means the new signals apply on every channel.
    pub fn append(&mut self, mut other: SignalMap, channel: u8) {
        if channel != 0 {
            for signal in &mut other.signals {
                if signal.channel == 0 {
                    signal.channel = channel;
                }
            }
        }
        let mut seen: HashSet<String> = self
            .signals
            .iter()
            .map(|signal| signal.name.clone())
            .collect();
        for mut signal in other.signals.drain(..) {
            let assigned = signal.channel;
            signal.name = unique_name(&seen, &signal.name, assigned, signal.message_id);
            seen.insert(signal.name.clone());
            self.signals.push(signal);
        }
        for message in other.messages {
            let clash = self
                .messages
                .iter()
                .any(|have| have.id == message.id && have.name == message.name);
            if !clash {
                self.messages.push(message);
            }
        }
        for warning in other.warnings {
            if self.warnings.len() < 32 && !self.warnings.iter().any(|have| have == &warning) {
                self.warnings.push(warning);
            }
        }
        if !other.name.is_empty() && self.name != other.name && !self.name.contains(&other.name) {
            self.name = format!("{} + {}", self.name, other.name);
        }
    }
}

pub(crate) fn unique_name(
    seen: &HashSet<String>,
    name: &str,
    channel: u8,
    message_id: u32,
) -> String {
    if !seen.contains(name) {
        return name.to_string();
    }
    let tagged = if channel == 0 {
        format!("{name}@{message_id:X}")
    } else {
        format!("{name}@{channel}")
    };
    if seen.contains(&tagged) {
        format!("{tagged}_{message_id:X}")
    } else {
        tagged
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMap {
    name: String,
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    messages: Vec<RawMessage>,
}

fn default_version() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(deserialize_with = "de_id")]
    id: u32,
    name: String,
    #[serde(default)]
    dlc: Option<u8>,
    #[serde(default, rename = "cycleUs")]
    cycle_us: Option<u64>,
    #[serde(default)]
    signals: Vec<RawSignal>,
    #[serde(default)]
    channel: Option<u8>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSignal {
    name: String,
    start_bit: u16,
    bit_length: u16,
    #[serde(default = "default_factor")]
    factor: f64,
    #[serde(default)]
    offset: f64,
    #[serde(default)]
    unit: String,
    #[serde(default)]
    signed: bool,
    #[serde(default)]
    endian: RawEndian,
    #[serde(default)]
    mux_switch: bool,
    #[serde(default)]
    mux_value: Option<u32>,
    #[serde(default)]
    table: Vec<RawTable>,
}

#[derive(Debug, Deserialize)]
struct RawTable {
    value: i64,
    label: String,
}

fn default_factor() -> f64 {
    1.0
}

#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
enum RawEndian {
    #[default]
    Little,
    Big,
}

fn de_id<'de, D>(deserializer: D) -> std::result::Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = u32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a CAN id as a number or hex string")
        }

        fn visit_u64<E: serde::de::Error>(self, value: u64) -> std::result::Result<u32, E> {
            u32::try_from(value).map_err(E::custom)
        }

        fn visit_i64<E: serde::de::Error>(self, value: i64) -> std::result::Result<u32, E> {
            u32::try_from(value).map_err(E::custom)
        }

        fn visit_str<E: serde::de::Error>(self, value: &str) -> std::result::Result<u32, E> {
            parse_can_id(value).map_err(E::custom)
        }
    }
    deserializer.deserialize_any(Visitor)
}

/// A CAN id in a map or CAN CSV: hex with a `0x` prefix or any `A-F` digit,
/// decimal otherwise.
pub fn parse_can_id(text: &str) -> std::result::Result<u32, String> {
    parse_id(text, IdBase::Auto).ok_or_else(|| format!("bad CAN id {}", text.trim()))
}
