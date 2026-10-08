//! Exports of the open log: CSV of signals or math channels, and a trimmed
//! SLOG, as text or as a file on disk.

use super::deck::Deck;
use super::Session;
use crate::error::{Error, Result};
use crate::index::{csv_header, truncation_note, Export, IndexedLog, EXPORT_ROW_CAP};
use crate::project;
use std::collections::BTreeSet;
use std::path::Path;

impl Session {
    pub fn export_csv(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<String> {
        Ok(self.export_csv_report(names, t0_us, t1_us)?.text)
    }

    /// The CSV export with its row count and whether the row cap cut it short.
    /// Physical signals and math channels export on their own, not together.
    pub fn export_csv_report(&self, names: &[String], t0_us: u64, t1_us: u64) -> Result<Export> {
        let (math, physical): (Vec<String>, Vec<String>) = names
            .iter()
            .cloned()
            .partition(|name| self.deck.is_math(name));
        match (math.is_empty(), physical.is_empty()) {
            (true, _) => self.log()?.export_csv(&physical, t0_us, t1_us),
            (false, true) => {
                export_math_csv(&self.deck, self.log()?, &math, t0_us, t1_us, EXPORT_ROW_CAP)
            }
            (false, false) => Err(Error::invalid(
                "export the math channel on its own, or export physical signals on their own",
            )),
        }
    }

    pub fn export_slog(&self, t0_us: u64, t1_us: u64) -> Result<String> {
        Ok(self.export_slog_report(t0_us, t1_us)?.text)
    }

    /// The trimmed SLOG with its row count and whether the row cap cut it short.
    pub fn export_slog_report(&self, t0_us: u64, t1_us: u64) -> Result<Export> {
        self.log()?.export_slog(t0_us, t1_us)
    }

    /// Export to a `.csv` file on disk. Returns what was written: its text and
    /// whether the row cap cut it short.
    pub fn save_csv(
        &self,
        path: &Path,
        names: &[String],
        t0_us: u64,
        t1_us: u64,
    ) -> Result<Export> {
        let export = self.export_csv_report(names, t0_us, t1_us)?;
        project::write_as(path, "csv", &export.text)?;
        Ok(export)
    }

    /// Trim the log to a `.slog` file on disk. Returns what was written.
    pub fn save_slog(&self, path: &Path, t0_us: u64, t1_us: u64) -> Result<Export> {
        let export = self.export_slog_report(t0_us, t1_us)?;
        project::write_as(path, "slog", &export.text)?;
        Ok(export)
    }
}

/// A wide CSV of math channels. The 4,000,000-sample limit on the raw signals
/// is enforced by `IndexedLog::samples`; `cap` limits the rows written.
fn export_math_csv(
    deck: &Deck,
    log: &IndexedLog,
    names: &[String],
    t0_us: u64,
    t1_us: u64,
    cap: usize,
) -> Result<Export> {
    let mut columns = Vec::new();
    for name in names {
        columns.push(deck.math_series(log, name, t0_us, t1_us)?);
    }
    let mut times = BTreeSet::new();
    for series in &columns {
        for (t, _) in &series.points {
            times.insert(*t);
        }
    }
    let truncated = times.len() > cap;
    let mut out = csv_header(names.iter().map(String::as_str));
    let mut cursors = vec![0usize; columns.len()];
    let mut rows = 0usize;
    for t in times.into_iter().take(cap) {
        out.push_str(&t.to_string());
        for (series, cursor) in columns.iter().zip(cursors.iter_mut()) {
            out.push(',');
            while *cursor < series.points.len() && series.points[*cursor].0 < t {
                *cursor += 1;
            }
            if let Some((_, value)) = series.points.get(*cursor).filter(|(stamp, _)| *stamp == t) {
                out.push_str(&format!("{value:.6}"));
            }
        }
        out.push('\n');
        rows += 1;
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

#[cfg(test)]
mod tests {
    use super::super::test_support::{math_points, math_session, RPM_LOG, RPM_MAP};
    use super::*;
    use crate::project::MathChannel;

    const COMMA_MAP: &str = r#"{"name":"dash","version":1,"messages":[{"id":"0x1A0","name":"Dash",
        "signals":[{"name":"Speed, km/h","startBit":0,"bitLength":16,"factor":0.25,"unit":"km/h"}]}]}"#;

    #[test]
    fn exported_math_values_are_the_plotted_values() {
        let session = math_session(&[("Diff", "A - B"), ("Smooth", "lp(A, 0.5)")]);
        let csv = session.export_csv(&["Diff".to_string()], 0, 9_000).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 19);
        assert_eq!(&lines[..3], ["t_us,Diff", "500,9.000000", "1000,11.000000"]);
        assert_eq!(lines[9], "4500,-27.000000");
        for (t, v) in math_points(&session, "Diff", 0, 1000) {
            assert!(csv.contains(&format!("\n{t},{v:.6}\n")), "{t}");
        }

        let csv = session
            .export_csv(&["Smooth".to_string()], 0, 9_000)
            .unwrap();
        assert!(csv.contains("\n3000,20.500000\n"));
        assert!(csv.contains("\n8000,12.296875\n"));
    }

    #[test]
    fn a_signal_name_with_a_comma_is_quoted_in_the_export_header() {
        let mut session = Session::new();
        session
            .open_bytes("main.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(COMMA_MAP).unwrap();
        let export = session
            .export_csv_report(&["Speed, km/h".to_string()], 0, 20_000)
            .unwrap();
        assert_eq!(export.text.lines().next(), Some("t_us,\"Speed, km/h\""));
        assert!(!export.truncated);
    }

    #[test]
    fn a_math_channel_name_with_a_quote_is_escaped_in_the_export_header() {
        let mut session = Session::new();
        session
            .open_bytes("main.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session
            .set_math(vec![MathChannel {
                name: "A\"B".to_string(),
                expr: "EngineRPM * 2".to_string(),
                unit: String::new(),
            }])
            .unwrap();
        let csv = session
            .export_csv(&["A\"B".to_string()], 0, 20_000)
            .unwrap();
        assert_eq!(csv.lines().next(), Some("t_us,\"A\"\"B\""));
    }

    #[test]
    fn a_math_export_cut_at_the_row_cap_says_so() {
        let mut session = Session::new();
        session
            .open_bytes("main.slog", RPM_LOG.as_bytes().to_vec())
            .unwrap();
        session.open_map_json(RPM_MAP).unwrap();
        session
            .set_math(vec![MathChannel {
                name: "Double".to_string(),
                expr: "EngineRPM * 2".to_string(),
                unit: String::new(),
            }])
            .unwrap();
        let names = ["Double".to_string()];
        let log = session.log().unwrap();
        let whole = export_math_csv(&session.deck, log, &names, 0, 20_000, 2).unwrap();
        assert_eq!((whole.rows, whole.truncated), (2, false));
        let cut = export_math_csv(&session.deck, log, &names, 0, 20_000, 1).unwrap();
        assert_eq!((cut.rows, cut.truncated), (1, true));
        assert_eq!(
            cut.text,
            "t_us,Double\n0,1600.000000\n# Truncated by Signal Loom at 1 rows. Narrow the window to export the rest.\n"
        );
    }
}
