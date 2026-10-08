//! SLOGv1 text: `F <t_us> <hex id> <hex payload>`, `E <t_us> <label>`, `X <t_us>`.

use super::id::{implied_extended, parse_id, IdBase};
use super::text::{parse_payload, parse_time, LineParser, LineResult};
use super::Rec;

/// Longest event label a text line may carry.
const MAX_LABEL: usize = 400;

pub(super) struct Slog;

impl LineParser for Slog {
    fn parse(&mut self, line: &str) -> LineResult {
        let mut parts = line.split_whitespace();
        let tag = parts.next().unwrap_or("");
        let t_us = parse_time(parts.next())?;
        match tag {
            "F" => {
                let id_tok = parts.next().unwrap_or("");
                let id = parse_id(id_tok, IdBase::PrefixedHex).ok_or_else(|| {
                    if id_tok.trim().is_empty() {
                        "missing CAN id".to_string()
                    } else {
                        format!("bad hex CAN id '{}'", id_tok.trim())
                    }
                })?;
                let data_tok = parts.next().unwrap_or("");
                if parts.next().is_some() {
                    return Err("frame record has extra columns".into());
                }
                let (data, dlc) = parse_payload(data_tok)?;
                Ok(Some(Rec::frame(
                    t_us,
                    id,
                    implied_extended(id),
                    0,
                    dlc,
                    data,
                )))
            }
            "E" => {
                let mut label = String::new();
                for part in parts {
                    if !label.is_empty() {
                        label.push(' ');
                    }
                    label.push_str(part);
                }
                if label.is_empty() {
                    return Err("event is missing a label".into());
                }
                if label.len() > MAX_LABEL {
                    return Err("event label is too long".into());
                }
                Ok(Some(Rec::event(t_us, label)))
            }
            "X" => {
                if parts.next().is_some() {
                    return Err("error frame has extra columns".into());
                }
                Ok(Some(Rec::event(t_us, "Error frame".to_string())))
            }
            _ => Err("expected an F frame, E event, or X error frame".into()),
        }
    }
}
