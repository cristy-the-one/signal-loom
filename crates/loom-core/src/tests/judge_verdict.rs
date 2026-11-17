//! SL-14: the `loom-verdict/1` writer.
//!
//! The committed `fixtures/judge/expected-verdict.json` is the byte contract.
//! A mismatch prints the differing lines. The other tests cover the two rules
//! that file cannot show: the 50-occurrence cap, and counts that stay complete.

use std::path::PathBuf;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::index::IndexedLog;
use crate::judge::{
    self, CaseOutcome, CheckOutcome, Evaluation, Evidence, Occurrence, Verdict, VerdictDbc,
    VerdictLog,
};
use crate::map::TimeoutFactor;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fixture_verdict() -> String {
    let dir = repo_root().join("fixtures/judge");
    let dbc_bytes = std::fs::read(dir.join("brake.dbc")).expect("read brake.dbc");
    let dbc_text = String::from_utf8(dbc_bytes.clone()).expect("brake.dbc is utf-8");
    let dbc = crate::dbc::parse(&dbc_text).expect("brake.dbc");
    let asc_bytes = std::fs::read(dir.join("brake.asc")).expect("read brake.asc");
    let log = IndexedLog::open_path(&dir.join("brake.asc"), Some(&dbc)).expect("open brake.asc");
    let request_bytes = std::fs::read(dir.join("request.json")).expect("read request.json");
    let request_text = String::from_utf8(request_bytes.clone()).expect("request.json is utf-8");
    let request = judge::check(&request_text, &dbc)
        .unwrap_or_else(|refusal| panic!("request refused: {}", refusal.problems.join("; ")));
    let evaluation = judge::evaluate(&log, &dbc, TimeoutFactor::default(), &request)
        .unwrap_or_else(|err| panic!("judge::evaluate failed: {err}"));
    judge::render_verdict(
        &evaluation,
        &sha256(&request_bytes),
        &VerdictLog {
            file: "brake.asc".to_string(),
            sha256: sha256(&asc_bytes),
            t_start_us: log.t_start_us(),
            t_end_us: log.t_end_us(),
            frames: log.frame_count(),
            skipped: log.skipped(),
            events_truncated: log.events_truncated(),
        },
        &[VerdictDbc {
            file: "brake.dbc".to_string(),
            sha256: sha256(&dbc_bytes),
            channel: 0,
        }],
    )
}

fn explain_mismatch(actual: &str, expected: &str) -> String {
    let mut message = format!(
        "writer produced {} bytes; expected {} bytes\n",
        actual.len(),
        expected.len()
    );
    let actual_at = actual
        .bytes()
        .zip(expected.bytes())
        .position(|(a, b)| a != b);
    let shared = actual.len().min(expected.len());
    match actual_at {
        Some(index) => message.push_str(&format!("first differing byte is {index}\n")),
        None if actual.len() != expected.len() => message.push_str(&format!(
            "identical through byte {shared}; the longer text continues\n"
        )),
        None => {}
    }
    let actual_lines: Vec<&str> = actual.split('\n').collect();
    let expected_lines: Vec<&str> = expected.split('\n').collect();
    let mut differing = Vec::new();
    for index in 0..actual_lines.len().max(expected_lines.len()) {
        if actual_lines.get(index) != expected_lines.get(index) {
            differing.push(index);
        }
    }
    message.push_str(&format!("{} line(s) differ\n", differing.len()));
    for index in differing.iter().take(40) {
        let expected_line = expected_lines
            .get(*index)
            .copied()
            .unwrap_or("<missing line>");
        let actual_line = actual_lines
            .get(*index)
            .copied()
            .unwrap_or("<missing line>");
        message.push_str(&format!(
            "line {}:\n  expected: {expected_line:?}\n  writer:   {actual_line:?}\n",
            index + 1
        ));
    }
    if differing.len() > 40 {
        message.push_str(&format!(
            "... {} more differing lines\n",
            differing.len() - 40
        ));
    }
    message
}

fn bare_log() -> VerdictLog {
    VerdictLog {
        file: "log.asc".to_string(),
        sha256: "abc".to_string(),
        t_start_us: 0,
        t_end_us: 1,
        frames: 1,
        skipped: 0,
        events_truncated: false,
    }
}

fn occurrence(anchor_us: u64, verdict: Verdict) -> Occurrence {
    Occurrence {
        anchor_us,
        verdict,
        evidence: Evidence::Sample {
            t_us: anchor_us + 1_000,
            value: 1.0,
        },
    }
}

fn one_check(check: CheckOutcome) -> Evaluation {
    Evaluation {
        integrity_complete: true,
        cases: vec![CaseOutcome {
            case_id: "TC-1".to_string(),
            verdict: check.verdict,
            checks: vec![check],
        }],
    }
}

#[test]
fn the_verdict_file_matches_expected_verdict_json_byte_for_byte() {
    let actual = fixture_verdict();
    let expected =
        std::fs::read_to_string(repo_root().join("fixtures/judge/expected-verdict.json"))
            .expect("read expected-verdict.json");
    assert!(
        actual == expected,
        "{}",
        explain_mismatch(&actual, &expected)
    );
}

#[test]
fn two_runs_write_identical_bytes() {
    let first = fixture_verdict();
    let second = fixture_verdict();
    assert!(first == second, "{}", explain_mismatch(&second, &first));
}

#[test]
fn sixty_anchors_keep_fifty_occurrences_and_full_counts() {
    // 30 pass, then 20 fail, then 6 inconclusive. Four anchors were skipped
    // and are counted but not listed. 30 + 20 + 6 + 4 = 60 anchors.
    let mut occurrences = Vec::new();
    for anchor in 0..30 {
        occurrences.push(occurrence(anchor * 10_000, Verdict::Pass));
    }
    for anchor in 30..50 {
        occurrences.push(occurrence(anchor * 10_000, Verdict::Fail));
    }
    for anchor in 50..56 {
        occurrences.push(occurrence(anchor * 10_000, Verdict::Inconclusive));
    }
    assert_eq!(occurrences.len(), 56);
    let evaluation = one_check(CheckOutcome {
        id: "c1".to_string(),
        verdict: Verdict::Fail,
        anchors: 60,
        skipped: 4,
        pass: 30,
        fail: 20,
        inconclusive: 6,
        occurrences,
    });
    let text = judge::render_verdict(&evaluation, "request", &bare_log(), &[]);
    let again = judge::render_verdict(&evaluation, "request", &bare_log(), &[]);
    assert_eq!(text, again, "the capped file is deterministic");

    let value: Value = serde_json::from_str(&text).expect("verdict json");
    let check = &value["cases"][0]["checks"][0];
    let listed = check["occurrences"].as_array().expect("occurrences");
    assert_eq!(listed.len(), 50, "the file keeps the first 50 occurrences");
    assert_eq!(check["occurrences_truncated"], true);
    assert_eq!(check["anchors"], 60);
    let covered = check["pass"].as_u64().unwrap()
        + check["fail"].as_u64().unwrap()
        + check["inconclusive"].as_u64().unwrap()
        + check["skipped"].as_u64().unwrap();
    assert_eq!(
        covered, 60,
        "counts still cover every anchor, including the 10 dropped from the list"
    );
    assert_eq!(check["pass"], 30);
    assert_eq!(check["fail"], 20);
    assert_eq!(check["inconclusive"], 6);
    assert_eq!(check["skipped"], 4);
    assert_eq!(listed[0]["anchor_us"], 0);
    assert_eq!(listed[49]["anchor_us"], 490_000);
    assert!(
        listed
            .iter()
            .all(|occurrence| occurrence["anchor_us"].as_u64().unwrap() < 500_000),
        "an occurrence past the cap was written"
    );
    assert!(
        listed
            .iter()
            .all(|occurrence| occurrence["verdict"] != "inconclusive"),
        "the six inconclusive occurrences are past the cap; their count must still be 6"
    );
}

#[test]
fn counts_sum_to_cases() {
    let verdicts = [
        Verdict::Pass,
        Verdict::Fail,
        Verdict::Fail,
        Verdict::Inconclusive,
        Verdict::NotExercised,
        Verdict::NotExercised,
        Verdict::NotJudgeable,
    ];
    let evaluation = Evaluation {
        integrity_complete: true,
        cases: verdicts
            .iter()
            .enumerate()
            .map(|(index, verdict)| CaseOutcome {
                case_id: format!("TC-{index}"),
                verdict: *verdict,
                checks: Vec::new(),
            })
            .collect(),
    };
    let text = judge::render_verdict(&evaluation, "request", &bare_log(), &[]);
    let value: Value = serde_json::from_str(&text).expect("verdict json");
    let counts = &value["counts"];
    let cases = counts["cases"].as_u64().expect("cases");
    let sum = [
        "pass",
        "fail",
        "inconclusive",
        "not_exercised",
        "not_judgeable",
    ]
    .iter()
    .map(|key| {
        counts[*key]
            .as_u64()
            .unwrap_or_else(|| panic!("counts.{key}"))
    })
    .sum::<u64>();
    assert_eq!(cases, verdicts.len() as u64);
    assert_eq!(
        sum, cases,
        "pass + fail + inconclusive + not_exercised + not_judgeable"
    );
    assert_eq!(counts["pass"], 1);
    assert_eq!(counts["fail"], 2);
    assert_eq!(counts["inconclusive"], 1);
    assert_eq!(counts["not_exercised"], 2);
    assert_eq!(counts["not_judgeable"], 1);
}
