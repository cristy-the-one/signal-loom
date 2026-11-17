//! A `loom-judge/1` request, and the refusal check that lists every problem
//! before a recording is read.
//!
//! Signal pairs are the DBC `BO_` name and `SG_` name. [`SignalMap::lookup`]
//! resolves that pair. An optional `channel` filters to the copy loaded on
//! that channel; without it, more than one loaded signal is ambiguous.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::ErrorKind;
use crate::map::{MappedSignal, SignalMap};

const FORMAT: &str = "loom-judge/1";
const MAX_WITHIN_MS: i64 = 3_600_000;
const OPS: &[&str] = &["==", "!=", ">", ">=", "<", "<="];
const MODES: &[&str] = &["eventually", "always", "never"];
const KINDS: &[&str] = &["positive"];

/// One judge request. Unknown keys are refused.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub format: String,
    pub spec_id: String,
    pub cases: Vec<Case>,
}

/// One test case. Zero checks is allowed: the case is not judgeable, not an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub case_id: String,
    pub verifies: Vec<String>,
    pub kind: Kind,
    pub checks: Vec<Check>,
}

/// `positive` is the only kind in `loom-judge/1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Positive,
}

/// A check against one anchor signal and one expected signal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub given: Vec<Condition>,
    pub when: Condition,
    pub expect: Condition,
    pub mode: Mode,
    pub within_ms: [i64; 2],
}

/// How the expected condition is judged inside the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Eventually,
    Always,
    Never,
}

/// A comparison of one signal to a number or a `VAL_` label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    pub signal: SignalRef,
    pub op: Op,
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f64>,
}

/// DBC message name, DBC signal name, and an optional channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignalRef {
    pub message: String,
    pub signal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<i64>,
}

/// `==`, `!=`, `>`, `>=`, `<`, `<=`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
}

/// A physical number, or a `VAL_` label standing for one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Number(f64),
    Label(String),
}

/// Every problem found in one request, in the order the request was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub problems: Vec<String>,
}

/// Refuse `text` with every problem, or return the parsed request.
///
/// `dbc` is the loaded map. A channel selects the copy on that channel and
/// hands the pair to [`SignalMap::lookup`]. No channel, and more than one
/// loaded signal, is ambiguous and names those channels.
pub fn check(text: &str, dbc: &SignalMap) -> std::result::Result<Request, Refusal> {
    let mut problems = Vec::new();
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) => {
            return Err(Refusal {
                problems: vec![format!("request is not JSON: {err}")],
            });
        }
    };
    let Some(root) = value.as_object() else {
        return Err(Refusal {
            problems: vec!["request must be a JSON object".to_string()],
        });
    };
    walk_request(root, dbc, &mut problems);
    if !problems.is_empty() {
        return Err(Refusal { problems });
    }
    match serde_json::from_str(text) {
        Ok(request) => Ok(request),
        Err(err) => Err(Refusal {
            problems: vec![format!("request does not match {FORMAT}: {err}")],
        }),
    }
}

fn walk_request(
    map: &serde_json::Map<String, serde_json::Value>,
    dbc: &SignalMap,
    problems: &mut Vec<String>,
) {
    unknown_keys(
        map,
        &["format", "spec_id", "cases"],
        "the request",
        problems,
    );
    require_key(map, "format", "the request", problems);
    require_key(map, "spec_id", "the request", problems);
    require_key(map, "cases", "the request", problems);
    if let Some(format) = map.get("format") {
        if format.as_str() != Some(FORMAT) {
            let shown = match format.as_str() {
                Some(text) => format!("\"{text}\""),
                None => "not a string".to_string(),
            };
            problems.push(format!("format is {shown}, expected {FORMAT}"));
        }
    }
    if let Some(spec_id) = map.get("spec_id") {
        if !spec_id.is_string() {
            problems.push("spec_id must be a string".to_string());
        }
    }
    if let Some(cases) = map.get("cases") {
        walk_cases(cases, dbc, problems);
    }
}

fn walk_cases(value: &serde_json::Value, dbc: &SignalMap, problems: &mut Vec<String>) {
    let Some(cases) = value.as_array() else {
        problems.push("cases must be a list of cases".to_string());
        return;
    };
    let mut seen = HashSet::new();
    for case in cases {
        walk_case(case, dbc, &mut seen, problems);
    }
}

fn walk_case(
    value: &serde_json::Value,
    dbc: &SignalMap,
    seen: &mut HashSet<String>,
    problems: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        problems.push("a case must be an object".to_string());
        return;
    };
    let case_id = map.get("case_id").and_then(serde_json::Value::as_str);
    let on = match case_id {
        Some(id) => format!("case \"{id}\""),
        None => "a case".to_string(),
    };
    unknown_keys(
        map,
        &["case_id", "verifies", "kind", "checks"],
        &on,
        problems,
    );
    match map.get("case_id") {
        None => problems.push(missing("case_id", &on)),
        Some(value) if value.is_string() => {}
        Some(_) => problems.push("case_id must be a string".to_string()),
    }
    if let Some(id) = case_id {
        if !seen.insert(id.to_string()) {
            problems.push(format!("case_id \"{id}\" is not unique"));
        }
    }
    match map.get("verifies") {
        None => problems.push(missing("verifies", &on)),
        Some(serde_json::Value::Array(items)) if items.iter().all(serde_json::Value::is_string) => {
        }
        Some(_) => problems.push("verifies must be a list of strings".to_string()),
    }
    match map.get("kind") {
        None => problems.push(missing("kind", &on)),
        Some(kind) => one_of(kind, "kind", KINDS, problems),
    }
    match map.get("checks") {
        None => problems.push(missing("checks", &on)),
        Some(checks) => walk_checks(checks, case_id, dbc, problems),
    }
}

fn walk_checks(
    value: &serde_json::Value,
    case_id: Option<&str>,
    dbc: &SignalMap,
    problems: &mut Vec<String>,
) {
    let Some(checks) = value.as_array() else {
        problems.push("checks must be a list of checks".to_string());
        return;
    };
    let mut seen = HashSet::new();
    for check in checks {
        walk_check(check, case_id, dbc, &mut seen, problems);
    }
}

fn walk_check(
    value: &serde_json::Value,
    case_id: Option<&str>,
    dbc: &SignalMap,
    seen: &mut HashSet<String>,
    problems: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        problems.push("a check must be an object".to_string());
        return;
    };
    let check_id = map.get("id").and_then(serde_json::Value::as_str);
    let on = match check_id {
        Some(id) => format!("check \"{id}\""),
        None => "a check".to_string(),
    };
    unknown_keys(
        map,
        &["id", "given", "when", "expect", "mode", "within_ms"],
        &on,
        problems,
    );
    match map.get("id") {
        None => problems.push(missing("id", &on)),
        Some(value) if value.is_string() => {}
        Some(_) => problems.push("id must be a string".to_string()),
    }
    if let (Some(case_id), Some(id)) = (case_id, check_id) {
        if !seen.insert(id.to_string()) {
            problems.push(format!(
                "check id \"{id}\" is not unique in case \"{case_id}\""
            ));
        }
    }
    if let Some(given) = map.get("given") {
        match given.as_array() {
            Some(items) => {
                for item in items {
                    walk_condition(item, dbc, problems);
                }
            }
            None => problems.push("given must be a list of conditions".to_string()),
        }
    }
    for key in ["when", "expect"] {
        match map.get(key) {
            None => problems.push(missing(key, &on)),
            Some(condition) => walk_condition(condition, dbc, problems),
        }
    }
    match map.get("mode") {
        None => problems.push(missing("mode", &on)),
        Some(mode) => one_of(mode, "mode", MODES, problems),
    }
    match map.get("within_ms") {
        None => problems.push(missing("within_ms", &on)),
        Some(window) => check_window(window, problems),
    }
}

fn walk_condition(value: &serde_json::Value, dbc: &SignalMap, problems: &mut Vec<String>) {
    let Some(map) = value.as_object() else {
        problems.push("a condition must be an object".to_string());
        return;
    };
    unknown_keys(
        map,
        &["signal", "op", "value", "tolerance"],
        "a condition",
        problems,
    );
    let signal = match map.get("signal") {
        None => {
            problems.push(missing("signal", "a condition"));
            None
        }
        Some(signal) => Some(signal),
    };
    let op = match map.get("op") {
        None => {
            problems.push(missing("op", "a condition"));
            None
        }
        Some(op) => read_op(op, problems),
    };
    let compared = match map.get("value") {
        None => {
            problems.push(missing("value", "a condition"));
            None
        }
        Some(compared) => {
            if !matches!(
                compared,
                serde_json::Value::Number(_) | serde_json::Value::String(_)
            ) {
                problems.push("value must be a number or a string".to_string());
            }
            Some(compared)
        }
    };
    if let Some(signal) = signal {
        walk_signal(signal, compared, dbc, problems);
    }
    if let Some(tolerance) = map.get("tolerance") {
        if !tolerance.is_null() {
            check_tolerance(tolerance, op, problems);
        }
    }
}

fn walk_signal(
    value: &serde_json::Value,
    compared: Option<&serde_json::Value>,
    dbc: &SignalMap,
    problems: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else {
        problems.push("a signal must be an object".to_string());
        return;
    };
    unknown_keys(map, &["message", "signal", "channel"], "a signal", problems);
    let message = read_name(map, "message", problems);
    let signal = read_name(map, "signal", problems);
    let channel = read_channel(map, problems);
    if let (Some(message), Some(signal), Some(channel)) = (message, signal, channel) {
        resolve(dbc, message, signal, channel, compared, problems);
    }
}

fn read_name<'a>(
    map: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
    problems: &mut Vec<String>,
) -> Option<&'a str> {
    match map.get(key) {
        None => {
            problems.push(missing(key, "a signal"));
            None
        }
        Some(serde_json::Value::String(text)) => Some(text),
        Some(_) => {
            problems.push(format!("{key} must be a string"));
            None
        }
    }
}

/// `None` means the channel value itself is unusable, so the pair is not resolved.
/// `Some(None)` means the reference names no channel.
fn read_channel(
    map: &serde_json::Map<String, serde_json::Value>,
    problems: &mut Vec<String>,
) -> Option<Option<i64>> {
    match map.get("channel") {
        None | Some(serde_json::Value::Null) => Some(None),
        Some(value) => match as_int(value) {
            Some(channel) => Some(Some(channel)),
            None => {
                problems.push("channel must be an integer".to_string());
                None
            }
        },
    }
}

fn resolve(
    dbc: &SignalMap,
    message: &str,
    signal: &str,
    channel: Option<i64>,
    compared: Option<&serde_json::Value>,
    problems: &mut Vec<String>,
) {
    if let Some(channel) = channel {
        // lookup has no channel argument. The view is the signals loaded on
        // this channel, so the pair rule stays the one in SignalMap::lookup.
        let view = on_channel(dbc, channel);
        decide(&view, message, signal, Some(channel), compared, problems);
        return;
    }
    decide(dbc, message, signal, None, compared, problems);
}

fn decide(
    dbc: &SignalMap,
    message: &str,
    signal: &str,
    channel: Option<i64>,
    compared: Option<&serde_json::Value>,
    problems: &mut Vec<String>,
) {
    match dbc.lookup(message, signal) {
        Ok(found) => reject_bad_label(&[found], message, signal, compared, problems),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let text = match channel {
                Some(channel) => format!("{err} on channel {channel}"),
                None => err.to_string(),
            };
            problems.push(text);
        }
        Err(_) => {
            problems.push(ambiguous(dbc, message, signal));
            let found = pairs(dbc, message, signal);
            reject_bad_label(&found, message, signal, compared, problems);
        }
    }
}

fn on_channel(dbc: &SignalMap, channel: i64) -> SignalMap {
    SignalMap {
        name: String::new(),
        signals: dbc
            .signals
            .iter()
            .filter(|signal| i64::from(signal.channel) == channel)
            .cloned()
            .collect(),
        messages: Vec::new(),
        warnings: Vec::new(),
    }
}

fn pairs<'a>(dbc: &'a SignalMap, message: &str, signal: &str) -> Vec<&'a MappedSignal> {
    dbc.signals
        .iter()
        .filter(|candidate| candidate.message_name == message && candidate.defined_name == signal)
        .collect()
}

fn ambiguous(dbc: &SignalMap, message: &str, signal: &str) -> String {
    let mut channels: Vec<u8> = pairs(dbc, message, signal)
        .into_iter()
        .map(|candidate| candidate.channel)
        .collect();
    channels.sort_unstable();
    let listed = match channels.as_slice() {
        [first, second] => format!("{first} and {second}"),
        other => other
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    };
    format!("signal pair ({message}, {signal}) is ambiguous; it matched channels {listed}")
}

fn reject_bad_label(
    signals: &[&MappedSignal],
    message: &str,
    signal: &str,
    compared: Option<&serde_json::Value>,
    problems: &mut Vec<String>,
) {
    let Some(text) = compared.and_then(serde_json::Value::as_str) else {
        return;
    };
    if signals.is_empty() {
        return;
    }
    let listed = signals
        .iter()
        .all(|candidate| candidate.table.iter().any(|(_, label)| label == text));
    if !listed {
        problems.push(format!(
            "value \"{text}\" is not a label of signal ({message}, {signal})"
        ));
    }
}

fn read_op<'a>(value: &'a serde_json::Value, problems: &mut Vec<String>) -> Option<&'a str> {
    match value.as_str() {
        Some(text) if OPS.contains(&text) => Some(text),
        Some(text) => {
            problems.push(format!("op \"{text}\" is not one of {}", OPS.join(", ")));
            Some(text)
        }
        None => {
            problems.push(format!("op is not one of {}", OPS.join(", ")));
            None
        }
    }
}

fn one_of(value: &serde_json::Value, what: &str, allowed: &[&str], problems: &mut Vec<String>) {
    let list = allowed.join(", ");
    match value.as_str() {
        Some(text) if allowed.contains(&text) => {}
        Some(text) => problems.push(format!("{what} \"{text}\" is not one of {list}")),
        None => problems.push(format!("{what} is not one of {list}")),
    }
}

fn check_window(value: &serde_json::Value, problems: &mut Vec<String>) {
    let shape = "within_ms must be two integers [min, max] with 0 <= min <= max <= 3600000";
    let Some(items) = value.as_array() else {
        problems.push(shape.to_string());
        return;
    };
    if items.len() != 2 {
        problems.push(shape.to_string());
        return;
    }
    let (Some(min), Some(max)) = (as_int(&items[0]), as_int(&items[1])) else {
        problems.push(shape.to_string());
        return;
    };
    if min < 0 || max < min || max > MAX_WITHIN_MS {
        problems.push(format!(
            "within_ms [{min}, {max}] breaks 0 <= min <= max <= 3600000"
        ));
    }
}

fn check_tolerance(value: &serde_json::Value, op: Option<&str>, problems: &mut Vec<String>) {
    match value.as_f64() {
        Some(number) if number >= 0.0 && number.is_finite() => {}
        Some(number) if number.is_finite() => {
            problems.push(format!("tolerance {number} is negative"));
        }
        _ => problems.push("tolerance must be a number".to_string()),
    }
    if let Some(op) = op {
        if op != "==" && op != "!=" {
            problems.push(format!(
                "tolerance on op \"{op}\" applies to == and != only"
            ));
        }
    }
}

fn as_int(value: &serde_json::Value) -> Option<i64> {
    let number = value.as_number()?;
    if let Some(value) = number.as_i64() {
        return Some(value);
    }
    number.as_u64().and_then(|value| i64::try_from(value).ok())
}

fn unknown_keys(
    map: &serde_json::Map<String, serde_json::Value>,
    allowed: &[&str],
    on: &str,
    problems: &mut Vec<String>,
) {
    for key in map.keys() {
        if !allowed.contains(&key.as_str()) {
            problems.push(format!("unknown key \"{key}\" on {on}"));
        }
    }
}

fn require_key(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    on: &str,
    problems: &mut Vec<String>,
) {
    if !map.contains_key(key) {
        problems.push(missing(key, on));
    }
}

fn missing(key: &str, on: &str) -> String {
    format!("missing key \"{key}\" on {on}")
}
