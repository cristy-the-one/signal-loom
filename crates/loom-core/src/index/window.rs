use super::query::Series;
use super::replay::{Phase, Step};
use super::signal::{decode_one, SignalFilter};
use super::time::ordered_range;
use super::IndexedLog;
use crate::error::{Error, Result};

/// Full-resolution reads for statistics and export. 16 bytes per sample.
const MAX_WINDOW_SAMPLES: usize = 4_000_000;

impl IndexedLog {
    /// Every sample of `names` in the window, each series led by the value held
    /// at `t0`. Unlike `query`, nothing is bucketed, so statistics and exports
    /// see each sample. A window over `MAX_WINDOW_SAMPLES` is refused, not cut.
    pub fn samples(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<Vec<Series>> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut wanted = Vec::with_capacity(names.len());
        let mut series = Vec::with_capacity(names.len());
        for name in names {
            let idx = self.require_signal(name)?;
            wanted.push(idx);
            series.push(Series {
                name: name.clone(),
                unit: self.signals[idx].unit.clone(),
                points: Vec::new(),
            });
        }
        if self.checkpoints.is_empty() {
            return Ok(series);
        }
        let filter = SignalFilter::new(&self.signals, &wanted);
        let mut total = 0usize;
        self.replay_wanted(t0, t1, &filter, |step, held| {
            let Step::Record(rec) = step else {
                for (out, value) in series.iter_mut().zip(held.iter().copied()) {
                    if let Some(value) = value {
                        out.points.push((t0, value));
                    }
                }
                return true;
            };
            self.touch_wanted(rec, &filter, held, |slot, value| {
                let points = &mut series[slot].points;
                if points.last().is_some_and(|(t, _)| *t == rec.t_us) {
                    points.pop();
                }
                points.push((rec.t_us, value));
                total += 1;
            });
            total <= MAX_WINDOW_SAMPLES
        })?;
        if total > MAX_WINDOW_SAMPLES {
            return Err(Error::invalid(format!(
                "that window holds more than {MAX_WINDOW_SAMPLES} samples. Narrow it and try again"
            )));
        }
        Ok(series)
    }

    pub fn stats(&self, name: &str, t0_us: u64, t1_us: u64) -> Result<crate::dto::WindowStats> {
        let idx = self.require_signal(name)?;
        if self.checkpoints.is_empty() {
            return Err(Error::invalid(format!(
                "no samples of {name} in that window"
            )));
        }
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut count = 0u64;
        let mut sum = 0.0f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut first = None;
        let mut last = None;
        self.scan_window(t0, t1, |phase, rec| {
            if phase == Phase::Window {
                if let Some(value) = decode_one(&self.signals, &self.msg_index, rec, idx) {
                    count += 1;
                    sum += value;
                    min = min.min(value);
                    max = max.max(value);
                    if first.is_none() {
                        first = Some(value);
                    }
                    last = Some(value);
                }
            }
            true
        })?;
        if count == 0 {
            return Err(Error::invalid(format!(
                "no samples of {name} in that window"
            )));
        }
        Ok(crate::dto::WindowStats {
            count,
            min,
            max,
            avg: sum / count as f64,
            first: first.unwrap_or(0.0),
            last: last.unwrap_or(0.0),
        })
    }
}
