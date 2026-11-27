use super::build::Built;
use super::IndexedLog;

pub(super) const CHECKPOINT_EVERY: u64 = 256;
const MAX_CHECKPOINTS: usize = 4_096;

#[derive(Clone, Copy)]
pub(super) struct Checkpoint {
    pub(super) t_us: u64,
    pub(super) offset: u64,
    pub(super) inner_offset: u32,
    pub(super) frame_ordinal: u64,
    /// Cumulative CAN bus bits on frames strictly before this checkpoint.
    pub(super) frame_bits: u64,
}

/// Min/max sample envelope for one signal inside one checkpoint interval.
/// Empty cells carry `min_v = f64::NAN`.
#[derive(Clone, Copy)]
pub(super) struct SignalSpan {
    pub(super) min_t: u64,
    pub(super) min_v: f64,
    pub(super) max_t: u64,
    pub(super) max_v: f64,
}

impl SignalSpan {
    pub(super) const EMPTY: Self = Self {
        min_t: 0,
        min_v: f64::NAN,
        max_t: 0,
        max_v: f64::NAN,
    };

    #[inline]
    pub(super) fn is_empty(self) -> bool {
        self.min_v.is_nan()
    }

    #[inline]
    pub(super) fn observe(&mut self, t: u64, value: f64) {
        if self.min_v.is_nan() {
            *self = Self {
                min_t: t,
                min_v: value,
                max_t: t,
                max_v: value,
            };
            return;
        }
        if value < self.min_v || ((value - self.min_v).abs() < 1e-15 && t < self.min_t) {
            self.min_v = value;
            self.min_t = t;
        }
        if value > self.max_v || ((value - self.max_v).abs() < 1e-15 && t > self.max_t) {
            self.max_v = value;
            self.max_t = t;
        }
    }

    #[inline]
    pub(super) fn merge(self, later: Self) -> Self {
        if self.is_empty() {
            return later;
        }
        if later.is_empty() {
            return self;
        }
        let mut out = self;
        out.observe(later.min_t, later.min_v);
        out.observe(later.max_t, later.max_v);
        out
    }
}

/// Flat row-major snapshot matrix (`rows × width`), using `NaN` for unset signals.
#[derive(Default)]
pub(super) struct SnapshotTable {
    width: usize,
    rows: usize,
    cells: Vec<f64>,
}

impl SnapshotTable {
    pub(super) fn with_width(width: usize) -> Self {
        Self {
            width,
            rows: 0,
            cells: Vec::new(),
        }
    }

    pub(super) fn grow_width(&mut self, new_width: usize) {
        if new_width <= self.width {
            return;
        }
        if self.rows == 0 {
            self.width = new_width;
            return;
        }
        let mut next = vec![f64::NAN; self.rows * new_width];
        for row in 0..self.rows {
            let src = &self.cells[row * self.width..(row + 1) * self.width];
            next[row * new_width..row * new_width + self.width].copy_from_slice(src);
        }
        self.width = new_width;
        self.cells = next;
    }

    pub(super) fn push(&mut self, held: &[Option<f64>], width: usize) {
        self.grow_width(width);
        let start = self.cells.len();
        self.cells.resize(start + self.width, f64::NAN);
        let dst = &mut self.cells[start..start + self.width];
        for (cell, value) in dst.iter_mut().zip(held.iter().copied()) {
            if let Some(v) = value {
                *cell = v;
            }
        }
        self.rows += 1;
    }

    /// Keep even-indexed rows (`0, 2, 4, ...`) in place when checkpoints halve.
    pub(super) fn decimate_even(&mut self) {
        let kept = self.rows.div_ceil(2);
        if self.width > 0 {
            for dst_row in 1..kept {
                let src_start = (dst_row * 2) * self.width;
                let dst_start = dst_row * self.width;
                self.cells
                    .copy_within(src_start..src_start + self.width, dst_start);
            }
            self.cells.truncate(kept * self.width);
        }
        self.rows = kept;
    }

    #[inline]
    pub(super) fn get(&self, row: usize, col: usize) -> Option<f64> {
        if row >= self.rows || col >= self.width {
            return None;
        }
        let value = self.cells[row * self.width + col];
        (!value.is_nan()).then_some(value)
    }

    pub(super) fn row_vec(&self, row: usize, total_signals: usize) -> Vec<Option<f64>> {
        let mut out = vec![None; total_signals];
        if row < self.rows {
            let take = self.width.min(total_signals);
            let slice = &self.cells[row * self.width..row * self.width + take];
            for (dst, &val) in out.iter_mut().zip(slice) {
                if !val.is_nan() {
                    *dst = Some(val);
                }
            }
        }
        out
    }
}

/// Flat row-major checkpoint-interval min/max table (`rows × width`).
/// Row `k` covers records processed while `checkpoints.len() - 1 == k` (i.e.
/// from checkpoint `k` up to checkpoint `k + 1`, or EOF for the last row).
#[derive(Default)]
pub(super) struct SpanTable {
    width: usize,
    rows: usize,
    cells: Vec<SignalSpan>,
}

impl SpanTable {
    pub(super) fn with_width(width: usize) -> Self {
        Self {
            width,
            rows: 0,
            cells: Vec::new(),
        }
    }

    pub(super) fn grow_width(&mut self, new_width: usize) {
        if new_width <= self.width {
            return;
        }
        if self.rows == 0 {
            self.width = new_width;
            return;
        }
        let mut next = vec![SignalSpan::EMPTY; self.rows * new_width];
        for row in 0..self.rows {
            let src = &self.cells[row * self.width..(row + 1) * self.width];
            next[row * new_width..row * new_width + self.width].copy_from_slice(src);
        }
        self.width = new_width;
        self.cells = next;
    }

    pub(super) fn push_empty(&mut self, width: usize) {
        self.grow_width(width);
        self.cells
            .resize(self.cells.len() + self.width, SignalSpan::EMPTY);
        self.rows += 1;
    }

    #[inline]
    pub(super) fn observe_last(&mut self, col: usize, t: u64, value: f64) {
        if self.rows == 0 {
            return;
        }
        if col >= self.width {
            self.grow_width(col + 1);
        }
        let row = self.rows - 1;
        self.cells[row * self.width + col].observe(t, value);
    }

    /// Merge `(2*k, 2*k + 1)` in place when checkpoints halve with `step_by(2)`.
    pub(super) fn merge_pairs(&mut self) {
        let kept = self.rows.div_ceil(2);
        if self.width > 0 {
            for k in 0..kept {
                let first = 2 * k;
                let second = first + 1;
                for col in 0..self.width {
                    let a = self.cells[first * self.width + col];
                    let merged = if second < self.rows {
                        a.merge(self.cells[second * self.width + col])
                    } else {
                        a
                    };
                    self.cells[k * self.width + col] = merged;
                }
            }
            self.cells.truncate(kept * self.width);
        }
        self.rows = kept;
    }

    #[inline]
    pub(super) fn get(&self, row: usize, col: usize) -> Option<SignalSpan> {
        if row >= self.rows || col >= self.width {
            return None;
        }
        let span = self.cells[row * self.width + col];
        (!span.is_empty()).then_some(span)
    }
}

pub(super) fn push_checkpoint(
    built: &mut Built,
    held: &[Option<f64>],
    t_us: u64,
    offset: u64,
    inner_offset: u32,
    stride: &mut u64,
) {
    if built.checkpoints.len() >= MAX_CHECKPOINTS {
        built.checkpoints = built.checkpoints.iter().step_by(2).copied().collect();
        built.snapshots.decimate_even();
        built.spans.merge_pairs();
        *stride = stride.saturating_mul(2).max(CHECKPOINT_EVERY);
    }
    let width = built.signals.len();
    built.checkpoints.push(Checkpoint {
        t_us,
        offset,
        inner_offset,
        frame_ordinal: built.frame_count,
        frame_bits: built.frame_bits,
    });
    built.snapshots.push(held, width);
    built.spans.push_empty(width);
}

pub(super) fn pad_snapshots(built: &mut Built) {
    let width = built.signals.len();
    built.snapshots.grow_width(width);
    built.spans.grow_width(width);
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
        while lo > 0 && self.checkpoints[lo].t_us == t_us && self.checkpoints[lo - 1].t_us == t_us {
            lo -= 1;
        }
        lo
    }

    pub(super) fn snapshot(&self, idx: usize) -> Vec<Option<f64>> {
        self.snapshots.row_vec(idx, self.signals.len())
    }
}
