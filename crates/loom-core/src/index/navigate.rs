use super::query::HeldValue;
use super::signal::SignalFilter;
use super::IndexedLog;
use crate::error::Result;
use crate::project::{ThresholdTrigger, TriggerOp};
use crate::scan::{hex_payload, Rec, RecKind};

#[derive(Debug, Clone)]
pub struct FrameHit {
    pub t_us: u64,
    pub ordinal: u64,
    pub message_id: Option<u32>,
    pub message_name: String,
    pub extended: bool,
    pub dlc: u8,
    pub data_hex: String,
}

impl IndexedLog {
    #[cfg(test)]
    pub(crate) fn step_frame(&self, t_us: u64, next: bool) -> Result<Option<FrameHit>> {
        Ok(self.step_frame_with_values(t_us, next)?.map(|(hit, _)| hit))
    }

    /// Step to the next or previous frame and return the held signal values at
    /// that frame's timestamp in one checkpoint scan.
    pub fn step_frame_with_values(
        &self,
        t_us: u64,
        next: bool,
    ) -> Result<Option<(FrameHit, Vec<HeldValue>)>> {
        if self.checkpoints.is_empty() {
            return Ok(None);
        }
        if next {
            let idx = self.floor_checkpoint(t_us);
            let mut ordinal = self.checkpoints[idx].frame_ordinal;
            let mut held = self.snapshot(idx);
            let mut found: Option<FrameHit> = None;
            self.scan_from(idx, |rec| {
                if let Some(hit) = &found {
                    if rec.t_us > hit.t_us {
                        return false;
                    }
                }
                self.touch(rec, &mut held, |_, _| {});
                if found.is_none() && is_timed_sample(rec) {
                    let current = ordinal;
                    ordinal += 1;
                    if rec.t_us > t_us {
                        found = Some(self.hit_from(rec, current));
                    }
                }
                true
            })?;
            return Ok(found.map(|hit| (hit, self.held_values(&held))));
        }

        let mut idx = self.floor_checkpoint(t_us);
        loop {
            let mut ordinal = self.checkpoints[idx].frame_ordinal;
            let mut held = self.snapshot(idx);
            let mut last = None;
            self.scan_from(idx, |rec| {
                if rec.t_us >= t_us {
                    return false;
                }
                self.touch(rec, &mut held, |_, _| {});
                if is_timed_sample(rec) {
                    last = Some(self.hit_from(rec, ordinal));
                    ordinal += 1;
                }
                true
            })?;
            if let Some(hit) = last {
                return Ok(Some((hit, self.held_values(&held))));
            }
            if idx == 0 {
                return Ok(None);
            }
            idx -= 1;
        }
    }
}

pub(crate) struct TriggerScan {
    pub(crate) hits: Vec<(u64, String)>,
    pub(crate) warnings: Vec<String>,
    pub(crate) cacheable: bool,
}

impl IndexedLog {
    /// Evaluate all `triggers` in a single sequential pass over the log,
    /// capping each trigger at 200 hits and stopping early once every trigger
    /// reaches its cap.
    pub(crate) fn crossings_many(&self, triggers: &[ThresholdTrigger]) -> TriggerScan {
        let mut wanted = Vec::with_capacity(triggers.len());
        let mut states = Vec::with_capacity(triggers.len());
        let mut warnings = Vec::new();
        let mut cacheable = true;

        struct TrigState {
            op: TriggerOp,
            level: f64,
            label: String,
            armed: bool,
            hits: Vec<(u64, String)>,
        }

        for trigger in triggers {
            match self.require_signal(&trigger.signal) {
                Ok(idx) => {
                    wanted.push(idx);
                    states.push(TrigState {
                        op: trigger.op,
                        level: trigger.value,
                        label: format!(
                            "Trigger {} {} {}",
                            trigger.signal,
                            trigger.op.symbol(),
                            trigger.value
                        ),
                        armed: true,
                        hits: Vec::new(),
                    });
                }
                Err(err) => {
                    warnings.push(format!(
                        "Trigger {} {} {} could not be evaluated: {err}",
                        trigger.signal,
                        trigger.op.symbol(),
                        trigger.value
                    ));
                    cacheable &= !self.has_signal(&trigger.signal);
                }
            }
        }

        if !self.checkpoints.is_empty() && !wanted.is_empty() {
            let filter = SignalFilter::new(&self.signals, &wanted);
            let mut active = wanted.len();
            if let Err(err) = self.scan_from(0, |rec| {
                self.decode_wanted(rec, &filter, |slot, value| {
                    let state = &mut states[slot];
                    if state.hits.len() >= 200 {
                        return;
                    }
                    let hot = state.op.holds(value, state.level);
                    if hot && state.armed {
                        state.hits.push((rec.t_us, state.label.clone()));
                        state.armed = false;
                        if state.hits.len() == 200 {
                            active -= 1;
                        }
                    } else if !hot {
                        state.armed = true;
                    }
                });
                active > 0
            }) {
                // Report per-trigger read failure without caching so a retry scans again.
                for trigger in triggers {
                    if self.has_signal(&trigger.signal) {
                        warnings.push(format!(
                            "Trigger {} {} {} could not be evaluated: {err}",
                            trigger.signal,
                            trigger.op.symbol(),
                            trigger.value
                        ));
                    }
                }
                return TriggerScan {
                    hits: Vec::new(),
                    warnings,
                    cacheable: false,
                };
            }
        }

        let mut hits = Vec::new();
        for state in states {
            hits.extend(state.hits);
        }
        TriggerScan {
            hits,
            warnings,
            cacheable,
        }
    }

    fn hit_from(&self, rec: &Rec, ordinal: u64) -> FrameHit {
        match &rec.kind {
            RecKind::Frame {
                id,
                dlc,
                data,
                extended,
                ..
            } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: Some(*id),
                message_name: self
                    .message_names
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| format!("0x{id:X}")),
                extended: *extended,
                dlc: *dlc,
                data_hex: hex_payload(data, *dlc),
            },
            RecKind::Sample { name, .. } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: None,
                message_name: name.clone(),
                extended: false,
                dlc: 0,
                data_hex: String::new(),
            },
            RecKind::Event { label } => FrameHit {
                t_us: rec.t_us,
                ordinal,
                message_id: None,
                message_name: label.clone(),
                extended: false,
                dlc: 0,
                data_hex: String::new(),
            },
        }
    }
}

fn is_timed_sample(rec: &Rec) -> bool {
    matches!(rec.kind, RecKind::Frame { .. } | RecKind::Sample { .. })
}
