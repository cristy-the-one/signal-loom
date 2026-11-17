//! `loom-dbc-dump/1`, written by `loom-judge --dump-dbc`.
//!
//! Each signal is one line, sorted by id, start bit, and the name on the
//! `SG_` line. A whole number is written as an integer, by the same rule as
//! the verdict file. Min and max are not in the file: a signal map does not
//! keep them.

use crate::decode::Endian;
use crate::json_number::json_number;
use crate::map::{MappedSignal, SignalMap};

const FORMAT: &str = "loom-dbc-dump/1";
/// Vector sets this bit on an extended `BO_` id.
const EXTENDED_ID: u32 = 0x8000_0000;

/// The `loom-dbc-dump/1` document for `map`, including its trailing newline.
pub fn dump_dbc(map: &SignalMap) -> String {
    let mut signals: Vec<&MappedSignal> = map.signals.iter().collect();
    signals.sort_by(|left, right| {
        dump_id(left.message_id)
            .cmp(&dump_id(right.message_id))
            .then(left.spec.start_bit.cmp(&right.spec.start_bit))
            .then(left.defined_name.cmp(&right.defined_name))
    });

    let mut out = String::from("{\n  \"format\": \"");
    out.push_str(FORMAT);
    out.push_str("\",\n  \"signals\": [\n");
    for (index, signal) in signals.iter().enumerate() {
        out.push_str("    ");
        out.push_str(&render_signal(signal));
        if index + 1 != signals.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  ]\n}\n");
    out
}

fn dump_id(id: u32) -> u32 {
    id & !EXTENDED_ID
}

fn render_signal(signal: &MappedSignal) -> String {
    let byte_order = match signal.spec.endian {
        Endian::Little => "little_endian",
        Endian::Big => "big_endian",
    };
    let multiplexed_by = match signal.mux_value {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    };
    let mut out = String::from("{");
    // The start bit is the one stored from the `SG_` line. Motorola is not
    // rewritten on import, so this is the bit the DBC wrote.
    field_str(&mut out, "message", &signal.message_name);
    field(&mut out, "id", &dump_id(signal.message_id).to_string());
    field_str(&mut out, "signal", &signal.defined_name);
    field(&mut out, "start_bit", &signal.spec.start_bit.to_string());
    field(&mut out, "length", &signal.spec.bit_length.to_string());
    field_str(&mut out, "byte_order", byte_order);
    field(&mut out, "signed", json_bool(signal.spec.signed));
    field(&mut out, "factor", &json_number(signal.spec.factor));
    field(&mut out, "offset", &json_number(signal.spec.offset));
    field_str(&mut out, "unit", &signal.unit);
    field(&mut out, "multiplexer", json_bool(signal.mux_switch));
    field(&mut out, "multiplexed_by", &multiplexed_by);
    field_last(&mut out, "labels", &render_labels(&signal.table));
    out.push('}');
    out
}

fn render_labels(table: &[(i64, String)]) -> String {
    let mut rows: Vec<&(i64, String)> = table.iter().collect();
    rows.sort_by_key(|row| row.0);
    let mut out = String::from("[");
    for (index, (raw, text)) in rows.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push('[');
        out.push_str(&raw.to_string());
        out.push_str(", ");
        out.push_str(&json_string(text));
        out.push(']');
    }
    out.push(']');
    out
}

fn field_str(out: &mut String, key: &str, value: &str) {
    field(out, key, &json_string(value));
}

fn field(out: &mut String, key: &str, value: &str) {
    push_field(out, key, value);
    out.push_str(", ");
}

fn field_last(out: &mut String, key: &str, value: &str) {
    push_field(out, key, value);
}

fn push_field(out: &mut String, key: &str, value: &str) {
    out.push_str(&json_string(key));
    out.push_str(": ");
    out.push_str(value);
}

fn json_bool(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).expect("a string has a JSON encoding")
}
