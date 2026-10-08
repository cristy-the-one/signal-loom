//! The built-in cluster sample, and where to look for the fixtures on disk.

use crate::error::Result;
use crate::map::SignalMap;
use std::path::PathBuf;

const SLOG: &str = include_str!("../../../../fixtures/cluster_drive.slog");
const MAP: &str = include_str!("../../../../fixtures/cluster.map.json");
pub(super) const LOG_NAME: &str = "cluster_drive.slog";
pub(super) const MAP_NAME: &str = "cluster.map.json";

pub(super) fn map() -> Result<SignalMap> {
    SignalMap::parse(MAP)
}

pub(super) fn log_bytes() -> Vec<u8> {
    SLOG.as_bytes().to_vec()
}

/// The name the sample map carries when it is not read from disk.
pub(super) fn embedded_map_path() -> PathBuf {
    PathBuf::from(format!("fixtures/{MAP_NAME}"))
}

/// The name the sample log carries when it is not read from disk.
pub(super) fn embedded_log_path() -> PathBuf {
    PathBuf::from(format!("fixtures/{LOG_NAME}"))
}

/// The long hypercar lap, when the repository's fixtures are around.
pub(super) fn hypercar_log() -> Option<PathBuf> {
    find_up("fixtures/hypercar_lap.slog")
}

/// The sample map on disk, or its embedded name.
pub(super) fn map_path() -> PathBuf {
    find_up(&format!("fixtures/{MAP_NAME}")).unwrap_or_else(embedded_map_path)
}

/// The sample log on disk. `None` means the embedded copy is all there is.
pub(super) fn log_file() -> Option<PathBuf> {
    find_up(&format!("fixtures/{LOG_NAME}"))
}

fn find_up(relative: &str) -> Option<PathBuf> {
    let mut starts = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd);
    }
    starts.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    for start in starts {
        let mut dir = Some(start.as_path());
        while let Some(current) = dir {
            let candidate = current.join(relative);
            if candidate.is_file() {
                return Some(candidate);
            }
            dir = current.parent();
        }
    }
    None
}
