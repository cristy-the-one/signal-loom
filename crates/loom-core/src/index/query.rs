use super::replay::Step;
use super::time::ordered_range;
use super::IndexedLog;
use crate::error::Result;

const MAX_QUERY_POINTS: usize = 8_000;

#[derive(Debug, Clone)]
pub struct Series {
    pub name: String,
    pub unit: String,
    pub points: Vec<(u64, f64)>,
}

#[derive(Debug, Clone)]
pub struct HeldValue {
    pub name: String,
    pub unit: String,
    pub value: f64,
    pub label: Option<String>,
}

pub struct QueryWindow {
    pub t0_us: u64,
    pub t1_us: u64,
    pub signals: Vec<String>,
    pub max_points: usize,
}

impl IndexedLog {
    pub fn query(&self, query: &QueryWindow) -> Result<Vec<Series>> {
        if self.checkpoints.is_empty() {
            return Ok(Vec::new());
        }
        let (t0, t1) = ordered_range(query.t0_us, query.t1_us);
        let max_points = query.max_points.clamp(2, MAX_QUERY_POINTS);
        let wanted: Vec<usize> = if query.signals.is_empty() {
            (0..self.signals.len()).collect()
        } else {
            query
                .signals
                .iter()
                .filter_map(|name| self.resolve_signal(name))
                .collect()
        };
        let mut buckets: Vec<Vec<Option<Bucket>>> = vec![Vec::new(); wanted.len()];
        let mut lead = vec![None; wanted.len()];
        let mut slot_of: Vec<Option<usize>> = vec![None; self.signals.len()];
        for (slot, &signal) in wanted.iter().enumerate() {
            slot_of[signal] = Some(slot);
        }

        self.replay(t0, t1, |step, held| {
            let Step::Record(rec) = step else {
                capture_lead(held, &wanted, &mut lead);
                return true;
            };
            self.touch(rec, held, |signal, value| {
                if let Some(slot) = slot_of.get(signal).copied().flatten() {
                    let row = &mut buckets[slot];
                    if row.is_empty() {
                        row.resize(bucket_count(max_points, lead[slot].is_some()), None);
                    }
                    push_bucket(row, t0, t1, rec.t_us, value);
                }
            });
            true
        })?;

        let mut series = Vec::with_capacity(wanted.len());
        for (slot, &signal_i) in wanted.iter().enumerate() {
            let signal = &self.signals[signal_i];
            let mut points = Vec::new();
            for bucket in buckets[slot].iter().flatten() {
                bucket.emit(&mut points);
            }
            if let Some(value) = lead[slot] {
                let replace = match points.first() {
                    None => true,
                    Some((t, existing)) => {
                        *t > t0 || (*t == t0 && (*existing - value).abs() > 1e-9)
                    }
                };
                if replace {
                    // Only a budget of two forces a choice: the lead and one bucket point.
                    // The earlier point gives way so the series still ends on the latest one.
                    let excess = (points.len() + 1).saturating_sub(max_points);
                    points.drain(..excess);
                    points.insert(0, (t0, value));
                }
            }
            points.truncate(max_points);
            series.push(Series {
                name: signal.name.clone(),
                unit: signal.unit.clone(),
                points,
            });
        }
        Ok(series)
    }

    pub fn values_at(&self, t_us: u64) -> Result<Vec<HeldValue>> {
        if self.checkpoints.is_empty() {
            return Ok(Vec::new());
        }
        let held = self.replay(t_us, t_us, |step, held| {
            if let Step::Record(rec) = step {
                self.touch(rec, held, |_, _| {});
            }
            true
        })?;
        Ok(self
            .signals
            .iter()
            .enumerate()
            .filter_map(|(i, signal)| {
                held.get(i).copied().flatten().map(|value| HeldValue {
                    name: signal.name.clone(),
                    unit: signal.unit.clone(),
                    value,
                    label: signal
                        .spec
                        .and_then(|spec| spec.raw_of(value))
                        .and_then(|raw| {
                            signal
                                .table
                                .iter()
                                .find_map(|(key, text)| (*key == raw).then(|| text.clone()))
                        }),
                })
            })
            .collect())
    }

    /// An upper bound on the frames a replay of `[t0, t1]` reads, from the
    /// checkpoint index alone: a replay starts at the checkpoint at or before
    /// `t0` and stops at the first frame after `t1`, which comes no later than
    /// the next checkpoint.
    pub(crate) fn records_in_window(&self, t0_us: u64, t1_us: u64) -> u64 {
        if self.checkpoints.is_empty() {
            return 0;
        }
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let start = self.checkpoints[self.floor_checkpoint(t0)].frame_ordinal;
        let end = match self.checkpoints.get(self.floor_checkpoint(t1) + 1) {
            Some(next) => next.frame_ordinal,
            None => self.frame_count,
        };
        end.saturating_sub(start)
    }
}

fn capture_lead(held: &[Option<f64>], wanted: &[usize], lead: &mut [Option<f64>]) {
    for (slot, &signal_i) in wanted.iter().enumerate() {
        lead[slot] = held.get(signal_i).copied().flatten();
    }
}

#[derive(Clone)]
struct Bucket {
    min_t: u64,
    min_v: f64,
    max_t: u64,
    max_v: f64,
}

impl Bucket {
    fn emit(&self, out: &mut Vec<(u64, f64)>) {
        if self.min_t == self.max_t {
            out.push((self.min_t, self.min_v));
        } else if self.min_t < self.max_t {
            out.push((self.min_t, self.min_v));
            out.push((self.max_t, self.max_v));
        } else {
            out.push((self.max_t, self.max_v));
            out.push((self.min_t, self.min_v));
        }
    }
}

/// Buckets for one signal. Each bucket emits up to two points and a lead point
/// takes one more, so with a lead the budget loses a slot before it is halved.
fn bucket_count(max_points: usize, has_lead: bool) -> usize {
    let budget = if has_lead { max_points - 1 } else { max_points };
    (budget / 2).max(1)
}

fn push_bucket(buckets: &mut [Option<Bucket>], t0: u64, t1: u64, t: u64, value: f64) {
    let bucket_count = buckets.len();
    let span = t1.saturating_sub(t0).max(1);
    let mut index = ((u128::from(t.saturating_sub(t0)) * u128::from(bucket_count as u64))
        / u128::from(span)) as usize;
    if index >= bucket_count {
        index = bucket_count - 1;
    }
    match &mut buckets[index] {
        None => {
            buckets[index] = Some(Bucket {
                min_t: t,
                min_v: value,
                max_t: t,
                max_v: value,
            });
        }
        Some(bucket) => {
            if value < bucket.min_v || ((value - bucket.min_v).abs() < 1e-15 && t < bucket.min_t) {
                bucket.min_v = value;
                bucket.min_t = t;
            }
            if value > bucket.max_v || ((value - bucket.max_v).abs() < 1e-15 && t > bucket.max_t) {
                bucket.max_v = value;
                bucket.max_t = t;
            }
        }
    }
}

/// Reduce a time-sorted series to at most `max_points` with the min/max
/// buckets and lead point `query` gives a physical series. A first point at the
/// window start takes the lead's place, whether it is the value held there or
/// a sample taken exactly at the start. Points must
/// lie in the window with strictly rising times, as `samples` and
/// `Compiled::eval_series` produce them.
pub(crate) fn decimate_points(
    points: Vec<(u64, f64)>,
    t0_us: u64,
    t1_us: u64,
    max_points: usize,
) -> Vec<(u64, f64)> {
    let (t0, t1) = ordered_range(t0_us, t1_us);
    let max_points = max_points.clamp(2, MAX_QUERY_POINTS);
    let lead = points.first().filter(|(t, _)| *t == t0).copied();
    let mut buckets = vec![None; bucket_count(max_points, lead.is_some())];
    for &(t, value) in &points[usize::from(lead.is_some())..] {
        push_bucket(&mut buckets, t0, t1, t, value);
    }
    let mut out = Vec::new();
    for bucket in buckets.iter().flatten() {
        bucket.emit(&mut out);
    }
    if let Some(lead) = lead {
        let excess = (out.len() + 1).saturating_sub(max_points);
        out.drain(..excess);
        out.insert(0, lead);
    }
    out.truncate(max_points);
    out
}

#[cfg(test)]
mod tests {
    use super::super::testing::ramp_log;
    use super::*;

    fn speed_points(log: &IndexedLog, t0_us: u64, max_points: usize) -> Vec<(u64, f64)> {
        let series = log
            .query(&QueryWindow {
                t0_us,
                t1_us: 1_000_000,
                signals: vec!["Speed".into()],
                max_points,
            })
            .unwrap();
        series[0].points.clone()
    }

    #[test]
    fn dense_window_keeps_last_sample_for_even_budgets() {
        let log = ramp_log();
        for max_points in (2..=64).step_by(2) {
            let points = speed_points(&log, 300_500, max_points);
            assert!(points.len() <= max_points, "{max_points}: {points:?}");
            assert_eq!(points.first(), Some(&(300_500, 300.0)), "{max_points}");
            assert_eq!(points.last(), Some(&(1_000_000, 1000.0)), "{max_points}");
        }
    }

    #[test]
    fn points_never_exceed_max_points() {
        let log = ramp_log();
        for max_points in 0..=40 {
            for t0_us in [0, 300_500] {
                let points = speed_points(&log, t0_us, max_points);
                assert!(
                    points.len() <= max_points.max(2),
                    "{max_points} at {t0_us}: {points:?}"
                );
                assert_eq!(points.last(), Some(&(1_000_000, 1000.0)));
            }
        }
    }

    #[test]
    fn signal_without_samples_in_window_keeps_its_lead_point() {
        let text = "t_us,signal,value,unit\n0,Speed,7,km/h\n0,RPM,800,rpm\n2000,Speed,9,km/h\n5000,Oil,30,kPa\n";
        let log = IndexedLog::open_bytes(text.as_bytes().to_vec(), None).unwrap();
        let series = log
            .query(&QueryWindow {
                t0_us: 1000,
                t1_us: 3000,
                signals: vec!["Speed".into(), "RPM".into(), "Oil".into()],
                max_points: 10,
            })
            .unwrap();
        let points = |name: &str| {
            series
                .iter()
                .find(|s| s.name == name)
                .unwrap()
                .points
                .clone()
        };
        assert_eq!(points("Speed"), vec![(1000, 7.0), (2000, 9.0)]);
        assert_eq!(points("RPM"), vec![(1000, 800.0)]);
        assert_eq!(points("Oil"), Vec::<(u64, f64)>::new());
    }

    #[test]
    fn a_window_replay_is_bounded_by_the_checkpoints_around_it() {
        let log = ramp_log();
        assert_eq!(log.frame_count(), 1001);
        assert_eq!(log.records_in_window(0, 100_000), 256);
        assert_eq!(log.records_in_window(300_000, 600_000), 512);
        assert_eq!(log.records_in_window(600_000, 300_000), 512);
        assert_eq!(log.records_in_window(0, 1_000_000), 1001);
        assert_eq!(log.records_in_window(900_000, 2_000_000), 233);
    }
}
