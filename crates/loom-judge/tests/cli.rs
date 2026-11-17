//! The loom-judge binary on `fixtures/judge/`, and the comparison CI runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_loom-judge")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(name: &str) -> PathBuf {
    repo_root().join("fixtures/judge").join(name)
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("loom-judge-{label}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn judge(args: &[&str]) -> std::process::Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("run loom-judge")
}

/// The comparison the rust CI job runs after `loom-judge` writes its verdict.
fn cmp_verdicts(actual: &Path, expected: &Path) -> std::process::ExitStatus {
    Command::new("cmp")
        .arg(actual)
        .arg(expected)
        .output()
        .expect("cmp")
        .status
}

fn sha256_file(path: &Path) -> String {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    assert!(
        output.status.success(),
        "sha256sum failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("sha256sum output")
        .split_whitespace()
        .next()
        .expect("sha256sum hash")
        .to_string()
}

fn judge_fixture(out: &Path) -> std::process::Output {
    let log = fixture("brake.asc");
    let dbc = fixture("brake.dbc");
    let request = fixture("request.json");
    judge(&[
        "--log",
        log.to_str().unwrap(),
        "--dbc",
        dbc.to_str().unwrap(),
        "--request",
        request.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ])
}

#[test]
fn the_contract_fixture_matches_expected_verdict_byte_for_byte() {
    let temp = TempDir::new("contract");
    let out = temp.path.join("verdict.json");
    let output = judge_fixture(&out);
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = fs::read(&out).expect("read verdict");
    let expected = fs::read(fixture("expected-verdict.json")).expect("read expected");
    if actual != expected {
        let at = actual
            .iter()
            .zip(expected.iter())
            .position(|(left, right)| left != right);
        panic!(
            "verdict differs at byte {at:?}; judge wrote {} bytes, expected {}",
            actual.len(),
            expected.len()
        );
    }
    assert!(
        cmp_verdicts(&out, &fixture("expected-verdict.json")).success(),
        "cmp must accept the judge output against the committed expected verdict"
    );

    let verdict: serde_json::Value = serde_json::from_slice(&actual).unwrap();
    let log_path = fixture("brake.asc");
    let dbc_path = fixture("brake.dbc");
    let request_path = fixture("request.json");
    assert_eq!(verdict["request_sha256"], sha256_file(&request_path));
    assert_eq!(verdict["log"]["sha256"], sha256_file(&log_path));
    assert_eq!(verdict["dbc"][0]["sha256"], sha256_file(&dbc_path));
    assert_eq!(verdict["log"]["file"], "brake.asc");
    assert_eq!(verdict["dbc"][0]["file"], "brake.dbc");
    assert_eq!(verdict["dbc"][0]["channel"], 0);
    let text = String::from_utf8(actual).unwrap();
    assert!(
        !text.contains(log_path.to_str().unwrap()),
        "the verdict must not contain the log path"
    );
    assert!(
        !text.contains(dbc_path.to_str().unwrap()),
        "the verdict must not contain the DBC path"
    );
}

#[test]
fn the_summary_line_prints_the_counts() {
    let temp = TempDir::new("summary");
    let out = temp.path.join("verdict.json");
    let output = judge_fixture(&out);
    assert!(output.status.success());
    let verdict: serde_json::Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    let counts = &verdict["counts"];
    let line = format!(
        "cases={} pass={} fail={} inconclusive={} not_exercised={} not_judgeable={}\n",
        counts["cases"],
        counts["pass"],
        counts["fail"],
        counts["inconclusive"],
        counts["not_exercised"],
        counts["not_judgeable"]
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap(), line);
    assert_eq!(
        line,
        "cases=4 pass=0 fail=2 inconclusive=0 not_exercised=1 not_judgeable=1\n"
    );
}

#[test]
fn a_refused_request_exits_2_and_a_missing_file_exits_1() {
    let temp = TempDir::new("exit");
    let dbc = fixture("brake.dbc");
    let request = temp.path.join("request.json");
    fs::write(
        &request,
        r#"{
  "format": "loom-judge/1",
  "spec_id": "x",
  "cases": [{
    "case_id": "TC-1",
    "verifies": ["REQ"],
    "kind": "positive",
    "checks": [{
      "id": "c1",
      "when": {"signal": {"message": "Nope", "signal": "Missing"}, "op": "==", "value": 1},
      "expect": {"signal": {"message": "Lamps", "signal": "BrakeLamp"}, "op": "==", "value": 1},
      "mode": "eventually",
      "within_ms": [0, 100]
    }]
  }]
}"#,
    )
    .unwrap();
    let out = temp.path.join("verdict.json");
    let refused = judge(&[
        "--log",
        fixture("brake.asc").to_str().unwrap(),
        "--dbc",
        dbc.to_str().unwrap(),
        "--request",
        request.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(refused.status.code(), Some(2));
    assert!(!out.exists(), "a refusal writes no verdict file");
    let stderr = String::from_utf8(refused.stderr).unwrap();
    assert!(
        stderr.contains("unknown signal pair (Nope, Missing)"),
        "{stderr}"
    );
    assert!(refused.stdout.is_empty());

    // The request is checked before the recording is read.
    let refused_unread = judge(&[
        "--log",
        temp.path.join("no-such-log.asc").to_str().unwrap(),
        "--dbc",
        dbc.to_str().unwrap(),
        "--request",
        request.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(refused_unread.status.code(), Some(2));
    assert!(!out.exists());

    let missing = judge(&[
        "--log",
        temp.path.join("no-such-log.asc").to_str().unwrap(),
        "--dbc",
        dbc.to_str().unwrap(),
        "--request",
        fixture("request.json").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(!out.exists());
    assert!(String::from_utf8(missing.stderr)
        .unwrap()
        .contains("could not read"));
}

#[test]
fn dbc_at_channel_is_recorded_and_the_hash_is_of_that_file() {
    let temp = TempDir::new("channel");
    let dbc = temp.path.join("brake.dbc");
    let mut bytes = fs::read(fixture("brake.dbc")).unwrap();
    bytes.extend(b"\n// hash probe\n");
    fs::write(&dbc, &bytes).unwrap();
    let out = temp.path.join("verdict.json");
    let output = judge(&[
        "--log",
        fixture("brake.asc").to_str().unwrap(),
        "--dbc",
        &format!("{}@1", dbc.to_str().unwrap()),
        "--request",
        fixture("request.json").to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict: serde_json::Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
    assert_eq!(verdict["dbc"][0]["file"], "brake.dbc");
    assert_eq!(verdict["dbc"][0]["channel"], 1);
    assert_eq!(verdict["dbc"][0]["sha256"], sha256_file(&dbc));
    assert_ne!(
        verdict["dbc"][0]["sha256"],
        sha256_file(&fixture("brake.dbc"))
    );
    assert_eq!(verdict["log"]["file"], "brake.asc");
    assert_eq!(
        verdict["request_sha256"],
        sha256_file(&fixture("request.json"))
    );
    let text = String::from_utf8(fs::read(&out).unwrap()).unwrap();
    assert!(!text.contains(dbc.to_str().unwrap()));
}

#[test]
fn the_ci_comparison_fails_when_the_expected_file_differs_by_one_byte() {
    let temp = TempDir::new("cmp");
    let out = temp.path.join("verdict.json");
    let output = judge_fixture(&out);
    assert!(output.status.success());
    let expected = fixture("expected-verdict.json");
    assert!(cmp_verdicts(&out, &expected).success());

    let mutated = temp.path.join("expected-verdict.json");
    let mut bytes = fs::read(&expected).unwrap();
    let index = bytes.len() / 2;
    bytes[index] ^= 0xff;
    fs::write(&mutated, &bytes).unwrap();
    let status = cmp_verdicts(&out, &mutated);
    assert!(
        !status.success(),
        "cmp must fail when the expected verdict differs by one byte"
    );
}
