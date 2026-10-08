//! Vector DBC import.
//!
//! This reads the message and signal records a cluster engineer actually
//! uses: `BO_`, `SG_`, `VAL_`, and `GenMsgCycleTime`.
//! A multiplexor (`M`) and its cases (`m0`, `m1`, …) are imported. A case is
//! decoded only while the multiplexor matches. A broken record is skipped
//! and listed on the map; the rest of the file is still used.

use crate::decode::{DecodeSpec, Endian};
use crate::error::{Error, Result};
use crate::map::{unique_name, MapMessage, MappedSignal, SignalMap};
use crate::scan::id::mask_29;
use std::collections::{HashMap, HashSet};

pub fn parse(text: &str) -> Result<SignalMap> {
    let mut messages: Vec<MessageBuild> = Vec::new();
    let mut current: Option<usize> = None;
    let mut cycles: HashMap<u32, u64> = HashMap::new();
    let mut warnings = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("BO_ ") {
            if rest.starts_with("TX_BU_") {
                current = None;
                continue;
            }
            match parse_bo(rest) {
                Ok(message) => {
                    current = Some(messages.len());
                    messages.push(message);
                }
                Err(err) => {
                    current = None;
                    push_warn(&mut warnings, err.to_string());
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("SG_ ") {
            let Some(index) = current else {
                push_warn(&mut warnings, "SG_ record appears before its BO_ message");
                continue;
            };
            match parse_sg(rest) {
                Ok(signal) => messages[index].signals.push(signal),
                Err(err) => push_warn(&mut warnings, err.to_string()),
            }
            continue;
        }
        if let Some(table) = value_table(line) {
            if let Some(message) = messages.iter_mut().find(|message| message.id == table.id) {
                if let Some(signal) = message
                    .signals
                    .iter_mut()
                    .find(|signal| signal.name == table.name)
                {
                    signal.table = table.rows;
                }
            }
            continue;
        }
        if let Some(id) = cycle_time(line) {
            cycles.insert(id.0, id.1);
        }
    }

    if messages.is_empty() {
        return Err(Error::msg("DBC has no BO_ messages"));
    }

    let mut signals = Vec::new();
    let mut seen = HashSet::new();
    let mut renamed: Vec<String> = Vec::new();
    let mut map_messages = Vec::new();
    for message in &messages {
        let cycle_us = cycles
            .get(&message.id)
            .copied()
            .map(|ms| ms.saturating_mul(1000));
        map_messages.push(MapMessage {
            id: message.id,
            name: message.name.clone(),
            dlc: message.dlc,
            cycle_us,
        });
        for signal in &message.signals {
            if let Err(err) = signal.spec.validate() {
                push_warn(
                    &mut warnings,
                    format!("skipped {} in {}: {err}", signal.name, message.name),
                );
                continue;
            }
            // `Counter` or `CRC` in several messages is normal. Later ones
            // take the same `Name@<id>` form a second DBC uses.
            let name = unique_name(&seen, &signal.name, 0, message.id);
            if name != signal.name {
                renamed.push(name.clone());
            }
            seen.insert(name.clone());
            signals.push(MappedSignal {
                name,
                unit: signal.unit.clone(),
                message_name: message.name.clone(),
                message_id: message.id,
                spec: signal.spec,
                channel: 0,
                mux_switch: signal.mux_switch,
                mux_value: signal.mux_value,
                table: signal.table.clone(),
            });
        }
    }
    if !renamed.is_empty() {
        let shown: Vec<&str> = renamed.iter().take(6).map(String::as_str).collect();
        let more = if renamed.len() > shown.len() {
            ", …"
        } else {
            ""
        };
        push_warn(
            &mut warnings,
            format!(
                "{} signal names repeat across messages and were renamed: {}{more}",
                renamed.len(),
                shown.join(", ")
            ),
        );
    }
    if signals.is_empty() {
        let extra = warnings
            .first()
            .map(|warning| format!(" {warning}"))
            .unwrap_or_default();
        return Err(Error::msg(format!(
            "DBC has no usable signals. Malformed records were skipped.{extra}"
        )));
    }
    Ok(SignalMap {
        name: "DBC import".to_string(),
        signals,
        messages: map_messages,
        warnings,
    })
}

struct MessageBuild {
    id: u32,
    name: String,
    dlc: u8,
    signals: Vec<SignalBuild>,
}

struct SignalBuild {
    name: String,
    unit: String,
    spec: DecodeSpec,
    mux_switch: bool,
    mux_value: Option<u32>,
    table: Vec<(i64, String)>,
}

fn parse_bo(rest: &str) -> Result<MessageBuild> {
    // 416 ECM_Engine: 8 ECM
    let rest = rest.trim().trim_end_matches(';');
    let mut parts = rest.split_whitespace();
    let id_tok = parts.next().unwrap_or("");
    let name_tok = parts.next().unwrap_or("");
    let dlc_tok = parts.next().unwrap_or("8");
    let id = normalize_id(parse_int(id_tok).map_err(Error::msg)?);
    let name = name_tok.trim_end_matches(':').to_string();
    if name.is_empty() {
        return Err(Error::msg(format!(
            "BO_ {id_tok} is missing a message name"
        )));
    }
    let dlc = parse_int(dlc_tok.trim_end_matches(':')).map_err(Error::msg)? as u8;
    Ok(MessageBuild {
        id,
        name,
        dlc: dlc.min(64),
        signals: Vec::new(),
    })
}

fn parse_sg(rest: &str) -> Result<SignalBuild> {
    // EngineRPM : 0|16@1+ (0.25,0) [0|16383] "rpm" TCU
    // Mode M : 0|8@1+ (1,0) [0|0] "" Vector__XXX
    // Gear m0 : 8|8@1+ (1,0) [0|0] "" Vector__XXX
    let rest = rest.trim().trim_end_matches(';');
    let Some((head, tail)) = rest.split_once(':') else {
        return Err(Error::msg(format!(
            "SG_ record is missing a layout: {rest}"
        )));
    };
    let head = head.trim();
    let mut parts = head.split_whitespace();
    let name = parts.next().unwrap_or("").to_string();
    if name.is_empty() {
        return Err(Error::msg(format!("SG_ record is missing a name: {rest}")));
    }
    let marker = parts.next().unwrap_or("");
    let (mux_switch, mux_value) = mux_marker(marker).ok_or_else(|| {
        Error::msg(format!(
            "signal {name} has a multiplex marker Signal Loom cannot import"
        ))
    })?;
    let mut signal = parse_sg_tail(&name, tail.trim()).ok_or_else(|| {
        Error::msg(format!(
            "signal {name} has a layout Signal Loom cannot import"
        ))
    })?;
    signal.mux_switch = mux_switch;
    signal.mux_value = mux_value;
    Ok(signal)
}

fn mux_marker(token: &str) -> Option<(bool, Option<u32>)> {
    if token.is_empty() {
        return Some((false, None));
    }
    if token == "M" {
        return Some((true, None));
    }
    let rest = token.strip_prefix('m')?;
    if rest.is_empty() || !rest.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((false, Some(rest.parse().ok()?)))
}

/// True when the text looks like a Vector DBC rather than a JSON signal map.
pub fn looks_like(text: &str) -> bool {
    let trimmed = text.trim_start().trim_start_matches('\u{feff}');
    if trimmed.starts_with('{') {
        return false;
    }
    trimmed.lines().any(|line| {
        let line = line.trim();
        line.starts_with("BO_ ") || line.starts_with("VERSION ") || line.starts_with("BS_:")
    })
}

fn parse_sg_tail(name: &str, tail: &str) -> Option<SignalBuild> {
    // 0|16@1+ (0.25,0) [0|16383.75] "rpm" TCU
    let (layout, after_layout) = tail.split_once(' ')?;
    let (start_len, order_sign) = layout.split_once('@')?;
    let (start_s, len_s) = start_len.split_once('|')?;
    let order = order_sign.chars().next()?;
    let sign = order_sign.chars().nth(1)?;
    let after_layout = after_layout.trim();
    let factor_offset = after_layout.strip_prefix('(')?.split_once(')')?.0;
    let (factor_s, offset_s) = factor_offset.split_once(',')?;
    let unit = quoted(after_layout).unwrap_or_default();
    let endian = if order == '1' {
        Endian::Little
    } else {
        Endian::Big
    };
    Some(SignalBuild {
        name: name.to_string(),
        unit,
        mux_switch: false,
        mux_value: None,
        table: Vec::new(),
        spec: DecodeSpec {
            start_bit: start_s.trim().parse().ok()?,
            bit_length: len_s.trim().parse().ok()?,
            factor: factor_s.trim().parse().ok()?,
            offset: offset_s.trim().parse().ok()?,
            signed: sign == '-',
            endian,
        },
    })
}

fn quoted(text: &str) -> Option<String> {
    let start = text.find('"')?;
    let end = text[start + 1..].find('"')?;
    Some(text[start + 1..start + 1 + end].to_string())
}

fn cycle_time(line: &str) -> Option<(u32, u64)> {
    // BA_ "GenMsgCycleTime" BO_ 416 10;
    let marker = "\"GenMsgCycleTime\"";
    let rest = line.split_once(marker)?.1;
    let rest = rest.trim().trim_start_matches("BO_").trim();
    let mut parts = rest.split_whitespace();
    let id = normalize_id(parse_int(parts.next()?).ok()?);
    let ms = parts.next()?.trim_end_matches(';').parse().ok()?;
    Some((id, ms))
}

struct ValueTable {
    id: u32,
    name: String,
    rows: Vec<(i64, String)>,
}

fn value_table(line: &str) -> Option<ValueTable> {
    // VAL_ 416 Gear 0 "N" 1 "D" ;
    let rest = line
        .strip_prefix("VAL_")?
        .trim()
        .trim_end_matches(';')
        .trim();
    let mut parts = rest.split_whitespace();
    let id = normalize_id(parse_int(parts.next()?).ok()?);
    let name = parts.next()?.to_string();
    let mut rows = Vec::new();
    let tail = rest.split_once(&name)?.1;
    let mut chars = tail.chars().peekable();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut number = String::new();
        if chars.peek() == Some(&'-') {
            number.push(chars.next()?);
        }
        while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
            number.push(chars.next()?);
        }
        if number.is_empty() || number == "-" {
            break;
        }
        let value: i64 = number.parse().ok()?;
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.next() != Some('"') {
            break;
        }
        let mut label = String::new();
        for ch in chars.by_ref() {
            if ch == '"' {
                break;
            }
            label.push(ch);
        }
        if rows.len() < 64 {
            rows.push((value, label));
        }
    }
    if rows.is_empty() {
        return None;
    }
    Some(ValueTable { id, name, rows })
}

fn push_warn(warnings: &mut Vec<String>, message: impl Into<String>) {
    if warnings.len() < 32 {
        let message = message.into();
        if !warnings.iter().any(|have| have == &message) {
            warnings.push(message);
        }
    }
}

fn parse_int(text: &str) -> std::result::Result<u32, String> {
    let text = text.trim().trim_end_matches(';');
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u32::from_str_radix(hex, 16).map_err(|_| format!("bad integer {text}"));
    }
    text.parse::<u32>()
        .map_err(|_| format!("bad integer {text}"))
}

/// Vector sets bit 31 on extended identifiers.
fn normalize_id(id: u32) -> u32 {
    mask_29(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
VERSION ""

NS_ :
BS_:
BU_: ECM TCU

BO_ 416 ECM_Engine: 8 ECM
 SG_ EngineRPM : 0|16@1+ (0.25,0) [0|16383.75] "rpm" TCU
 SG_ VehicleSpeed : 16|16@1+ (0.01,0) [0|655.35] "km/h" TCU
 SG_ Gear m0 : 32|3@1+ (1,0) [0|7] "" TCU

CM_ BO_ 416 "Powertrain";
BA_DEF_ BO_ "GenMsgCycleTime" INT 0 3600000;
BA_ "GenMsgCycleTime" BO_ 416 10;

BO_ 256 MotorolaDemo: 8 ECM
 SG_ BigWord : 7|16@0+ (1,0) [0|65535] "" Vector__XXX
"#;

    #[test]
    fn imports_intel_layout_and_multiplex_cases() {
        let map = parse(SAMPLE).unwrap();
        assert_eq!(map.name, "DBC import");
        assert_eq!(map.signals.len(), 4);
        let rpm = map.signals.iter().find(|s| s.name == "EngineRPM").unwrap();
        assert_eq!(rpm.message_id, 0x1A0);
        assert_eq!(rpm.unit, "rpm");
        let payload = hex_payload("800C881378640000");
        let value = rpm.spec.decode(&payload);
        assert!((value - 800.0).abs() < 1e-6);
        let speed = map
            .signals
            .iter()
            .find(|s| s.name == "VehicleSpeed")
            .unwrap();
        assert!((speed.spec.decode(&payload) - 50.0).abs() < 1e-3);
        let gear = map.signals.iter().find(|s| s.name == "Gear").unwrap();
        assert_eq!(gear.mux_value, Some(0));
        assert!(!gear.mux_switch);
        let message = map.messages.iter().find(|m| m.id == 0x1A0).unwrap();
        assert_eq!(message.cycle_us, Some(10_000));
        assert_eq!(message.name, "ECM_Engine");
    }

    #[test]
    fn rejects_a_broken_plain_signal() {
        let text = "BO_ 1 Only: 8 ECM\n SG_ Broken : not-a-layout\n";
        let err = parse(text).unwrap_err();
        assert!(err.to_string().contains("Broken"), "{err}");
    }

    #[test]
    fn imports_motorola_start_bit() {
        let map = parse(SAMPLE).unwrap();
        let word = map.signals.iter().find(|s| s.name == "BigWord").unwrap();
        let payload = hex_payload("1234000000000000");
        assert_eq!(word.spec.decode(&payload) as u64, 0x1234);
    }

    fn hex_payload(text: &str) -> [u8; 8] {
        let mut data = [0u8; 8];
        for i in 0..8 {
            data[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
        }
        data
    }
}
