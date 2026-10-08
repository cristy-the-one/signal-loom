use super::replay::{Phase, Step};
use super::time::ordered_range;
use super::IndexedLog;
use crate::error::{Error, Result};
use crate::scan::{hex_payload, RecKind};

/// Most data rows one export writes. A longer window is cut here, and the
/// export says so.
pub(crate) const EXPORT_ROW_CAP: usize = 500_000;

/// One export: its text, how many data rows it holds, and whether the row cap
/// cut it short. A cut export also ends with a `#` line saying so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Export {
    pub text: String,
    pub rows: usize,
    pub truncated: bool,
}

pub(crate) fn truncation_note(cap: usize) -> String {
    format!("# Truncated by Signal Loom at {cap} rows. Narrow the window to export the rest.\n")
}

/// The first line of a wide CSV export: `t_us`, then one column per name. A
/// name holding a comma, a double quote, CR or LF is quoted, and its quotes
/// doubled (RFC 4180).
pub(crate) fn csv_header<'a>(columns: impl IntoIterator<Item = &'a str>) -> String {
    let mut line = String::from("t_us");
    for name in columns {
        line.push(',');
        if name.contains([',', '"', '\r', '\n']) {
            line.push('"');
            line.push_str(&name.replace('"', "\"\""));
            line.push('"');
        } else {
            line.push_str(name);
        }
    }
    line.push('\n');
    line
}

impl IndexedLog {
    pub fn export_csv(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<Export> {
        self.export_csv_capped(names, t0_us, t1_us, EXPORT_ROW_CAP)
    }

    fn export_csv_capped(
        &self,
        names: &[String],
        t0_us: u64,
        t1_us: u64,
        cap: usize,
    ) -> Result<Export> {
        if names.is_empty() {
            return Err(Error::msg("export needs at least one signal"));
        }
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut indexes = Vec::new();
        for name in names {
            indexes.push(self.require_signal(name)?);
        }
        let mut out = csv_header(names.iter().map(String::as_str));
        let mut rows = 0usize;
        let mut truncated = false;
        self.replay(t0, t1, |step, held| {
            let Step::Record(rec) = step else {
                return true;
            };
            let mut dirty = false;
            self.touch(rec, held, |signal, _value| {
                if indexes.contains(&signal) {
                    dirty = true;
                }
            });
            if dirty {
                if rows == cap {
                    truncated = true;
                    return false;
                }
                out.push_str(&rec.t_us.to_string());
                for idx in &indexes {
                    out.push(',');
                    if let Some(value) = held.get(*idx).copied().flatten() {
                        out.push_str(&format!("{value:.6}"));
                    }
                }
                out.push('\n');
                rows += 1;
            }
            true
        })?;
        if rows == 0 {
            return Err(Error::msg("that window has no samples to export"));
        }
        if truncated {
            out.push_str(&truncation_note(cap));
        }
        Ok(Export {
            text: out,
            rows,
            truncated,
        })
    }

    pub fn export_slog(&self, t0_us: u64, t1_us: u64) -> Result<Export> {
        self.export_slog_capped(t0_us, t1_us, EXPORT_ROW_CAP)
    }

    fn export_slog_capped(&self, t0_us: u64, t1_us: u64, cap: usize) -> Result<Export> {
        let (t0, t1) = ordered_range(t0_us, t1_us);
        let mut out = String::from(
            "SLOGv1\n# Trimmed by Signal Loom. Synthetic or captured, this is only the selected window.\n",
        );
        let mut rows = 0usize;
        let mut truncated = false;
        self.scan_window(t0, t1, |phase, rec| {
            if phase == Phase::Lead {
                return true;
            }
            let line = match &rec.kind {
                RecKind::Frame { id, dlc, data, .. } => {
                    format!("F {} {id:X} {}\n", rec.t_us, hex_payload(data, *dlc))
                }
                RecKind::Event { label } => format!("E {} {label}\n", rec.t_us),
                RecKind::Sample { .. } => return true,
            };
            if rows == cap {
                truncated = true;
                return false;
            }
            out.push_str(&line);
            rows += 1;
            true
        })?;
        if rows == 0 {
            return Err(Error::msg("that window has no frames to export"));
        }
        if truncated {
            out.push_str(&truncation_note(cap));
        }
        Ok(Export {
            text: out,
            rows,
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::ramp_log;
    use super::*;
    use crate::map::SignalMap;

    #[test]
    fn a_signal_name_with_a_comma_is_quoted_in_the_csv_header() {
        let map = SignalMap::parse(
            r#"{"name":"dash","version":1,"messages":[{"id":"0x1A0","name":"Dash",
            "signals":[{"name":"Speed, km/h","startBit":0,"bitLength":16,"factor":0.25,"unit":"km/h"}]}]}"#,
        )
        .unwrap();
        let log = IndexedLog::open_bytes(
            b"SLOGv1\nF 0 1A0 800C881378640000\nF 10000 1A0 800C881378640000\n".to_vec(),
            Some(&map),
        )
        .unwrap();
        let export = log
            .export_csv(&["Speed, km/h".to_string()], 0, 20_000)
            .unwrap();
        assert_eq!(export.text.lines().next(), Some("t_us,\"Speed, km/h\""));
        assert!(!export.truncated);
    }

    #[test]
    fn a_csv_export_cut_at_the_row_cap_says_so() {
        let log = ramp_log();
        let names = ["Speed".to_string()];
        let whole = log.export_csv_capped(&names, 0, 1_000_000, 1001).unwrap();
        assert_eq!((whole.rows, whole.truncated), (1001, false));
        assert!(!whole.text.contains("Truncated"));
        let cut = log.export_csv_capped(&names, 0, 1_000_000, 3).unwrap();
        assert_eq!((cut.rows, cut.truncated), (3, true));
        assert_eq!(
            cut.text,
            "t_us,Speed\n0,0.000000\n1000,1.000000\n2000,2.000000\n# Truncated by Signal Loom at 3 rows. Narrow the window to export the rest.\n"
        );
    }

    #[test]
    fn a_slog_export_cut_at_the_row_cap_says_so() {
        let text = b"SLOGv1\nF 0 1A0 01\nF 1000 1A0 02\nF 2000 1A0 03\n".to_vec();
        let log = IndexedLog::open_bytes(text, None).unwrap();
        let whole = log.export_slog_capped(0, 2_000, 3).unwrap();
        assert_eq!((whole.rows, whole.truncated), (3, false));
        let cut = log.export_slog_capped(0, 2_000, 2).unwrap();
        assert_eq!((cut.rows, cut.truncated), (2, true));
        assert_eq!(cut.text.lines().count(), 5);
        assert_eq!(
            cut.text.lines().last(),
            Some("# Truncated by Signal Loom at 2 rows. Narrow the window to export the rest.")
        );
    }
}
