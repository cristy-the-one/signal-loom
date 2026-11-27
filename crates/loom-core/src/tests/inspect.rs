use super::support::*;
use crate::index::IndexedLog;
use crate::inspect::stats_for_log;
use crate::map::TimeoutFactor;

fn lap_map() -> crate::map::SignalMap {
    crate::dbc::parse(&std::fs::read_to_string(lap_fixture("hypercar_lap.dbc")).unwrap()).unwrap()
}

/// `--stats` gathers every signal in the indexing pass. It must report what a
/// separate `IndexedLog::stats` replay per signal reports, to the last bit.
#[test]
fn single_pass_stats_match_a_replay_per_signal() {
    let map = lap_map();
    let path = lap_fixture("hypercar_lap.slog");
    let log =
        IndexedLog::open_path_timed(&path, Some(&map), TimeoutFactor::default(), None).unwrap();
    let windows = [
        (None, None),
        (Some(123_456_789), None),
        (None, Some(200_000_000)),
        (Some(300_000_000), Some(100_000_000)),
        (Some(410_000_000), Some(410_500_000)),
    ];
    for (from, to) in windows {
        let report = stats_for_log(&path, Some(&map), &[], from, to).unwrap();
        assert!(!report.signals.is_empty());
        let t0 = from.unwrap_or_else(|| log.t_start_us());
        let t1 = to.unwrap_or_else(|| log.t_end_us());
        let mut expected = 0;
        for info in log.signals().filter(|s| s.min.is_some()) {
            match log.stats(&info.name, t0, t1) {
                Ok(want) => {
                    expected += 1;
                    let got = report
                        .signals
                        .iter()
                        .find(|s| s.name == info.name)
                        .unwrap_or_else(|| panic!("{} missing for {from:?}..{to:?}", info.name));
                    let g = &got.stats;
                    assert_eq!(
                        (
                            g.count,
                            g.min.to_bits(),
                            g.max.to_bits(),
                            g.avg.to_bits(),
                            g.first.to_bits(),
                            g.last.to_bits()
                        ),
                        (
                            want.count,
                            want.min.to_bits(),
                            want.max.to_bits(),
                            want.avg.to_bits(),
                            want.first.to_bits(),
                            want.last.to_bits()
                        ),
                        "{} over {from:?}..{to:?}",
                        info.name
                    );
                }
                Err(_) => assert!(
                    report.signals.iter().all(|s| s.name != info.name),
                    "{} has no sample in {from:?}..{to:?} but was reported",
                    info.name
                ),
            }
        }
        assert_eq!(report.signals.len(), expected, "{from:?}..{to:?}");
    }
}

#[test]
fn named_stats_keep_their_errors() {
    let map = lap_map();
    let path = lap_fixture("hypercar_lap.slog");
    let names = ["VehicleSpeed".to_string(), "EngineRPM".to_string()];
    let report = stats_for_log(&path, Some(&map), &names, None, None).unwrap();
    assert_eq!(
        report
            .signals
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["VehicleSpeed", "EngineRPM"]
    );
    let unknown = stats_for_log(&path, Some(&map), &["NoSuchSignal".to_string()], None, None);
    assert!(unknown.unwrap_err().to_string().contains("NoSuchSignal"));
    // A window before the first frame holds no sample of a named signal.
    let empty = stats_for_log(&path, Some(&map), &names[..1], Some(0), Some(1));
    assert!(empty
        .unwrap_err()
        .to_string()
        .contains("no samples of VehicleSpeed"));
}

#[test]
fn decoded_csv_stats_fall_back_to_the_index() {
    let path = fixtures_dir().join("decoded_snippet.csv");
    let report = stats_for_log(&path, None, &[], None, None).unwrap();
    assert!(!report.signals.is_empty());
    let log = IndexedLog::open_path(&path, None).unwrap();
    for named in &report.signals {
        let want = log.stats(&named.name, report.t0_us, report.t1_us).unwrap();
        assert_eq!(named.stats.count, want.count, "{}", named.name);
        assert_eq!(
            named.stats.avg.to_bits(),
            want.avg.to_bits(),
            "{}",
            named.name
        );
    }
}
