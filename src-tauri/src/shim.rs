use std::fs;
use std::path::PathBuf;

use crate::runtime::{find_node, runtime_bin_js};
#[cfg(windows)]
use crate::runtime::desktop_home;
use crate::settings::{load_settings, save_settings};

#[cfg(windows)]
fn shim_path() -> Result<PathBuf, String> {
    Ok(desktop_home()?.join("bin").join("dsh.cmd"))
}

#[cfg(not(windows))]
fn shim_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or_else(|| "cannot resolve HOME".to_string())?;
    Ok(PathBuf::from(home).join(".local").join("bin").join("dsh"))
}

pub fn shim_status() -> Result<serde_json::Value, String> {
    let path = shim_path()?;
    let exists = path.is_file();
    let settings = load_settings();
    #[cfg(windows)]
    let hint = "Windows：shim 位于 ~/.dsh-desktop/bin/dsh.cmd，可将该目录加入用户 PATH（无需管理员）。";
    #[cfg(not(windows))]
    let hint = "macOS/Linux：shim 位于 ~/.local/bin/dsh。若终端找不到 dsh，请将 ~/.local/bin 加入 PATH（不会自动改 shell rc）。";
    Ok(serde_json::json!({
        "path": path.to_string_lossy(),
        "installed": exists,
        "enabled": settings.cli_shim_enabled,
        "hint": hint,
        "platform": if cfg!(windows) { "windows" } else { "unix" },
    }))
}

fn write_unix_shim(node: &str, bin_js: &PathBuf, dest: &PathBuf) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let script = format!(
        "#!/bin/sh\n# Managed by DSH Desktop — invokes local runtime.\nexec \"{node}\" \"{bin}\" \"$@\"\n",
        node = node.replace('"', "\\\""),
        bin = bin_js.display(),
    );
    fs::write(dest, script).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(dest).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(dest, perms).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(windows)]
fn write_windows_shim(node: &str, bin_js: &PathBuf, dest: &PathBuf) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let script = format!(
        "@echo off\r\nREM Managed by DSH Desktop — invokes local runtime.\r\n\"{node}\" \"{bin}\" %*\r\n",
        node = node,
        bin = bin_js.display(),
    );
    fs::write(dest, script).map_err(|e| e.to_string())
}

pub fn install_shim() -> Result<serde_json::Value, String> {
    let node = find_node()?;
    let bin_js = runtime_bin_js()?;
    if !bin_js.is_file() {
        return Err("运行时尚未就绪，请先完成启动安装。".into());
    }
    let dest = shim_path()?;
    #[cfg(windows)]
    write_windows_shim(&node, &bin_js, &dest)?;
    #[cfg(not(windows))]
    write_unix_shim(&node, &bin_js, &dest)?;

    let mut settings = load_settings();
    settings.cli_shim_enabled = true;
    save_settings(&settings)?;
    shim_status()
}

pub fn remove_shim() -> Result<serde_json::Value, String> {
    let dest = shim_path()?;
    if dest.exists() {
        fs::remove_file(&dest).map_err(|e| e.to_string())?;
    }
    let mut settings = load_settings();
    settings.cli_shim_enabled = false;
    save_settings(&settings)?;
    shim_status()
}

/// After runtime is ready, optionally ensure shim matches preference.
pub fn maybe_install_shim_after_ready() {
    let settings = load_settings();
    if !settings.cli_shim_enabled {
        return;
    }
    let _ = install_shim();
}
