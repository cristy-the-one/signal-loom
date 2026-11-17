//! `loom-verdict/1`, the file described in plan §5.
//!
//! The same evaluation, request hash, and log and DBC metadata always produce
//! the same bytes. The file has no clock, no host, and no path: `file` is the
//! base name the caller passes in.

use crate::index::evaluate::{
    CaseOutcome, CheckOutcome, Evaluation, EventKind, Evidence, Occurrence, Reason, Verdict,
};

const FORMAT: &str = "loom-verdict/1";
const TOOL: &str = "loom-judge 0.1.0";
/// Occurrences kept per check. The counts are never cut down to this.
const OCCURRENCE_CAP: usize = 50;

/// The recording, as §5 records it. `file` is a base name.
pub(crate) struct VerdictLog {
    pub file: String,
    pub sha256: String,
    pub t_start_us: u64,
    pub t_end_us: u64,
    pub frames: u64,
    pub skipped: u64,
    pub events_truncated: bool,
}

/// One DBC named in the verdict. `file` is a base name.
pub(crate) struct VerdictDbc {
    pub file: String,
    pub sha256: String,
    pub channel: u8,
}

/// The `loom-verdict/1` document, with its trailing newline.
///
/// Each check writes the first 50 occurrences, in the order `evaluate`
/// produced them (time order). `occurrences_truncated` is present, and
/// `true`, only when a check had more than that. The pass, fail,
/// inconclusive, and skipped counts are copied unchanged, so they still
/// cover every anchor.
///
/// An occurrence is one line, unless it carries an integrity event: the
/// occurrence is then expanded and the event object stays on one line. Empty
/// lists are `[]`. That is the layout of the hand-written contract fixture.
/// One width limit cannot produce it: the 61-character TC-4 case is expanded,
/// while a 78-character occurrence line is not.
pub(crate) fn render_verdict(
    evaluation: &Evaluation,
    request_sha256: &str,
    log: &VerdictLog,
    dbc: &[VerdictDbc],
) -> String {
    let mut out = String::from("{\n");
    kv_str(&mut out, 1, "format", FORMAT, true);
    kv_str(&mut out, 1, "tool", TOOL, true);
    kv_str(&mut out, 1, "request_sha256", request_sha256, true);
    kv_raw(&mut out, 1, "log", &render_log(1, log), true);
    kv_raw(&mut out, 1, "dbc", &render_dbcs(1, dbc), true);
    kv_raw(
        &mut out,
        1,
        "cases",
        &render_cases(1, &evaluation.cases),
        true,
    );
    kv_raw(
        &mut out,
        1,
        "counts",
        &render_counts(1, &evaluation.cases),
        false,
    );
    out.push_str("}\n");
    out
}

fn render_log(indent: usize, log: &VerdictLog) -> String {
    let mut out = String::from("{\n");
    kv_str(&mut out, indent + 1, "file", &log.file, true);
    kv_str(&mut out, indent + 1, "sha256", &log.sha256, true);
    kv_u64(&mut out, indent + 1, "t_start_us", log.t_start_us, true);
    kv_u64(&mut out, indent + 1, "t_end_us", log.t_end_us, true);
    kv_u64(&mut out, indent + 1, "frames", log.frames, true);
    kv_u64(&mut out, indent + 1, "skipped", log.skipped, true);
    kv_bool(
        &mut out,
        indent + 1,
        "events_truncated",
        log.events_truncated,
        false,
    );
    close(out, indent)
}

fn render_dbcs(indent: usize, dbcs: &[VerdictDbc]) -> String {
    let elements = dbcs
        .iter()
        .map(|dbc| render_dbc(indent + 1, dbc))
        .collect::<Vec<_>>();
    render_array(indent, &elements)
}

fn render_dbc(indent: usize, dbc: &VerdictDbc) -> String {
    let mut out = String::from("{\n");
    kv_str(&mut out, indent + 1, "file", &dbc.file, true);
    kv_str(&mut out, indent + 1, "sha256", &dbc.sha256, true);
    kv_u64(
        &mut out,
        indent + 1,
        "channel",
        u64::from(dbc.channel),
        false,
    );
    close(out, indent)
}

fn render_cases(indent: usize, cases: &[CaseOutcome]) -> String {
    let elements = cases
        .iter()
        .map(|case| render_case(indent + 1, case))
        .collect::<Vec<_>>();
    render_array(indent, &elements)
}

fn render_case(indent: usize, case: &CaseOutcome) -> String {
    let mut out = String::from("{\n");
    kv_str(&mut out, indent + 1, "case_id", &case.case_id, true);
    kv_str(
        &mut out,
        indent + 1,
        "verdict",
        verdict_name(case.verdict),
        true,
    );
    kv_raw(
        &mut out,
        indent + 1,
        "checks",
        &render_checks(indent + 1, &case.checks),
        false,
    );
    close(out, indent)
}

fn render_checks(indent: usize, checks: &[CheckOutcome]) -> String {
    let elements = checks
        .iter()
        .map(|check| render_check(indent + 1, check))
        .collect::<Vec<_>>();
    render_array(indent, &elements)
}

fn render_check(indent: usize, check: &CheckOutcome) -> String {
    let kept = check.occurrences.len().min(OCCURRENCE_CAP);
    let truncated = check.occurrences.len() > OCCURRENCE_CAP;
    let occurrences = check.occurrences[..kept]
        .iter()
        .map(|occurrence| render_occurrence(indent + 2, occurrence))
        .collect::<Vec<_>>();
    let mut out = String::from("{\n");
    kv_str(&mut out, indent + 1, "id", &check.id, true);
    kv_str(
        &mut out,
        indent + 1,
        "verdict",
        verdict_name(check.verdict),
        true,
    );
    kv_u64(&mut out, indent + 1, "anchors", check.anchors, true);
    kv_u64(&mut out, indent + 1, "skipped", check.skipped, true);
    kv_u64(&mut out, indent + 1, "pass", check.pass, true);
    kv_u64(&mut out, indent + 1, "fail", check.fail, true);
    kv_u64(
        &mut out,
        indent + 1,
        "inconclusive",
        check.inconclusive,
        true,
    );
    // A comma follows the list only when `occurrences_truncated` is next.
    kv_raw(
        &mut out,
        indent + 1,
        "occurrences",
        &render_array(indent + 1, &occurrences),
        truncated,
    );
    if truncated {
        kv_bool(&mut out, indent + 1, "occurrences_truncated", true, false);
    }
    close(out, indent)
}

/// `indent` is the brace level of an expanded occurrence. A one-line
/// occurrence ignores it; the array writer supplies that indent.
fn render_occurrence(indent: usize, occurrence: &Occurrence) -> String {
    match &occurrence.evidence {
        Evidence::Sample { t_us, value } => format!(
            "{{\"anchor_us\": {}, \"verdict\": {}, \"t_us\": {t_us}, \"value\": {}}}",
            occurrence.anchor_us,
            json_string(verdict_name(occurrence.verdict)),
            json_number(*value),
        ),
        Evidence::Reason {
            reason,
            event: None,
        } => format!(
            "{{\"anchor_us\": {}, \"verdict\": {}, \"reason\": {}}}",
            occurrence.anchor_us,
            json_string(verdict_name(occurrence.verdict)),
            json_string(reason_name(*reason)),
        ),
        Evidence::Reason {
            reason,
            event: Some((t_us, kind)),
        } => {
            let event = format!(
                "{{\"t_us\": {t_us}, \"kind\": {}}}",
                json_string(kind_name(*kind))
            );
            let mut out = String::from("{\n");
            kv_u64(
                &mut out,
                indent + 1,
                "anchor_us",
                occurrence.anchor_us,
                true,
            );
            kv_str(
                &mut out,
                indent + 1,
                "verdict",
                verdict_name(occurrence.verdict),
                true,
            );
            kv_str(&mut out, indent + 1, "reason", reason_name(*reason), true);
            kv_raw(&mut out, indent + 1, "event", &event, false);
            close(out, indent)
        }
    }
}

fn render_counts(indent: usize, cases: &[CaseOutcome]) -> String {
    let mut pass = 0u64;
    let mut fail = 0u64;
    let mut inconclusive = 0u64;
    let mut not_exercised = 0u64;
    let mut not_judgeable = 0u64;
    for case in cases {
        match case.verdict {
            Verdict::Pass => pass += 1,
            Verdict::Fail => fail += 1,
            Verdict::Inconclusive => inconclusive += 1,
            Verdict::NotExercised => not_exercised += 1,
            Verdict::NotJudgeable => not_judgeable += 1,
        }
    }
    let mut out = String::from("{\n");
    kv_u64(&mut out, indent + 1, "cases", cases.len() as u64, true);
    kv_u64(&mut out, indent + 1, "pass", pass, true);
    kv_u64(&mut out, indent + 1, "fail", fail, true);
    kv_u64(&mut out, indent + 1, "inconclusive", inconclusive, true);
    kv_u64(&mut out, indent + 1, "not_exercised", not_exercised, true);
    kv_u64(&mut out, indent + 1, "not_judgeable", not_judgeable, false);
    close(out, indent)
}

fn render_array(indent: usize, elements: &[String]) -> String {
    if elements.is_empty() {
        return "[]".to_string();
    }
    let mut out = String::from("[\n");
    for (index, element) in elements.iter().enumerate() {
        indent_to(&mut out, indent + 1);
        out.push_str(element);
        if index + 1 != elements.len() {
            out.push(',');
        }
        out.push('\n');
    }
    indent_to(&mut out, indent);
    out.push(']');
    out
}

fn close(mut out: String, indent: usize) -> String {
    indent_to(&mut out, indent);
    out.push('}');
    out
}

fn kv_str(out: &mut String, indent: usize, key: &str, value: &str, comma: bool) {
    kv_raw(out, indent, key, &json_string(value), comma);
}

fn kv_u64(out: &mut String, indent: usize, key: &str, value: u64, comma: bool) {
    kv_raw(out, indent, key, &value.to_string(), comma);
}

fn kv_bool(out: &mut String, indent: usize, key: &str, value: bool, comma: bool) {
    kv_raw(
        out,
        indent,
        key,
        if value { "true" } else { "false" },
        comma,
    );
}

fn kv_raw(out: &mut String, indent: usize, key: &str, value: &str, comma: bool) {
    indent_to(out, indent);
    out.push_str(&json_string(key));
    out.push_str(": ");
    out.push_str(value);
    if comma {
        out.push(',');
    }
    out.push('\n');
}

fn indent_to(out: &mut String, levels: usize) {
    for _ in 0..levels {
        out.push_str("  ");
    }
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).expect("a string has a JSON encoding")
}

/// Whole numbers inside the exact integer range of `f64` are written without
/// a fraction, as the contract fixture writes `1` and `0`. Anything else uses
/// serde_json's number format. A non-finite value has no JSON number, so it
/// is `null`.
fn json_number(value: f64) -> String {
    const EXACT: f64 = (1_i64 << 53) as f64;
    if value.is_finite() && (-EXACT..=EXACT).contains(&value) {
        let int = value as i64;
        if int as f64 == value {
            return int.to_string();
        }
    }
    serde_json::Number::from_f64(value)
        .map(|number| number.to_string())
        .unwrap_or_else(|| "null".to_string())
}

fn verdict_name(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Pass => "pass",
        Verdict::Fail => "fail",
        Verdict::Inconclusive => "inconclusive",
        Verdict::NotExercised => "not_exercised",
        Verdict::NotJudgeable => "not_judgeable",
    }
}

fn reason_name(reason: Reason) -> &'static str {
    match reason {
        Reason::GivenUnknown => "given_unknown",
        Reason::WindowPastEnd => "window_past_end",
        Reason::NoValue => "no_value",
        Reason::Integrity => "integrity",
        Reason::IntegrityUnknown => "integrity_unknown",
    }
}

fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Timeout => "timeout",
        EventKind::Checksum => "checksum",
        EventKind::Counter => "counter",
    }
}
