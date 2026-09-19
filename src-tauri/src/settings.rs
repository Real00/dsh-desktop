use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::runtime::{desktop_home, runtime_dir};

pub const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmjs.org";
pub const NPMMIRROR_REGISTRY: &str = "https://registry.npmmirror.com";
pub const DEFAULT_GLOBAL_SHORTCUT: &str = "CommandOrControl+Shift+D";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_registry: Option<String>,
    /// First-run plugin wizard finished (or migrated for existing installs).
    #[serde(default)]
    pub wizard_completed: bool,
    /// Plugin ids selected in the wizard (install missing from this set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_plugins: Option<Vec<String>>,
    /// Launch app at login (synced with autostart plugin).
    #[serde(default)]
    pub autostart: bool,
    /// Optional global shortcut accelerator string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_shortcut: Option<String>,
    /// Whether the global shortcut is active.
    #[serde(default)]
    pub global_shortcut_enabled: bool,
    /// Whether the user-local `dsh` PATH shim should exist.
    #[serde(default)]
    pub cli_shim_enabled: bool,
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(desktop_home()?.join("settings.json"))
}

pub fn load_settings() -> Settings {
    let Ok(path) = settings_path() else {
        return Settings::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Settings::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    let path = settings_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(&path, format!("{body}\n")).map_err(|e| e.to_string())
}

/// Returns configured custom registry, or None when using npm default.
pub fn npm_registry() -> Option<String> {
    load_settings()
        .npm_registry
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .filter(|s| s != DEFAULT_NPM_REGISTRY)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn web_profile_dir() -> Option<PathBuf> {
    Some(home_dir()?.join(".dsh").join("profiles").join("web"))
}

fn bootstrap_marker_exists() -> bool {
    home_dir()
        .map(|h| h.join(".dsh-desktop").join("bootstrap-plugins.json").is_file())
        .unwrap_or(false)
}

fn runtime_ready_quick() -> bool {
    crate::runtime::runtime_bin_js()
        .map(|p| p.is_file())
        .unwrap_or(false)
}

/// Existing installs (pre-wizard) skip the wizard once.
pub fn migrate_wizard_if_needed(default_plugin_ids: &[&str]) {
    let mut settings = load_settings();
    if settings.wizard_completed {
        return;
    }
    if bootstrap_marker_exists() || runtime_ready_quick() {
        settings.wizard_completed = true;
        if settings.selected_plugins.is_none() {
            settings.selected_plugins = Some(
                default_plugin_ids
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            );
        }
        let _ = save_settings(&settings);
    }
}

fn write_or_clear_npmrc(dir: &Path, registry: Option<&str>) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let npmrc = dir.join(".npmrc");
    match registry {
        Some(url) if !url.is_empty() && url != DEFAULT_NPM_REGISTRY => {
            let existing = fs::read_to_string(&npmrc).unwrap_or_default();
            let mut lines: Vec<String> = existing
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    !t.starts_with("registry=") && !t.starts_with("registry =")
                })
                .map(|l| l.to_string())
                .collect();
            lines.push(format!("registry={url}"));
            let body = if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            };
            fs::write(&npmrc, body).map_err(|e| e.to_string())?;
        }
        _ => {
            if !npmrc.exists() {
                return Ok(());
            }
            let existing = fs::read_to_string(&npmrc).unwrap_or_default();
            let lines: Vec<&str> = existing
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    !t.is_empty()
                        && !t.starts_with("registry=")
                        && !t.starts_with("registry =")
                })
                .collect();
            if lines.is_empty() {
                let _ = fs::remove_file(&npmrc);
            } else {
                fs::write(&npmrc, format!("{}\n", lines.join("\n"))).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// Write `.npmrc` with registry into runtime + web profile dirs (or clear registry line).
pub fn apply_npm_registry_files() -> Result<(), String> {
    let registry = npm_registry();
    let reg_ref = registry.as_deref();

    let runtime = runtime_dir()?;
    write_or_clear_npmrc(&runtime, reg_ref)?;

    if let Some(web) = web_profile_dir() {
        write_or_clear_npmrc(&web, reg_ref)?;
    }
    Ok(())
}

/// Set `npm_config_registry` on a Command when a custom registry is configured.
pub fn apply_npm_registry_env(command: &mut Command) {
    if let Some(registry) = npm_registry() {
        command.env("npm_config_registry", registry);
    }
}

pub fn presets() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "id": "official",
            "label": "官方 (npmjs)",
            "url": DEFAULT_NPM_REGISTRY,
        }),
        serde_json::json!({
            "id": "npmmirror",
            "label": "淘宝镜像 (npmmirror)",
            "url": NPMMIRROR_REGISTRY,
        }),
        serde_json::json!({
            "id": "custom",
            "label": "自定义",
            "url": "",
        }),
    ]
}

pub fn cmd_get_npm_settings() -> Result<serde_json::Value, String> {
    let registry = npm_registry().unwrap_or_else(|| DEFAULT_NPM_REGISTRY.to_string());
    Ok(serde_json::json!({
        "registry": registry,
        "presets": presets(),
    }))
}

pub fn cmd_set_npm_registry(registry: String) -> Result<serde_json::Value, String> {
    let trimmed = registry.trim().to_string();
    let mut settings = load_settings();
    if trimmed.is_empty() || trimmed == DEFAULT_NPM_REGISTRY {
        settings.npm_registry = None;
    } else {
        settings.npm_registry = Some(trimmed);
    }
    save_settings(&settings)?;
    apply_npm_registry_files()?;
    cmd_get_npm_settings()
}

pub fn selected_plugin_ids() -> Vec<String> {
    let settings = load_settings();
    settings
        .selected_plugins
        .unwrap_or_default()
}

pub fn cmd_get_desktop_settings() -> Result<serde_json::Value, String> {
    let s = load_settings();
    Ok(serde_json::json!({
        "wizardCompleted": s.wizard_completed,
        "selectedPlugins": s.selected_plugins.clone().unwrap_or_default(),
        "autostart": s.autostart,
        "globalShortcut": s.global_shortcut.clone().unwrap_or_else(|| DEFAULT_GLOBAL_SHORTCUT.to_string()),
        "globalShortcutEnabled": s.global_shortcut_enabled,
        "cliShimEnabled": s.cli_shim_enabled,
        "defaultShortcut": DEFAULT_GLOBAL_SHORTCUT,
    }))
}

pub fn cmd_complete_plugin_wizard(
    selected: Vec<String>,
    use_recommended: bool,
    recommended: &[&str],
) -> Result<serde_json::Value, String> {
    let mut settings = load_settings();
    let ids = if use_recommended {
        recommended.iter().map(|s| (*s).to_string()).collect()
    } else {
        selected
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    };
    settings.wizard_completed = true;
    settings.selected_plugins = Some(ids.clone());
    save_settings(&settings)?;
    Ok(serde_json::json!({
        "wizardCompleted": true,
        "selectedPlugins": ids,
    }))
}

pub fn cmd_set_autostart_pref(enabled: bool) -> Result<serde_json::Value, String> {
    let mut settings = load_settings();
    settings.autostart = enabled;
    save_settings(&settings)?;
    Ok(serde_json::json!({ "autostart": enabled }))
}

pub fn cmd_set_global_shortcut_pref(
    shortcut: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    let trimmed = shortcut.trim().to_string();
    let mut settings = load_settings();
    settings.global_shortcut = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.clone())
    };
    settings.global_shortcut_enabled = enabled && !trimmed.is_empty();
    save_settings(&settings)?;
    Ok(serde_json::json!({
        "globalShortcut": settings.global_shortcut.clone().unwrap_or_default(),
        "globalShortcutEnabled": settings.global_shortcut_enabled,
    }))
}

#[allow(dead_code)]
pub fn cmd_set_cli_shim_pref(enabled: bool) -> Result<serde_json::Value, String> {
    let mut settings = load_settings();
    settings.cli_shim_enabled = enabled;
    save_settings(&settings)?;
    Ok(serde_json::json!({ "cliShimEnabled": enabled }))
}
