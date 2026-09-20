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

fn path_sep() -> char {
    if cfg!(windows) { ';' } else { ':' }
}

fn path_contains(parts: &[String], candidate: &str) -> bool {
    let sep = path_sep();
    parts.iter().any(|p| p.split(sep).any(|x| x.eq_ignore_ascii_case(candidate)))
}

fn prepend_if_dir(parts: &mut Vec<String>, dir: PathBuf) {
    if dir.is_dir() {
        let s = dir.to_string_lossy().to_string();
        if !path_contains(parts, &s) {
            parts.insert(0, s);
        }
    }
}

/// Expand PATH so GUI launches can find Node/npm (Homebrew/nvm on Unix;
/// Program Files / nvm-windows / fnm on Windows).
pub fn ensure_path_for_gui() {
    let mut parts: Vec<String> = Vec::new();
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }

    #[cfg(windows)]
    {
        let mut win_extras: Vec<PathBuf> = Vec::new();
        if let Ok(pf) = std::env::var("ProgramFiles") {
            win_extras.push(PathBuf::from(&pf).join("nodejs"));
        }
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
            win_extras.push(PathBuf::from(&pf86).join("nodejs"));
        }
        // Fallbacks when env vars are missing
        win_extras.push(PathBuf::from(r"C:\Program Files\nodejs"));
        win_extras.push(PathBuf::from(r"C:\Program Files (x86)\nodejs"));

        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            win_extras.push(PathBuf::from(&local).join("Programs").join("node"));
            // fnm default install root
            win_extras.push(PathBuf::from(&local).join("fnm_multishells"));
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            win_extras.push(PathBuf::from(&appdata).join("npm"));
            // nvm-windows symlink root often under APPDATA\nvm or ProgramFiles\nvm
            win_extras.push(PathBuf::from(&appdata).join("nvm"));
        }
        if let Ok(nvm_home) = std::env::var("NVM_HOME") {
            win_extras.push(PathBuf::from(nvm_home));
        }
        if let Ok(nvm_symlink) = std::env::var("NVM_SYMLINK") {
            win_extras.push(PathBuf::from(nvm_symlink));
        }

        for e in win_extras {
            prepend_if_dir(&mut parts, e);
        }

        // nvm-windows: pick latest version under NVM_HOME\v*
        if let Ok(nvm_home) = std::env::var("NVM_HOME") {
            let nvm = PathBuf::from(nvm_home);
            if nvm.is_dir() {
                if let Ok(rd) = fs::read_dir(&nvm) {
                    let mut versions: Vec<_> = rd.filter_map(|e| e.ok()).collect();
                    versions.sort_by_key(|e| e.file_name());
                    if let Some(last) = versions.last() {
                        prepend_if_dir(&mut parts, last.path());
                    }
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        let extras = [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
        ];
        for e in extras {
            if !path_contains(&parts, e) {
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
                        prepend_if_dir(&mut parts, bin);
                    }
                }
            }
        }
    }

    let joined = parts.join(&path_sep().to_string());
    std::env::set_var("PATH", &joined);
}

/// Pick a CreateProcess-friendly path from `where` output on Windows.
/// Prefer `*.cmd` / `*.exe`; skip `.ps1` and extensionless shims.
#[cfg(windows)]
fn pick_windows_executable(stdout: &str, preferred_exts: &[&str]) -> Option<String> {
    let lines: Vec<&str> = stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    for ext in preferred_exts {
        for line in &lines {
            let lower = line.to_ascii_lowercase();
            if lower.ends_with(ext) {
                return Some((*line).to_string());
            }
        }
    }
    // Last resort: any non-.ps1 path with an extension
    for line in &lines {
        let lower = line.to_ascii_lowercase();
        if lower.ends_with(".ps1") {
            continue;
        }
        let name = Path::new(line)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        if name.contains('.') {
            return Some((*line).to_string());
        }
    }
    None
}

pub fn which(bin: &str) -> Option<String> {
    #[cfg(windows)]
    {
        // Prefer explicit Win32 launchers for node/npm.
        let (query, preferred) = match bin {
            "npm" => ("npm.cmd", &[".cmd", ".exe"][..]),
            "node" => ("node.exe", &[".exe"][..]),
            other => (other, &[".exe", ".cmd", ".bat"][..]),
        };

        // Try the preferred name first (e.g. npm.cmd)
        if let Ok(output) = Command::new("where").arg(query).output() {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if let Some(p) = pick_windows_executable(&stdout, preferred) {
                    return Some(p);
                }
            }
        }

        // Fallback: `where npm` / `where node`, then filter shims
        if query != bin {
            if let Ok(output) = Command::new("where").arg(bin).output() {
                if output.status.success() {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    if let Some(p) = pick_windows_executable(&stdout, preferred) {
                        return Some(p);
                    }
                }
            }
        }

        None
    }
    #[cfg(not(windows))]
    {
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
/// Progress callback: `(installing, message)` — `installing` is false when
/// reusing an existing local runtime.
pub fn ensure_managed_runtime<F>(mut progress: F) -> Result<(String, PathBuf), String>
where
    F: FnMut(bool, String),
{
    ensure_path_for_gui();
    let node = find_node()?;
    let dir = runtime_dir()?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    if runtime_ready() {
        let ver = read_installed_version().unwrap_or_else(|| "unknown".into());
        progress(false, format!("使用本地运行时（{ver}）…"));
        return Ok((node, runtime_bin_js()?));
    }

    progress(
        true,
        format!("正在安装运行时 {DSH_PACKAGE}@{PINNED_DSH_VERSION}…"),
    );

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
    let output = cmd.output().map_err(|e| {
        format!("npm install failed to start (resolved npm={npm}): {e}")
    })?;

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
    progress(false, format!("运行时已就绪：{PINNED_DSH_VERSION}"));
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
    let output = cmd.output().map_err(|e| {
        format!("npm update failed to start (resolved npm={npm}): {e}")
    })?;
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
    let output = cmd.output().map_err(|e| {
        format!("npm view failed to start (resolved npm={npm}): {e}")
    })?;
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
