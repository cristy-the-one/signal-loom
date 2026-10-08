use super::replay::Phase;
use super::time::ordered_range;
use super::IndexedLog;
use crate::error::Result;
use crate::scan::RecKind;

impl IndexedLog {
    pub fn bus_load(&self, t0_us: u64, t1_us: u64) -> Result<crate::dto::BusLoad> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut frames = 0u64;
        let mut bits = 0u64;
        self.scan_window(t0, t1, |phase, rec| {
            if phase == Phase::Window {
                if let RecKind::Frame { dlc, .. } = rec.kind {
                    frames += 1;
                    bits += 47 + u64::from(dlc.min(8)) * 8;
                }
            }
            true
        })?;
        let dt = ((t1.saturating_sub(t0)) as f64 / 1_000_000.0).max(1.0e-6);
        Ok(crate::dto::BusLoad {
            frames,
            rate: frames as f64 / dt,
            load: (bits as f64 / dt) / 500_000.0,
        })
    }
}
