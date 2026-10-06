use loom_core::{ProjectFile, Query, Session, StepDir};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

struct AppState {
    session: Mutex<Session>,
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
            session: Mutex::new(Session::new()),
        })
        .invoke_handler(tauri::generate_handler![
            open_sample,
            open_log,
            open_signal_map,
            query_series,
            values_at,
            frame_at,
            step_frame,
            open_project,
            write_project
        ])
        .run(tauri::generate_context!())
        .expect("error while running Signal Loom");
}
