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
    pub fn step_frame(&self, t_us: u64, next: bool) -> Result<Option<FrameHit>> {
        if self.checkpoints.is_empty() {
            return Ok(None);
        }
        if next {
            let idx = self.floor_checkpoint(t_us);
            let mut ordinal = self.checkpoints[idx].frame_ordinal;
            let mut found = None;
            self.scan_from(idx, |rec| {
                let current = ordinal;
                if is_timed_sample(rec) {
                    ordinal += 1;
                    if rec.t_us > t_us {
                        found = Some(self.hit_from(rec, current));
                        return false;
                    }
                }
                true
            })?;
            return Ok(found);
        }

        let mut idx = self.floor_checkpoint(t_us);
        loop {
            let mut ordinal = self.checkpoints[idx].frame_ordinal;
            let mut last = None;
            self.scan_from(idx, |rec| {
                if rec.t_us >= t_us {
                    return false;
                }
                if is_timed_sample(rec) {
                    last = Some(self.hit_from(rec, ordinal));
                    ordinal += 1;
                }
                true
            })?;
            if last.is_some() {
                return Ok(last);
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
        let label = format!("Trigger {name} {} {level}", op.symbol());
        let mut events = Vec::new();
        let mut armed = true;
        let mut held = self.snapshot(0);
        self.scan_from(0, |rec| {
            self.touch(rec, &mut held, |signal, value| {
                if signal != idx {
                    return;
                }
                let hot = op.holds(value, level);
                if hot && armed {
                    events.push((rec.t_us, label.clone()));
                    armed = false;
                } else if !hot {
                    armed = true;
                }
            });
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
