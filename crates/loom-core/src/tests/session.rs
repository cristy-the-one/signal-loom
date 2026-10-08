use super::support::*;
use crate::index::IndexedLog;
use crate::project::{MathChannel, ProjectFile};
use crate::session::Session;
use crate::StepDir;

#[test]
fn sample_drive_has_the_expected_shape() {
    let root = fixtures_dir();
    let mut session = open_cluster_drive();
    let summary = session.summary().unwrap();
    assert!(summary.frame_count > 2_000, "{}", summary.frame_count);
    assert!(summary.checkpoint_count * 256 >= summary.frame_count - 256);
    assert!(summary.checkpoint_count < summary.frame_count / 32);
    let labels: Vec<_> = summary.events.iter().map(|e| e.label.as_str()).collect();
    for label in [
        "Key on", "Pullaway", "Braking", "Key off", "Into 2nd", "Into 3rd",
    ] {
        assert!(labels.contains(&label), "missing {label} in {labels:?}");
    }
    let at = |t: u64, name: &str| {
        session
            .values_at(t)
            .unwrap()
            .into_iter()
            .find(|v| v.name == name)
            .map(|v| v.value)
            .unwrap_or(f64::NAN)
    };
    assert!(at(0, "VehicleSpeed") < 0.5);
    assert!((at(0, "EngineRPM") - 800.0).abs() < 40.0);
    assert!(at(12_000_000, "VehicleSpeed") > 40.0);
    assert!(at(34_000_000, "BrakePressure") > 10.0);

    let project =
        ProjectFile::parse(&std::fs::read_to_string(root.join("demo.loom")).unwrap()).unwrap();
    assert_eq!(project.bookmarks.len(), 3);
    assert_eq!(
        project.view.plotted,
        ["VehicleSpeed", "EngineRPM", "BrakePressure"]
    );

    let opened = session.load_project_file(&root.join("demo.loom")).unwrap();
    assert_eq!(opened.project.view.playhead_us, 2_000_000);
    assert!(opened
        .summary
        .signals
        .iter()
        .any(|s| s.name == "VehicleSpeed"));
    let stepped = session.step(0, StepDir::Next).unwrap().unwrap();
    assert!(stepped.t_us > 0);
}

#[test]
fn fractional_timestamps_round_to_microseconds() {
    let query: crate::Query = serde_json::from_str(
        r#"{"t0Us":10.6,"t1Us":1000.2,"signals":["VehicleSpeed"],"maxPoints":10}"#,
    )
    .unwrap();
    assert_eq!(query.t0_us, 11);
    assert_eq!(query.t1_us, 1000);
}

#[test]
fn a_new_log_drops_math_and_reports_bus_load() {
    let text = "SLOGv1\nF 0 1A0 0000000000000000\nF 10000 1A0 0000000000000000\nF 20000 1A0 0000000000000000\n";
    let mut session = Session::new();
    session
        .open_bytes("a.slog", text.as_bytes().to_vec())
        .unwrap();
    session
        .set_math(vec![MathChannel {
            name: "Twice".into(),
            unit: String::new(),
            expr: "1+1".into(),
        }])
        .unwrap();
    let summary = session
        .open_bytes("b.slog", text.as_bytes().to_vec())
        .unwrap();
    assert!(summary.signals.iter().all(|signal| signal.name != "Twice"));
    let load = session.bus_load(0, 20_000).unwrap();
    assert_eq!(load.frames, 3);
    assert!(load.rate > 100.0);
    assert!(load.load > 0.0 && load.load < 1.0);
}

#[test]
fn a_cancelled_or_failed_load_keeps_the_deck_that_was_open() {
    let root = fixtures_dir();
    let mut session = open_cluster_drive();
    let before = session.summary().unwrap();
    let names = |summary: &crate::Summary| {
        summary
            .signals
            .iter()
            .map(|signal| signal.name.clone())
            .collect::<Vec<_>>()
    };

    let cancelled = crate::IndexControl::default();
    cancelled.request_cancel();
    let dbc = lap_fixture("hypercar_lap.dbc");
    assert!(session
        .add_map_path_controlled(&dbc, 0, Some(&cancelled))
        .is_err());
    let after_cancel = session.summary().unwrap();
    assert_eq!(names(&after_cancel), names(&before));
    assert_eq!(after_cancel.map_label, before.map_label);

    let added = session.add_map_path(&dbc, 0).unwrap();
    let mut once = open_cluster_drive();
    let expected = once.add_map_path(&dbc, 0).unwrap();
    assert_eq!(
        names(&added),
        names(&expected),
        "the retry added the DBC once"
    );

    let broken = r#"{"format":"signal-loom","version":1,"logPath":"nowhere.slog",
        "signalMapPath":"hypercar_lap.dbc","view":{"playheadUs":0,"spanUs":1000000,"plotted":[]}}"#;
    let mut fresh = open_cluster_drive();
    let kept = fresh.summary().unwrap();
    assert!(fresh.load_project_json(broken, Some(&root)).is_err());
    let after_project = fresh.summary().unwrap();
    assert_eq!(after_project.map_label, kept.map_label);
    assert_eq!(names(&after_project), names(&kept));
}

#[test]
fn cancel_stops_before_the_file_is_committed() {
    let mut text = String::from("SLOGv1\n");
    for i in 0..400u64 {
        text.push_str(&format!("F {} 1A0 0100\n", i * 1000));
    }
    let control = crate::index::IndexControl::default();
    control.request_cancel();
    let log = IndexedLog::open_bytes(text.into_bytes(), None).unwrap();
    assert_eq!(log.frame_count(), 400, "bytes open has no control handle");
    let dir = TempDir::new("cancel");
    let path = dir.join("cancel.slog");
    std::fs::write(&path, {
        let mut body = String::from("SLOGv1\n");
        for i in 0..400u64 {
            body.push_str(&format!("F {} 1A0 0100\n", i * 1000));
        }
        body
    })
    .unwrap();
    let err = match IndexedLog::open_path_controlled(&path, None, Some(&control)) {
        Ok(_) => panic!("cancel should stop the index"),
        Err(err) => err,
    };
    assert_eq!(err.to_string(), "indexing cancelled");
}
