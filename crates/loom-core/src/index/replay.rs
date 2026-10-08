use super::time::{order_time, TimeOrder};
use super::IndexedLog;
use crate::error::Result;
use crate::scan::{Rec, RecKind, Scanner};

/// Where a replayed record falls against the window being read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase {
    /// Before the window start: it only brings the held values up to date.
    Lead,
    /// Inside `[t0, t1]`.
    Window,
}

/// What `replay` hands its visitor.
pub(super) enum Step<'a> {
    /// The held values now are the values at `t0`: every record before it has
    /// been applied and none inside the window has. Fires once, before the
    /// first record at or after `t0`, or at the end of the log when none follows.
    /// What the visitor returns for it is ignored.
    Start,
    /// A record inside the window. The visitor applies it with `touch`.
    Record(&'a Rec),
}

impl IndexedLog {
    /// Replay from checkpoint `idx`. Times pass through the same `order_time`
    /// the build used, seeded with the checkpoint's time, so a replay sees the
    /// timestamps the index stored.
    pub(super) fn scan_from(&self, idx: usize, mut visit: impl FnMut(&Rec) -> bool) -> Result<()> {
        let checkpoint = &self.checkpoints[idx];
        self.source.with_reader(|reader| {
            let mut scanner =
                Scanner::resume_at(reader, self.body, checkpoint.offset, checkpoint.t_us)?;
            let mut last = Some(checkpoint.t_us);
            while let Some(mut rec) = scanner.next_rec()? {
                match order_time(&mut last, rec.t_us) {
                    TimeOrder::InOrder(t_us) | TimeOrder::Clamped(t_us) => rec.t_us = t_us,
                    TimeOrder::Skipped => continue,
                }
                if !visit(&rec) {
                    break;
                }
            }
            Ok(())
        })
    }

    /// Replay the checkpoint at or before `t0` up to the first record after
    /// `t1`, telling the visitor which side of `t0` each record is on. The
    /// visitor says whether to keep reading. Callers keep their own policy for
    /// what the window holds; this owns finding where it starts and where it ends.
    pub(super) fn scan_window(
        &self,
        t0: u64,
        t1: u64,
        mut visit: impl FnMut(Phase, &Rec) -> bool,
    ) -> Result<()> {
        self.scan_from(self.floor_checkpoint(t0), |rec| {
            if rec.t_us > t1 {
                return false;
            }
            let phase = if rec.t_us < t0 {
                Phase::Lead
            } else {
                Phase::Window
            };
            visit(phase, rec)
        })
    }

    /// `scan_window` that keeps the held value of every signal. The held values
    /// start from the checkpoint's snapshot and follow the lead records, so a
    /// window record sees every signal as it stood. Returns the held values
    /// after the last record read.
    pub(super) fn replay(
        &self,
        t0: u64,
        t1: u64,
        mut visit: impl FnMut(Step<'_>, &mut Vec<Option<f64>>) -> bool,
    ) -> Result<Vec<Option<f64>>> {
        let mut held = self.snapshot(self.floor_checkpoint(t0));
        let mut started = false;
        self.scan_window(t0, t1, |phase, rec| match phase {
            Phase::Lead => {
                self.touch(rec, &mut held, |_, _| {});
                true
            }
            Phase::Window => {
                if !started {
                    started = true;
                    visit(Step::Start, &mut held);
                }
                visit(Step::Record(rec), &mut held)
            }
        })?;
        if !started {
            visit(Step::Start, &mut held);
        }
        Ok(held)
    }

    /// Apply `rec` to `held`, reporting each signal it updates.
    pub(super) fn touch(
        &self,
        rec: &Rec,
        held: &mut Vec<Option<f64>>,
        mut on_update: impl FnMut(usize, f64),
    ) {
        if held.len() < self.signals.len() {
            held.resize(self.signals.len(), None);
        }
        match &rec.kind {
            RecKind::Frame {
                id,
                dlc,
                data,
                channel,
                ..
            } => {
                if let Some(indices) = self.msg_index.get(id) {
                    super::signal::decode_frame(
                        &self.signals,
                        indices,
                        *channel,
                        *dlc,
                        data,
                        |idx, value| {
                            held[idx] = Some(value);
                            on_update(idx, value);
                        },
                    );
                }
            }
            RecKind::Sample { name, value, .. } => {
                if let Some(idx) = self.resolve_signal(name) {
                    held[idx] = Some(*value);
                    on_update(idx, *value);
                }
            }
            RecKind::Event { .. } => {}
        }
    }
}
