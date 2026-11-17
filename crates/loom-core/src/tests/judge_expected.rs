//! SL-16a: `judge::evaluate` on `fixtures/judge/` against the hand-written
//! `expected-verdict.json`.
//!
//! The verdict file writer is SL-14, so this compares the evaluator itself:
//! every case, check, and occurrence, field by field, plus the case counts
//! those verdicts roll up to. A mismatch is printed and the expected file is
//! left as written.

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::index::IndexedLog;
use crate::judge::{
    self, CaseOutcome, CheckOutcome, EventKind, Evidence, Occurrence, Reason, Verdict,
};
use crate::map::TimeoutFactor;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
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

fn occurrence_json(occurrence: &Occurrence) -> Value {
    let mut fields = serde_json::Map::new();
    fields.insert("anchor_us".to_string(), json!(occurrence.anchor_us));
    fields.insert(
        "verdict".to_string(),
        json!(verdict_name(occurrence.verdict)),
    );
    match &occurrence.evidence {
        Evidence::Sample { t_us, value } => {
            fields.insert("t_us".to_string(), json!(t_us));
            fields.insert("value".to_string(), json!(value));
        }
        Evidence::Reason { reason, event } => {
            fields.insert("reason".to_string(), json!(reason_name(*reason)));
            if let Some((t_us, kind)) = event {
                fields.insert(
                    "event".to_string(),
                    json!({"t_us": t_us, "kind": kind_name(*kind)}),
                );
            }
        }
    }
    Value::Object(fields)
}

fn check_json(check: &CheckOutcome) -> Value {
    json!({
        "id": check.id,
        "verdict": verdict_name(check.verdict),
        "anchors": check.anchors,
        "skipped": check.skipped,
        "pass": check.pass,
        "fail": check.fail,
        "inconclusive": check.inconclusive,
        "occurrences": check.occurrences.iter().map(occurrence_json).collect::<Vec<_>>(),
    })
}

fn case_json(case: &CaseOutcome) -> Value {
    json!({
        "case_id": case.case_id,
        "verdict": verdict_name(case.verdict),
        "checks": case.checks.iter().map(check_json).collect::<Vec<_>>(),
    })
}

fn counts_json(cases: &[CaseOutcome]) -> Value {
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
    json!({
        "cases": cases.len(),
        "pass": pass,
        "fail": fail,
        "inconclusive": inconclusive,
        "not_exercised": not_exercised,
        "not_judgeable": not_judgeable,
    })
}

fn show(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
}

fn same_number(left: &Value, right: &Value) -> bool {
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}

fn diff(path: &str, expected: &Value, actual: &Value, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Object(expected), Value::Object(actual)) => {
            let mut keys: Vec<&String> = expected.keys().chain(actual.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys {
                let child = format!("{path}.{key}");
                match (expected.get(key), actual.get(key)) {
                    (Some(expected), Some(actual)) => diff(&child, expected, actual, out),
                    (Some(expected), None) => out.push(format!(
                        "{child}: missing from the judge; expected {}",
                        show(expected)
                    )),
                    (None, Some(actual)) => out.push(format!(
                        "{child}: judge has {}, the hand-written verdict does not",
                        show(actual)
                    )),
                    (None, None) => {}
                }
            }
        }
        (Value::Array(expected), Value::Array(actual)) => {
            if expected.len() != actual.len() {
                out.push(format!(
                    "{path}: expected {} entries, judge {}",
                    expected.len(),
                    actual.len()
                ));
            }
            let shared = expected.len().min(actual.len());
            for index in 0..shared {
                diff(
                    &format!("{path}[{index}]"),
                    &expected[index],
                    &actual[index],
                    out,
                );
            }
            for (index, value) in expected.iter().enumerate().skip(shared) {
                out.push(format!(
                    "{path}[{index}]: missing from the judge; expected {}",
                    show(value)
                ));
            }
            for (index, value) in actual.iter().enumerate().skip(shared) {
                out.push(format!(
                    "{path}[{index}]: judge has {}, the hand-written verdict does not",
                    show(value)
                ));
            }
        }
        (Value::Number(_), Value::Number(_)) if same_number(expected, actual) => {}
        _ => {
            if expected != actual {
                out.push(format!(
                    "{path}: expected {}, judge {}",
                    show(expected),
                    show(actual)
                ));
            }
        }
    }
}

fn case_label(index: usize, value: &Value) -> String {
    match value.get("case_id").and_then(Value::as_str) {
        Some(id) => format!("cases[{index}] {id}"),
        None => format!("cases[{index}]"),
    }
}

#[test]
fn evaluate_matches_the_hand_written_verdict() {
    let dir = repo_root().join("fixtures/judge");
    let dbc_text = std::fs::read_to_string(dir.join("brake.dbc")).expect("read brake.dbc");
    let dbc = crate::dbc::parse(&dbc_text).expect("brake.dbc");
    let log = IndexedLog::open_path(&dir.join("brake.asc"), Some(&dbc)).expect("open brake.asc");
    let request_text =
        std::fs::read_to_string(dir.join("request.json")).expect("read request.json");
    let request = judge::check(&request_text, &dbc)
        .unwrap_or_else(|refusal| panic!("request refused: {}", refusal.problems.join("; ")));
    let evaluation = judge::evaluate(&log, &dbc, TimeoutFactor::default(), &request)
        .unwrap_or_else(|err| panic!("judge::evaluate failed: {err}"));

    let expected: Value = serde_json::from_slice(
        &std::fs::read(dir.join("expected-verdict.json")).expect("read expected-verdict.json"),
    )
    .expect("expected-verdict.json");
    let Some(expected_cases) = expected["cases"].as_array() else {
        panic!("expected-verdict.json has no cases array");
    };
    let actual_cases: Vec<Value> = evaluation.cases.iter().map(case_json).collect();

    let mut lines = Vec::new();
    let mut diffs = Vec::new();
    if expected_cases.len() != actual_cases.len() {
        diffs.push(format!(
            "cases: expected {} cases, judge {}",
            expected_cases.len(),
            actual_cases.len()
        ));
    }
    let shared = expected_cases.len().max(actual_cases.len());
    for index in 0..shared {
        let (expected_case, actual_case) = (expected_cases.get(index), actual_cases.get(index));
        let label = case_label(index, expected_case.or(actual_case).unwrap_or(&Value::Null));
        let before = diffs.len();
        match (expected_case, actual_case) {
            (Some(expected_case), Some(actual_case)) => {
                diff(&label, expected_case, actual_case, &mut diffs);
            }
            (Some(expected_case), None) => diffs.push(format!(
                "{label}: missing from the judge; expected {}",
                show(expected_case)
            )),
            (None, Some(actual_case)) => diffs.push(format!(
                "{label}: judge has {}, the hand-written verdict does not",
                show(actual_case)
            )),
            (None, None) => {}
        }
        if diffs.len() == before {
            let verdict = actual_case
                .and_then(|case| case["verdict"].as_str())
                .unwrap_or("?");
            lines.push(format!("{label}: agree (verdict {verdict})"));
        } else {
            lines.push(format!("{label}: disagree"));
        }
    }

    let before = diffs.len();
    diff(
        "counts",
        &expected["counts"],
        &counts_json(&evaluation.cases),
        &mut diffs,
    );
    if diffs.len() == before {
        lines.push("counts: agree".to_string());
    } else {
        lines.push("counts: disagree".to_string());
    }

    let report = lines.join("\n");
    println!("{report}");
    assert!(
        diffs.is_empty(),
        "judge disagrees with the hand-written verdict:\n{report}\n{}",
        diffs.join("\n")
    );
}
