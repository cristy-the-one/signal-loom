use crate::map::SignalMap;
use crate::session::Session;
use std::ops::Deref;
use std::path::{Path, PathBuf};

pub(crate) fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// hypercar_lap.slog and hypercar_lap.dbc are generated, not tracked.
pub(crate) fn lap_fixture(name: &str) -> PathBuf {
    let path = fixtures_dir().join(name);
    assert!(
        path.is_file(),
        "{} is missing: run python3 scripts/gen_fixture.py",
        path.display()
    );
    path
}

/// The sample drive with its shipped signal map.
pub(crate) fn open_cluster_drive() -> Session {
    let root = fixtures_dir();
    let mut session = Session::new();
    session
        .open_path(&root.join("cluster_drive.slog"))
        .expect("sample log");
    // Sibling map is cluster_drive.map.json, which does not exist; load the shipped map.
    session
        .open_map_path(&root.join("cluster.map.json"))
        .unwrap();
    session
}

/// A scratch directory under the system temp dir, removed when dropped, even
/// if the test panics.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("loom-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn tiny_map() -> SignalMap {
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

/// Parses the hex text of a payload into a zeroed 64-byte frame.
pub(crate) fn hex_payload_bytes(text: &str) -> [u8; 64] {
    let mut data = [0u8; 64];
    let pairs = text.len() / 2;
    for i in 0..pairs.min(64) {
        data[i] = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap();
    }
    data
}
