use crate::index::IndexedLog;
use crate::map::SignalMap;
use crate::scan::hex_payload;
use crate::session::Session;

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
        hex_payload(&frame(1), 8),
        hex_payload(&frame(4), 8),
        hex_payload(&bad, 8)
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
            op: crate::project::TriggerOp::Gt,
            value: 99.0,
        }])
        .unwrap();
    let summary = session.summary().unwrap();
    let events: Vec<(u64, &str)> = summary
        .events
        .iter()
        .map(|event| (event.t_us, event.label.as_str()))
        .collect();
    assert_eq!(events, [(0, "Trigger WheelFL > 99")]);
    assert!(summary.signals.iter().any(|signal| signal.name == "Slip"));
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
