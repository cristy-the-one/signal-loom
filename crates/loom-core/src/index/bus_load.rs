use super::time::ordered_range;
use super::IndexedLog;
use crate::error::Result;
use crate::scan::RecKind;

#[inline]
pub(super) fn frame_wire_bits(dlc: u8) -> u64 {
    47 + u64::from(dlc.min(8)) * 8
}

impl IndexedLog {
    pub fn bus_load(&self, t0_us: u64, t1_us: u64) -> Result<crate::dto::BusLoad> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let dt = ((t1.saturating_sub(t0)) as f64 / 1_000_000.0).max(1.0e-6);
        if self.checkpoints.is_empty() || self.format == crate::scan::LogFormat::DecodedCsv {
            return Ok(crate::dto::BusLoad {
                frames: 0,
                rate: 0.0,
                load: 0.0,
            });
        }
        let mut c_start = self.floor_checkpoint(t0);
        if c_start > 0 && self.checkpoints[c_start].t_us == t0 {
            c_start -= 1;
        }
        let c_end = self.floor_checkpoint(t1);
        let mut frames = 0u64;
        let mut bits = 0u64;

        // Fast path: when [t0, t1] covers the entire recording, answer in O(1) with 0 I/O.
        if t0 <= self.t_start_us && t1 >= self.t_end_us {
            frames = self.frame_count;
            bits = self.frame_bits;
        } else if c_start + 1 < c_end && self.checkpoints[c_start + 1].t_us >= t0 {
            // Scan left boundary interval [c_start, c_start + 1), add interior
            // checkpoints in O(1), then scan right boundary interval [c_end, ...].
            self.scan_checkpoint_range(c_start, Some(c_start + 1), t1, |rec| {
                if rec.t_us >= t0 {
                    if let RecKind::Frame { dlc, .. } = rec.kind {
                        frames += 1;
                        bits += frame_wire_bits(dlc);
                    }
                }
                true
            })?;
            let left_cp = &self.checkpoints[c_start + 1];
            let right_cp = &self.checkpoints[c_end];
            frames += right_cp.frame_ordinal - left_cp.frame_ordinal;
            bits += right_cp.frame_bits - left_cp.frame_bits;
            self.scan_checkpoint_range(c_end, None, t1, |rec| {
                if rec.t_us >= t0 {
                    if let RecKind::Frame { dlc, .. } = rec.kind {
                        frames += 1;
                        bits += frame_wire_bits(dlc);
                    }
                }
                true
            })?;
        } else {
            self.scan_checkpoint_range(c_start, None, t1, |rec| {
                if rec.t_us >= t0 {
                    if let RecKind::Frame { dlc, .. } = rec.kind {
                        frames += 1;
                        bits += frame_wire_bits(dlc);
                    }
                }
                true
            })?;
        }

        Ok(crate::dto::BusLoad {
            frames,
            rate: frames as f64 / dt,
            load: (bits as f64 / dt) / 500_000.0,
        })
    }
}
