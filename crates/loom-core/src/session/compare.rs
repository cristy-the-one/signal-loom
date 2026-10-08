//! The second drive laid over the first: its log, where it came from, and the
//! time offset between the two.

use super::Session;
use crate::dto::{Query, Summary};
use crate::error::{Error, Result};
use crate::index::{IndexControl, IndexedLog, QueryWindow, Series};
use std::path::Path;

/// The compare log, the path a project would name it by, and the offset that
/// shifts it onto the main log's clock. The path can outlive the log: a
/// project that names a file that is not there keeps naming it.
#[derive(Default)]
pub(super) struct Compare {
    log: Option<IndexedLog>,
    path: Option<String>,
    offset_us: i64,
}

impl Compare {
    /// The state a project left: its offset, and the path it names.
    pub(super) fn named(path: Option<String>, offset_us: i64) -> Self {
        Self {
            log: None,
            path,
            offset_us,
        }
    }

    pub(super) fn log(&self) -> Option<&IndexedLog> {
        self.log.as_ref()
    }

    pub(super) fn path(&self) -> Option<&str> {
        self.path.as_deref()
    }

    pub(super) fn offset_us(&self) -> i64 {
        self.offset_us
    }

    /// Use `log`, which came from `path` unless it was uploaded.
    pub(super) fn set_log(&mut self, log: IndexedLog, path: Option<String>) {
        self.log = Some(log);
        self.path = path;
    }

    /// Swap in a rebuilt index of the same log.
    pub(super) fn replace_log(&mut self, log: IndexedLog) {
        self.log = Some(log);
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The compare log's series for `signals` over the query window, shifted
    /// by the offset and named `… · B`. Empty without a compare log.
    pub(super) fn series(&self, signals: &[String], query: &Query) -> Result<Vec<Series>> {
        let Some(compare) = &self.log else {
            return Ok(Vec::new());
        };
        let offset_us = self.offset_us;
        let start = query.t0_us as i64 - offset_us;
        let end = query.t1_us as i64 - offset_us;
        if end < 0 {
            return Ok(Vec::new());
        }
        let series = compare.query(&QueryWindow {
            t0_us: start.max(0) as u64,
            t1_us: end.max(0) as u64,
            signals: signals.to_vec(),
            max_points: query.max_points,
        })?;
        Ok(series
            .into_iter()
            .map(|mut series| {
                series.name = format!("{} · B", series.name);
                for (t, _) in &mut series.points {
                    let shifted = *t as i64 + offset_us;
                    *t = shifted.max(0) as u64;
                }
                series
            })
            .collect())
    }
}

impl Session {
    pub fn open_compare_path(&mut self, path: &Path) -> Result<Summary> {
        self.open_compare_path_controlled(path, None)
    }

    /// Index `path` as the compare log without touching the current one until
    /// the scan finishes. `control` publishes progress and can cancel the scan.
    pub fn open_compare_path_controlled(
        &mut self,
        path: &Path,
        control: Option<&IndexControl>,
    ) -> Result<Summary> {
        if !path.is_file() {
            return Err(Error::not_found(format!(
                "compare log not found: {}",
                path.display()
            )));
        }
        let log = IndexedLog::open_path_timed(path, self.maps.map(), self.timeout, control)?;
        self.compare.set_log(log, Some(path.display().to_string()));
        self.summary()
    }

    pub fn open_compare_bytes(&mut self, bytes: Vec<u8>) -> Result<Summary> {
        let log = IndexedLog::open_bytes_timed(bytes, self.maps.map(), self.timeout)?;
        self.compare.set_log(log, None);
        self.summary()
    }

    pub fn set_compare_offset(&mut self, offset_us: i64) {
        self.compare.offset_us = offset_us;
    }

    pub fn clear_compare(&mut self) {
        self.compare.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{compare_values, rpm_query, RPM_LOG, RPM_MAP};
    use super::*;

    #[test]
    fn a_map_applied_later_decodes_the_compare_log_too() {
        let mut session = Session::new();
        session
            .open_bytes("main.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session
            .open_compare_bytes(RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        assert_eq!(compare_values(&session, &rpm_query()), [800.0, 800.0]);
    }

    #[test]
    fn a_timeout_change_rebuilds_the_compare_log_with_the_same_map() {
        let mut session = Session::new();
        session
            .open_bytes("main.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session
            .open_compare_bytes(RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.set_timeout_factor(4.0).unwrap();
        assert_eq!(compare_values(&session, &rpm_query()), [800.0, 800.0]);
        assert_eq!(
            session.compare.log().unwrap().frame_count(),
            session.log.as_ref().unwrap().frame_count()
        );
    }
}
