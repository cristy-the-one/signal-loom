//! Fixtures and helpers the session submodules' tests share.

use super::Session;
use crate::dto::Query;
use crate::project::MathChannel;
use std::path::PathBuf;

pub(super) const RPM_LOG: &str = "SLOGv1\nF 0 1A0 800C881378640000\nF 10000 1A0 800C881378640000\n";
pub(super) const RPM_MAP: &str = r#"{"name":"rpm","version":1,"messages":[{"id":"0x1A0","name":"Powertrain",
        "signals":[{"name":"EngineRPM","startBit":0,"bitLength":16,"factor":0.25,"unit":"rpm"}]}]}"#;

pub(super) fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

pub(super) fn rpm_query() -> Query {
    Query {
        t0_us: 0,
        t1_us: 20_000,
        signals: vec!["EngineRPM".to_string()],
        max_points: 100,
        include_compare: true,
    }
}

pub(super) fn compare_values(session: &Session, query: &Query) -> Vec<f64> {
    session
        .query(query)
        .unwrap()
        .into_iter()
        .filter(|series| series.name.ends_with(" · B"))
        .flat_map(|series| series.points.into_iter().map(|point| point.v))
        .collect()
}

pub(super) fn math(name: &str, expr: &str) -> MathChannel {
    MathChannel {
        name: name.to_string(),
        unit: String::new(),
        expr: expr.to_string(),
    }
}

/// A rises 10, 12, 11, 30, 13, 12, 14, 11, 12, 13 at every 1000 µs from 0.
/// B is 1, 2, 3, 4, 40, 6, 7, 8, 9, 10 at every 1000 µs from 500.
pub(super) fn math_session(channels: &[(&str, &str)]) -> Session {
    let a = [10, 12, 11, 30, 13, 12, 14, 11, 12, 13];
    let b = [1, 2, 3, 4, 40, 6, 7, 8, 9, 10];
    let mut text = String::from("t_us,signal,value,unit\n");
    for i in 0..10 {
        text.push_str(&format!("{},A,{},\n", i * 1000, a[i]));
        text.push_str(&format!("{},B,{},\n", i * 1000 + 500, b[i]));
    }
    let mut session = Session::new();
    session.open_bytes("math.csv", text.into_bytes()).unwrap();
    session
        .set_math(
            channels
                .iter()
                .map(|(name, expr)| math(name, expr))
                .collect(),
        )
        .unwrap();
    session
}

pub(super) fn math_points(
    session: &Session,
    name: &str,
    t0_us: u64,
    max_points: usize,
) -> Vec<(u64, f64)> {
    let series = session
        .query(&Query {
            t0_us,
            t1_us: 9_000,
            signals: vec![name.to_string()],
            max_points,
            include_compare: false,
        })
        .unwrap();
    assert_eq!(series.len(), 1);
    series[0].points.iter().map(|p| (p.t, p.v)).collect()
}

/// A project file for the cluster fixtures with `extra` fields added.
pub(super) fn cluster_project(extra: serde_json::Value) -> String {
    let root = fixtures();
    let mut project = serde_json::json!({
        "format": "signal-loom",
        "version": 1,
        "logPath": root.join("cluster_drive.slog"),
        "signalMapPath": root.join("cluster.map.json"),
        "view": { "playheadUs": 0, "spanUs": 1_000_000, "plotted": [] },
    });
    project
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    project.to_string()
}
