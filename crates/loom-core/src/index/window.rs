use super::query::Series;
use super::replay::{Phase, Step};
use super::signal::SignalFilter;
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
                if let Some(value) = self.decode_one(rec, idx) {
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

    /// Stream a compiled math channel over `[t0_us, t1_us]` in O(1) memory,
    /// matching `Compiled::eval_series(samples(...))` without the 4M-sample cap.
    pub(crate) fn math_stats(
        &self,
        channel_name: &str,
        compiled: &crate::analyze::Compiled,
        t0_us: u64,
        t1_us: u64,
    ) -> Result<crate::dto::WindowStats> {
        let deps = compiled.dependencies();
        let (t0, t1) = ordered_range(t0_us, t1_us);
        if deps.is_empty() {
            let mut eval_state = crate::analyze::EvalState::default();
            if !self.checkpoints.is_empty() {
                if let Some(value) = compiled.eval_at(t0, &[], &mut eval_state) {
                    return Ok(crate::dto::WindowStats {
                        count: 1,
                        min: value,
                        max: value,
                        avg: value,
                        first: value,
                        last: value,
                    });
                }
            }
            return Err(Error::invalid(format!(
                "no samples of {channel_name} in that window"
            )));
        }
        let mut wanted = Vec::with_capacity(deps.len());
        for dep in deps {
            wanted.push(self.require_signal(dep)?);
        }
        if self.checkpoints.is_empty() {
            return Err(Error::invalid(format!(
                "no samples of {channel_name} in that window"
            )));
        }
        let filter = SignalFilter::new(&self.signals, &wanted);
        let mut vars = vec![0.0; deps.len()];
        let mut ready = vec![false; deps.len()];
        let mut ready_count = 0usize;
        let mut eval_state = crate::analyze::EvalState::default();
        let mut count = 0u64;
        let mut sum = 0.0f64;
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut first = None;
        let mut last = None;

        let mut push_eval = |compiled: &crate::analyze::Compiled, t_us: u64, vars: &[f64]| {
            if let Some(value) = compiled.eval_at(t_us, vars, &mut eval_state) {
                count += 1;
                sum += value;
                min = min.min(value);
                max = max.max(value);
                if first.is_none() {
                    first = Some(value);
                }
                last = Some(value);
            }
        };

        let mut cur_t: Option<u64> = None;
        let mut cur_dirty = false;
        let mut had_t0_lead = false;

        self.replay_wanted(t0, t1, &filter, |step, held| {
            match step {
                Step::Start => {
                    for (slot, value) in held.iter().copied().enumerate() {
                        if let Some(v) = value {
                            vars[slot] = v;
                            ready[slot] = true;
                            ready_count += 1;
                        }
                    }
                    had_t0_lead = ready_count > 0;
                }
                Step::Record(rec) => {
                    let mut updated = false;
                    self.decode_wanted(rec, &filter, |_slot, _value| {
                        updated = true;
                    });
                    if !updated {
                        return true;
                    }
                    if let Some(prev_t) = cur_t {
                        if rec.t_us > prev_t {
                            if cur_dirty && ready_count == deps.len() {
                                push_eval(compiled, prev_t, &vars);
                            }
                            cur_t = Some(rec.t_us);
                            cur_dirty = false;
                        }
                    } else if had_t0_lead && rec.t_us > t0 {
                        // Emit the t0 lead step before advancing past t0.
                        if ready_count == deps.len() {
                            push_eval(compiled, t0, &vars);
                        }
                        cur_t = Some(rec.t_us);
                    } else {
                        cur_t = Some(rec.t_us);
                    }
                    self.touch_wanted(rec, &filter, held, |slot, value| {
                        if !ready[slot] {
                            ready[slot] = true;
                            ready_count += 1;
                        }
                        vars[slot] = value;
                        cur_dirty = true;
                    });
                }
            }
            true
        })?;

        if let Some(t) = cur_t {
            if cur_dirty && ready_count == deps.len() {
                push_eval(compiled, t, &vars);
            }
        } else if had_t0_lead && ready_count == deps.len() {
            push_eval(compiled, t0, &vars);
        }

        if count == 0 {
            return Err(Error::invalid(format!(
                "no samples of {channel_name} in that window"
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
