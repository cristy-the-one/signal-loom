use crate::index::IndexedLog;
use crate::map::SignalMap;
use crate::project::ProjectFile;
use crate::scan::{encode_slb1, Rec, RecKind};
use crate::session::Session;
use crate::StepDir;
use std::path::PathBuf;
use std::time::Instant;

fn tiny_map() -> SignalMap {
    SignalMap::parse(
        r#"{
            "name": "tiny",
            "version": 1,
            "messages": [{
                "id": "0x1A0",
                "name": "Powertrain",
                "signals": [
                    {"name": "EngineRPM", "startBit": 0, "bitLength": 16, "factor": 0.25, "unit": "rpm"},
                    {"name": "VehicleSpeed", "startBit": 16, "bitLength": 16, "factor": 0.01, "unit": "km/h"},
                    {"name": "CoolantTemp", "startBit": 32, "bitLength": 8, "factor": 1, "offset": -40, "unit": "C"},
                    {"name": "Throttle", "startBit": 40, "bitLength": 8, "factor": 0.4, "unit": "%"}
                ]
            }]
        }"#,
    )
    .unwrap()
}

#[test]
fn decodes_known_powertrain_frame() {
    // rpm 800, speed 50 km/h, coolant 80 C, throttle 40%
    let text = "SLOGv1\nF 0 1A0 800C881378640000\nF 1000 1A0 800C881378640000\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&tiny_map())).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| values.iter().find(|v| v.name == name).unwrap().value;
    assert!((get("EngineRPM") - 800.0).abs() < 1e-6);
    assert!((get("VehicleSpeed") - 50.0).abs() < 1e-6);
    assert!((get("CoolantTemp") - 80.0).abs() < 1e-6);
    assert!((get("Throttle") - 40.0).abs() < 1e-6);
}

#[test]
fn steps_frames_and_reads_events() {
    let text = "\
SLOGv1
E 0 Key on
F 0 1A0 800C000000000000
F 20000 1A0 800C000000000000
E 20000 Pullaway
F 40000 1A0 800C000000000000
";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&tiny_map())).unwrap();
    assert_eq!(log.frame_count(), 3);
    assert_eq!(log.events().len(), 2);
    let next = log.step_frame(0, true).unwrap().unwrap();
    assert_eq!(next.t_us, 20_000);
    assert_eq!(next.ordinal, 1);
    let prev = log.step_frame(40_000, false).unwrap().unwrap();
    assert_eq!(prev.t_us, 20_000);
    assert!(log.step_frame(40_000, true).unwrap().is_none());
    assert!(log.step_frame(0, false).unwrap().is_none());
}

#[test]
fn binary_roundtrip_matches_text() {
    let map = tiny_map();
    let data = hex_payload_bytes("800C881378640000");
    let records = vec![
        Rec {
            offset: 0,
            t_us: 0,
            kind: RecKind::Frame {
                id: 0x1A0,
                dlc: 8,
                data,
            },
        },
        Rec {
            offset: 0,
            t_us: 5_000,
            kind: RecKind::Event {
                label: "Mark".into(),
            },
        },
        Rec {
            offset: 0,
            t_us: 10_000,
            kind: RecKind::Frame {
                id: 0x1A0,
                dlc: 8,
                data,
            },
        },
    ];
    let log = IndexedLog::open_bytes(encode_slb1(&records), Some(&map)).unwrap();
    assert_eq!(log.format().label(), "SLB1");
    assert_eq!(log.frame_count(), 2);
    assert_eq!(log.events()[0].1, "Mark");
    let speed = log
        .values_at(10_000)
        .unwrap()
        .into_iter()
        .find(|v| v.name == "VehicleSpeed")
        .unwrap()
        .value;
    assert!((speed - 50.0).abs() < 1e-6);
}

#[test]
fn can_csv_and_decoded_csv() {
    let csv = "t_us,id,data\n0,0x1A0,800C881378640000\n20000,416,800C881378640000\n";
    let log = IndexedLog::open_bytes(csv.as_bytes().to_vec(), Some(&tiny_map())).unwrap();
    assert_eq!(log.format().label(), "CAN CSV");
    assert_eq!(log.frame_count(), 2);

    let decoded = "\
t_us,signal,value,unit
0,VehicleSpeed,0,km/h
0,EngineRPM,800,rpm
1000,VehicleSpeed,10,km/h
";
    let log = IndexedLog::open_bytes(decoded.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.format().label(), "Decoded CSV");
    let series = log
        .query(&crate::index::QueryWindow {
            t0_us: 0,
            t1_us: 1000,
            signals: vec!["VehicleSpeed".into()],
            max_points: 20,
        })
        .unwrap();
    assert_eq!(series.len(), 1);
    assert!(series[0].points.len() >= 2);
    assert!((series[0].points.last().unwrap().1 - 10.0).abs() < 1e-6);
}

#[test]
fn query_window_stays_bounded_on_a_large_log() {
    let mut text = String::from("SLOGv1\n");
    text.reserve(100_000 * 32);
    for i in 0..100_000u64 {
        let t = i * 1000;
        text.push_str(&format!("F {t} 1A0 800C000000000000\n"));
    }
    let started = Instant::now();
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&tiny_map())).unwrap();
    assert!(
        started.elapsed().as_secs() < 5,
        "indexing 100k frames took {:?}",
        started.elapsed()
    );
    assert_eq!(log.frame_count(), 100_000);
    assert!(log.checkpoint_count() < 1_000);
    assert!(log.checkpoint_count() * 256 >= 100_000 - 256);
    let series = log
        .query(&crate::index::QueryWindow {
            t0_us: 0,
            t1_us: 99_999_000,
            signals: vec!["EngineRPM".into()],
            max_points: 100,
        })
        .unwrap();
    assert!(series[0].points.len() <= 100);
    assert!(series[0].points.iter().all(|p| (p.1 - 800.0).abs() < 1e-6));
}

#[test]
fn rejects_backwards_time_and_duplicate_signals() {
    let text = "SLOGv1\nF 10 1A0 0000\nF 5 1A0 0000\n";
    let err = match IndexedLog::open_bytes(text.as_bytes().to_vec(), None) {
        Err(err) => err,
        Ok(_) => panic!("backwards log should fail"),
    };
    assert!(err.to_string().contains("backwards"), "{err}");

    let err = SignalMap::parse(
        r#"{"name":"x","version":1,"messages":[
            {"id":1,"name":"A","signals":[{"name":"Speed","startBit":0,"bitLength":8}]},
            {"id":2,"name":"B","signals":[{"name":"Speed","startBit":0,"bitLength":8}]}
        ]}"#,
    )
    .unwrap_err();
    assert!(err.to_string().contains("two signals"), "{err}");
}

#[test]
fn big_endian_signal_through_the_map() {
    let map = SignalMap::parse(
        r#"{"name":"be","version":1,"messages":[{"id":"0x10","name":"M","signals":[
            {"name":"Word","startBit":7,"bitLength":16,"endian":"big","factor":1}
        ]}]}"#,
    )
    .unwrap();
    let text = "F 0 10 1234000000000000\nF 1 10 1234000000000000\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let value = log.values_at(0).unwrap()[0].value;
    assert!((value - f64::from(0x1234)).abs() < 1e-6);
}

#[test]
fn sample_drive_has_the_expected_shape() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut session = Session::new();
    session
        .open_path(&root.join("cluster_drive.slog"))
        .expect("sample log");
    // Sibling map is cluster_drive.map.json, which does not exist; load the shipped map.
    session
        .open_map_path(&root.join("cluster.map.json"))
        .unwrap();
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
fn decoded_csv_does_not_pretend_to_use_a_signal_map() {
    let bytes = include_bytes!("../../../fixtures/decoded_snippet.csv");
    let mut session = Session::new();
    session.open_sample().unwrap();
    let summary = session
        .open_bytes("decoded_snippet.csv", bytes.to_vec())
        .unwrap();
    assert_eq!(summary.format, "Decoded CSV");
    assert!(summary.map_label.is_none());
    assert!(summary
        .signals
        .iter()
        .any(|signal| signal.name == "VehicleSpeed"));
    assert!(summary.signals.iter().all(|signal| !signal.from_map));
}

fn hex_payload_bytes(text: &str) -> [u8; 8] {
    let mut data = [0u8; 8];
    for i in 0..8 {
        data[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    data
}
