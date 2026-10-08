use super::build::{note_warn, Built};

/// Real captures interleave Tx and Rx lines a few milliseconds out of order.
pub(super) const REORDER_TOLERANCE_US: u64 = 50_000;

pub(super) enum TimeOrder {
    InOrder(u64),
    /// A small step back: kept, at the previous time.
    Clamped(u64),
    /// A jump back past the tolerance: a broken log, so the record is dropped.
    Skipped,
}

pub(super) fn order_time(last: &mut Option<u64>, t_us: u64) -> TimeOrder {
    match *last {
        Some(prev) if t_us < prev => {
            if prev - t_us <= REORDER_TOLERANCE_US {
                TimeOrder::Clamped(prev)
            } else {
                TimeOrder::Skipped
            }
        }
        _ => {
            *last = Some(t_us);
            TimeOrder::InOrder(t_us)
        }
    }
}

pub(super) fn accept_time(built: &mut Built, last: &mut Option<u64>, t_us: u64) -> Option<u64> {
    let prev = *last;
    match order_time(last, t_us) {
        TimeOrder::InOrder(t_us) => Some(t_us),
        TimeOrder::Clamped(t_us) => {
            built.reordered += 1;
            Some(t_us)
        }
        TimeOrder::Skipped => {
            built.skipped += 1;
            note_warn(
                built,
                format!(
                    "skipped backwards timestamp at {t_us} µs (previous {} µs)",
                    prev.unwrap_or(0)
                ),
            );
            None
        }
    }
}

pub(super) fn ordered_range(t0: u64, t1: u64) -> (u64, u64) {
    if t0 <= t1 {
        (t0, t1)
    } else {
        (t1, t0)
    }
}
