use crate::index::IndexedLog;
use crate::map::SignalMap;
use crate::project::{MathChannel, ProjectFile};
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
            starts_container: false,
            kind: RecKind::Frame {
                id: 0x1A0,
                extended: false,
                channel: 0,
                dlc: 8,
                data,
            },
        },
        Rec {
            offset: 0,
            t_us: 5_000,
            starts_container: false,
            kind: RecKind::Event {
                label: "Mark".into(),
            },
        },
        Rec {
            offset: 0,
            t_us: 10_000,
            starts_container: false,
            kind: RecKind::Frame {
                id: 0x1A0,
                extended: false,
                channel: 0,
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
fn skips_backwards_time_and_rejects_duplicate_signals() {
    // 5 µs back is Tx/Rx interleave: kept at 10. 80 ms back is a broken log: skipped.
    let text = "SLOGv1\nF 10 1A0 0000\nF 5 1A0 0000\nF 20 1A0 0100\nF 100000 1A0 0000\nF 20000 1A0 0000\nF 100010 1A0 0000\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.frame_count(), 5);
    assert_eq!(log.skipped(), 1);
    let warned = |needle: &str| {
        log.warnings()
            .iter()
            .any(|warning| warning.contains(needle))
    };
    assert!(warned("backwards"), "{:?}", log.warnings());
    assert!(warned("out of order"), "{:?}", log.warnings());
    assert!(
        log.step_frame(10, false).unwrap().is_none(),
        "replay keeps the 5 µs frame at 10 too, so nothing comes before 10"
    );

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
fn asc_and_candump_decode_the_same_frame() {
    let asc = "\
date Mon Jan 6 12:00:00.000 2026
base hex  timestamps absolute
internal events logged
0.000000 1 1A0 Rx d 8 80 0C 88 13 78 64 00 00
0.010000 1 1A0 Rx d 8 80 0C 88 13 78 64 00 00
0.015000 1 ErrorFrame
";
    let candump = "\
(1700000000.000000) can0 1A0#800C881378640000
(1700000000.010000) can0 1A0#800C881378640000
(1700000000.015000) can0 ERRORFRAME
";
    let relative = "\
timestamps relative
0.005000 1 100 Rx d 1 AA
0.005000 1 100 Rx d 1 BB
";
    let untimed = "can0 100 [2] 01 02\ncan0 100 [2] 03 04\n";
    for (name, text) in [("tiny.asc", asc), ("candump.log", candump)] {
        let mut session = Session::new();
        session.open_bytes(name, text.as_bytes().to_vec()).unwrap();
        session
            .open_map_json(include_str!("../../../fixtures/cluster.map.json"))
            .unwrap();
        let summary = session.summary().unwrap();
        assert!(
            summary.format == "Vector ASC" || summary.format == "candump",
            "{}",
            summary.format
        );
        let rpm = session
            .values_at(0)
            .unwrap()
            .into_iter()
            .find(|value| value.name == "EngineRPM")
            .unwrap();
        assert!((rpm.value - 800.0).abs() < 1e-6, "{}", rpm.value);
        assert_eq!(summary.frame_count, 2);
        assert!(summary
            .events
            .iter()
            .any(|event| event.label == "Error frame"));
        let later = session.step(0, StepDir::Next).unwrap().unwrap();
        assert_eq!(later.t_us, 10_000);
    }
    let log = IndexedLog::open_bytes(relative.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.format().label(), "Vector ASC");
    let second = log.step_frame(5_000, true).unwrap().unwrap();
    assert_eq!(second.t_us, 10_000, "relative ASC should accumulate");
    let plain = IndexedLog::open_bytes(untimed.as_bytes().to_vec(), None).unwrap();
    assert_eq!(plain.format().label(), "candump");
    assert_eq!(plain.frame_count(), 2);
}

#[test]
fn math_stats_export_and_integrity() {
    let map = SignalMap::parse(
        r#"{"name":"bus","version":1,"messages":[
            {"id":"0x100","name":"ECM_Fast","cycleUs":10000,"signals":[
                {"name":"FastCounter","startBit":0,"bitLength":4,"factor":1},
                {"name":"FastChecksum","startBit":56,"bitLength":8,"factor":1}
            ]},
            {"id":"0x300","name":"ABS_Wheels","signals":[
                {"name":"WheelFL","startBit":0,"bitLength":8,"factor":1,"unit":"km/h"},
                {"name":"WheelFR","startBit":8,"bitLength":8,"factor":1,"unit":"km/h"}
            ]}
        ]}"#,
    )
    .unwrap();
    // Checksum byte 7 is the XOR of the rest. Counter 1, then 4 (a skip) in a
    // good frame, then a frame with a bad checksum: rejected before its counter.
    let frame = |counter: u8| {
        let mut data = [0u8; 8];
        data[0] = counter;
        data[7] = data[..7].iter().fold(0, |acc, byte| acc ^ byte);
        data
    };
    let mut bad = frame(5);
    bad[7] ^= 0xFF;
    let text = format!(
        "SLOGv1\nF 0 100 {}\nF 80000 100 {}\nF 90000 100 {}\n",
        hex(&frame(1)),
        hex(&frame(4)),
        hex(&bad)
    );
    let wheels = "F 0 300 6460\nF 10000 300 6260\n";
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    let labels: Vec<_> = log
        .events()
        .iter()
        .map(|(_, label)| label.as_str())
        .collect();
    assert!(
        labels.iter().any(|label| label.contains("Timeout")),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("Counter")),
        "{labels:?}"
    );
    assert!(
        labels.iter().any(|label| label.contains("Checksum")),
        "{labels:?}"
    );

    let mut session = Session::new();
    session
        .open_bytes("math.slog", wheels.as_bytes().to_vec())
        .unwrap();
    session
        .open_map_json(
            r#"{"name":"bus","version":1,"messages":[{"id":"0x300","name":"ABS","signals":[
            {"name":"WheelFL","startBit":0,"bitLength":8,"factor":1,"unit":"km/h"},
            {"name":"WheelFR","startBit":8,"bitLength":8,"factor":1,"unit":"km/h"}
        ]}]}"#,
        )
        .unwrap();
    session
        .set_math(vec![crate::MathChannel {
            name: "Slip".into(),
            unit: "km/h".into(),
            expr: "WheelFL - WheelFR".into(),
        }])
        .unwrap();
    let series = session
        .query(&crate::Query {
            t0_us: 0,
            t1_us: 20_000,
            signals: vec!["Slip".into()],
            max_points: 10,
            include_compare: false,
        })
        .unwrap();
    assert_eq!(series.len(), 1);
    assert!(
        (series[0].points[0].v - 4.0).abs() < 1e-6,
        "{:?}",
        series[0].points
    );
    let stats = session.stats("WheelFL", 0, 20_000).unwrap();
    assert_eq!(stats.count, 2);
    assert!((stats.min - 98.0).abs() < 1e-6, "{}", stats.min);
    assert!((stats.max - 100.0).abs() < 1e-6, "{}", stats.max);
    assert!((stats.avg - 99.0).abs() < 1e-6, "{}", stats.avg);
    let csv = session.export_csv(&["WheelFL".into()], 0, 20_000).unwrap();
    assert!(csv.starts_with("t_us,WheelFL"));
    let slog = session.export_slog(0, 20_000).unwrap();
    assert!(slog.contains("SLOGv1"));
    assert!(slog.contains("F "));
    session
        .set_triggers(vec![crate::ThresholdTrigger {
            id: "hot".into(),
            signal: "WheelFL".into(),
            op: ">".into(),
            value: 99.0,
        }])
        .unwrap();
    let summary = session.summary().unwrap();
    assert!(summary
        .events
        .iter()
        .any(|event| event.label.contains("Trigger")));
    assert!(summary.signals.iter().any(|signal| signal.name == "Slip"));
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02X}")).collect()
}

#[test]
fn error_frames_are_events() {
    let text = "SLOGv1\nF 0 100 00\nX 1500\nF 3000 100 11\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.event_count(), 1);
    assert_eq!(log.events()[0].1, "Error frame");
}

#[test]
fn hypercar_lap_decodes_like_a_drive() {
    use sha2::{Digest, Sha256};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let lap = std::fs::read(root.join("hypercar_lap.slog")).expect("synthetic lap");
    assert_eq!(
        lap.len(),
        7_699_372,
        "regenerate with scripts/gen_fixture.py"
    );
    let digest = Sha256::digest(&lap);
    let sha = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        sha,
        "807e35542bd5fe4599f25da0991ca8a2ee2a50f301a3910615c194aa5556b6c3"
    );
    let mut session = Session::new();
    session
        .open_path(&root.join("hypercar_lap.slog"))
        .expect("synthetic lap");
    let summary = session.summary().unwrap();
    assert!(summary.t_end_us > 590_000_000, "{}", summary.t_end_us);
    assert!(summary.frame_count > 180_000, "{}", summary.frame_count);
    assert!(
        summary
            .map_label
            .as_deref()
            .unwrap_or("")
            .contains("hypercar_lap"),
        "{:?}",
        summary.map_label
    );
    let labels: Vec<&str> = summary
        .events
        .iter()
        .map(|event| event.label.as_str())
        .collect();
    for needle in [
        "DTC P0301",
        "Bus-off",
        "ECM_Fast missing",
        "ABS checksum",
        "Error frame",
        "ABS",
        "Upshift",
        "Checksum ",
        "Timeout ",
        "Counter ",
    ] {
        assert!(
            labels.iter().any(|label| label.contains(needle)),
            "missing {needle} in {labels:?}"
        );
    }
    let at = |t: u64, name: &str| {
        session
            .values_at(t)
            .unwrap()
            .into_iter()
            .find(|value| value.name == name)
            .map(|value| value.value)
            .unwrap_or(f64::NAN)
    };
    assert!(at(2_000_000, "VehicleSpeed") < 1.0);
    assert!(at(200_000_000, "VehicleSpeed") > 150.0);
    assert!(at(200_000_000, "Gear") >= 5.0);
    assert!(at(200_000_000, "EngineRPM") < 9000.0);
    assert!(at(200_000_000, "BrakePressure") < 0.5);
    assert!(at(299_000_000, "BrakePressure") > 40.0);
    assert!(at(500_000_000, "CoolantTemp") > at(5_000_000, "CoolantTemp") + 20.0);
    assert!(at(299_000_000, "AbsActive") > 0.5);
    assert!(at(560_000_000, "DoorFL") > 0.5);
}

#[test]
fn signals_carry_their_decode_step() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut session = Session::new();
    session
        .open_path(&root.join("hypercar_lap.slog"))
        .expect("synthetic lap");
    let summary = session.summary().unwrap();
    let step = |name: &str| {
        summary
            .signals
            .iter()
            .find(|signal| signal.name == name)
            .unwrap_or_else(|| panic!("{name}"))
            .step
    };
    assert_eq!(step("VehicleSpeed"), Some(0.01));
    assert_eq!(step("EngineRPM"), Some(0.25));
    assert_eq!(step("BrakePressure"), Some(0.5));
    assert_eq!(step("Gear"), Some(1.0));

    let csv = b"t_us,signal,value,unit\n0,Speed,1.5,km/h\n";
    let mut decoded = Session::new();
    decoded.open_bytes("decoded.csv", csv.to_vec()).unwrap();
    assert_eq!(decoded.summary().unwrap().signals[0].step, None);
}

#[test]
fn deck_bound_signals_are_live_mid_drive() {
    // Names `readingsFrom` in src/gauges.ts looks up. A rename in the
    // generator or DBC leaves the cluster drawing blanks.
    let names = [
        "VehicleSpeed",
        "EngineRPM",
        "DisplayedRPM",
        "CoolantTemp",
        "OilTemp",
        "Soc",
        "FuelLevel",
        "Gear",
        "GearActual",
        "MilLamp",
        "TelltaleMil",
        "AbsActive",
        "TelltaleAbs",
        "TurnLeft",
        "TelltaleLeft",
        "TurnRight",
        "TelltaleRight",
        "DoorFL",
        "EscActive",
    ];
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut session = Session::new();
    session
        .open_path(&root.join("hypercar_lap.slog"))
        .expect("synthetic lap");
    let summary = session.summary().unwrap();
    let have: std::collections::HashSet<&str> = summary
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    for name in names {
        assert!(have.contains(name), "lap is missing {name}");
    }
    // 4:58, the deck playhead: braking off the straight, MIL already set.
    let held = session.values_at(298_000_000).unwrap();
    for name in names {
        let value = held.iter().find(|item| item.name == name);
        assert!(
            value.is_some_and(|item| item.value.is_finite()),
            "{name} did not decode at 4:58"
        );
    }
    let mil = held.iter().find(|item| item.name == "MilLamp").unwrap();
    assert!(mil.value > 0.5, "MIL should be lit mid-drive");
    let abs = held.iter().find(|item| item.name == "AbsActive").unwrap();
    assert!(abs.value > 0.5, "ABS should be lit on the straight's brake");
}

#[test]
fn dbc_text_decodes_the_open_log() {
    let text = "SLOGv1\nF 0 1A0 800C881378640000\nF 10000 1A0 800C881378640000\n";
    let mut session = Session::new();
    session
        .open_bytes("tiny.slog", text.as_bytes().to_vec())
        .unwrap();
    let summary = session
        .open_map_json(
            r#"
VERSION ""
BO_ 416 ECM_Engine: 8 ECM
 SG_ EngineRPM : 0|16@1+ (0.25,0) [0|16383.75] "rpm" TCU
BA_ "GenMsgCycleTime" BO_ 416 10;
"#,
        )
        .unwrap();
    assert!(summary
        .map_label
        .as_deref()
        .unwrap_or("")
        .starts_with("DBC import"));
    let rpm = session
        .values_at(0)
        .unwrap()
        .into_iter()
        .find(|value| value.name == "EngineRPM")
        .unwrap();
    assert!((rpm.value - 800.0).abs() < 1e-6, "{}", rpm.value);
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

fn hex_payload_bytes(text: &str) -> [u8; 64] {
    let mut data = [0u8; 64];
    let pairs = text.len() / 2;
    for i in 0..pairs.min(64) {
        data[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    data
}

#[test]
fn skips_malformed_lines_and_keeps_the_good_frames() {
    let text = "\
SLOGv1
F 0 1A0 0102
this is not a frame
F 1000 1A0 zz
F 2000 1A0 0304
";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.frame_count(), 2);
    assert!(log.skipped() >= 2);
    assert!(!log.warnings().is_empty());
}

#[test]
fn projects_are_written_only_as_loom_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let text = std::fs::read_to_string(root.join("demo.loom")).unwrap();
    let project = crate::ProjectFile::parse(&text).unwrap();
    let dir = std::env::temp_dir().join(format!("loom-write-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let session = Session::new();
    assert!(session
        .write_project(&dir.join("startup.bat"), &project)
        .is_err());
    assert!(!dir.join("startup.bat").exists());
    session
        .write_project(&dir.join("drive.LOOM"), &project)
        .unwrap();
    assert!(dir.join("drive.LOOM").is_file());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn projects_keep_cluster_assignments() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let text = std::fs::read_to_string(root.join("demo.loom")).unwrap();
    let mut project = ProjectFile::parse(&text).unwrap();
    assert!(project.cluster.is_empty(), "older projects have none");
    project.cluster.insert("lamp1".into(), "led_cmd_19".into());
    project
        .cluster
        .insert("speed".into(), "SteeringAngle".into());
    project.cluster.insert("lamp2".into(), "   ".into());
    let saved = project.to_json().unwrap();
    let reopened = ProjectFile::parse(&saved).unwrap();
    assert_eq!(
        reopened.cluster.get("lamp1").map(String::as_str),
        Some("led_cmd_19")
    );
    assert_eq!(
        reopened.cluster.get("speed").map(String::as_str),
        Some("SteeringAngle")
    );
    assert!(
        !reopened.cluster.contains_key("lamp2"),
        "a blank assignment is dropped"
    );
    let opened = Session::new()
        .load_project_json(&saved, Some(&root))
        .unwrap();
    assert_eq!(opened.project.cluster.len(), 2);
}

#[test]
fn a_map_that_fits_no_message_is_not_carried_into_the_next_log() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let open_cluster = || {
        let mut session = Session::new();
        session.open_path(&root.join("cluster_drive.slog")).unwrap();
        session
            .open_map_path(&root.join("cluster.map.json"))
            .unwrap();
        session
    };
    // A diagnostic log on ids the cluster map never mentions.
    let other_bus = "SLOGv1\nF 0 7F0 0102\nF 1000 784 0304\n";

    let mut uploaded = open_cluster();
    let summary = uploaded
        .open_bytes("flash.slog", other_bus.as_bytes().to_vec())
        .unwrap();
    assert!(summary.map_label.is_none(), "{:?}", summary.map_label);
    assert!(summary.signals.is_empty());
    assert!(
        summary.warnings.iter().any(|w| w.contains("set aside")),
        "{:?}",
        summary.warnings
    );

    let dir = std::env::temp_dir().join(format!("loom-fit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let flash = dir.join("flash.slog");
    std::fs::write(&flash, other_bus).unwrap();
    let mut opened = open_cluster();
    let summary = opened.open_path(&flash).unwrap();
    assert!(summary.map_label.is_none());
    assert!(summary.warnings.iter().any(|w| w.contains("set aside")));

    // A log on the same bus keeps the map, and says how much of it fits.
    let same_bus = dir.join("same.slog");
    std::fs::write(&same_bus, "SLOGv1\nF 0 1A0 4006102700000000\n").unwrap();
    let mut kept = open_cluster();
    let summary = kept.open_path(&same_bus).unwrap();
    assert!(summary.map_label.is_some());
    let fit = summary.map_match.expect("a frame log with a map");
    assert_eq!((fit.matched, fit.total), (1, 3));

    // A map the user loads for this log stays, with a warning if nothing fits.
    let summary = opened
        .open_map_path(&root.join("cluster.map.json"))
        .unwrap();
    assert!(summary.map_label.is_some());
    let fit = summary.map_match.unwrap();
    assert_eq!((fit.matched, fit.total), (0, 3));
    assert!(
        summary.warnings[0].contains("None of the 3 messages"),
        "{:?}",
        summary.warnings
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn projects_do_not_follow_network_paths() {
    use crate::project::is_network_path;
    for network in [
        r"\\server\share\drive.slog",
        "//server/share/drive.slog",
        r"\\?\UNC\server\x",
    ] {
        assert!(is_network_path(network), "{network}");
    }
    for local in [
        r"C:\logs\drive.slog",
        "fixtures/drive.slog",
        "drive.slog",
        "/home/me/drive.slog",
    ] {
        assert!(!is_network_path(local), "{local}");
    }

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let project = |log: &str, map: &str, compare: &str| {
        format!(
            r#"{{"format":"signal-loom","version":1,"logPath":{log:?},"signalMapPath":{map:?},
            "comparePath":{compare:?},"view":{{"playheadUs":0,"spanUs":1000000,"plotted":[]}}}}"#
        )
    };
    let err = Session::new()
        .load_project_json(&project(r"\\server\share\drive.slog", "", ""), Some(&root))
        .unwrap_err();
    assert!(err.to_string().contains("network path"), "{err}");

    let opened = Session::new()
        .load_project_json(
            &project(
                "cluster_drive.slog",
                r"\\server\maps\x.dbc",
                "//server/logs/b.slog",
            ),
            Some(&root),
        )
        .unwrap();
    assert!(
        opened.summary.map_label.is_none(),
        "the network map is not read"
    );
    let network_notes = opened
        .warnings
        .iter()
        .filter(|warning| warning.contains("network path"))
        .count();
    assert_eq!(network_notes, 2, "{:?}", opened.warnings);
}

#[test]
fn oversized_text_files_are_refused() {
    let dir = std::env::temp_dir().join(format!("loom-cap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("big.dbc");
    std::fs::write(&file, vec![b'x'; 2 * 1024 * 1024 + 1]).unwrap();
    let err = crate::project::read_text_capped(&file, 2 * 1024 * 1024, "signal map").unwrap_err();
    assert!(err.to_string().contains("at most 2 MB"), "{err}");
    assert!(crate::project::read_text_capped(&file, 3 * 1024 * 1024, "signal map").is_ok());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn exports_save_to_disk_only_as_csv_and_slog() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut session = Session::new();
    session.open_path(&root.join("cluster_drive.slog")).unwrap();
    session
        .open_map_path(&root.join("cluster.map.json"))
        .unwrap();
    let dir = std::env::temp_dir().join(format!("loom-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let names = vec!["VehicleSpeed".to_string()];

    let csv = dir.join("speed.csv");
    let bytes = session.save_csv(&csv, &names, 0, 5_000_000).unwrap();
    let written = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(bytes, written.len() as u64);
    assert_eq!(written, session.export_csv(&names, 0, 5_000_000).unwrap());

    let slog = dir.join("trim.SLOG");
    session.save_slog(&slog, 0, 5_000_000).unwrap();
    assert!(std::fs::read_to_string(&slog)
        .unwrap()
        .starts_with("SLOGv1\n"));

    for refused in ["speed.bat", "speed", "trim.slog.exe"] {
        assert!(session
            .save_csv(&dir.join(refused), &names, 0, 5_000_000)
            .is_err());
        assert!(session.save_slog(&dir.join(refused), 0, 5_000_000).is_err());
        assert!(!dir.join(refused).exists(), "{refused} must not be created");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn non_ascii_payloads_are_skipped_not_panics() {
    let slog = "SLOGv1\nF 0 1A0 0102\nF 1000 1A0 a\u{e9}0\nF 2000 1A0 0304\n";
    let log = IndexedLog::open_bytes(slog.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.frame_count(), 2);
    assert_eq!(log.skipped(), 1);

    let candump = "(0.000) can0 1A0#0102\n(0.001) can0 123##\u{e9}a\n(0.002) can0 1A0#0304\n";
    let log = IndexedLog::open_bytes(candump.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.frame_count(), 2);
    assert_eq!(log.skipped(), 1);
}

#[test]
fn asc_relative_absolute_fd_extended_and_channels() {
    let absolute = "\
date Mon
base hex timestamps absolute
0.000000 1 1A0 Rx d 1 0A
0.001000 2 18FF50E5x Rx d 1 AA
0.002000 CANFD 1 Rx 1A0 0 0 d 15 4 01 02 03 04
not a line
";
    let log = IndexedLog::open_bytes(absolute.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.format().label(), "Vector ASC");
    assert_eq!(log.frame_count(), 3);
    assert!(log.skipped() >= 1);
    let extended = log.step_frame(0, true).unwrap().unwrap();
    assert_eq!(extended.message_id, Some(0x18FF50E5));
    let fd = log.step_frame(extended.t_us, true).unwrap().unwrap();
    assert_eq!(fd.t_us, 2_000);
    assert_eq!(fd.dlc, 4);
    assert_eq!(fd.data_hex, "01020304");

    let relative = "\
base hex timestamps relative
0.010000 1 100 Rx d 1 01
0.010000 1 100 Rx d 1 02
";
    let log = IndexedLog::open_bytes(relative.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.t_end_us(), 20_000);
    assert_eq!(log.frame_count(), 2);
}

#[test]
fn candump_with_and_without_an_interface() {
    let text = "\
(0.000000) 1A0#AABB
(0.001000) can1 1A0#CCDD
(0.002000) can0 1A0##101020304
1A0#EE
garbage
";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.format().label(), "candump");
    assert_eq!(log.frame_count(), 4, "skipped {}", log.skipped());
    assert!(log.skipped() >= 1);
    assert_eq!(log.t_end_us(), 2_000);
    let first = log.step_frame(u64::MAX, false).unwrap();
    assert!(first.is_some());
}

#[test]
fn channel_filter_ignores_the_other_bus() {
    let map = SignalMap::parse(
        r#"{"name":"ch","version":1,"messages":[{"id":"0x1A0","name":"M","channel":1,"signals":[
            {"name":"Byte","startBit":0,"bitLength":8,"factor":1}
        ]}]}"#,
    )
    .unwrap();
    let text = "\
base hex timestamps absolute
0.000000 1 1A0 Rx d 1 0A
0.001000 2 1A0 Rx d 1 14
";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let value = log.values_at(1_000).unwrap()[0].value;
    assert!((value - 10.0).abs() < 1e-6, "{value}");
}

#[test]
fn a_cancelled_or_failed_load_keeps_the_deck_that_was_open() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let mut session = Session::new();
    session.open_path(&root.join("cluster_drive.slog")).unwrap();
    let before = session
        .open_map_path(&root.join("cluster.map.json"))
        .unwrap();
    let names = |summary: &crate::Summary| {
        summary
            .signals
            .iter()
            .map(|signal| signal.name.clone())
            .collect::<Vec<_>>()
    };

    let cancelled = crate::IndexControl::default();
    cancelled.request_cancel();
    let dbc = root.join("hypercar_lap.dbc");
    assert!(session
        .add_map_path_controlled(&dbc, 0, Some(&cancelled))
        .is_err());
    let after_cancel = session.summary().unwrap();
    assert_eq!(names(&after_cancel), names(&before));
    assert_eq!(after_cancel.map_label, before.map_label);

    let added = session.add_map_path(&dbc, 0).unwrap();
    let mut once = Session::new();
    once.open_path(&root.join("cluster_drive.slog")).unwrap();
    once.open_map_path(&root.join("cluster.map.json")).unwrap();
    let expected = once.add_map_path(&dbc, 0).unwrap();
    assert_eq!(
        names(&added),
        names(&expected),
        "the retry added the DBC once"
    );

    let broken = r#"{"format":"signal-loom","version":1,"logPath":"nowhere.slog",
        "signalMapPath":"hypercar_lap.dbc","view":{"playheadUs":0,"spanUs":1000000,"plotted":[]}}"#;
    let mut fresh = Session::new();
    fresh.open_path(&root.join("cluster_drive.slog")).unwrap();
    let kept = fresh.open_map_path(&root.join("cluster.map.json")).unwrap();
    assert!(fresh.load_project_json(broken, Some(&root)).is_err());
    let after_project = fresh.summary().unwrap();
    assert_eq!(after_project.map_label, kept.map_label);
    assert_eq!(names(&after_project), names(&kept));
}

#[test]
fn math_stats_and_export_read_every_sample() {
    // 20,000 samples: 0 with a 100 spike every 100th, so the true average is 1.
    let mut csv = String::from("t_us,signal,value,unit\n");
    for i in 0..20_000u64 {
        let value = if i % 100 == 0 { 100 } else { 0 };
        csv.push_str(&format!("{},A,{value},\n", i * 1000));
    }
    let mut session = Session::new();
    session.open_bytes("spiky.csv", csv.into_bytes()).unwrap();
    session
        .set_math(vec![crate::MathChannel {
            name: "Copy".into(),
            unit: String::new(),
            expr: "A * 1".into(),
        }])
        .unwrap();
    let stats = session.stats("Copy", 0, 19_999_000).unwrap();
    assert_eq!(stats.count, 20_000);
    assert!((stats.avg - 1.0).abs() < 1e-9, "{}", stats.avg);
    let exported = session.export_csv(&["Copy".into()], 0, 19_999_000).unwrap();
    assert_eq!(
        exported.lines().count(),
        20_001,
        "header plus one row per sample"
    );
}

#[test]
fn a_zero_divisor_leaves_a_gap_not_a_blank_plot() {
    let csv = "t_us,signal,value,unit\n0,T,10,\n0,S,5,\n1000,S,0,\n2000,S,2,\n";
    let mut session = Session::new();
    session
        .open_bytes("ratio.csv", csv.as_bytes().to_vec())
        .unwrap();
    session
        .set_math(vec![crate::MathChannel {
            name: "Ratio".into(),
            unit: String::new(),
            expr: "T / S".into(),
        }])
        .unwrap();
    let series = session
        .query(&crate::Query {
            t0_us: 0,
            t1_us: 2000,
            signals: vec!["T".into(), "Ratio".into()],
            max_points: 100,
            include_compare: false,
        })
        .expect("one zero divisor must not fail the whole query");
    let ratio = series.iter().find(|series| series.name == "Ratio").unwrap();
    let points: Vec<(u64, f64)> = ratio
        .points
        .iter()
        .map(|point| (point.t, point.v))
        .collect();
    assert_eq!(points, vec![(0, 2.0), (2000, 5.0)]);
}

#[test]
fn value_tables_and_mux_ids_match_raw_values() {
    let dbc = r#"
BO_ 200 Scaled: 8 ECM
 SG_ Sel M : 0|8@1+ (0.5,10) [0|255] "" Vector__XXX
 SG_ Mode : 8|8@1+ (2,-4) [0|255] "" Vector__XXX
 SG_ Branch m3 : 16|8@1+ (1,0) [0|255] "" Vector__XXX
VAL_ 200 Mode 3 "Sport" 2 "Comfort" ;
"#;
    let map = crate::dbc::parse(dbc).unwrap();
    // Sel raw 3 (11.5 scaled), Mode raw 3 (2 scaled), Branch 42.
    let log = IndexedLog::open_bytes(b"F 0 C8 03032A0000000000\n".to_vec(), Some(&map)).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| values.iter().find(|value| value.name == name);
    let mode = get("Mode").expect("Mode");
    assert_eq!(mode.value, 2.0);
    assert_eq!(mode.label.as_deref(), Some("Sport"));
    assert_eq!(get("Branch").map(|value| value.value), Some(42.0));
}

#[test]
fn short_frames_leave_their_missing_signals_unset() {
    let dbc = r#"
BO_ 300 Short: 8 ECM
 SG_ Head : 0|8@1+ (1,0) [0|255] "" Vector__XXX
 SG_ TailLe : 48|16@1+ (1,0) [0|65535] "" Vector__XXX
 SG_ TailBe : 55|16@0+ (1,0) [0|65535] "" Vector__XXX
BO_ 400 Wide: 8 ECM
 SG_ Odo : 0|64@1+ (1,0) [0|0] "" Vector__XXX
"#;
    let map = crate::dbc::parse(dbc).unwrap();
    let text = "F 0 12C 07\nF 1000 190 FFFFFFFFFFFFFFFF\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let values = log.values_at(1000).unwrap();
    let get = |name: &str| {
        values
            .iter()
            .find(|value| value.name == name)
            .map(|value| value.value)
    };
    assert_eq!(get("Head"), Some(7.0));
    assert_eq!(
        get("TailLe"),
        None,
        "a 1-byte frame does not carry bytes 6-7"
    );
    assert_eq!(
        get("TailBe"),
        None,
        "a 1-byte frame does not carry bytes 6-7"
    );
    assert_eq!(
        get("Odo"),
        Some(u64::MAX as f64),
        "unsigned 64-bit stays positive"
    );
}

#[test]
fn multiplex_value_table_and_a_second_dbc() {
    let dbc = r#"
BO_ 100 MuxMsg: 8 ECM
 SG_ Mode M : 0|8@1+ (1,0) [0|3] "" Vector__XXX
 SG_ Gear m0 : 8|8@1+ (1,0) [0|7] "" Vector__XXX
 SG_ Temp m1 : 8|8@1- (1,-40) [0|200] "C" Vector__XXX
VAL_ 100 Gear 0 "N" 1 "D" 2 "R" ;
"#;
    let map = crate::dbc::parse(dbc).unwrap();
    let text = "F 0 64 0001000000000000\nF 1000 64 0105000000000000\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), Some(&map)).unwrap();
    let at0 = log.values_at(0).unwrap();
    let gear = at0.iter().find(|value| value.name == "Gear").unwrap();
    assert!((gear.value - 1.0).abs() < 1e-6);
    assert_eq!(gear.label.as_deref(), Some("D"));
    assert!(at0.iter().all(|value| value.name != "Temp"));
    let at1 = log.values_at(1_000).unwrap();
    let temp = at1.iter().find(|value| value.name == "Temp").unwrap();
    assert!((temp.value - (-35.0)).abs() < 1e-6);
    let gear_held = at1.iter().find(|value| value.name == "Gear").unwrap();
    assert!((gear_held.value - 1.0).abs() < 1e-6);

    let mut session = Session::new();
    session
        .open_bytes("mux.slog", text.as_bytes().to_vec())
        .unwrap();
    session
        .open_map_json(
            r#"{"name":"left","version":1,"messages":[{"id":100,"name":"Mux","signals":[
        {"name":"Mode","startBit":0,"bitLength":8,"muxSwitch":true}
    ]}]}"#,
        )
        .unwrap();
    let summary = session.add_map_json(
        r#"{"name":"right","version":1,"messages":[{"id":100,"name":"Mux","channel":2,"signals":[
            {"name":"Mode","startBit":0,"bitLength":8,"muxSwitch":true}
        ]}]}"#,
        2,
    )
    .unwrap();
    let names: Vec<_> = summary
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    assert!(names.contains(&"Mode"), "{names:?}");
    assert!(
        names.iter().any(|name| name.starts_with("Mode@")),
        "{names:?}"
    );
}

#[test]
fn the_timeout_factor_is_a_setting_with_a_2_5_cycle_default() {
    // A 100 ms message with gaps of 260 ms and 290 ms.
    let log = "F 0 120 00\nF 100000 120 00\nF 200000 120 00\nF 460000 120 00\nF 750000 120 00\n";
    let map = r#"{"name":"bus","version":1,"messages":[
        {"id":"0x120","name":"Leds","cycleUs":100000,"signals":[
            {"name":"Lamp","startBit":0,"bitLength":8}]}]}"#;
    let timeouts = |summary: &crate::Summary| {
        summary
            .events
            .iter()
            .filter(|event| event.label.starts_with("Timeout"))
            .map(|event| event.t_us)
            .collect::<Vec<_>>()
    };
    let mut session = Session::new();
    session
        .open_bytes("leds.slog", log.as_bytes().to_vec())
        .unwrap();
    let summary = session.open_map_json(map).unwrap();
    assert_eq!(summary.timeout_factor, 2.5);
    assert_eq!(timeouts(&summary), vec![460_000, 750_000]);

    let relaxed = session.set_timeout_factor(3.0).unwrap();
    assert!(timeouts(&relaxed).is_empty(), "both gaps are under 300 ms");
    assert!(session.set_timeout_factor(0.5).is_err());
    assert_eq!(session.summary().unwrap().timeout_factor, 3.0);

    // A project carries its factor, and it applies to the log it opens.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let text = std::fs::read_to_string(root.join("demo.loom")).unwrap();
    let mut project = ProjectFile::parse(&text).unwrap();
    assert_eq!(project.timeout_factor, None, "older projects have none");
    project.timeout_factor = Some(4.0);
    let saved = project.to_json().unwrap();
    assert!(saved.contains("\"timeoutFactor\": 4.0"), "{saved}");
    let opened = Session::new()
        .load_project_json(&saved, Some(&root))
        .unwrap();
    assert_eq!(opened.summary.timeout_factor, 4.0);
}

#[test]
fn checksum_crcs_match_their_published_check_values() {
    use crate::index::ChecksumAlgo;
    assert_eq!(ChecksumAlgo::CrcJ1850.compute(b"123456789"), 0x4B);
    assert_eq!(ChecksumAlgo::Crc8H2F.compute(b"123456789"), 0xDF);
}

/// An SWM-style message: CRC-8/SAE-J1850 in byte 0 over bytes 1..DLC-1, a
/// 4-bit counter in the low nibble of byte 1, DLC 5, 100 ms apart.
fn swm_style_log(checksum_of: impl Fn(usize, &[u8]) -> u8) -> String {
    let mut text = String::new();
    for i in 0..20usize {
        let mut frame = [0u8, ((i + 1) % 16) as u8 | 0x30, 0x12, i as u8, 0x7F];
        frame[0] = checksum_of(i, &frame[1..]);
        text.push_str(&format!("F {} 334 {}\n", i * 100_000, hex(&frame)));
    }
    text
}

const SWM_DBC: &str = r#"
BO_ 820 ZcuLeds: 5 ZCU
 SG_ Checksum : 0|8@1+ (1,0) [0|255] "" SWM
 SG_ Counter : 8|4@1+ (1,0) [0|15] "" SWM
 SG_ Led : 12|4@1+ (1,0) [0|15] "" SWM
"#;

#[test]
fn a_crc_checksum_is_recognised_and_a_bad_frame_is_seen_as_the_ecu_sees_it() {
    use crate::index::ChecksumAlgo;
    let map = crate::dbc::parse(SWM_DBC).unwrap();
    // Frame 10 is corrupted in its checksum byte only.
    let text = swm_style_log(|i, covered| {
        let crc = ChecksumAlgo::CrcJ1850.compute(covered);
        if i == 10 {
            crc ^ 0x55
        } else {
            crc
        }
    });
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    let mut events: Vec<(u64, String)> = log.events().to_vec();
    events.sort();
    // The rejected frame leaves the counter reference at frame 9, so frame 11
    // arrives as a jump, as the receiving ECU sees it.
    assert_eq!(
        events,
        vec![
            (1_000_000, "Checksum Checksum".to_string()),
            (1_100_000, "Counter Counter".to_string()),
        ]
    );
}

#[test]
fn an_unknown_checksum_scheme_is_noted_not_flagged() {
    let map = crate::dbc::parse(SWM_DBC).unwrap();
    let text = swm_style_log(|i, _| (i as u8).wrapping_mul(37).wrapping_add(11));
    let log = IndexedLog::open_bytes(text.into_bytes(), Some(&map)).unwrap();
    assert!(log.events().is_empty(), "{:?}", log.events());
    assert!(
        log.warnings()
            .iter()
            .any(|warning| warning.contains("not checked")),
        "{:?}",
        log.warnings()
    );
}

#[test]
fn asc_from_a_logger_that_writes_microseconds_and_names() {
    // Whole-number stamps in µs, a Start marker, a tool-level `Node.Message`
    // line, a hex DLC code (c = 24 bytes) and one Rx line 2 ms out of order.
    let text = "\
date Fri, Jul 24, 2026, 13:16:47
base hex  timestamps absolute
internal events logged
Begin Triggerblock Fri, Jul 24, 2026, 13:16:47
0.000000 Start of measurement
0.000000 1  335             Rx   d 8 01 02 03 04 05 06 07 08
9843.000000 1  335             Rx   d 8 01 02 03 04 05 06 07 08
10356.000000 1  33A             Rx   d 8 01 02 03 04 05 06 07 08
12000.000000 1  NODE_1.TRANSFERDATA Tx   d f 01 02 03
29257.000000 1  339             Tx   d 5 01 02 03 04 05
27257.000000 1  336             Rx   d 5 01 02 03 04 05
48690.000000 1  336             Rx   d c 00 01 02 03 04 05 06 07 08 09 0a 0b 0c 0d 0e 0f 10 11 12 13 14 15 16 17
100609.000000 1  334             Tx   d 5 01 02 03 04 05
133699.000000 1  337             Tx   d 5 01 02 03 04 05
85710697.000000 1  334             Tx   d 5 01 02 03 04 05
End TriggerBlock
";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.t_end_us(), 85_710_697, "the stamps are microseconds");
    assert_eq!(log.frame_count(), 9);
    assert_eq!(log.skipped(), 0, "{:?}", log.warnings());
    let warned = |needle: &str| {
        log.warnings()
            .iter()
            .any(|warning| warning.contains(needle))
    };
    assert!(warned("microseconds"), "{:?}", log.warnings());
    assert!(warned("1 symbolic Node.Message"), "{:?}", log.warnings());
    assert!(warned("out of order"), "{:?}", log.warnings());
    let wide = log
        .step_frame(48_000, true)
        .unwrap()
        .expect("the 24-byte frame");
    assert_eq!(wide.t_us, 48_690);
    assert_eq!(wide.dlc, 24);
}

#[test]
fn repeated_dbc_signal_names_are_renamed_and_still_decode() {
    let text = r#"
BO_ 1 Brake: 8 ESC
 SG_ Pressure : 0|8@1+ (1,0) [0|255] "bar" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
BO_ 2 Steer: 8 SWM
 SG_ Angle : 0|8@1+ (1,0) [0|255] "deg" IC
 SG_ Checksum : 56|8@1+ (1,0) [0|255] "" IC
VAL_ 2 Checksum 171 "Fixed" ;
"#;
    let map = crate::dbc::parse(text).expect("a repeated name must not reject the DBC");
    let names: Vec<&str> = map
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    assert_eq!(names, vec!["Pressure", "Checksum", "Angle", "Checksum@2"]);
    assert!(map
        .warnings
        .iter()
        .any(|warning| warning.contains("Checksum@2")));

    let log = "F 0 1 11000000000000AA\nF 0 2 22000000000000AB\n";
    let log = IndexedLog::open_bytes(log.as_bytes().to_vec(), Some(&map)).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| values.iter().find(|value| value.name == name).unwrap();
    assert_eq!(get("Checksum").value, 0xAA as f64);
    assert_eq!(get("Checksum@2").value, 0xAB as f64);
    assert_eq!(get("Checksum@2").label.as_deref(), Some("Fixed"));
}

#[test]
fn a_bad_layout_skips_one_signal_and_can_fd_signals_decode_past_byte_8() {
    let text = r#"
BO_ 1 Fd: 64 ADAS
 SG_ Head : 0|8@1+ (1,0) [0|255] "" IC
 SG_ LateLe : 400|16@1+ (0.5,0) [0|1000] "" IC
 SG_ LateBe : 407|16@0+ (1,0) [0|65535] "" IC
 SG_ PastTheEnd : 600|16@1+ (1,0) [0|65535] "" IC
"#;
    let map = crate::dbc::parse(text).expect("one bad layout must not reject the DBC");
    let names: Vec<&str> = map
        .signals
        .iter()
        .map(|signal| signal.name.as_str())
        .collect();
    assert_eq!(names, vec!["Head", "LateLe", "LateBe"]);
    assert!(map
        .warnings
        .iter()
        .any(|warning| warning.contains("PastTheEnd")));

    // Byte 50-51 little-endian 0x0123 (291 × 0.5), byte 50-51 big-endian 0x2301.
    let mut payload = [0u8; 64];
    payload[0] = 7;
    payload[50] = 0x23;
    payload[51] = 0x01;
    let line = format!("F 0 1 {}\n", hex(&payload));
    let log = IndexedLog::open_bytes(line.into_bytes(), Some(&map)).unwrap();
    let values = log.values_at(0).unwrap();
    let get = |name: &str| {
        values
            .iter()
            .find(|value| value.name == name)
            .map(|value| value.value)
    };
    assert_eq!(get("Head"), Some(7.0));
    assert_eq!(get("LateLe"), Some(145.5));
    assert_eq!(get("LateBe"), Some(0x2301 as f64));
}

#[test]
fn a_broken_signal_does_not_drop_the_rest_of_the_dbc() {
    let text = "\
BO_ 1 Only: 8 ECM
 SG_ Broken : not-a-layout
 SG_ Ok : 0|8@1+ (1,0) [0|255] \"\" Vector__XXX
";
    let map = crate::dbc::parse(text).unwrap();
    assert_eq!(map.signals.len(), 1);
    assert!(map
        .warnings
        .iter()
        .any(|warning| warning.contains("Broken")));
}

#[test]
fn parsers_do_not_panic_on_garbage() {
    let mut state = 0x516C4F4Du64;
    let mut next = |len: usize| {
        let mut bytes = vec![0u8; len];
        for byte in &mut bytes {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            *byte = (state >> 33) as u8;
        }
        bytes
    };
    let mut samples = vec![
        Vec::new(),
        b"SLB1".to_vec(),
        b"LOGG".to_vec(),
        b"SLOGv1\nF not a frame\n".to_vec(),
        b"date\nbase hex\nnope\n".to_vec(),
        b"(1.0) not#zz\n".to_vec(),
    ];
    for n in 0..48 {
        let mut bytes = next(32 + (n * 97) % 3000);
        if n % 5 == 0 {
            bytes.splice(0..0, b"LOGG".iter().copied());
        }
        if n % 5 == 1 {
            bytes.splice(0..0, b"SLB1".iter().copied());
        }
        samples.push(bytes);
    }
    for bytes in samples {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = crate::scan::sniff(&bytes);
            let _ = IndexedLog::open_bytes(bytes.clone(), None);
            if let Ok(text) = String::from_utf8(bytes.clone()) {
                let _ = crate::dbc::parse(&text);
                let _ = SignalMap::parse(&text);
            }
        }));
        assert!(result.is_ok(), "parser panicked");
    }
}

#[test]
fn cancel_stops_before_the_file_is_committed() {
    let mut text = String::from("SLOGv1\n");
    for i in 0..400u64 {
        text.push_str(&format!("F {} 1A0 0100\n", i * 1000));
    }
    let control = crate::index::IndexControl::default();
    control.request_cancel();
    let err = IndexedLog::open_bytes(text.into_bytes(), None);
    assert!(err.is_ok(), "bytes open has no control handle");
    let dir = std::env::temp_dir().join(format!("loom-cancel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
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
    assert!(err.to_string().contains("cancelled"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn streams_a_large_log_with_bounded_checkpoints() {
    let dir = std::env::temp_dir().join(format!("loom-stream-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wide.slog");
    let mut file = std::fs::File::create(&path).unwrap();
    use std::io::Write;
    writeln!(file, "SLOGv1").unwrap();
    let lines = 1_100_000u64;
    for i in 0..lines {
        if i == 50_000 {
            writeln!(file, "{}", "A".repeat(1_000_100)).unwrap();
        } else if i % 2_000 == 7 {
            writeln!(file, "NOT A FRAME {i}").unwrap();
        } else if i % 5_000 == 3 {
            writeln!(file, "F {i} 1A0 zz").unwrap();
        } else {
            writeln!(file, "F {} 1A0 0100", i * 1_000).unwrap();
        }
    }
    file.flush().unwrap();
    drop(file);
    let bytes = std::fs::metadata(&path).unwrap().len();
    assert!(bytes > 12 * 1024 * 1024, "generated {bytes} bytes");
    let log = IndexedLog::open_path(&path, None).unwrap();
    assert!(log.path().is_some());
    assert!(log.frame_count() > 1_000_000, "{}", log.frame_count());
    assert!(log.skipped() > 100, "{}", log.skipped());
    assert!(
        log.checkpoint_count() <= 4_096,
        "{}",
        log.checkpoint_count()
    );
    assert!(!log.warnings().is_empty());
    let hit = log.step_frame(0, true).unwrap().unwrap();
    assert!(hit.t_us > 0);
    let _ = std::fs::remove_dir_all(&dir);
}
