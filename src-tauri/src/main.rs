#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod recorder;

use std::path::PathBuf;
use std::sync::Mutex;

use recorder::Recording;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
struct AppState {
    recording: Mutex<Option<Recording>>,
}

#[derive(Serialize, Deserialize, Default)]
struct Config {
    storage_path: Option<String>,
}

fn config_file(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("config.json"))
}

fn load_config(app: &AppHandle) -> Config {
    config_file(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn get_storage_path(app: AppHandle) -> Result<String, String> {
    if let Some(p) = load_config(&app).storage_path {
        return Ok(p);
    }
    let downloads = app.path().download_dir().map_err(|e| e.to_string())?;
    Ok(downloads.to_string_lossy().into_owned())
}

#[tauri::command]
fn set_storage_path(app: AppHandle, path: String) -> Result<(), String> {
    let file = config_file(&app)?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let config = Config { storage_path: Some(path) };
    let json = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(file, json).map_err(|e| e.to_string())
}

#[tauri::command]
fn start_recording(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut slot = state.recording.lock().unwrap();
    if slot.is_some() {
        return Err("Already recording".into());
    }

    let dir = PathBuf::from(get_storage_path(app.clone())?);
    if !dir.is_dir() {
        return Err(format!("Folder does not exist: {}", dir.display()));
    }
    let name = chrono::Local::now().format("Recording %Y-%m-%d %H-%M-%S.mp3").to_string();

    *slot = Some(Recording::start(app, dir.join(name))?);
    Ok(())
}

#[tauri::command]
async fn stop_recording(state: State<'_, AppState>) -> Result<String, String> {
    let recording = state.recording.lock().unwrap().take().ok_or("Not recording")?;
    let path = tauri::async_runtime::spawn_blocking(move || recording.stop())
        .await
        .map_err(|e| e.to_string())??;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn cancel_recording(state: State<AppState>) {
    if let Some(recording) = state.recording.lock().unwrap().take() {
        recording.cancel();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_storage_path,
            set_storage_path,
            start_recording,
            stop_recording,
            cancel_recording
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
