mod app_update;
mod harness;
mod runtime;
mod settings;

use std::sync::Arc;

use app_update::{cmd_check_app_update, cmd_download_app_update};
use harness::{
    cmd_check_dsh_update, cmd_runtime_info, cmd_update_dsh_runtime, start_harness, HarnessManager,
};
use settings::{cmd_get_npm_settings, cmd_set_npm_registry};

#[tauri::command]
fn restart_harness(app: tauri::AppHandle, state: tauri::State<'_, Arc<HarnessManager>>) {
    start_harness(app, state.inner().clone());
}

#[tauri::command]
fn harness_url(state: tauri::State<'_, Arc<HarnessManager>>) -> Option<String> {
    state.current_url()
}

#[tauri::command]
fn check_dsh_update() -> Result<serde_json::Value, String> {
    cmd_check_dsh_update()
}

#[tauri::command]
fn update_dsh_runtime() -> Result<serde_json::Value, String> {
    cmd_update_dsh_runtime()
}

#[tauri::command]
fn runtime_info() -> Result<serde_json::Value, String> {
    cmd_runtime_info()
}

#[tauri::command]
fn get_npm_settings() -> Result<serde_json::Value, String> {
    cmd_get_npm_settings()
}

#[tauri::command]
fn set_npm_registry(registry: String) -> Result<serde_json::Value, String> {
    cmd_set_npm_registry(registry)
}

#[tauri::command]
fn check_app_update() -> Result<serde_json::Value, String> {
    cmd_check_app_update()
}

#[tauri::command]
fn download_app_update(url: String) -> Result<serde_json::Value, String> {
    cmd_download_app_update(url)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let manager = Arc::new(HarnessManager::new());
    let manager_for_setup = manager.clone();
    let manager_for_exit = manager.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(manager)
        .invoke_handler(tauri::generate_handler![
            restart_harness,
            harness_url,
            check_dsh_update,
            update_dsh_runtime,
            runtime_info,
            get_npm_settings,
            set_npm_registry,
            check_app_update,
            download_app_update
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            start_harness(handle, manager_for_setup);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |_app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                manager_for_exit.stop();
            }
        });
}
