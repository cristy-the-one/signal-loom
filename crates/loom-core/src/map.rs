use crate::decode::{DecodeSpec, Endian};
use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::HashSet;

/// A JSON signal map: the subset of a DBC we actually decode.
#[derive(Debug, Clone)]
pub struct SignalMap {
    pub name: String,
    pub signals: Vec<MappedSignal>,
}

#[derive(Debug, Clone)]
pub struct MappedSignal {
    pub name: String,
    pub unit: String,
    pub message_name: String,
    pub message_id: u32,
    pub spec: DecodeSpec,
}

impl SignalMap {
    pub fn parse(text: &str) -> Result<Self> {
        let raw: RawMap = serde_json::from_str(text)
            .map_err(|err| Error::msg(format!("signal map is not valid JSON: {err}")))?;
        if raw.version != 1 {
            return Err(Error::msg(format!(
                "signal map version {} is not supported (expected 1)",
                raw.version
            )));
        }
        let name = raw.name.trim();
        if name.is_empty() {
            return Err(Error::msg("signal map is missing a name"));
        }
        let mut signals = Vec::new();
        let mut seen = HashSet::new();
        for message in raw.messages {
            let message_name = message.name.trim().to_string();
            if message_name.is_empty() {
                return Err(Error::msg("a message in the signal map has no name"));
            }
            for signal in message.signals {
                let signal_name = signal.name.trim().to_string();
                if signal_name.is_empty() {
                    return Err(Error::msg(format!(
                        "message {message_name} has a signal with no name"
                    )));
                }
                if !seen.insert(signal_name.clone()) {
                    return Err(Error::msg(format!(
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
                spec.validate()
                    .map_err(|message| Error::msg(format!("signal {signal_name}: {message}")))?;
                signals.push(MappedSignal {
                    name: signal_name,
                    unit: signal.unit,
                    message_name: message_name.clone(),
                    message_id: message.id,
                    spec,
                });
            }
        }
        if signals.is_empty() {
            return Err(Error::msg("signal map has no signals"));
        }
        Ok(Self {
            name: name.to_string(),
            signals,
        })
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
    signals: Vec<RawSignal>,
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

pub fn parse_can_id(text: &str) -> std::result::Result<u32, String> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).map_err(|_| format!("bad CAN id {text}"));
    }
    if text.chars().any(|c| matches!(c, 'a'..='f' | 'A'..='F')) {
        return u32::from_str_radix(text, 16).map_err(|_| format!("bad CAN id {text}"));
    }
    text.parse::<u32>()
        .map_err(|_| format!("bad CAN id {text}"))
}
