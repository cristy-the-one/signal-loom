//! The judge's evaluator: plan `docs/plan-judging-2026-11.md` §4.
//!
//! One replay over the whole log collects every sample of the signals a
//! request names, and the frame times and channels of the messages its
//! `expect` conditions live on. Everything after that is in memory, and each
//! anchor finds its window by binary search. The windowed `query` path is not
//! used: it decimates to a point budget, and a judgement needs every sample.
//!
//! Interpretations the plan leaves open, fixed here and covered by tests:
//!
//! - The window start is evaluated on the value held there. When samples sit
//!   exactly on the start, each of them is evaluated, as every sample inside
//!   the window is; the value held from before is not, since they replace it.
//!   An evaluation of the held value is reported at the window start.
//! - Silence counts while it lasts. A message that went silent before the
//!   window opened and is still silent inside it is an integrity problem at
//!   the window start. The stretch after a message's last frame, up to the end
//!   of the recording, is silence too.
//! - Silence is a gap longer than the timeout threshold, compared exactly as
//!   the indexer's timeout check compares it; its moment is the last frame
//!   plus the threshold, rounded up to a whole microsecond.
//! - A frame counts for a signal's silence only on a channel the signal is
//!   decoded on: the same rule as `decode_frame`.
//! - `always` and `never` cannot pass with an unknown start: an unseen stretch
//!   at the head of the window could hold the very value they rule out. That
//!   is `no_value`, as for a start with no sample at all.
//! - No evaluation is made after the recording ends.

use super::integrity::IntegrityKind;
use super::IndexedLog;
use crate::error::{Error, Result};
use crate::judge::{Check, Condition, Mode, Op, Request, SignalRef, Value};
use crate::map::{MappedSignal, SignalMap, TimeoutFactor};
use crate::scan::RecKind;
use std::collections::{BTreeMap, BTreeSet};

/// A verdict on an occurrence, a check or a case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
    /// No anchor survived `given`.
    NotExercised,
    /// A case with no checks.
    NotJudgeable,
}

impl Verdict {
    /// `fail` > `inconclusive` > `not_exercised` > `pass` (plan §4.3).
    fn rank(self) -> u8 {
        match self {
            Self::Fail => 4,
            Self::Inconclusive => 3,
            Self::NotExercised => 2,
            Self::NotJudgeable => 1,
            Self::Pass => 0,
        }
    }
}

/// Why an occurrence could not be decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    GivenUnknown,
    WindowPastEnd,
    NoValue,
    Integrity,
    /// The log's event list was truncated, so a pass cannot be confirmed.
    IntegrityUnknown,
}

/// The kind of integrity problem behind a `Reason::Integrity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Timeout,
    Checksum,
    Counter,
}

/// What an occurrence's verdict rests on.
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    /// The evaluation that decided it: its time and the value held then.
    Sample { t_us: u64, value: f64 },
    /// Why it was not decided. For `Integrity`, the problem's time and kind.
    Reason {
        reason: Reason,
        event: Option<(u64, EventKind)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Occurrence {
    pub anchor_us: u64,
    pub verdict: Verdict,
    pub evidence: Evidence,
    /// Evidence, not a verdict. True only for an `eventually` pass whose
    /// expect already held just before the anchor.
    pub already_held: bool,
}

/// One check. `occurrences` holds every judged anchor, in time order; skipped
/// anchors are counted, not listed. The verdict file caps the list (SL-14).
#[derive(Debug, Clone, PartialEq)]
pub struct CheckOutcome {
    pub id: String,
    pub verdict: Verdict,
    pub anchors: u64,
    pub skipped: u64,
    pub pass: u64,
    pub fail: u64,
    pub inconclusive: u64,
    /// How many of `pass` are `already_held`. Evidence, not a verdict.
    pub pass_already_held: u64,
    pub occurrences: Vec<Occurrence>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseOutcome {
    pub case_id: String,
    pub verdict: Verdict,
    pub checks: Vec<CheckOutcome>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Evaluation {
    /// False when the log's event list was truncated (plan §4.3).
    pub integrity_complete: bool,
    pub cases: Vec<CaseOutcome>,
}

/// Judge `request`, already accepted by `judge::check` against `map`, on `log`.
/// `timeout` must be the factor `log` was indexed with: silence uses the same
/// threshold as the timeout events.
pub(crate) fn evaluate(
    log: &IndexedLog,
    map: &SignalMap,
    timeout: TimeoutFactor,
    request: &Request,
) -> Result<Evaluation> {
    let plan = Plan::from_map(map, timeout, request)?;
    for target in plan.targets.values() {
        if log.signals.get(target.idx).map(|s| s.name.as_str()) != Some(target.name.as_str()) {
            return Err(Error::not_found(format!(
                "signal {} is not in the indexed log",
                target.name
            )));
        }
    }
    let data = plan.collect(log)?;
    evaluate_with_data(log, &plan, &data, request)
}

/// Index `path` and collect `request`'s samples and cyclic frame timestamps in
/// the same pass so `loom-judge` does not scan a multi-GB recording twice.
pub(crate) fn evaluate_path(
    path: &std::path::Path,
    map: &SignalMap,
    timeout: TimeoutFactor,
    request: &Request,
) -> Result<(IndexedLog, Evaluation)> {
    let plan = Plan::from_map(map, timeout, request)?;
    let format = super::source::sniff_path(path)?;
    if format == crate::scan::LogFormat::DecodedCsv {
        let log = IndexedLog::open_path_timed(path, Some(map), timeout, None)?;
        let eval = evaluate(&log, map, timeout, request)?;
        return Ok((log, eval));
    }
    let wanted: Vec<usize> = plan
        .targets
        .values()
        .map(|target| target.idx)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut slot_of = vec![usize::MAX; map.signals.len()];
    for (slot, &idx) in wanted.iter().enumerate() {
        slot_of[idx] = slot;
    }
    let mut rows: Vec<Vec<(u64, f64)>> = vec![Vec::new(); wanted.len()];
    let mut frames: BTreeMap<u32, Vec<FrameStamp>> = plan
        .thresholds
        .iter()
        .filter_map(|(&id, threshold)| threshold.map(|_| (id, Vec::new())))
        .collect();
    let mut tap = |t_us: u64, id: u32, channel: u8, decoded: &[(usize, f64)]| {
        if let Some(times) = frames.get_mut(&id) {
            times.push(FrameStamp::pack(t_us, channel));
        }
        for &(idx, value) in decoded {
            if let Some(&slot) = slot_of.get(idx).filter(|&&s| s != usize::MAX) {
                rows[slot].push((t_us, value));
            }
        }
    };
    let log = IndexedLog::build_with_tap(
        super::source::Source::Path(path.to_path_buf()),
        format,
        Some(map),
        timeout,
        None,
        Some(&mut tap),
    )?;
    let data = Data {
        series: wanted.into_iter().zip(rows).collect(),
        frames,
    };
    let evaluation = evaluate_with_data(&log, &plan, &data, request)?;
    Ok((log, evaluation))
}

fn evaluate_with_data(
    log: &IndexedLog,
    plan: &Plan,
    data: &Data,
    request: &Request,
) -> Result<Evaluation> {
    let integrity_complete = !log.events_truncated();
    let ctx = Ctx {
        log,
        data,
        t_end_us: log.t_end_us(),
        integrity_complete,
    };
    let mut cases = Vec::with_capacity(request.cases.len());
    for case in &request.cases {
        let mut checks = Vec::with_capacity(case.checks.len());
        for check in &case.checks {
            checks.push(ctx.check(check, plan)?);
        }
        let verdict = if checks.is_empty() {
            Verdict::NotJudgeable
        } else {
            checks
                .iter()
                .map(|check| check.verdict)
                .max_by_key(|verdict| verdict.rank())
                .unwrap_or(Verdict::Pass)
        };
        cases.push(CaseOutcome {
            case_id: case.case_id.clone(),
            verdict,
            checks,
        });
    }
    Ok(Evaluation {
        integrity_complete,
        cases,
    })
}

// ---- resolving what the request names --------------------------------------

/// A resolved signal: its index in the map/log and what the evaluator needs of it.
#[derive(Clone)]
struct Target {
    idx: usize,
    name: String,
    message_id: u32,
    /// 0 is decoded on every channel.
    channel: u8,
    /// Half the factor: the default `==` tolerance (plan §3.1).
    half_step: f64,
    factor: f64,
    offset: f64,
    table: Vec<(i64, String)>,
}

impl Target {
    /// `decode_frame`'s rule: a frame on `channel` carries this signal.
    fn on_channel(&self, channel: u8) -> bool {
        self.channel == 0 || channel == 0 || self.channel == channel
    }
}

struct Plan {
    targets: BTreeMap<SignalKey, Target>,
    /// Silence threshold per message an `expect` lives on, in µs. A message
    /// without a cycle time has no threshold and is never silent.
    thresholds: BTreeMap<u32, Option<f64>>,
}

type SignalKey = (String, String, Option<i64>);

fn key(signal: &SignalRef) -> SignalKey {
    (
        signal.message.clone(),
        signal.signal.clone(),
        signal.channel,
    )
}

impl Plan {
    fn from_map(map: &SignalMap, timeout: TimeoutFactor, request: &Request) -> Result<Self> {
        let mut targets = BTreeMap::new();
        let mut thresholds = BTreeMap::new();
        for case in &request.cases {
            for check in &case.checks {
                for condition in check.given.iter().chain([&check.when, &check.expect]) {
                    let signal = &condition.signal;
                    if targets.contains_key(&key(signal)) {
                        continue;
                    }
                    let (idx, mapped) = resolve(map, signal)?;
                    targets.insert(
                        key(signal),
                        Target {
                            idx,
                            name: mapped.name.clone(),
                            message_id: mapped.message_id,
                            channel: mapped.channel,
                            half_step: mapped.spec.factor.abs() / 2.0,
                            factor: mapped.spec.factor,
                            offset: mapped.spec.offset,
                            table: mapped.table.clone(),
                        },
                    );
                }
                let expect = &targets[&key(&check.expect.signal)];
                let cycle = map
                    .messages
                    .iter()
                    .find(|message| message.id == expect.message_id)
                    .and_then(|message| message.cycle_us)
                    .filter(|cycle| *cycle > 0);
                thresholds.insert(
                    expect.message_id,
                    cycle.map(|cycle| cycle as f64 * timeout.get()),
                );
            }
        }
        Ok(Self {
            targets,
            thresholds,
        })
    }

    fn target(&self, signal: &SignalRef) -> &Target {
        &self.targets[&key(signal)]
    }

    /// One pass over an already-indexed log, decoding only the signals the
    /// request names and recording frame timestamps only for cyclic `expect` messages.
    fn collect(&self, log: &IndexedLog) -> Result<Data> {
        let wanted: Vec<usize> = self
            .targets
            .values()
            .map(|target| target.idx)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let filter = super::signal::SignalFilter::new(&log.signals, &wanted);
        let mut rows: Vec<Vec<(u64, f64)>> = vec![Vec::new(); wanted.len()];
        let mut frames: BTreeMap<u32, Vec<FrameStamp>> = self
            .thresholds
            .iter()
            .filter_map(|(&id, threshold)| threshold.map(|_| (id, Vec::new())))
            .collect();
        if !log.checkpoints.is_empty() && (!wanted.is_empty() || !frames.is_empty()) {
            log.scan_from(0, |rec| {
                if let RecKind::Frame { id, channel, .. } = &rec.kind {
                    if let Some(times) = frames.get_mut(id) {
                        times.push(FrameStamp::pack(rec.t_us, *channel));
                    }
                }
                log.decode_wanted(rec, &filter, |slot, value| {
                    rows[slot].push((rec.t_us, value));
                });
                true
            })?;
        }
        let series = wanted.into_iter().zip(rows).collect();
        Ok(Data { series, frames })
    }
}

/// The signal a reference names (with its index in `map.signals`), using the
/// channel rule of plan §3.1: a named channel first, then channel 0.
fn resolve<'a>(map: &'a SignalMap, signal: &SignalRef) -> Result<(usize, &'a MappedSignal)> {
    let pairs = |channel: Option<u8>| -> Vec<(usize, &'a MappedSignal)> {
        map.signals
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                candidate.message_name == signal.message
                    && candidate.defined_name == signal.signal
                    && channel.is_none_or(|channel| candidate.channel == channel)
            })
            .collect()
    };
    let found = match signal.channel {
        None => pairs(None),
        Some(channel) => {
            let channel = u8::try_from(channel)
                .map_err(|_| Error::invalid(format!("channel {channel} is not a CAN channel")))?;
            let named = pairs(Some(channel));
            if named.is_empty() && channel != 0 {
                pairs(Some(0))
            } else {
                named
            }
        }
    };
    match found.as_slice() {
        [one] => Ok(*one),
        [] => Err(Error::not_found(crate::map::unknown_pair_message(
            &signal.message,
            &signal.signal,
        ))),
        _ => Err(Error::invalid(format!(
            "signal pair ({}, {}) is ambiguous",
            signal.message, signal.signal
        ))),
    }
}

// ---- evaluation ----------------------------------------------------------------

/// Compact 8-byte `(t_us, channel)` stamp (56-bit µs covers > 2,280 years).
#[derive(Clone, Copy)]
struct FrameStamp(u64);

impl FrameStamp {
    const TIME_MASK: u64 = (1u64 << 56) - 1;

    #[inline]
    fn pack(t_us: u64, channel: u8) -> Self {
        Self((t_us.min(Self::TIME_MASK)) | (u64::from(channel) << 56))
    }

    #[inline]
    fn unpack(self) -> (u64, u8) {
        (self.0 & Self::TIME_MASK, (self.0 >> 56) as u8)
    }
}

struct Data {
    series: BTreeMap<usize, Vec<(u64, f64)>>,
    /// Packed frame time and channel, per cyclic `expect` message id.
    frames: BTreeMap<u32, Vec<FrameStamp>>,
}

struct Ctx<'a> {
    log: &'a IndexedLog,
    data: &'a Data,
    t_end_us: u64,
    integrity_complete: bool,
}

/// A condition with its signal's samples and its value resolved to a number.
struct Bound<'a> {
    points: &'a [(u64, f64)],
    op: Op,
    value: f64,
    tolerance: f64,
}

impl Bound<'_> {
    fn holds(&self, x: f64) -> bool {
        match self.op {
            Op::Eq => (x - self.value).abs() <= self.tolerance,
            Op::Ne => (x - self.value).abs() > self.tolerance,
            Op::Gt => x > self.value,
            Op::Ge => x >= self.value,
            Op::Lt => x < self.value,
            Op::Le => x <= self.value,
        }
    }

    /// The last sample at or before `t`.
    fn held_at(&self, t: u64) -> Option<f64> {
        let after = self.points.partition_point(|(ts, _)| *ts <= t);
        after.checked_sub(1).map(|i| self.points[i].1)
    }

    /// The last sample strictly before `t`.
    fn held_before(&self, t: u64) -> Option<f64> {
        let after = self.points.partition_point(|(ts, _)| *ts < t);
        after.checked_sub(1).map(|i| self.points[i].1)
    }

    /// Samples with `from <= t <= to`.
    fn within(&self, from: u64, to: u64) -> &[(u64, f64)] {
        let lo = self.points.partition_point(|(ts, _)| *ts < from);
        let hi = self.points.partition_point(|(ts, _)| *ts <= to);
        &self.points[lo..hi.max(lo)]
    }
}

/// Integrity problems on one `expect` signal, each list in time order.
struct Problems {
    /// `(from, until)`: silent from `from`, until the next frame or the end.
    silences: Vec<(u64, u64)>,
    /// Checksum and counter events on the signal.
    events: Vec<(u64, EventKind)>,
}

impl Problems {
    /// The first problem inside `[start, end]`, with the time it applies from.
    fn first_in(&self, start: u64, end: u64) -> Option<(u64, EventKind)> {
        let open = self.silences.partition_point(|(_, until)| *until <= start);
        let silence = self.silences[open..]
            .first()
            .filter(|(from, _)| *from <= end)
            .map(|(from, _)| ((*from).max(start), EventKind::Timeout));
        let next = self.events.partition_point(|(t, _)| *t < start);
        let event = self.events[next..]
            .first()
            .filter(|(t, _)| *t <= end)
            .copied();
        match (silence, event) {
            (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
            (a, b) => a.or(b),
        }
    }
}

impl Ctx<'_> {
    fn bind<'s>(&'s self, plan: &Plan, condition: &Condition) -> Result<Bound<'s>> {
        let target = plan.target(&condition.signal);
        let value = match &condition.value {
            Value::Number(number) => *number,
            Value::Label(label) => {
                let raw = target
                    .table
                    .iter()
                    .find(|(_, text)| text == label)
                    .map(|(raw, _)| *raw)
                    .ok_or_else(|| {
                        Error::invalid(format!(
                            "value \"{label}\" is not a label of signal ({}, {})",
                            condition.signal.message, condition.signal.signal
                        ))
                    })?;
                raw as f64 * target.factor + target.offset
            }
        };
        Ok(Bound {
            points: &self.data.series[&target.idx],
            op: condition.op,
            value,
            tolerance: condition.tolerance.unwrap_or(target.half_step),
        })
    }

    /// Silences and events of `target`, computed once per check.
    fn problems(&self, target: &Target, threshold: Option<f64>) -> Problems {
        let mut silences = Vec::new();
        if let Some(threshold) = threshold {
            let step = threshold.ceil() as u64;
            let mut prev: Option<u64> = None;
            for stamp in &self.data.frames[&target.message_id] {
                let (t, channel) = stamp.unpack();
                if !target.on_channel(channel) {
                    continue;
                }
                if let Some(last) = prev {
                    // The indexer's comparison: a gap longer than the threshold.
                    if t.saturating_sub(last) as f64 > threshold {
                        silences.push((last.saturating_add(step), t));
                    }
                }
                prev = Some(t);
            }
            if let Some(last) = prev {
                if self.t_end_us.saturating_sub(last) as f64 > threshold {
                    silences.push((last.saturating_add(step), self.t_end_us));
                }
            }
        }
        let events = self
            .log
            .integrity_events_for(target.message_id, &target.name)
            .map(|event| {
                let kind = match event.kind {
                    IntegrityKind::Checksum => EventKind::Checksum,
                    IntegrityKind::Counter => EventKind::Counter,
                };
                (event.t_us, kind)
            })
            .collect();
        Problems { silences, events }
    }

    fn check(&self, check: &Check, plan: &Plan) -> Result<CheckOutcome> {
        let when = self.bind(plan, &check.when)?;
        let expect = self.bind(plan, &check.expect)?;
        let given = check
            .given
            .iter()
            .map(|condition| self.bind(plan, condition))
            .collect::<Result<Vec<_>>>()?;
        let target = plan.target(&check.expect.signal);
        let problems = self.problems(target, plan.thresholds[&target.message_id]);

        let mut outcome = CheckOutcome {
            id: check.id.clone(),
            verdict: Verdict::Pass,
            anchors: 0,
            skipped: 0,
            pass: 0,
            fail: 0,
            inconclusive: 0,
            pass_already_held: 0,
            occurrences: Vec::new(),
        };
        for anchor in anchors(&when) {
            outcome.anchors += 1;
            let mut known = true;
            let mut skip = false;
            for condition in &given {
                match condition.held_at(anchor) {
                    None => known = false,
                    Some(x) if !condition.holds(x) => skip = true,
                    Some(_) => {}
                }
            }
            if skip {
                outcome.skipped += 1;
                continue;
            }
            let (verdict, evidence) = if known {
                let (start, end) = window(anchor, check.within_ms);
                let problem = problems.first_in(start, end);
                self.occurrence(&expect, check.mode, (start, end), problem)
            } else {
                (Verdict::Inconclusive, reason(Reason::GivenUnknown))
            };
            let (verdict, evidence) = if verdict == Verdict::Pass && !self.integrity_complete {
                (Verdict::Inconclusive, reason(Reason::IntegrityUnknown))
            } else {
                (verdict, evidence)
            };
            match verdict {
                Verdict::Pass => outcome.pass += 1,
                Verdict::Fail => outcome.fail += 1,
                _ => outcome.inconclusive += 1,
            }
            // Evidence only. An eventually pass whose expect already held
            // just before the anchor did not depend on the trigger. Always
            // and never never set it, and a non-pass never does either.
            let already_held = check.mode == Mode::Eventually
                && verdict == Verdict::Pass
                && expect
                    .held_before(anchor)
                    .is_some_and(|value| expect.holds(value));
            if already_held {
                outcome.pass_already_held += 1;
            }
            outcome.occurrences.push(Occurrence {
                anchor_us: anchor,
                verdict,
                evidence,
                already_held,
            });
        }
        outcome.verdict = if outcome.fail > 0 {
            Verdict::Fail
        } else if outcome.inconclusive > 0 {
            Verdict::Inconclusive
        } else if outcome.pass == 0 {
            Verdict::NotExercised
        } else {
            Verdict::Pass
        };
        Ok(outcome)
    }

    /// Plan §4.2 for one anchor. `problem` is the first integrity problem in
    /// the window. A decision made at a time strictly before it stands.
    fn occurrence(
        &self,
        expect: &Bound<'_>,
        mode: Mode,
        (start, end): (u64, u64),
        problem: Option<(u64, EventKind)>,
    ) -> (Verdict, Evidence) {
        // Evaluations, in time order, never after the log ends: the value held
        // from before the window (unless samples sit exactly on its start),
        // then every sample from the start to the end.
        let seen = if start <= self.t_end_us {
            expect.within(start, end.min(self.t_end_us))
        } else {
            &[]
        };
        let on_start = seen.first().is_some_and(|(t, _)| *t == start);
        let before = if start <= self.t_end_us && !on_start {
            expect.held_before(start).map(|x| (start, x))
        } else {
            None
        };
        let known_start = on_start || before.is_some();
        let evaluations = before.iter().chain(seen.iter()).copied();

        // The value a decision is looking for: true for eventually and never,
        // false for always.
        let decisive = mode != Mode::Always;
        for (t, x) in evaluations {
            if expect.holds(x) != decisive {
                continue;
            }
            if let Some((at, kind)) = problem {
                if at <= t {
                    return integrity(at, kind);
                }
            }
            let verdict = if mode == Mode::Eventually {
                Verdict::Pass
            } else {
                Verdict::Fail
            };
            return (verdict, Evidence::Sample { t_us: t, value: x });
        }

        // Nothing decided early: the whole window has to be seen, from a known start.
        if end > self.t_end_us {
            return (Verdict::Inconclusive, reason(Reason::WindowPastEnd));
        }
        if !known_start && (mode != Mode::Eventually || seen.is_empty()) {
            return (Verdict::Inconclusive, reason(Reason::NoValue));
        }
        if let Some((at, kind)) = problem {
            return integrity(at, kind);
        }
        let last = expect.held_at(end).unwrap_or_default();
        let verdict = if mode == Mode::Eventually {
            Verdict::Fail
        } else {
            Verdict::Pass
        };
        (
            verdict,
            Evidence::Sample {
                t_us: end,
                value: last,
            },
        )
    }
}

/// Rising edges of `when`. The first sample is never one: there is no earlier
/// state to rise from.
fn anchors(when: &Bound<'_>) -> Vec<u64> {
    let mut found = Vec::new();
    let mut previous: Option<bool> = None;
    for (t, x) in when.points {
        let now = when.holds(*x);
        if now && previous == Some(false) {
            found.push(*t);
        }
        previous = Some(now);
    }
    found
}

fn window(anchor: u64, within_ms: [i64; 2]) -> (u64, u64) {
    let ms = |value: i64| u64::try_from(value).unwrap_or(0).saturating_mul(1000);
    (
        anchor.saturating_add(ms(within_ms[0])),
        anchor.saturating_add(ms(within_ms[1])),
    )
}

fn reason(reason: Reason) -> Evidence {
    Evidence::Reason {
        reason,
        event: None,
    }
}

fn integrity(at: u64, kind: EventKind) -> (Verdict, Evidence) {
    (
        Verdict::Inconclusive,
        Evidence::Reason {
            reason: Reason::Integrity,
            event: Some((at, kind)),
        },
    )
}
