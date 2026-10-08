//! The deck: math channels and threshold triggers. Setting them, checking
//! them against the log, and turning them into series and events.

use super::{LogSlot, Session};
use crate::analyze::Compiled;
use crate::dto::{EventDto, Query, SignalDto, Summary};
use crate::error::{Error, Result};
use crate::index::{decimate_points, IndexedLog, QueryWindow, Series};
use crate::project::{compile_math, MathChannel, ThresholdTrigger};

/// `Session::query` evaluates a math channel on raw samples only when the
/// window replays at most this many frames, so a refresh stays quick on a huge
/// log. The hypercar lap fixture (about 235,000 frames, 0.2 s to replay in a
/// release build) is below it.
const MATH_RAW_RECORDS: u64 = 500_000;

/// The math channels and triggers of the open recording.
#[derive(Default)]
pub(super) struct Deck {
    math: Vec<MathChannel>,
    triggers: Vec<ThresholdTrigger>,
}

impl Deck {
    pub(super) fn math(&self) -> &[MathChannel] {
        &self.math
    }

    pub(super) fn triggers(&self) -> &[ThresholdTrigger] {
        &self.triggers
    }

    /// Take the channels and triggers a project held, already checked.
    pub(super) fn load(&mut self, math: Vec<MathChannel>, triggers: Vec<ThresholdTrigger>) {
        self.math = math;
        self.triggers = triggers;
    }

    /// A newly opened recording starts without the previous deck setup.
    pub(super) fn clear(&mut self) {
        self.math.clear();
        self.triggers.clear();
    }

    pub(super) fn set_math(&mut self, channels: Vec<MathChannel>) -> Result<()> {
        for channel in &channels {
            channel.validate(&channels)?;
        }
        self.math = channels;
        Ok(())
    }

    /// Replace the triggers with `triggers`, which must each fit `log`.
    pub(super) fn set_triggers(
        &mut self,
        log: &IndexedLog,
        triggers: Vec<ThresholdTrigger>,
    ) -> Result<()> {
        for trigger in &triggers {
            trigger.validate(Some(log), &self.math)?;
        }
        self.triggers = triggers;
        Ok(())
    }

    pub(super) fn is_math(&self, name: &str) -> bool {
        self.math.iter().any(|channel| channel.name == name)
    }

    fn math_channel(&self, name: &str) -> Option<&MathChannel> {
        self.math.iter().find(|channel| channel.name == name)
    }

    /// The math channels as the signals the summary lists.
    pub(super) fn signals(&self) -> impl Iterator<Item = SignalDto> + '_ {
        self.math.iter().map(|channel| SignalDto {
            name: channel.name.clone(),
            unit: channel.unit.clone(),
            message_name: "Math".to_string(),
            message_id: None,
            min: None,
            max: None,
            step: None,
            from_map: false,
        })
    }

    /// Split the names a query asks for into logged signals and compiled math
    /// channels, each once.
    pub(super) fn split(&self, names: &[String]) -> Result<(Vec<String>, Vec<Derived<'_>>)> {
        let mut physical = Vec::new();
        let mut derived: Vec<Derived> = Vec::new();
        for name in names {
            if let Some(channel) = self.math_channel(name) {
                if derived.iter().all(|(item, _)| item.name != *name) {
                    derived.push((channel, compile_math(&self.math, channel)?));
                }
            } else if !physical.contains(name) {
                physical.push(name.clone());
            }
        }
        Ok((physical, derived))
    }

    /// Math channels for a plot. Each is evaluated on the raw samples of its
    /// signals in the window, the same ones `stats` and the CSV export use,
    /// and the result is then bucketed like a physical series, so its values
    /// do not depend on `max_points`. When replaying the window would take
    /// more than `MATH_RAW_RECORDS` frames, or its raw samples cannot be read
    /// (see `IndexedLog::samples`), the channel is evaluated on the bucketed
    /// series of its signals instead, as that is all the plot can afford; its
    /// values then depend on the zoom.
    pub(super) fn evaluate(
        &self,
        log: &IndexedLog,
        derived: &[Derived<'_>],
        query: &Query,
    ) -> Result<Vec<Series>> {
        let replay_is_cheap = log.records_in_window(query.t0_us, query.t1_us) <= MATH_RAW_RECORDS;
        let mut series = Vec::new();
        for (channel, compiled) in derived {
            let deps = compiled.dependencies();
            let raw = replay_is_cheap
                .then(|| log.samples(deps, query.t0_us, query.t1_us))
                .and_then(Result::ok);
            let result = match raw {
                Some(raw) => {
                    let mut result = eval_channel(channel, compiled, &raw)?;
                    result.points =
                        decimate_points(result.points, query.t0_us, query.t1_us, query.max_points);
                    result
                }
                None => {
                    let bucketed = log.query(&QueryWindow {
                        t0_us: query.t0_us,
                        t1_us: query.t1_us,
                        signals: deps.to_vec(),
                        max_points: query.max_points,
                    })?;
                    eval_channel(channel, compiled, &bucketed)?
                }
            };
            series.push(result);
        }
        Ok(series)
    }

    /// The math channel `name` over the raw samples of its signals in the window.
    pub(super) fn math_series(
        &self,
        log: &IndexedLog,
        name: &str,
        t0_us: u64,
        t1_us: u64,
    ) -> Result<Series> {
        let channel = self
            .math_channel(name)
            .ok_or_else(|| Error::not_found(format!("no math channel named {name}")))?;
        let compiled = compile_math(&self.math, channel)?;
        let raw = log.samples(compiled.dependencies(), t0_us, t1_us)?;
        eval_channel(channel, &compiled, &raw)
    }

    /// The event lane and any trigger that could not be evaluated: the log's
    /// own events merged with the trigger crossings. Cached per log and
    /// trigger set; a read failure is reported but not cached, so a retry
    /// scans again.
    pub(super) fn events(&self, slot: &LogSlot, log: &IndexedLog) -> (Vec<EventDto>, Vec<String>) {
        if let Some(hit) = slot.cached_events(&self.triggers) {
            return hit;
        }
        let mut events: Vec<EventDto> = log
            .events()
            .iter()
            .map(|(t_us, label)| EventDto {
                t_us: *t_us,
                label: label.clone(),
            })
            .collect();
        let mut warnings = Vec::new();
        let mut cacheable = true;
        for trigger in &self.triggers {
            match log.crossings(&trigger.signal, trigger.op, trigger.value) {
                Ok(hits) => events.extend(
                    hits.into_iter()
                        .map(|(t_us, label)| EventDto { t_us, label }),
                ),
                Err(err) => {
                    warnings.push(format!(
                        "Trigger {} {} {} could not be evaluated: {err}",
                        trigger.signal,
                        trigger.op.symbol(),
                        trigger.value
                    ));
                    cacheable &= !log.has_signal(&trigger.signal);
                }
            }
        }
        events.sort_by_key(|event| event.t_us);
        events.truncate(5_000);
        if cacheable {
            slot.cache_events(&self.triggers, &events, &warnings);
        }
        (events, warnings)
    }
}

/// A math channel with its compiled plan.
pub(super) type Derived<'a> = (&'a MathChannel, Compiled);

/// Evaluate a compiled channel over `base`, which holds a series for each of
/// its signals. Names are matched to the plan's variables once.
fn eval_channel(channel: &MathChannel, compiled: &Compiled, base: &[Series]) -> Result<Series> {
    let inputs = compiled
        .dependencies()
        .iter()
        .map(|dep| {
            base.iter()
                .find(|series| series.name == *dep)
                .map(|series| series.points.as_slice())
                .ok_or_else(|| Error::invalid(format!("math channel {} needs {dep}", channel.name)))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Series {
        name: channel.name.clone(),
        unit: channel.unit.clone(),
        points: compiled.eval_series(&inputs),
    })
}

impl Session {
    pub fn set_math(&mut self, channels: Vec<MathChannel>) -> Result<Summary> {
        self.deck.set_math(channels)?;
        self.summary()
    }

    pub fn set_triggers(&mut self, triggers: Vec<ThresholdTrigger>) -> Result<Summary> {
        self.deck.set_triggers(self.log.get()?, triggers)?;
        self.summary()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{math, math_points, math_session, RPM_MAP};
    use super::*;
    use crate::project::TriggerOp;

    #[test]
    fn a_plotted_difference_has_the_stats_extremes_at_any_zoom() {
        let session = math_session(&[("Diff", "A - B")]);
        let stats = session.stats("Diff", 0, 9_000).unwrap();
        assert_eq!((stats.count, stats.min, stats.max), (18, -28.0, 27.0));

        let zoomed_out = math_points(&session, "Diff", 0, 4);
        assert_eq!(
            zoomed_out,
            [(2500, 8.0), (3000, 27.0), (5000, -28.0), (6000, 8.0)]
        );
        let detailed = math_points(&session, "Diff", 0, 1000);
        assert_eq!(detailed.len(), 18);
        for points in [zoomed_out, detailed] {
            let min = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
            let max = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
            assert_eq!((min, max), (stats.min, stats.max));
        }
    }

    #[test]
    fn a_low_pass_channel_has_the_same_value_at_every_zoom() {
        let session = math_session(&[("Smooth", "lp(A, 0.5)")]);
        let zoomed_out = math_points(&session, "Smooth", 0, 6);
        assert_eq!(
            zoomed_out,
            [
                (0, 10.0),
                (1000, 11.0),
                (3000, 20.5),
                (5000, 14.375),
                (8000, 12.296875)
            ]
        );
        let detailed = math_points(&session, "Smooth", 0, 1000);
        assert_eq!(detailed.len(), 10);
        assert_eq!(detailed[3], (3000, 20.5));
        assert_eq!(detailed[8], (8000, 12.296875));
        let stats = session.stats("Smooth", 0, 9_000).unwrap();
        assert_eq!((stats.min, stats.max), (10.0, 20.5));
    }

    #[test]
    fn a_math_channel_is_bucketed_like_a_physical_one_from_a_mid_log_start() {
        let session = math_session(&[("SameA", "A + 0")]);
        let physical = math_points(&session, "A", 2250, 4);
        assert_eq!(physical, [(2250, 11.0), (3000, 30.0), (7000, 11.0)]);
        assert_eq!(math_points(&session, "SameA", 2250, 4), physical);
    }

    #[test]
    fn a_math_channel_cannot_use_another_math_channel() {
        let mut session = math_session(&[("Diff", "A - B")]);
        let err = session
            .set_math(vec![math("Diff", "A - B"), math("Twice", "Diff * 2")])
            .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("math channel Twice uses math channel Diff."),
            "{err}"
        );
        assert_eq!(session.deck.math().len(), 1);
    }

    const SWING_LOG: &str = "SLOGv1
F 0 1A0 800C000000000000
F 10000 1A0 401F000000000000
F 20000 1A0 800C000000000000
F 30000 1A0 401F000000000000
";
    const SWING_MAP_HALF: &str = r#"{"name":"rpm","version":1,"messages":[{"id":"0x1A0","name":"Powertrain",
        "signals":[{"name":"EngineRPM","startBit":0,"bitLength":16,"factor":0.5,"unit":"rpm"}]}]}"#;
    const SWING_MAP_RENAMED: &str = r#"{"name":"rpm","version":1,"messages":[{"id":"0x1A0","name":"Powertrain",
        "signals":[{"name":"Revs","startBit":0,"bitLength":16,"factor":0.25,"unit":"rpm"}]}]}"#;

    fn rpm_trigger(op: TriggerOp, value: f64) -> ThresholdTrigger {
        ThresholdTrigger {
            id: "rev".into(),
            signal: "EngineRPM".into(),
            op,
            value,
        }
    }

    fn trigger_events(summary: &Summary) -> Vec<(u64, &str)> {
        summary
            .events
            .iter()
            .map(|event| (event.t_us, event.label.as_str()))
            .collect()
    }

    fn swing_session() -> Session {
        let mut session = Session::new();
        session
            .open_bytes("swing.slog", SWING_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session
    }

    #[test]
    fn trigger_ops_keep_their_wire_strings() {
        for (op, wire) in [
            (TriggerOp::Gt, ">"),
            (TriggerOp::Lt, "<"),
            (TriggerOp::Ge, ">="),
            (TriggerOp::Le, "<="),
        ] {
            let json = serde_json::to_string(&op).unwrap();
            assert_eq!(json, format!("\"{wire}\""));
            assert_eq!(serde_json::from_str::<TriggerOp>(&json).unwrap(), op);
        }
        for (word, op) in [
            ("gt", TriggerOp::Gt),
            ("lt", TriggerOp::Lt),
            ("ge", TriggerOp::Ge),
            ("le", TriggerOp::Le),
        ] {
            let json = format!("\"{word}\"");
            assert_eq!(serde_json::from_str::<TriggerOp>(&json).unwrap(), op);
        }
        assert!(serde_json::from_str::<TriggerOp>("\"=\"").is_err());
        let trigger = rpm_trigger(TriggerOp::Ge, 1500.0);
        assert_eq!(
            serde_json::to_string(&trigger).unwrap(),
            r#"{"id":"rev","signal":"EngineRPM","op":">=","value":1500.0}"#
        );
    }

    #[test]
    fn a_trigger_on_an_unknown_signal_is_refused_by_name() {
        let mut session = swing_session();
        let mut ghost = rpm_trigger(TriggerOp::Gt, 1500.0);
        ghost.signal = "EngineRMP".into();
        let err = session.set_triggers(vec![ghost]).unwrap_err().to_string();
        assert!(err.contains("EngineRMP"), "{err}");
        assert!(session.deck.triggers().is_empty());
    }

    #[test]
    fn a_trigger_on_a_math_channel_is_refused() {
        let mut session = swing_session();
        session
            .set_math(vec![MathChannel {
                name: "Half".into(),
                unit: "rpm".into(),
                expr: "EngineRPM / 2".into(),
            }])
            .unwrap();
        let mut on_math = rpm_trigger(TriggerOp::Gt, 100.0);
        on_math.signal = "Half".into();
        let err = session.set_triggers(vec![on_math]).unwrap_err().to_string();
        assert!(err.contains("Half") && err.contains("math"), "{err}");
    }

    #[test]
    fn trigger_events_follow_the_triggers_and_the_log() {
        let mut session = swing_session();
        let summary = session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        assert_eq!(
            trigger_events(&summary),
            [
                (10_000, "Trigger EngineRPM > 1500"),
                (30_000, "Trigger EngineRPM > 1500")
            ]
        );
        assert!(session.log.has_cached_events());

        let summary = session
            .set_triggers(vec![rpm_trigger(TriggerOp::Le, 800.0)])
            .unwrap();
        assert_eq!(
            trigger_events(&summary),
            [
                (0, "Trigger EngineRPM <= 800"),
                (20_000, "Trigger EngineRPM <= 800")
            ]
        );

        session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        session.reindex_controlled(None).unwrap();
        assert!(!session.log.has_cached_events());

        // Doubling the factor puts 800 rpm at 1600: hot from the first frame.
        let summary = session.open_map_json(SWING_MAP_HALF).unwrap();
        assert_eq!(trigger_events(&summary), [(0, "Trigger EngineRPM > 1500")]);
    }

    #[test]
    fn a_trigger_whose_signal_leaves_the_log_is_reported_not_dropped_silently() {
        let mut session = swing_session();
        session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        let summary = session.open_map_json(SWING_MAP_RENAMED).unwrap();
        assert!(trigger_events(&summary).is_empty());
        assert_eq!(
            summary.warnings,
            ["Trigger EngineRPM > 1500 could not be evaluated: no signal named EngineRPM"]
        );
    }

    #[test]
    fn opening_a_log_clears_the_triggers_and_their_events() {
        let mut session = swing_session();
        session
            .set_triggers(vec![rpm_trigger(TriggerOp::Gt, 1500.0)])
            .unwrap();
        let summary = session
            .open_bytes("again.slog", SWING_LOG.as_bytes().to_vec())
            .unwrap();
        assert!(trigger_events(&summary).is_empty());
    }
}
