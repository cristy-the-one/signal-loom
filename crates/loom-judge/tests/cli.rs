//! The loom-judge binary on `fixtures/judge/`, and the comparison CI runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

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

/// Same check the rust CI job runs with `cmp`, done here with the file bytes.
/// The workflow's own `cmp` step is unchanged.
fn cmp_verdicts(actual: &Path, expected: &Path) -> bool {
    let left = fs::read(actual).unwrap_or_else(|err| panic!("read {}: {err}", actual.display()));
    let right =
        fs::read(expected).unwrap_or_else(|err| panic!("read {}: {err}", expected.display()));
    left == right
}

fn sha256_file(path: &Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
        cmp_verdicts(&out, &fixture("expected-verdict.json")),
        "judge output must match the committed expected verdict byte for byte"
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
    assert!(cmp_verdicts(&out, &expected));

    let mutated = temp.path.join("expected-verdict.json");
    let mut bytes = fs::read(&expected).unwrap();
    let index = bytes.len() / 2;
    bytes[index] ^= 0xff;
    fs::write(&mutated, &bytes).unwrap();
    assert!(
        !cmp_verdicts(&out, &mutated),
        "the verdicts must differ when the expected file changes by one byte"
    );
}

#[test]
fn dump_of_hypercar_lap_matches_the_committed_file_byte_for_byte() {
    assert_dump_matches(
        &repo_root().join("fixtures/hypercar_lap.dbc"),
        &repo_root().join("fixtures/dbc-dump/hypercar_lap.json"),
    );
}

#[test]
fn dump_of_parity_dbc_matches_the_committed_file_byte_for_byte() {
    assert_dump_matches(
        &repo_root().join("fixtures/dbc-dump/parity.dbc"),
        &repo_root().join("fixtures/dbc-dump/parity.json"),
    );
}

fn assert_dump_matches(dbc: &Path, expected_path: &Path) {
    let output = judge(&["--dump-dbc", dbc.to_str().unwrap()]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let expected = fs::read(expected_path).expect("read committed dump");
    if output.stdout != expected {
        let at = output
            .stdout
            .iter()
            .zip(expected.iter())
            .position(|(left, right)| left != right);
        panic!(
            "dump differs at byte {at:?}; command wrote {} bytes, expected {}",
            output.stdout.len(),
            expected.len()
        );
    }
}

#[test]
fn dump_dbc_of_a_file_that_cannot_be_read_exits_1() {
    let missing = judge(&["--dump-dbc", "/tmp/loom-judge-no-such-dbc.dbc"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    let stderr = String::from_utf8(missing.stderr).unwrap();
    assert!(stderr.contains("could not read"), "{stderr}");

    let temp = TempDir::new("dump-bad");
    let bad = temp.path.join("notes.txt");
    fs::write(&bad, "this is not a dbc\n").unwrap();
    let output = judge(&["--dump-dbc", bad.to_str().unwrap()]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("could not read"));
}

#[test]
fn help_and_version_exit_zero_with_clean_stdout() {
    for flag in ["--help", "-h"] {
        let out = judge(&[flag]);
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stderr.is_empty());
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("usage:"));
        assert!(text.contains("--inspect"));
        assert!(text.contains("--check"));
    }
    for flag in ["--version", "-V"] {
        let out = judge(&[flag]);
        assert_eq!(out.status.code(), Some(0));
        assert!(out.stderr.is_empty());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            concat!("loom-judge ", env!("CARGO_PKG_VERSION"), "\n")
        );
    }
}

#[test]
fn check_validates_request_against_dbc_without_a_recording() {
    let dbc = fixture("brake.dbc");
    let request = fixture("request.json");
    let ok = judge(&[
        "--check",
        "--dbc",
        dbc.to_str().unwrap(),
        "--request",
        request.to_str().unwrap(),
    ]);
    assert_eq!(ok.status.code(), Some(0), "{:?}", ok);
    assert!(ok.stderr.is_empty());
    assert_eq!(
        String::from_utf8(ok.stdout).unwrap(),
        "ok spec_id=default@REQ-BRK-001 cases=4 checks=3\n"
    );

    let mut bad_child = Command::new(bin())
        .args(["--check", "--dbc", dbc.to_str().unwrap(), "--request", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    bad_child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"format":"wrong","spec_id":"bad","cases":[]}"#)
        .unwrap();
    let bad = bad_child.wait_with_output().unwrap();
    assert_eq!(bad.status.code(), Some(2));
    assert!(bad.stdout.is_empty());
    assert!(String::from_utf8(bad.stderr)
        .unwrap()
        .contains("request refused"));
}

#[test]
fn stdin_request_and_stdout_verdict_match_expected_byte_for_byte() {
    use std::io::Write;
    let log = fixture("brake.asc");
    let dbc = fixture("brake.dbc");
    let req_bytes = fs::read(fixture("request.json")).unwrap();
    let mut child = Command::new(bin())
        .args([
            "--log",
            log.to_str().unwrap(),
            "--dbc",
            dbc.to_str().unwrap(),
            "--request",
            "-",
            "--out",
            "-",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&req_bytes).unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let expected = fs::read(fixture("expected-verdict.json")).unwrap();
    assert_eq!(out.stdout, expected);
    assert_eq!(
        String::from_utf8(out.stderr).unwrap(),
        "cases=4 pass=0 fail=2 inconclusive=0 not_exercised=1 not_judgeable=1\n"
    );
}

#[test]
fn inspect_stats_samples_and_map_json_work_end_to_end() {
    let log = fixture("brake.asc");
    let dbc = fixture("brake.dbc");
    let inspected = judge(&[
        "--inspect",
        "--log",
        log.to_str().unwrap(),
        "--dbc",
        dbc.to_str().unwrap(),
    ]);
    assert_eq!(inspected.status.code(), Some(0));
    assert!(inspected.stderr.is_empty());
    let doc: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(doc["format"], "loom-inspect/1");
    assert_eq!(doc["sha256"], sha256_file(&log));
    assert!(doc["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|m| m["matched_dbc"] == true));
    assert!(doc["events"]["timeouts"].as_u64().unwrap() >= 1);

    let fast = judge(&["--inspect", "--no-hash", "--log", log.to_str().unwrap()]);
    assert_eq!(fast.status.code(), Some(0));
    let fast_doc: serde_json::Value = serde_json::from_slice(&fast.stdout).unwrap();
    assert!(fast_doc.get("sha256").is_none());

    let cluster_log = repo_root().join("fixtures/cluster_drive.slog");
    let cluster_map = repo_root().join("fixtures/cluster.map.json");
    let dump_map = judge(&["--dump-dbc", cluster_map.to_str().unwrap()]);
    assert_eq!(dump_map.status.code(), Some(0));
    let dump_doc: serde_json::Value = serde_json::from_slice(&dump_map.stdout).unwrap();
    assert_eq!(dump_doc["format"], "loom-dbc-dump/1");

    let stats = judge(&[
        "--stats",
        "VehicleSpeed",
        "--log",
        cluster_log.to_str().unwrap(),
        "--dbc",
        cluster_map.to_str().unwrap(),
    ]);
    assert_eq!(stats.status.code(), Some(0));
    let stats_doc: serde_json::Value = serde_json::from_slice(&stats.stdout).unwrap();
    assert_eq!(stats_doc["format"], "loom-stats/1");
    assert_eq!(stats_doc["signals"][0]["name"], "VehicleSpeed");
    assert!(stats_doc["signals"][0]["stats"]["count"].as_u64().unwrap() > 0);

    let samples = judge(&[
        "--samples",
        "VehicleSpeed",
        "--log",
        cluster_log.to_str().unwrap(),
        "--dbc",
        cluster_map.to_str().unwrap(),
        "--max-points",
        "12",
    ]);
    assert_eq!(samples.status.code(), Some(0));
    let samples_doc: serde_json::Value = serde_json::from_slice(&samples.stdout).unwrap();
    assert_eq!(samples_doc["format"], "loom-samples/1");
    assert_eq!(samples_doc["decimated"], true);
    let pts = samples_doc["series"][0]["points"].as_array().unwrap();
    assert!(!pts.is_empty() && pts.len() <= 16);
}
