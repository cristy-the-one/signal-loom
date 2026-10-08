use super::support::*;
use crate::project::ProjectFile;
use crate::session::Session;

#[test]
fn a_map_that_fits_no_message_is_not_carried_into_the_next_log() {
    let root = fixtures_dir();
    // A diagnostic log on ids the cluster map never mentions.
    let other_bus = "SLOGv1\nF 0 7F0 0102\nF 1000 784 0304\n";

    let mut uploaded = open_cluster_drive();
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

    let dir = TempDir::new("fit");
    let flash = dir.join("flash.slog");
    std::fs::write(&flash, other_bus).unwrap();
    let mut opened = open_cluster_drive();
    let summary = opened.open_path(&flash).unwrap();
    assert!(summary.map_label.is_none());
    assert!(summary.warnings.iter().any(|w| w.contains("set aside")));

    // A log on the same bus keeps the map, and says how much of it fits.
    let same_bus = dir.join("same.slog");
    std::fs::write(&same_bus, "SLOGv1\nF 0 1A0 4006102700000000\n").unwrap();
    let mut kept = open_cluster_drive();
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
    let root = fixtures_dir();
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
