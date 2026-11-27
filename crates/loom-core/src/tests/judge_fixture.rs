//! The §4.4 contract fixture: the generator reproduces the committed files,
//! and `fixtures/judge/SHA256SUMS` covers every one of them.
//!
//! The recording is Vector ASC (`brake.asc`). loom-core already reads that
//! format (`scan/asc.rs`), and §5 names this file. Times are absolute seconds
//! with six decimals, which that reader converts with `seconds_to_us`.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::judge::{self, Kind, Mode, Value};
use crate::scan::{LogFormat, RecKind, Scanner};
use crate::session::Session;

const END_US: u64 = 10_000_000;
const STEP_US: u64 = 10_000;
/// No `Lamps` frame from 4990 ms through 5200 ms. The last one before that is 4980 ms.
const GAP_LO_US: u64 = 4_990_000;
const GAP_HI_US: u64 = 5_200_000;

const NAMES: [&str; 5] = [
    "SHA256SUMS",
    "brake.asc",
    "brake.dbc",
    "expected-verdict.json",
    "request.json",
];

struct Frame {
    t_us: u64,
    id: u32,
    extended: bool,
    channel: u8,
    dlc: u8,
    byte0: u8,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn judge_dir() -> PathBuf {
    repo_root().join("fixtures/judge")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn snapshot(dir: &Path) -> Vec<Vec<u8>> {
    NAMES.iter().map(|name| read(&dir.join(name))).collect()
}

fn assert_unchanged(dir: &Path, before: &[Vec<u8>]) {
    let after = snapshot(dir);
    for (name, (left, right)) in NAMES.iter().zip(before.iter().zip(after.iter())) {
        assert!(
            left == right,
            "{name} changed when the generator ran ({} bytes, then {})",
            left.len(),
            right.len()
        );
    }
}

/// A directory this test owns. The generator writes here and nowhere under
/// `fixtures/judge`.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("loom-judge-fixture-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap_or_else(|err| panic!("temp dir: {err}"));
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Run `scripts/gen_judge_fixture.py`.
///
/// On Windows `python3` is often the Store execution alias: it can be spawned,
/// then exits 9009. Try `python3`, then `python`, then `py -3`, and move on
/// only when that command cannot be spawned or exits 9009. Any other failure
/// is reported as the generator's own failure.
fn run_generator(script: &Path, out: &Path) {
    let candidates: &[(&str, &[&str])] = &[("python3", &[]), ("python", &[]), ("py", &["-3"])];
    for (index, &(program, prefix)) in candidates.iter().enumerate() {
        let last = index + 1 == candidates.len();
        let mut command = Command::new(program);
        command.args(prefix).arg(script).arg("--out").arg(out);
        let output = match command.output() {
            Ok(output) => output,
            Err(err) if last => panic!("could not run {}: {err}", script.display()),
            Err(_) => continue,
        };
        if output.status.code() == Some(9009) && !last {
            continue;
        }
        assert!(
            output.status.success(),
            "generator exited {}\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    panic!("could not run {}", script.display());
}

fn generated_matches_committed(generated: &Path, committed: &Path) {
    let mut names: Vec<String> = std::fs::read_dir(generated)
        .unwrap_or_else(|err| panic!("read {}: {err}", generated.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|err| panic!("read {}: {err}", generated.display()))
                .file_name()
                .into_string()
                .unwrap_or_else(|name| panic!("file name {name:?} is not utf-8"))
        })
        .collect();
    names.sort();
    let expected: Vec<String> = NAMES.iter().map(|name| (*name).to_string()).collect();
    assert_eq!(
        names, expected,
        "the generator wrote a different set of files than the committed fixture"
    );
    for name in &names {
        let left = read(&generated.join(name));
        let right = read(&committed.join(name));
        assert!(
            left == right,
            "{name} from the generator differs from the committed fixture ({} bytes, committed {})",
            left.len(),
            right.len()
        );
    }
}

/// Pedal holds the value set at the last §4.4 edge at or before `t_us`.
fn pedal(t_us: u64) -> u8 {
    const EDGES: [(u64, u8); 8] = [
        (0, 0),
        (1_000_000, 1),
        (2_000_000, 0),
        (3_000_000, 1),
        (4_000_000, 0),
        (5_000_000, 1),
        (6_000_000, 0),
        (9_990_000, 1),
    ];
    let mut value = 0;
    for (at, next) in EDGES {
        if t_us >= at {
            value = next;
        }
    }
    value
}

/// Lamp is 0, then 1 at 1040 ms, then 0 at 2030 ms for the rest of the log.
fn lamp(t_us: u64) -> u8 {
    if t_us >= 2_030_000 {
        0
    } else if t_us >= 1_040_000 {
        1
    } else {
        0
    }
}

fn lamps_absent(t_us: u64) -> bool {
    (GAP_LO_US..=GAP_HI_US).contains(&t_us)
}

fn scheduled_frames() -> u64 {
    let mut count = 0;
    let mut t_us = 0;
    while t_us <= END_US {
        count += 1;
        if !lamps_absent(t_us) {
            count += 1;
        }
        t_us += STEP_US;
    }
    count
}

fn scan_asc(path: &Path) -> Vec<Frame> {
    let mut file = File::open(path).unwrap_or_else(|err| panic!("open {}: {err}", path.display()));
    let mut scanner = Scanner::open(&mut file, LogFormat::Asc).expect("ASC reader");
    let mut frames = Vec::new();
    while let Some(rec) = scanner.next_rec().expect("scan") {
        match rec.kind {
            RecKind::Frame {
                id,
                extended,
                channel,
                dlc,
                data,
            } => frames.push(Frame {
                t_us: rec.t_us,
                id,
                extended,
                channel,
                dlc,
                byte0: data[0],
            }),
            RecKind::Event { label } => panic!("unexpected event at {} µs: {label}", rec.t_us),
            RecKind::Sample { name, .. } => panic!("unexpected sample at {} µs: {name}", rec.t_us),
        }
    }
    assert_eq!(scanner.skipped(), 0, "skipped lines: {:?}", scanner.notes());
    assert!(
        scanner.notes().is_empty(),
        "reader notes: {:?}",
        scanner.notes()
    );
    frames
}

fn held(session: &Session, t_us: u64, name: &str) -> f64 {
    session
        .values_at(t_us)
        .unwrap_or_else(|err| panic!("values at {t_us}: {err}"))
        .into_iter()
        .find(|value| value.name == name)
        .unwrap_or_else(|| panic!("no held {name} at {t_us}"))
        .value
}

#[test]
fn judge_contract_fixture_matches_its_generator_and_sums() {
    let root = repo_root();
    let script = root.join("scripts/gen_judge_fixture.py");
    assert!(
        script.is_file(),
        "missing generator {} — SL-16 writes the §4.4 fixture from this script",
        script.display()
    );
    let dir = judge_dir();
    for name in NAMES {
        let path = dir.join(name);
        assert!(
            path.is_file(),
            "missing {} — the contract fixture is not committed yet",
            path.display()
        );
    }

    let before = snapshot(&dir);
    // The hand-written verdict is not produced by the generator. Copy it into
    // the temp directory so SHA256SUMS can cover it. Nothing is written back
    // to fixtures/judge.
    let temp = TempDir::new();
    std::fs::copy(
        dir.join("expected-verdict.json"),
        temp.path.join("expected-verdict.json"),
    )
    .unwrap_or_else(|err| panic!("copy expected-verdict.json: {err}"));
    run_generator(&script, &temp.path);
    generated_matches_committed(&temp.path, &dir);
    run_generator(&script, &temp.path);
    generated_matches_committed(&temp.path, &dir);
    assert_unchanged(&dir, &before);

    let listed: Vec<String> = String::from_utf8(read(&dir.join("SHA256SUMS")))
        .expect("SHA256SUMS is utf-8")
        .lines()
        .map(|line| {
            let (hash, name) = line
                .split_once("  ")
                .unwrap_or_else(|| panic!("SHA256SUMS line is not `hash  name`: {line}"));
            assert_eq!(hash.len(), 64, "{line}");
            assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()), "{line}");
            let file = read(&dir.join(name));
            assert_eq!(sha256(&file), hash, "{name}");
            name.to_string()
        })
        .collect();
    assert_eq!(
        listed,
        vec![
            "brake.asc".to_string(),
            "brake.dbc".to_string(),
            "expected-verdict.json".to_string(),
            "request.json".to_string(),
        ],
        "SHA256SUMS must cover every fixture file"
    );

    let asc = dir.join("brake.asc");
    let dbc_path = dir.join("brake.dbc");
    let frames = scan_asc(&asc);
    let expect_frames = scheduled_frames();
    assert_eq!(frames.len() as u64, expect_frames);
    let mut seen_brake = 0u64;
    let mut seen_lamps = 0u64;
    for frame in &frames {
        assert!(!frame.extended);
        assert_eq!(frame.channel, 1, "ASC channel at {}", frame.t_us);
        assert_eq!(frame.dlc, 8);
        assert_eq!(frame.t_us % STEP_US, 0);
        assert!(frame.t_us <= END_US);
        match frame.id {
            0x100 => {
                assert_eq!(
                    frame.byte0,
                    pedal(frame.t_us),
                    "BrakePedal at {}",
                    frame.t_us
                );
                seen_brake += 1;
            }
            0x200 => {
                assert!(
                    !lamps_absent(frame.t_us),
                    "Lamps frame inside the gap at {}",
                    frame.t_us
                );
                assert_eq!(frame.byte0, lamp(frame.t_us), "BrakeLamp at {}", frame.t_us);
                seen_lamps += 1;
            }
            other => panic!("unexpected id {other:#x} at {}", frame.t_us),
        }
    }
    let mut t_us = 0;
    let mut brake_at = std::collections::HashSet::new();
    let mut lamps_at = std::collections::HashSet::new();
    for frame in &frames {
        if frame.id == 0x100 {
            brake_at.insert(frame.t_us);
        } else {
            lamps_at.insert(frame.t_us);
        }
    }
    while t_us <= END_US {
        assert!(brake_at.contains(&t_us), "missing BrakeCmd at {t_us}");
        if lamps_absent(t_us) {
            assert!(!lamps_at.contains(&t_us), "Lamps present at {t_us}");
        } else {
            assert!(lamps_at.contains(&t_us), "missing Lamps at {t_us}");
        }
        t_us += STEP_US;
    }
    assert!(!lamps_at.contains(&GAP_LO_US));
    assert!(!lamps_at.contains(&GAP_HI_US));
    assert!(lamps_at.contains(&(GAP_LO_US - STEP_US)));
    assert!(lamps_at.contains(&(GAP_HI_US + STEP_US)));
    assert_eq!(seen_brake + seen_lamps, expect_frames);

    let dbc_text = String::from_utf8(read(&dbc_path)).expect("dbc utf-8");
    let dbc = crate::dbc::parse(&dbc_text).expect("brake.dbc");
    assert!(dbc.warnings.is_empty(), "{:?}", dbc.warnings);
    let brake = dbc
        .messages
        .iter()
        .find(|message| message.name == "BrakeCmd")
        .unwrap();
    let lamps = dbc
        .messages
        .iter()
        .find(|message| message.name == "Lamps")
        .unwrap();
    assert_eq!(brake.id, 0x100);
    assert_eq!(lamps.id, 0x200);
    assert_eq!(brake.cycle_us, Some(STEP_US));
    assert_eq!(lamps.cycle_us, Some(STEP_US));
    assert!(dbc
        .signals
        .iter()
        .any(|signal| signal.defined_name == "BrakePedal" && signal.message_name == "BrakeCmd"));
    assert!(dbc
        .signals
        .iter()
        .any(|signal| signal.defined_name == "BrakeLamp" && signal.message_name == "Lamps"));

    let request_bytes = read(&dir.join("request.json"));
    let request_text = String::from_utf8(request_bytes.clone()).expect("request utf-8");
    let request = judge::check(&request_text, &dbc)
        .unwrap_or_else(|refusal| panic!("request refused: {}", refusal.problems.join("; ")));
    assert_eq!(request.format, "loom-judge/1");
    assert_eq!(request.spec_id, "default@REQ-BRK-001");
    assert_eq!(request.cases.len(), 4);
    assert_eq!(request.cases[0].case_id, "TC-1");
    assert_eq!(request.cases[0].kind, Kind::Positive);
    assert_eq!(request.cases[0].verifies, vec!["REQ-BRK-001".to_string()]);
    assert_eq!(request.cases[0].checks.len(), 1);
    let check = &request.cases[0].checks[0];
    assert_eq!(check.id, "c1");
    assert!(check.given.is_empty());
    assert_eq!(check.mode, Mode::Eventually);
    assert_eq!(check.within_ms, [0, 100]);
    assert_eq!(check.when.signal.message, "BrakeCmd");
    assert_eq!(check.when.signal.signal, "BrakePedal");
    assert_eq!(check.when.signal.channel, None);
    assert_eq!(check.when.value, Value::Number(1.0));
    assert_eq!(check.expect.signal.message, "Lamps");
    assert_eq!(check.expect.signal.signal, "BrakeLamp");
    assert_eq!(check.expect.value, Value::Number(1.0));

    let negative = &request.cases[1];
    assert_eq!(negative.case_id, "TC-2");
    assert_eq!(negative.kind, Kind::Negative);
    assert_eq!(negative.checks.len(), 1);
    assert_eq!(negative.checks[0].id, "c1");
    assert_eq!(negative.checks[0].mode, Mode::Never);
    assert_eq!(negative.checks[0].within_ms, [0, 100]);
    assert_eq!(negative.checks[0].when.value, Value::Number(1.0));
    assert_eq!(negative.checks[0].expect.signal.signal, "BrakeLamp");
    assert_eq!(negative.checks[0].expect.value, Value::Number(1.0));

    let absent = &request.cases[2];
    assert_eq!(absent.case_id, "TC-3");
    assert_eq!(absent.kind, Kind::Positive);
    assert_eq!(absent.checks.len(), 1);
    assert_eq!(absent.checks[0].mode, Mode::Eventually);
    assert_eq!(absent.checks[0].within_ms, [0, 100]);
    assert_eq!(absent.checks[0].when.signal.signal, "BrakePedal");
    assert_eq!(absent.checks[0].when.value, Value::Number(7.0));

    let empty = &request.cases[3];
    assert_eq!(empty.case_id, "TC-4");
    assert_eq!(empty.kind, Kind::Positive);
    assert!(empty.checks.is_empty());

    let mut session = Session::new();
    let summary = session.open_path(&asc).expect("open brake.asc");
    assert_eq!(summary.format, "Vector ASC");
    assert_eq!(summary.frame_count, expect_frames);
    assert_eq!(summary.t_start_us, 0);
    assert_eq!(summary.t_end_us, END_US);
    assert_eq!(summary.skipped_records, 0);
    assert!(!summary.events_truncated);
    session.open_map_path(&dbc_path).expect("open brake.dbc");
    let samples = [
        (0, 0.0, 0.0),
        (1_000_000, 1.0, 0.0),
        (1_040_000, 1.0, 1.0),
        (2_000_000, 0.0, 1.0),
        (2_030_000, 0.0, 0.0),
        (3_000_000, 1.0, 0.0),
        (3_100_000, 1.0, 0.0),
        (4_980_000, 0.0, 0.0),
        (5_000_000, 1.0, 0.0),
        (5_200_000, 1.0, 0.0),
        (5_210_000, 1.0, 0.0),
        (6_000_000, 0.0, 0.0),
        (9_980_000, 0.0, 0.0),
        (9_990_000, 1.0, 0.0),
        (END_US, 1.0, 0.0),
    ];
    for (t_us, pedal_value, lamp_value) in samples {
        assert_eq!(
            held(&session, t_us, "BrakePedal"),
            pedal_value,
            "pedal at {t_us}"
        );
        assert_eq!(
            held(&session, t_us, "BrakeLamp"),
            lamp_value,
            "lamp at {t_us}"
        );
    }
    let indexed = session.summary().expect("summary");
    let timeout = indexed
        .events
        .iter()
        .find(|event| event.label == "Timeout Lamps")
        .unwrap_or_else(|| panic!("no Timeout Lamps in {:?}", indexed.events));
    assert_eq!(
        timeout.t_us, 5_210_000,
        "the existing timeout check fires when the late frame arrives"
    );

    let verdict: serde_json::Value =
        serde_json::from_slice(&read(&dir.join("expected-verdict.json"))).expect("verdict json");
    assert_eq!(verdict["format"], "loom-verdict/1");
    assert_eq!(
        verdict["tool"],
        concat!("loom-judge ", env!("CARGO_PKG_VERSION")),
        "regenerate expected-verdict.json and SHA256SUMS on a version bump"
    );
    assert_eq!(verdict["request_sha256"], sha256(&request_bytes));
    assert_eq!(verdict["log"]["file"], "brake.asc");
    assert_eq!(verdict["log"]["sha256"], sha256(&read(&asc)));
    assert_eq!(verdict["log"]["t_start_us"], 0);
    assert_eq!(verdict["log"]["t_end_us"], END_US);
    assert_eq!(verdict["log"]["frames"], expect_frames);
    assert_eq!(verdict["log"]["skipped"], 0);
    assert_eq!(verdict["log"]["events_truncated"], false);
    assert_eq!(verdict["dbc"][0]["file"], "brake.dbc");
    assert_eq!(verdict["dbc"][0]["sha256"], sha256(&read(&dbc_path)));
    assert_eq!(verdict["dbc"][0]["channel"], 0);
    assert_eq!(verdict["cases"].as_array().map(Vec::len), Some(4));
    assert_eq!(verdict["cases"][0]["case_id"], "TC-1");
    assert_eq!(verdict["cases"][0]["verdict"], "fail");
    let check_verdict = &verdict["cases"][0]["checks"][0];
    assert_eq!(check_verdict["id"], "c1");
    assert_eq!(check_verdict["verdict"], "fail");
    assert_eq!(check_verdict["anchors"], 4);
    assert_eq!(check_verdict["skipped"], 0);
    assert_eq!(check_verdict["pass"], 1);
    assert_eq!(check_verdict["fail"], 1);
    assert_eq!(check_verdict["inconclusive"], 2);
    assert!(check_verdict.get("occurrences_truncated").is_none());
    assert_eq!(
        check_verdict["occurrences"],
        serde_json::json!([
            {"anchor_us": 1_000_000, "verdict": "pass", "t_us": 1_040_000, "value": 1},
            {"anchor_us": 3_000_000, "verdict": "fail", "t_us": 3_100_000, "value": 0},
            {"anchor_us": 5_000_000, "verdict": "inconclusive", "reason": "integrity",
             "event": {"t_us": 5_005_000, "kind": "timeout"}},
            {"anchor_us": 9_990_000, "verdict": "inconclusive", "reason": "window_past_end"}
        ])
    );
    assert_eq!(
        verdict["counts"],
        serde_json::json!({"cases": 4, "pass": 0, "fail": 2, "inconclusive": 0, "not_exercised": 1, "not_judgeable": 1})
    );
}
