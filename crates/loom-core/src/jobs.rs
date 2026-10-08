//! The one session, and the background indexing job that fills it.
//!
//! Both adapters (the Tauri shell and the preview server) hold an
//! `Arc<Engine>`. A job holds the session lock for the whole build, so the
//! job endpoints never touch that lock while it runs: `progress` and `cancel`
//! read only the phase and the atomic counters of the `IndexControl`.

use crate::{Error, IndexControl, IndexStatus, Result, Session};
use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone, Default)]
enum Phase {
    #[default]
    Idle,
    Running,
    Ready,
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

    /// Run `f` with the session locked. Waits while a job is running.
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
            let outcome = match engine.session.lock() {
                Ok(mut session) => {
                    catch_unwind(AssertUnwindSafe(|| work(&mut session, &engine.control)))
                }
                Err(err) => Ok(Err(Error::msg(err.to_string()))),
            };
            if let Ok(mut phase) = engine.phase.lock() {
                *phase = match outcome {
                    Ok(Ok(())) => Phase::Ready,
                    Ok(Err(err)) => Phase::Failed(err.to_string()),
                    Err(payload) => Phase::Failed(panic_message(payload.as_ref())),
                };
            }
        });
        Ok(())
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
            Phase::Ready => match lock(&self.session) {
                Ok(session) => match session.summary() {
                    Ok(summary) => {
                        status.done = true;
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
                    *phase = Phase::Ready;
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
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

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
}
