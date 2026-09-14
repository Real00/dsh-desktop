use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::runtime::{desktop_home, runtime_dir};

pub const DEFAULT_NPM_REGISTRY: &str = "https://registry.npmjs.org";
pub const NPMMIRROR_REGISTRY: &str = "https://registry.npmmirror.com";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub npm_registry: Option<String>,
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
            // Using default: remove registry= line (or delete file if only that).
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
