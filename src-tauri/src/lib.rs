use loom_core::{
    Engine, Export, IndexStatus, MathChannel, ProjectView, Query, StepDir, ThresholdTrigger,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;

type AppState = Arc<Engine>;

fn lift<T>(result: loom_core::Result<T>) -> Result<T, String> {
    result.map_err(|err| err.to_string())
}

/// A text export and the facts the UI needs to warn about a cut-short one.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportReply {
    text: String,
    rows: usize,
    truncated: bool,
}

impl From<Export> for ExportReply {
    fn from(export: Export) -> Self {
        Self {
            text: export.text,
            rows: export.rows,
            truncated: export.truncated,
        }
    }
}

/// What a save wrote: its size, and the same row facts as an export.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveReply {
    bytes: u64,
    rows: usize,
    truncated: bool,
}

impl From<Export> for SaveReply {
    fn from(export: Export) -> Self {
        Self {
            bytes: export.text.len() as u64,
            rows: export.rows,
            truncated: export.truncated,
        }
    }
}

#[tauri::command(async)]
fn open_sample(state: State<'_, AppState>) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| session.open_sample()))
}

#[tauri::command(async)]
fn begin_open_log(state: State<'_, AppState>, path: String) -> Result<(), String> {
    lift(state.begin(move |session, control| {
        session
            .open_path_controlled(PathBuf::from(path).as_path(), Some(control))
            .map(|_| ())
    }))
}

#[tauri::command(async)]
fn begin_open_map(state: State<'_, AppState>, path: String) -> Result<(), String> {
    lift(state.begin(move |session, control| {
        session
            .open_map_path_controlled(PathBuf::from(path).as_path(), Some(control))
            .map(|_| ())
    }))
}

#[tauri::command(async)]
fn begin_add_map(state: State<'_, AppState>, path: String, channel: u8) -> Result<(), String> {
    lift(state.begin(move |session, control| {
        session
            .add_map_path_controlled(PathBuf::from(path).as_path(), channel, Some(control))
            .map(|_| ())
    }))
}

#[tauri::command(async)]
fn index_progress(state: State<'_, AppState>) -> Result<IndexStatus, String> {
    lift(state.progress(true))
}

#[tauri::command(async)]
fn cancel_index(state: State<'_, AppState>) -> Result<IndexStatus, String> {
    lift(state.cancel())
}

#[tauri::command(async)]
fn query_series(
    state: State<'_, AppState>,
    query: Query,
) -> Result<Vec<loom_core::SeriesDto>, String> {
    lift(state.with_session(|session| session.query(&query)))
}

#[tauri::command(async)]
fn values_at(state: State<'_, AppState>, t_us: u64) -> Result<Vec<loom_core::ValueDto>, String> {
    lift(state.with_session(|session| session.values_at(t_us)))
}

#[tauri::command(async)]
fn frame_at(state: State<'_, AppState>, t_us: u64) -> Result<Option<loom_core::FrameDto>, String> {
    lift(state.with_session(|session| session.frame_at(t_us)))
}

#[tauri::command(async)]
fn step_frame(
    state: State<'_, AppState>,
    t_us: u64,
    direction: StepDir,
) -> Result<Option<loom_core::FrameDto>, String> {
    lift(state.with_session(|session| session.step(t_us, direction)))
}

#[tauri::command(async)]
fn open_project(
    state: State<'_, AppState>,
    path: String,
) -> Result<loom_core::ProjectOpen, String> {
    lift(state.with_session(|session| session.load_project_file(PathBuf::from(path).as_path())))
}

#[tauri::command(async)]
fn signal_stats(
    state: State<'_, AppState>,
    name: String,
    t0_us: u64,
    t1_us: u64,
) -> Result<loom_core::WindowStats, String> {
    lift(state.with_session(|session| session.stats(&name, t0_us, t1_us)))
}

#[tauri::command(async)]
fn export_csv(
    state: State<'_, AppState>,
    names: Vec<String>,
    t0_us: u64,
    t1_us: u64,
) -> Result<ExportReply, String> {
    lift(state.with_session(|session| {
        session
            .export_csv_report(&names, t0_us, t1_us)
            .map(ExportReply::from)
    }))
}

#[tauri::command(async)]
fn export_slog(state: State<'_, AppState>, t0_us: u64, t1_us: u64) -> Result<ExportReply, String> {
    lift(state.with_session(|session| {
        session
            .export_slog_report(t0_us, t1_us)
            .map(ExportReply::from)
    }))
}

/// Writes the export to a `.csv` file; the session refuses any other extension.
#[tauri::command(async)]
fn save_csv(
    state: State<'_, AppState>,
    path: String,
    names: Vec<String>,
    t0_us: u64,
    t1_us: u64,
) -> Result<SaveReply, String> {
    lift(state.with_session(|session| {
        session
            .save_csv(PathBuf::from(path).as_path(), &names, t0_us, t1_us)
            .map(SaveReply::from)
    }))
}

/// Writes the trimmed log to a `.slog` file; the session refuses any other extension.
#[tauri::command(async)]
fn save_slog(
    state: State<'_, AppState>,
    path: String,
    t0_us: u64,
    t1_us: u64,
) -> Result<SaveReply, String> {
    lift(state.with_session(|session| {
        session
            .save_slog(PathBuf::from(path).as_path(), t0_us, t1_us)
            .map(SaveReply::from)
    }))
}

#[tauri::command(async)]
fn set_math(
    state: State<'_, AppState>,
    channels: Vec<MathChannel>,
) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| session.set_math(channels)))
}

#[tauri::command(async)]
fn set_triggers(
    state: State<'_, AppState>,
    triggers: Vec<ThresholdTrigger>,
) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| session.set_triggers(triggers)))
}

#[tauri::command(async)]
fn set_timeout_factor(
    state: State<'_, AppState>,
    factor: f64,
) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| session.set_timeout_factor(factor)))
}

#[tauri::command(async)]
fn open_compare(state: State<'_, AppState>, path: String) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| session.open_compare_path(PathBuf::from(path).as_path())))
}

#[tauri::command(async)]
fn set_compare_offset(
    state: State<'_, AppState>,
    offset_us: i64,
) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| {
        session.set_compare_offset(offset_us);
        session.summary()
    }))
}

#[tauri::command(async)]
fn bus_load(
    state: State<'_, AppState>,
    t0_us: u64,
    t1_us: u64,
) -> Result<loom_core::BusLoad, String> {
    lift(state.with_session(|session| session.bus_load(t0_us, t1_us)))
}

#[tauri::command(async)]
fn capture_can(
    state: State<'_, AppState>,
    iface: String,
    duration_ms: u64,
) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| session.capture_socketcan(&iface, duration_ms)))
}

#[tauri::command(async)]
fn clear_compare(state: State<'_, AppState>) -> Result<loom_core::Summary, String> {
    lift(state.with_session(|session| {
        session.clear_compare();
        session.summary()
    }))
}

/// Saves the open deck with the UI's `view` of it. The session fills in what it owns.
#[tauri::command(async)]
fn write_project(
    state: State<'_, AppState>,
    path: String,
    view: ProjectView,
) -> Result<(), String> {
    lift(state.with_session(|session| session.write_project(PathBuf::from(path).as_path(), &view)))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new(Engine::new()))
        .invoke_handler(tauri::generate_handler![
            open_sample,
            begin_open_log,
            begin_open_map,
            begin_add_map,
            index_progress,
            cancel_index,
            query_series,
            values_at,
            frame_at,
            step_frame,
            open_project,
            write_project,
            signal_stats,
            export_csv,
            export_slog,
            save_csv,
            save_slog,
            set_math,
            set_triggers,
            set_timeout_factor,
            open_compare,
            set_compare_offset,
            clear_compare,
            bus_load,
            capture_can
        ])
        .run(tauri::generate_context!())
        .expect("error while running Signal Loom");
}
