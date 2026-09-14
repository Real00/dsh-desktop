mod harness;
mod runtime;

use std::sync::Arc;

use harness::{
    cmd_check_dsh_update, cmd_runtime_info, cmd_update_dsh_runtime, start_harness, HarnessManager,
};

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
            runtime_info
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
