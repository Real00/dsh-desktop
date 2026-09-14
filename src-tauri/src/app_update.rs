//! Application (desktop shell) remote update via GitHub Releases.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::runtime::desktop_home;

pub const GITHUB_REPO: &str = "Real00/dsh-desktop";
const USER_AGENT: &str = "dsh-desktop-updater";

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

fn current_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Semver-ish compare for `x.y.z` (numeric parts). Returns true if `latest > current`.
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            .split(|c: char| !c.is_ascii_digit())
            .filter(|p| !p.is_empty())
            .filter_map(|p| p.parse::<u64>().ok())
            .collect()
    };
    let a = parse(latest);
    let b = parse(current);
    let len = a.len().max(b.len());
    for i in 0..len {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

fn pick_asset(assets: &[GhAsset]) -> Option<&GhAsset> {
    #[cfg(target_os = "macos")]
    {
        let dmg = assets.iter().find(|a| {
            let n = a.name.to_lowercase();
            n.ends_with(".dmg") && (n.contains("aarch64") || n.contains("arm64"))
        });
        if let Some(a) = dmg {
            return Some(a);
        }
        return assets.iter().find(|a| {
            let n = a.name.to_lowercase();
            n.ends_with(".app.tar.gz")
                || (n.contains("aarch64") && n.ends_with(".tar.gz"))
        });
    }

    #[cfg(target_os = "windows")]
    {
        let setup = assets.iter().find(|a| {
            let n = a.name.to_lowercase();
            n.contains("setup") && n.ends_with(".exe")
        });
        if let Some(a) = setup {
            return Some(a);
        }
        return assets.iter().find(|a| a.name.to_lowercase().ends_with(".exe"));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = assets;
        None
    }
}

fn updates_dir() -> Result<PathBuf, String> {
    let dir = desktop_home()?.join("updates");
    fs::create_dir_all(&dir).map_err(|e| format!("create updates dir: {e}"))?;
    Ok(dir)
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(120))
        .build()
}

/// Check GitHub latest release against the running app version.
pub fn check_app_update() -> Result<serde_json::Value, String> {
    let current = current_app_version();
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let agent = http_agent();
    let resp = agent
        .get(&url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("GitHub API request failed: {e}"))?;

    if !(200..300).contains(&resp.status()) {
        return Err(format!(
            "GitHub API returned HTTP {}: {}",
            resp.status(),
            resp.into_string().unwrap_or_default()
        ));
    }

    let body = resp
        .into_string()
        .map_err(|e| format!("read GitHub response: {e}"))?;
    let release: GhRelease =
        serde_json::from_str(&body).map_err(|e| format!("parse GitHub release JSON: {e}"))?;

    let latest = release
        .tag_name
        .trim()
        .trim_start_matches('v')
        .to_string();
    let update_available = is_newer(&latest, &current);
    let asset = pick_asset(&release.assets);
    let (download_url, asset_name) = match asset {
        Some(a) => (
            serde_json::Value::String(a.browser_download_url.clone()),
            serde_json::Value::String(a.name.clone()),
        ),
        None => (serde_json::Value::Null, serde_json::Value::Null),
    };

    Ok(json!({
        "updateAvailable": update_available,
        "current": current,
        "latest": latest,
        "notes": release.body.unwrap_or_default(),
        "downloadUrl": download_url,
        "assetName": asset_name,
    }))
}

/// Download an update asset to `~/.dsh-desktop/updates/` and return the local path.
/// `progress` is called periodically with `(received_bytes, total_bytes_opt)`.
pub fn download_app_update<F>(url: &str, mut progress: F) -> Result<serde_json::Value, String>
where
    F: FnMut(u64, Option<u64>),
{
    if url.is_empty() {
        return Err("download url is empty".into());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("download url must be http(s)".into());
    }

    let dir = updates_dir()?;
    let name = url
        .rsplit('/')
        .next()
        .unwrap_or("update.bin")
        .split('?')
        .next()
        .unwrap_or("update.bin");
    // Sanitize filename
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' ) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dest = dir.join(if safe.is_empty() { "update.bin" } else { &safe });

    let agent = http_agent();
    let resp = agent
        .get(url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;

    if !(200..300).contains(&resp.status()) {
        return Err(format!("download HTTP {}", resp.status()));
    }

    let total = resp
        .header("Content-Length")
        .and_then(|s| s.parse::<u64>().ok());

    let mut reader = resp.into_reader();
    let mut file = File::create(&dest).map_err(|e| format!("create file: {e}"))?;
    let mut buf = [0u8; 64 * 1024];
    let mut received: u64 = 0;
    let max_bytes: u64 = 1024 * 1024 * 1024;
    let mut last_emit = 0u64;

    progress(0, total);

    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("read download: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write file: {e}"))?;
        received = received.saturating_add(n as u64);
        if received > max_bytes {
            return Err("download exceeded 1 GiB limit".into());
        }
        // Emit at least every 256 KiB to keep the splash responsive without flooding.
        if received == n as u64 || received - last_emit >= 256 * 1024 || total == Some(received) {
            progress(received, total);
            last_emit = received;
        }
    }
    progress(received, total.or(Some(received)));

    Ok(json!({
        "path": dest.to_string_lossy(),
        "bytes": received,
    }))
}

pub fn updates_dir_path() -> Result<String, String> {
    Ok(updates_dir()?.to_string_lossy().to_string())
}

pub fn cmd_check_app_update() -> Result<serde_json::Value, String> {
    check_app_update()
}

pub fn cmd_download_app_update(
    app: &tauri::AppHandle,
    url: String,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    download_app_update(&url, |received, total| {
        let _ = app.emit(
            "app-update",
            json!({
                "kind": "progress",
                "received": received,
                "total": total,
            }),
        );
    })
}
