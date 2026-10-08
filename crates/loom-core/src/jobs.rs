//! The one session, and the background jobs that fill it.
//!
//! Both adapters (the Tauri shell and the preview server) hold an
//! `Arc<Engine>`. A job that indexes holds the session lock for the whole
//! build, so the job endpoints never touch that lock while it runs: `progress`
//! and `cancel` read only the phase and the atomic counters of the
//! `IndexControl`. A capture holds no lock while it listens; it takes the lock
//! only to open what it recorded.

use crate::dto::OpenedProject;
use crate::{Error, IndexControl, IndexStatus, ProjectOpen, Result, Session};
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Default)]
enum Phase {
    #[default]
    Idle,
    Running,
    /// Finished; the project is set when the job opened one.
    Ready(Option<Box<OpenedProject>>),
    Failed(String),
}

/// The session behind one lock, plus the state of its background job.
#[derive(Default)]
pub struct Engine {
    session: Mutex<Session>,
    control: IndexControl,
    phase: Mutex<Phase>,
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `f` with the session locked. Waits while a job holds the lock.
    pub fn with_session<T>(&self, f: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
        f(&mut *lock(&self.session)?)
    }

    /// Start `work` on a background thread. It holds the session lock until it
    /// returns. Refuses while another job runs. A panic in `work` is recorded
    /// as a failed job, not propagated.
    pub fn begin(
        self: &Arc<Self>,
        work: impl FnOnce(&mut Session, &IndexControl) -> Result<()> + Send + 'static,
    ) -> Result<()> {
        self.start(move |engine| engine.locked(work).map(|()| None))
    }

    /// Open the project at `path` as a job. The finished status carries the
    /// project and its warnings beside the summary.
    pub fn begin_project_file(self: &Arc<Self>, path: PathBuf) -> Result<()> {
        self.begin_project(move |session, control| {
            session.load_project_file_controlled(&path, Some(control))
        })
    }

    /// Open the project in `json` as a job. `base` resolves its relative paths.
    pub fn begin_project_json(self: &Arc<Self>, json: String, base: Option<PathBuf>) -> Result<()> {
        self.begin_project(move |session, control| {
            session.load_project_json_controlled(&json, base.as_deref(), Some(control))
        })
    }

    fn begin_project(
        self: &Arc<Self>,
        work: impl FnOnce(&mut Session, &IndexControl) -> Result<ProjectOpen> + Send + 'static,
    ) -> Result<()> {
        self.start(move |engine| {
            engine.locked(work).map(|open| {
                Some(Box::new(OpenedProject {
                    project: open.project,
                    warnings: open.warnings,
                }))
            })
        })
    }

    /// Listen on a SocketCAN interface as a job, then open what was recorded as
    /// the log. The capture holds no session lock; its progress is the elapsed
    /// share of `duration_ms`, and cancel ends it early.
    pub fn begin_capture(self: &Arc<Self>, iface: String, duration_ms: u64) -> Result<()> {
        self.start(move |engine| {
            let text = crate::socketcan::capture_slog(&iface, duration_ms, Some(&engine.control))?;
            engine
                .locked(|session, _| session.open_capture(&iface, text))
                .map(|_| None)
        })
    }

    fn start(
        self: &Arc<Self>,
        run: impl FnOnce(&Engine) -> Result<Option<Box<OpenedProject>>> + Send + 'static,
    ) -> Result<()> {
        {
            let mut phase = lock(&self.phase)?;
            if matches!(*phase, Phase::Running) {
                return Err(Error::msg("an index is already running"));
            }
            self.control.reset(0);
            *phase = Phase::Running;
        }
        let engine = Arc::clone(self);
        std::thread::spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| run(&engine)));
            if let Ok(mut phase) = engine.phase.lock() {
                *phase = match outcome {
                    Ok(Ok(opened)) => Phase::Ready(opened),
                    Ok(Err(err)) => Phase::Failed(err.to_string()),
                    Err(payload) => Phase::Failed(panic_message(payload.as_ref())),
                };
            }
        });
        Ok(())
    }

    /// Run `work` with the session locked. A panic in `work` becomes an error
    /// while the lock is still held, so it does not poison the session.
    fn locked<T>(&self, work: impl FnOnce(&mut Session, &IndexControl) -> Result<T>) -> Result<T> {
        let mut session = lock(&self.session)?;
        catch_unwind(AssertUnwindSafe(|| work(&mut session, &self.control)))
            .unwrap_or_else(|payload| Err(Error::msg(panic_message(payload.as_ref()))))
    }

    /// `take` hands a finished result to the caller and resets to idle. Cancel
    /// only peeks, so a result that lands as Cancel is clicked still reaches the poll.
    pub fn progress(&self, take: bool) -> Result<IndexStatus> {
        let mut phase = lock(&self.phase)?;
        let (bytes_done, bytes_total, frames, skipped) = self.control.snapshot();
        let mut status = IndexStatus {
            running: false,
            done: false,
            idle: false,
            bytes_done,
            bytes_total,
            frames,
            skipped,
            summary: None,
            project: None,
            error: None,
        };
        let current = if take {
            std::mem::replace(&mut *phase, Phase::Idle)
        } else {
            phase.clone()
        };
        match current {
            Phase::Idle => status.idle = true,
            Phase::Running => {
                *phase = Phase::Running;
                status.running = true;
            }
            Phase::Ready(opened) => match lock(&self.session) {
                Ok(session) => match session.summary() {
                    Ok(summary) => {
                        status.done = true;
                        status.project = opened.map(|project| *project);
                        status.bytes_done = summary.bytes;
                        status.frames = summary.frame_count;
                        status.skipped = summary.skipped_records;
                        status.summary = Some(summary);
                    }
                    Err(err) => {
                        status.done = true;
                        status.error = Some(err.to_string());
                    }
                },
                Err(err) => {
                    *phase = Phase::Ready(opened);
                    return Err(err);
                }
            },
            Phase::Failed(message) => {
                status.done = true;
                status.error = Some(message);
            }
        }
        Ok(status)
    }

    /// Ask the running job to stop, then report its status without consuming it.
    pub fn cancel(&self) -> Result<IndexStatus> {
        self.control.request_cancel();
        self.progress(false)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex.lock().map_err(|err| Error::msg(err.to_string()))
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned());
    match detail {
        Some(detail) => format!("indexing stopped unexpectedly: {detail}"),
        None => "indexing stopped unexpectedly".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::Query;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures")
            .join(name)
    }

    fn fixture_len(name: &str) -> u64 {
        std::fs::metadata(fixture(name)).unwrap().len()
    }

    fn project_json(log: &str, compare: Option<&str>) -> String {
        serde_json::json!({
            "format": "signal-loom",
            "version": 1,
            "logPath": fixture(log),
            "signalMapPath": fixture("cluster.map.json"),
            "comparePath": compare.map(fixture),
            "view": { "playheadUs": 0, "spanUs": 1_000_000, "plotted": [] },
        })
        .to_string()
    }

    fn compare_values(engine: &Engine) -> Vec<f64> {
        let query = Query {
            t0_us: 12_000_000,
            t1_us: 12_500_000,
            signals: vec!["VehicleSpeed".to_string()],
            max_points: 50,
            include_compare: true,
        };
        let series = engine
            .with_session(|session| session.query(&query))
            .unwrap();
        series
            .into_iter()
            .filter(|series| series.name.ends_with(" · B"))
            .flat_map(|series| series.points.into_iter().map(|point| point.v))
            .collect()
    }

    /// Start a job while the test holds the session lock and cancel it before
    /// it can run, so the cancel always lands mid-job.
    fn begin_cancelled(engine: &Arc<Engine>, start: impl FnOnce(&Arc<Engine>) -> Result<()>) {
        engine
            .with_session(|_| {
                start(engine)?;
                engine.cancel().map(|_| ())
            })
            .unwrap();
    }

    fn wait_until_settled(engine: &Engine) -> IndexStatus {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let status = engine.progress(true).unwrap();
            if !status.running {
                return status;
            }
            assert!(Instant::now() < deadline, "job never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn progress_and_cancel_do_not_wait_for_a_running_job() {
        let engine = Arc::new(Engine::new());
        let (started_tx, started_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        engine
            .begin(move |session, _control| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                session.open_sample().map(|_| ())
            })
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(30)).unwrap();

        let status = engine.progress(true).unwrap();
        assert!(status.running);
        assert!(!status.done && !status.idle);
        let status = engine.cancel().unwrap();
        assert!(status.running);
        assert_eq!(
            engine.begin(|_, _| Ok(())).unwrap_err().to_string(),
            "an index is already running"
        );

        release_tx.send(()).unwrap();
        let status = wait_until_settled(&engine);
        assert!(status.done);
        assert_eq!(status.error, None);
        let summary = status.summary.expect("finished job carries a summary");
        assert!(summary.frame_count > 0);
        assert_eq!(status.frames, summary.frame_count);
        assert!(engine.progress(true).unwrap().idle);
    }

    #[test]
    fn a_panicking_job_fails_and_leaves_the_engine_usable() {
        let engine = Arc::new(Engine::new());
        engine
            .begin(|_, _| panic!("malformed record at byte 12"))
            .unwrap();
        let status = wait_until_settled(&engine);
        assert!(status.done);
        assert_eq!(
            status.error.as_deref(),
            Some("indexing stopped unexpectedly: malformed record at byte 12")
        );
        assert!(status.summary.is_none());

        let summary = engine
            .with_session(|session| session.open_sample())
            .unwrap();
        assert!(summary.frame_count > 0);
        engine
            .begin(|session, _| session.open_sample().map(|_| ()))
            .unwrap();
        let status = wait_until_settled(&engine);
        assert!(status.done);
        assert_eq!(status.error, None);
        assert!(status.summary.is_some());
    }

    #[test]
    fn a_project_job_reports_progress_and_hands_over_the_project() {
        let engine = Arc::new(Engine::new());
        let text = project_json("cluster_drive.slog", Some("cluster_drive.slog"));
        engine.begin_project_json(text, None).unwrap();
        let status = wait_until_settled(&engine);
        assert_eq!(status.error, None);
        assert!(status.done);
        let summary = status.summary.expect("finished job carries a summary");
        assert_eq!(summary.log_label, "cluster_drive.slog");
        let opened = status.project.expect("a project job carries the project");
        assert!(opened.warnings.is_empty(), "{:?}", opened.warnings);
        assert_eq!(opened.project.compare_offset_us, 0);

        let (done, total, frames, _) = engine.control.snapshot();
        assert_eq!(done, total);
        assert_eq!(total, fixture_len("cluster_drive.slog"));
        assert_eq!(frames, summary.frame_count);
        assert!(!compare_values(&engine).is_empty());
    }

    #[test]
    fn a_cancelled_project_job_leaves_the_open_deck_unchanged() {
        let engine = Arc::new(Engine::new());
        let before = engine
            .with_session(|session| {
                session.open_path(&fixture("cluster_drive.slog"))?;
                session.summary()
            })
            .unwrap();

        let text = project_json("hypercar_lap.slog", None);
        begin_cancelled(&engine, |engine| engine.begin_project_json(text, None));
        let status = wait_until_settled(&engine);
        assert_eq!(status.error.as_deref(), Some("indexing cancelled"));
        assert!(status.project.is_none() && status.summary.is_none());

        let after = engine.with_session(|session| session.summary()).unwrap();
        assert_eq!(after.log_label, "cluster_drive.slog");
        assert_eq!(after.frame_count, before.frame_count);
        assert_eq!(after.bytes, before.bytes);
    }

    #[test]
    fn a_compare_job_opens_the_log_and_a_cancelled_one_keeps_the_old_compare() {
        let engine = Arc::new(Engine::new());
        let open = |name: &'static str| {
            let engine = Arc::clone(&engine);
            move || {
                engine.begin(move |session, control| {
                    session
                        .open_compare_path_controlled(&fixture(name), Some(control))
                        .map(|_| ())
                })
            }
        };
        engine
            .with_session(|session| {
                session.open_path(&fixture("cluster_drive.slog"))?;
                session.open_map_path(&fixture("cluster.map.json"))
            })
            .unwrap();
        assert!(compare_values(&engine).is_empty());

        open("cluster_drive.slog")().unwrap();
        let status = wait_until_settled(&engine);
        assert_eq!(status.error, None);
        let first = compare_values(&engine);
        assert!(!first.is_empty());

        begin_cancelled(&engine, |_| open("hypercar_lap.slog")());
        let status = wait_until_settled(&engine);
        assert_eq!(status.error.as_deref(), Some("indexing cancelled"));
        assert_eq!(compare_values(&engine), first);
    }

    #[test]
    fn a_failed_capture_job_reports_why_and_keeps_the_open_log() {
        let engine = Arc::new(Engine::new());
        engine
            .with_session(|session| session.open_path(&fixture("cluster_drive.slog")))
            .unwrap();
        engine.begin_capture("can0;reboot".to_string(), 10).unwrap();
        let status = wait_until_settled(&engine);
        assert!(status.done && status.summary.is_none());
        assert!(
            status
                .error
                .as_deref()
                .is_some_and(|text| text.contains("interface name")),
            "{:?}",
            status.error
        );
        let summary = engine.with_session(|session| session.summary()).unwrap();
        assert_eq!(summary.log_label, "cluster_drive.slog");
    }
}
