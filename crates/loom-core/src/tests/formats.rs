use super::support::*;
use crate::index::IndexedLog;
use crate::scan::{encode_slb1, Rec, RecKind};
use crate::session::Session;
use crate::StepDir;

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
            .open_map_json(include_str!("../../../../fixtures/cluster.map.json"))
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
fn error_frames_are_events() {
    let text = "SLOGv1\nF 0 100 00\nX 1500\nF 3000 100 11\n";
    let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
    assert_eq!(log.event_count(), 1);
    assert_eq!(log.events()[0].1, "Error frame");
}

#[test]
fn decoded_csv_does_not_pretend_to_use_a_signal_map() {
    let bytes = include_bytes!("../../../../fixtures/decoded_snippet.csv");
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
    assert_eq!(
        log.warnings(),
        [
            "line 3: timestamp 'is' is not an integer microsecond count",
            "line 4: bad payload hex 'zz'",
        ]
    );
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
    // The untimed `1A0#EE` line inherits the last timestamp, 2 ms.
    let first = log
        .step_frame(u64::MAX, false)
        .unwrap()
        .expect("the last frame");
    assert_eq!(
        (first.ordinal, first.t_us, first.data_hex.as_str()),
        (3, 2_000, "EE")
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
