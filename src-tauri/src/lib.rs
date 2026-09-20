mod app_update;
mod harness;
mod runtime;
mod settings;
mod shim;

use std::sync::Arc;

use app_update::{cmd_check_app_update, cmd_download_app_update, updates_dir_path};
use harness::{
    cmd_check_dsh_update, cmd_list_installed_plugins, cmd_plugin_catalog, cmd_remove_plugin,
    cmd_runtime_info, cmd_safe_disable_all_plugins, cmd_update_dsh_runtime, default_plugin_ids,
    focus_main_or_harness, show_splash_window, start_harness, HarnessManager,
};
use settings::{
    cmd_complete_plugin_wizard, cmd_get_desktop_settings, cmd_get_npm_settings,
    cmd_set_autostart_pref, cmd_set_global_shortcut_pref, cmd_set_npm_registry,
    load_settings, DEFAULT_GLOBAL_SHORTCUT,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    Emitter,
};

#[cfg(desktop)]
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[tauri::command]
fn restart_harness(app: tauri::AppHandle, state: tauri::State<'_, Arc<HarnessManager>>) {
    show_splash_window(&app);
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
fn download_app_update(
    app: tauri::AppHandle,
    url: String,
) -> Result<serde_json::Value, String> {
    cmd_download_app_update(&app, url)
}

#[tauri::command]
fn get_updates_dir() -> Result<String, String> {
    updates_dir_path()
}

#[tauri::command]
fn get_desktop_settings() -> Result<serde_json::Value, String> {
    cmd_get_desktop_settings()
}

#[tauri::command]
fn get_plugin_catalog() -> Result<serde_json::Value, String> {
    cmd_plugin_catalog()
}

#[tauri::command]
fn complete_plugin_wizard(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<HarnessManager>>,
    selected: Vec<String>,
    use_recommended: bool,
) -> Result<serde_json::Value, String> {
    let ids = default_plugin_ids();
    let result = cmd_complete_plugin_wizard(selected, use_recommended, &ids)?;
    start_harness(app, state.inner().clone());
    Ok(result)
}

#[tauri::command]
fn open_plugin_wizard(app: tauri::AppHandle) -> Result<(), String> {
    show_splash_window(&app);
    let catalog = cmd_plugin_catalog()?;
    let plugins = catalog
        .get("plugins")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    let plugins_vec = plugins.as_array().cloned().unwrap_or_default();
    let _ = app.emit(
        "harness",
        harness::HarnessEvent::NeedsWizard {
            plugins: plugins_vec,
            message: "选择要安装的推荐插件（可跳过）".into(),
        },
    );
    Ok(())
}

#[tauri::command]
fn show_desktop_settings(app: tauri::AppHandle) -> Result<(), String> {
    show_splash_window(&app);
    Ok(())
}

#[tauri::command]
fn set_autostart(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let manager = app.autolaunch();
        if enabled {
            manager.enable().map_err(|e| e.to_string())?;
        } else {
            manager.disable().map_err(|e| e.to_string())?;
        }
    }
    cmd_set_autostart_pref(enabled)
}

#[tauri::command]
fn get_autostart_enabled(app: tauri::AppHandle) -> Result<bool, String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        return app.autolaunch().is_enabled().map_err(|e| e.to_string());
    }
    #[cfg(not(desktop))]
    {
        let _ = app;
        Ok(false)
    }
}

#[tauri::command]
fn set_global_shortcut(
    app: tauri::AppHandle,
    shortcut: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let result = cmd_set_global_shortcut_pref(shortcut, enabled)?;
    #[cfg(desktop)]
    apply_global_shortcut(&app)?;
    Ok(result)
}

#[tauri::command]
fn get_shim_status() -> Result<serde_json::Value, String> {
    shim::shim_status()
}

#[tauri::command]
fn install_cli_shim() -> Result<serde_json::Value, String> {
    shim::install_shim()
}

#[tauri::command]
fn remove_cli_shim() -> Result<serde_json::Value, String> {
    shim::remove_shim()
}

#[tauri::command]
fn set_cli_shim_enabled(enabled: bool) -> Result<serde_json::Value, String> {
    if enabled {
        shim::install_shim()
    } else {
        shim::remove_shim()
    }
}

#[cfg(desktop)]
fn apply_global_shortcut(app: &tauri::AppHandle) -> Result<(), String> {
    let _ = app.global_shortcut().unregister_all();
    let settings = load_settings();
    if !settings.global_shortcut_enabled {
        return Ok(());
    }
    let accel = settings
        .global_shortcut
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(DEFAULT_GLOBAL_SHORTCUT);
    app.global_shortcut()
        .on_shortcut(accel, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                focus_main_or_harness(app);
            }
        })
        .map_err(|e| format!("注册全局快捷键失败：{e}"))?;
    Ok(())
}

#[cfg(desktop)]
fn sync_autostart_from_settings(app: &tauri::AppHandle) {
    use tauri_plugin_autostart::ManagerExt;
    let want = load_settings().autostart;
    let manager = app.autolaunch();
    let current = manager.is_enabled().unwrap_or(false);
    if want && !current {
        let _ = manager.enable();
    } else if !want && current {
        let _ = manager.disable();
    }
}


#[tauri::command]
fn list_installed_plugins() -> Result<serde_json::Value, String> {
    cmd_list_installed_plugins()
}

#[tauri::command]
fn remove_plugin(name: String) -> Result<serde_json::Value, String> {
    cmd_remove_plugin(name)
}

#[tauri::command]
fn safe_disable_all_plugins() -> Result<serde_json::Value, String> {
    cmd_safe_disable_all_plugins()
}

#[tauri::command]
fn open_plugin_manager(app: tauri::AppHandle) -> Result<(), String> {
    show_splash_window(&app);
    let _ = app.emit("open-plugin-manager", serde_json::json!({
        "hint": "若因不兼容插件无法启动，可在此卸载后重启"
    }));
    Ok(())
}

fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let restart = MenuItem::with_id(app, "restart_harness", "重启 dsh", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "desktop_settings", "桌面设置", true, None::<&str>)?;
    let wizard = MenuItem::with_id(app, "plugin_wizard", "插件向导", true, None::<&str>)?;
    let plugin_mgr = MenuItem::with_id(app, "plugin_manager", "插件管理", true, None::<&str>)?;
    let edit = Submenu::with_items(
        app,
        "编辑",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let app_menu = Submenu::with_items(
        app,
        "DSH Desktop",
        true,
        &[&restart, &settings, &wizard, &plugin_mgr],
    )?;
    Menu::with_items(app, &[&edit, &app_menu])
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let manager = Arc::new(HarnessManager::new());
    let manager_for_setup = manager.clone();
    let manager_for_exit = manager.clone();
    let manager_for_menu = manager.clone();

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
            download_app_update,
            get_updates_dir,
            get_desktop_settings,
            get_plugin_catalog,
            complete_plugin_wizard,
            open_plugin_wizard,
            show_desktop_settings,
            set_autostart,
            get_autostart_enabled,
            set_global_shortcut,
            get_shim_status,
            install_cli_shim,
            remove_cli_shim,
            set_cli_shim_enabled,
            list_installed_plugins,
            remove_plugin,
            safe_disable_all_plugins,
            open_plugin_manager,
        ])
        .setup(move |app| {
            #[cfg(desktop)]
            {
                app.handle().plugin(
                    tauri_plugin_window_state::Builder::default().build(),
                )?;
                app.handle().plugin(tauri_plugin_autostart::init(
                    tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                    None,
                ))?;
                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new().build(),
                )?;
                sync_autostart_from_settings(app.handle());
                let _ = apply_global_shortcut(app.handle());
            }

            if let Ok(menu) = build_app_menu(app.handle()) {
                let _ = app.set_menu(menu);
            }

            let handle = app.handle().clone();
            let mgr = manager_for_menu.clone();
            app.on_menu_event(move |app, event| match event.id().as_ref() {
                "restart_harness" => {
                    show_splash_window(app);
                    start_harness(app.clone(), mgr.clone());
                }
                "desktop_settings" => {
                    show_splash_window(app);
                }
                "plugin_wizard" => {
                    show_splash_window(app);
                    if let Ok(catalog) = cmd_plugin_catalog() {
                        let plugins = catalog
                            .get("plugins")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!([]));
                        let plugins_vec = plugins.as_array().cloned().unwrap_or_default();
                        let _ = app.emit(
                            "harness",
                            harness::HarnessEvent::NeedsWizard {
                                plugins: plugins_vec,
                                message: "选择要安装的推荐插件（可跳过）".into(),
                            },
                        );
                    }
                }
                "plugin_manager" => {
                    show_splash_window(app);
                    let _ = app.emit(
                        "open-plugin-manager",
                        serde_json::json!({
                            "hint": "若因不兼容插件无法启动，可在此卸载后重启"
                        }),
                    );
                }
                _ => {}
            });

            let handle2 = handle.clone();
            start_harness(handle2, manager_for_setup);
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
