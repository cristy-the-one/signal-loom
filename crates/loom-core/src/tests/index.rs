use super::support::*;
use crate::index::IndexedLog;
use crate::map::SignalMap;
use std::time::Instant;

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
fn streams_a_large_log_with_bounded_checkpoints() {
    let dir = TempDir::new("stream");
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
    assert_eq!(log.path(), Some(path.as_path()));
    assert!(log.frame_count() > 1_000_000, "{}", log.frame_count());
    assert!(log.skipped() > 100, "{}", log.skipped());
    assert!(
        log.checkpoint_count() <= 4_096,
        "{}",
        log.checkpoint_count()
    );
    assert_eq!(log.warnings().len(), 32, "warnings stop at the cap");
    assert_eq!(log.warnings()[0], "line 5: bad payload hex 'zz'");
    let hit = log.step_frame(0, true).unwrap().unwrap();
    assert!(hit.t_us > 0);
}
