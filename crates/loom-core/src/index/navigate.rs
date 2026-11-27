use super::query::HeldValue;
use super::signal::decode_one;
use super::IndexedLog;
use crate::error::Result;
use crate::project::TriggerOp;
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
    pub fn step_frame(&self, t_us: u64, next: bool) -> Result<Option<FrameHit>> {
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

    /// Times where `name` first satisfies `op` against `level`, until it stops
    /// doing so. At most 200 hits.
    pub fn crossings(&self, name: &str, op: TriggerOp, level: f64) -> Result<Vec<(u64, String)>> {
        let idx = self.require_signal(name)?;
        if self.checkpoints.is_empty() {
            return Ok(Vec::new());
        }
        let label = format!("Trigger {name} {} {level}", op.symbol());
        let mut events = Vec::new();
        let mut armed = true;
        self.scan_from(0, |rec| {
            if let Some(value) = decode_one(&self.signals, &self.msg_index, rec, idx) {
                let hot = op.holds(value, level);
                if hot && armed {
                    events.push((rec.t_us, label.clone()));
                    armed = false;
                } else if !hot {
                    armed = true;
                }
            }
            events.len() < 200
        })?;
        Ok(events)
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
