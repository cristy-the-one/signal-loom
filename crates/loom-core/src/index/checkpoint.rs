use super::build::Built;
use super::IndexedLog;

pub(super) const CHECKPOINT_EVERY: u64 = 256;
const MAX_CHECKPOINTS: usize = 4_096;

#[derive(Clone)]
pub(super) struct Checkpoint {
    pub(super) t_us: u64,
    pub(super) offset: u64,
    pub(super) frame_ordinal: u64,
}

pub(super) fn push_checkpoint(
    built: &mut Built,
    held: &[Option<f64>],
    t_us: u64,
    offset: u64,
    stride: &mut u64,
) {
    if built.checkpoints.len() >= MAX_CHECKPOINTS {
        let checkpoints = built.checkpoints.iter().step_by(2).cloned().collect();
        let snapshots = built.snapshots.iter().step_by(2).cloned().collect();
        built.checkpoints = checkpoints;
        built.snapshots = snapshots;
        *stride = stride.saturating_mul(2).max(CHECKPOINT_EVERY);
    }
    built.checkpoints.push(Checkpoint {
        t_us,
        offset,
        frame_ordinal: built.frame_count,
    });
    let mut snap = held.to_vec();
    snap.resize(built.signals.len(), None);
    built.snapshots.push(snap);
}

pub(super) fn pad_snapshots(built: &mut Built) {
    let width = built.signals.len();
    for snap in &mut built.snapshots {
        snap.resize(width, None);
    }
}

impl IndexedLog {
    pub(super) fn floor_checkpoint(&self, t_us: u64) -> usize {
        let mut lo = 0usize;
        let mut hi = self.checkpoints.len();
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.checkpoints[mid].t_us <= t_us {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub(super) fn snapshot(&self, idx: usize) -> Vec<Option<f64>> {
        let mut held = self.snapshots.get(idx).cloned().unwrap_or_default();
        held.resize(self.signals.len(), None);
        held
    }
}
