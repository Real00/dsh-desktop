use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned default; update flow can move past this.
pub const PINNED_DSH_VERSION: &str = "0.1.5-rc.1";
pub const DSH_PACKAGE: &str = "@deepseek-ai/dsh";

pub fn desktop_home() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| "cannot resolve home directory".to_string())?;
    Ok(PathBuf::from(home).join(".dsh-desktop"))
}

pub fn runtime_dir() -> Result<PathBuf, String> {
    Ok(desktop_home()?.join("runtime"))
}

pub fn runtime_bin_js() -> Result<PathBuf, String> {
    Ok(runtime_dir()?
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js"))
}

pub fn installed_version_file() -> Result<PathBuf, String> {
    Ok(runtime_dir()?.join("dsh-version.txt"))
}

pub fn read_installed_version() -> Option<String> {
    let path = installed_version_file().ok()?;
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Expand PATH so macOS .app launches can find Homebrew / nvm node.
pub fn ensure_path_for_gui() {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    let extras = [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
    ];
    for e in extras {
        if !parts.iter().any(|p| p.split(':').any(|x| x == e)) {
            parts.insert(0, e.to_string());
        }
    }
    // nvm default alias if present
    if let Some(home) = std::env::var_os("HOME") {
        let nvm = PathBuf::from(&home).join(".nvm/versions/node");
        if nvm.is_dir() {
            if let Ok(rd) = fs::read_dir(&nvm) {
                let mut versions: Vec<_> = rd.filter_map(|e| e.ok()).collect();
                versions.sort_by_key(|e| e.file_name());
                if let Some(last) = versions.last() {
                    let bin = last.path().join("bin");
                    if bin.is_dir() {
                        parts.insert(0, bin.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    let joined = parts.join(":");
    std::env::set_var("PATH", &joined);
}

pub fn which(bin: &str) -> Option<String> {
    #[cfg(windows)]
    let output = Command::new("where").arg(bin).output().ok()?;
    #[cfg(not(windows))]
    let output = Command::new("sh")
        .arg("-lc")
        .arg(format!("command -v {bin}"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .to_string();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

pub fn find_node() -> Result<String, String> {
    ensure_path_for_gui();
    which("node").ok_or_else(|| {
        "未找到 Node.js。请安装 Node.js ≥ 22.19 后重试。\n\
         Node.js not found. Install Node.js ≥ 22.19 and retry."
            .to_string()
    })
}

pub fn find_npm() -> Result<String, String> {
    ensure_path_for_gui();
    which("npm").ok_or_else(|| {
        "未找到 npm。请安装 Node.js（自带 npm）后重试。".to_string()
    })
}

fn runtime_ready() -> bool {
    runtime_bin_js()
        .map(|p| p.is_file())
        .unwrap_or(false)
}

/// Ensure a local managed install exists under ~/.dsh-desktop/runtime.
/// Does NOT use npx on subsequent launches.
pub fn ensure_managed_runtime<F>(mut progress: F) -> Result<(String, PathBuf), String>
where
    F: FnMut(String),
{
    ensure_path_for_gui();
    let node = find_node()?;
    let dir = runtime_dir()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    if runtime_ready() {
        progress(format!(
            "使用本地托管 dsh（{}）",
            read_installed_version().unwrap_or_else(|| "unknown".into())
        ));
        return Ok((node, runtime_bin_js()?));
    }

    progress(format!(
        "首次安装托管运行时 {DSH_PACKAGE}@{PINNED_DSH_VERSION}（只需一次）…"
    ));

    // Minimal package.json so npm install is reproducible in this folder.
    let pkg_json = dir.join("package.json");
    if !pkg_json.exists() {
        fs::write(
            &pkg_json,
            r#"{
  "name": "dsh-desktop-runtime",
  "private": true,
  "description": "Managed DeepSeek Harness runtime for DSH Desktop"
}
"#,
        )
        .map_err(|e| e.to_string())?;
    }

    let npm = find_npm()?;
    let _ = crate::settings::apply_npm_registry_files();
    let spec = format!("{DSH_PACKAGE}@{PINNED_DSH_VERSION}");
    let mut cmd = Command::new(&npm);
    cmd.args(["install", "--no-fund", "--no-audit", &spec])
        .current_dir(&dir);
    crate::settings::apply_npm_registry_env(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| format!("npm install failed to start: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "npm install {spec} failed: {}\n{stderr}\n{stdout}",
            output.status
        ));
    }

    if !runtime_ready() {
        return Err(format!(
            "安装后未找到 {}，请检查网络或手动安装。",
            runtime_bin_js().unwrap_or_default().display()
        ));
    }

    fs::write(installed_version_file()?, PINNED_DSH_VERSION).map_err(|e| e.to_string())?;
    progress(format!("托管运行时已就绪：{PINNED_DSH_VERSION}"));
    Ok((node, runtime_bin_js()?))
}

/// Upgrade managed runtime to latest (or a given version).
pub fn update_managed_runtime(version: Option<&str>) -> Result<String, String> {
    ensure_path_for_gui();
    let npm = find_npm()?;
    let dir = runtime_dir()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let _ = crate::settings::apply_npm_registry_files();
    let ver = version.unwrap_or("latest");
    let spec = format!("{DSH_PACKAGE}@{ver}");
    let mut cmd = Command::new(&npm);
    cmd.args(["install", "--no-fund", "--no-audit", &spec])
        .current_dir(&dir);
    crate::settings::apply_npm_registry_env(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "更新失败: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // Resolve installed version from package.json
    let installed_pkg = dir
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh")
        .join("package.json");
    let installed = fs::read_to_string(&installed_pkg)
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.contains("\"version\""))
                .map(|l| {
                    l.split(':')
                        .nth(1)
                        .unwrap_or("")
                        .trim()
                        .trim_matches([',', '"', ' '])
                        .to_string()
                })
        })
        .unwrap_or_else(|| ver.to_string());
    fs::write(installed_version_file()?, &installed).map_err(|e| e.to_string())?;
    Ok(installed)
}

pub fn latest_npm_version() -> Result<String, String> {
    ensure_path_for_gui();
    let npm = find_npm()?;
    let _ = crate::settings::apply_npm_registry_files();
    let mut cmd = Command::new(&npm);
    cmd.args(["view", DSH_PACKAGE, "version"]);
    crate::settings::apply_npm_registry_env(&mut cmd);
    let output = cmd
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn check_dsh_update() -> Result<(String, String, bool), String> {
    let current = read_installed_version().unwrap_or_else(|| "none".into());
    let latest = latest_npm_version()?;
    let update_available = current == "none" || current != latest;
    Ok((current, latest, update_available))
}

#[allow(dead_code)]
pub fn path_exists(p: &Path) -> bool {
    p.exists()
}
