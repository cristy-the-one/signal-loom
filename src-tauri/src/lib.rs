use loom_core::{
    IndexControl, IndexStatus, MathChannel, ProjectFile, Query, Session, StepDir, ThresholdTrigger,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::State;

struct AppState {
    session: Arc<Mutex<Session>>,
    control: Arc<IndexControl>,
    phase: Arc<Mutex<Phase>>,
}

enum Phase {
    Idle,
    Running,
    Ready,
    Failed(String),
}

fn lock(state: &AppState) -> Result<std::sync::MutexGuard<'_, Session>, String> {
    state.session.lock().map_err(|err| err.to_string())
}

fn lift<T>(result: loom_core::Result<T>) -> Result<T, String> {
    result.map_err(|err| err.to_string())
}

#[tauri::command]
fn open_sample(state: State<'_, AppState>) -> Result<loom_core::Summary, String> {
    lift(lock(&state)?.open_sample())
}

#[tauri::command]
fn open_log(state: State<'_, AppState>, path: String) -> Result<loom_core::Summary, String> {
    lift(lock(&state)?.open_path(PathBuf::from(path).as_path()))
}

#[tauri::command]
fn begin_open_log(state: State<'_, AppState>, path: String) -> Result<(), String> {
    begin(&state, move |session, control| {
        session
            .open_path_controlled(PathBuf::from(path).as_path(), Some(control))
            .map(|_| ())
    })
}

#[tauri::command]
fn begin_open_map(state: State<'_, AppState>, path: String) -> Result<(), String> {
    begin(&state, move |session, control| {
        session
            .open_map_path_controlled(PathBuf::from(path).as_path(), Some(control))
            .map(|_| ())
    })
}

#[tauri::command]
fn begin_add_map(state: State<'_, AppState>, path: String, channel: u8) -> Result<(), String> {
    begin(&state, move |session, control| {
        session
            .add_map_path_controlled(PathBuf::from(path).as_path(), channel, Some(control))
            .map(|_| ())
    })
}

#[tauri::command]
fn index_progress(state: State<'_, AppState>) -> Result<IndexStatus, String> {
    progress(&state)
}

#[tauri::command]
fn cancel_index(state: State<'_, AppState>) -> Result<IndexStatus, String> {
    state.control.request_cancel();
    progress(&state)
}

fn begin(
    state: &AppState,
    work: impl FnOnce(&mut Session, &IndexControl) -> loom_core::Result<()> + Send + 'static,
) -> Result<(), String> {
    {
        let mut phase = state.phase.lock().map_err(|err| err.to_string())?;
        if matches!(*phase, Phase::Running) {
            return Err("an index is already running".into());
        }
        state.control.reset(0);
        *phase = Phase::Running;
    }
    let session = Arc::clone(&state.session);
    let control = Arc::clone(&state.control);
    let phase = Arc::clone(&state.phase);
    std::thread::spawn(move || {
        let result = {
            let mut session = match session.lock() {
                Ok(guard) => guard,
                Err(err) => {
                    if let Ok(mut phase) = phase.lock() {
                        *phase = Phase::Failed(err.to_string());
                    }
                    return;
                }
            };
            work(&mut session, &control)
        };
        if let Ok(mut phase) = phase.lock() {
            *phase = match result {
                Ok(()) => Phase::Ready,
                Err(err) => Phase::Failed(err.to_string()),
            };
        }
    });
    Ok(())
}

fn progress(state: &AppState) -> Result<IndexStatus, String> {
    let mut phase = state.phase.lock().map_err(|err| err.to_string())?;
    let (bytes_done, bytes_total, frames, skipped) = state.control.snapshot();
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
    match std::mem::replace(&mut *phase, Phase::Idle) {
        Phase::Idle => status.idle = true,
        Phase::Running => {
            *phase = Phase::Running;
            status.running = true;
        }
        Phase::Ready => match state.session.lock() {
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
                return Err(err.to_string());
            }
        },
        Phase::Failed(message) => {
            status.done = true;
            status.error = Some(message);
        }
    }
    Ok(status)
}

#[tauri::command]
fn open_signal_map(state: State<'_, AppState>, path: String) -> Result<loom_core::Summary, String> {
    lift(lock(&state)?.open_map_path(PathBuf::from(path).as_path()))
}

#[tauri::command]
fn query_series(
    state: State<'_, AppState>,
    query: Query,
) -> Result<Vec<loom_core::SeriesDto>, String> {
    lift(lock(&state)?.query(&query))
}

#[tauri::command]
fn values_at(state: State<'_, AppState>, t_us: u64) -> Result<Vec<loom_core::ValueDto>, String> {
    lift(lock(&state)?.values_at(t_us))
}

#[tauri::command]
fn frame_at(state: State<'_, AppState>, t_us: u64) -> Result<Option<loom_core::FrameDto>, String> {
    lift(lock(&state)?.frame_at(t_us))
}

#[tauri::command]
fn step_frame(
    state: State<'_, AppState>,
    t_us: u64,
    direction: StepDir,
) -> Result<Option<loom_core::FrameDto>, String> {
    lift(lock(&state)?.step(t_us, direction))
}

#[tauri::command]
fn open_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<loom_core::ProjectOpen, String> {
    lift(lock(&state)?.load_project_file(PathBuf::from(path).as_path()))
}

#[tauri::command]
fn signal_stats(
    state: State<'_, AppState>,
    name: String,
    t0_us: u64,
    t1_us: u64,
) -> Result<loom_core::WindowStats, String> {
    lift(lock(&state)?.stats(&name, t0_us, t1_us))
}

#[tauri::command]
fn export_csv(
    state: State<'_, AppState>,
    names: Vec<String>,
    t0_us: u64,
    t1_us: u64,
) -> Result<String, String> {
    lift(lock(&state)?.export_csv(&names, t0_us, t1_us))
}

#[tauri::command]
fn export_slog(state: State<'_, AppState>, t0_us: u64, t1_us: u64) -> Result<String, String> {
    lift(lock(&state)?.export_slog(t0_us, t1_us))
}

#[tauri::command]
fn set_math(
    state: State<'_, AppState>,
    channels: Vec<MathChannel>,
) -> Result<loom_core::Summary, String> {
    lift(lock(&state)?.set_math(channels))
}

#[tauri::command]
fn set_triggers(
    state: State<'_, AppState>,
    triggers: Vec<ThresholdTrigger>,
) -> Result<loom_core::Summary, String> {
    lift(lock(&state)?.set_triggers(triggers))
}

#[tauri::command]
fn open_compare(state: State<'_, AppState>, path: String) -> Result<loom_core::Summary, String> {
    lift(lock(&state)?.open_compare_path(PathBuf::from(path).as_path()))
}

#[tauri::command]
fn set_compare_offset(
    state: State<'_, AppState>,
    offset_us: i64,
) -> Result<loom_core::Summary, String> {
    let mut session = lock(&state)?;
    session.set_compare_offset(offset_us);
    lift(session.summary())
}

#[tauri::command]
fn bus_load(
    state: State<'_, AppState>,
    t0_us: u64,
    t1_us: u64,
) -> Result<loom_core::BusLoad, String> {
    lift(lock(&state)?.bus_load(t0_us, t1_us))
}

#[tauri::command]
fn capture_can(
    state: State<'_, AppState>,
    iface: String,
    duration_ms: u64,
) -> Result<loom_core::Summary, String> {
    lift(lock(&state)?.capture_socketcan(&iface, duration_ms))
}

#[tauri::command]
fn clear_compare(state: State<'_, AppState>) -> Result<loom_core::Summary, String> {
    let mut session = lock(&state)?;
    session.clear_compare();
    lift(session.summary())
}

#[tauri::command]
fn write_project(
    state: State<'_, AppState>,
    path: String,
    project: ProjectFile,
) -> Result<(), String> {
    lift(lock(&state)?.write_project(PathBuf::from(path).as_path(), &project))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            session: Arc::new(Mutex::new(Session::new())),
            control: Arc::new(IndexControl::default()),
            phase: Arc::new(Mutex::new(Phase::Idle)),
        })
        .invoke_handler(tauri::generate_handler![
            open_sample,
            open_log,
            begin_open_log,
            begin_open_map,
            begin_add_map,
            index_progress,
            cancel_index,
            open_signal_map,
            query_series,
            values_at,
            frame_at,
            step_frame,
            open_project,
            write_project,
            signal_stats,
            export_csv,
            export_slog,
            set_math,
            set_triggers,
            open_compare,
            set_compare_offset,
            clear_compare,
            bus_load,
            capture_can
        ])
        .run(tauri::generate_context!())
        .expect("error while running Signal Loom");
}
