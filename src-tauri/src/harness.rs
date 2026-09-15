 

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::runtime::{
    check_dsh_update, ensure_managed_runtime, ensure_path_for_gui, update_managed_runtime,
};

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const PLUGIN_INSTALL_TIMEOUT: Duration = Duration::from_secs(240);
const POLL_INTERVAL: Duration = Duration::from_millis(400);

struct DefaultPlugin {
    id: &'static str,
    install_spec: &'static str,
    detect_needles: &'static [&'static str],
}

const DEFAULT_PLUGINS: &[DefaultPlugin] = &[
    DefaultPlugin {
        id: "dshmarket",
        install_spec: "dshmarket",
        detect_needles: &["dshmarket"],
    },
    // dsh-modellix currently crashes `dsh web` with:
    // cannot get property "webServer" without inject — omit until compatible.
    DefaultPlugin {
        id: "loopx",
        install_spec: "github:huangruiteng/loopx",
        detect_needles: &["loopx", "dsh-loopx", "dsh-loopx-plugin"],
    },
];

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HarnessEvent {
    Checking { message: String },
    Installing { message: String },
    Starting { message: String },
    Ready { url: String },
    Error { message: String },
    UpdateAvailable { current: String, latest: String },
}

pub struct HarnessManager {
    inner: Mutex<HarnessInner>,
}

struct HarnessInner {
    child: Option<Child>,
    url: Option<String>,
    port: Option<u16>,
}

impl HarnessManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HarnessInner {
                child: None,
                url: None,
                port: None,
            }),
        }
    }

    pub fn stop(&self) {
        let mut guard = self.inner.lock().expect("harness lock");
        if let Some(mut child) = guard.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        guard.url = None;
        guard.port = None;
    }

    pub fn current_url(&self) -> Option<String> {
        self.inner.lock().expect("harness lock").url.clone()
    }
}

fn emit(app: &AppHandle, event: HarnessEvent) {
    let _ = app.emit("harness", event);
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn web_profile_package_json() -> Option<PathBuf> {
    Some(
        home_dir()?
            .join(".dsh")
            .join("profiles")
            .join("web")
            .join("package.json"),
    )
}

fn bootstrap_marker_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".dsh-desktop").join("bootstrap-plugins.json"))
}

fn text_mentions_any(text: &str, needles: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    needles
        .iter()
        .any(|n| lower.contains(&n.to_ascii_lowercase()))
}

fn is_plugin_installed(plugin: &DefaultPlugin) -> bool {
    if let Some(pkg) = web_profile_package_json() {
        if let Ok(text) = std::fs::read_to_string(&pkg) {
            if text_mentions_any(&text, plugin.detect_needles) {
                return true;
            }
        }
    }
    if let Some(marker) = bootstrap_marker_path() {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if text_mentions_any(&text, plugin.detect_needles)
                || text_mentions_any(&text, &[plugin.id])
            {
                return true;
            }
        }
    }
    false
}

fn write_bootstrap_marker(installed_ids: &[&str]) -> Result<(), String> {
    let path = bootstrap_marker_path().ok_or_else(|| "cannot resolve home dir".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let plugins_json = installed_ids
        .iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let body = format!(
        "{{\n  \"plugins\": [{plugins_json}],\n  \"installedAt\": \"{}\"\n}}\n",
        chrono_like_now()
    );
    std::fs::write(&path, body).map_err(|e| e.to_string())
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn find_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    Ok(port)
}

fn run_command(program: &str, args: &[String]) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }
    crate::settings::apply_npm_registry_env(&mut command);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("执行失败 / command failed ({program}): {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "命令失败 / command exited {}: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

fn install_one_plugin(node: &str, bin_js: &PathBuf, spec: &str) -> Result<(), String> {
    let args = vec![
        bin_js.to_string_lossy().to_string(),
        "plugin".into(),
        "--profile".into(),
        "web".into(),
        "add".into(),
        spec.into(),
    ];
    let node = node.to_string();
    let handle = thread::spawn(move || run_command(&node, &args));
    let started = Instant::now();
    loop {
        if handle.is_finished() {
            break;
        }
        if started.elapsed() > PLUGIN_INSTALL_TIMEOUT {
            return Err(format!("安装超时 / install timed out: {spec}"));
        }
        thread::sleep(Duration::from_millis(200));
    }
    handle
        .join()
        .map_err(|_| "install thread panicked".to_string())?
}

fn ensure_default_plugins(app: &AppHandle, node: &str, bin_js: &PathBuf) {
    let missing: Vec<&DefaultPlugin> = DEFAULT_PLUGINS
        .iter()
        .filter(|p| !is_plugin_installed(p))
        .collect();
    if missing.is_empty() {
        return;
    }

    let mut installed_ids: Vec<&str> = DEFAULT_PLUGINS
        .iter()
        .filter(|p| is_plugin_installed(p))
        .map(|p| p.id)
        .collect();

    for plugin in missing {
        emit(
            app,
            HarnessEvent::Installing {
                message: format!("安装插件 {}…", plugin.id),
            },
        );
        match install_one_plugin(node, bin_js, plugin.install_spec) {
            Ok(()) => {
                installed_ids.push(plugin.id);
                emit(
                    app,
                    HarnessEvent::Installing {
                        message: format!("插件 {} 已就绪", plugin.id),
                    },
                );
            }
            Err(e) => {
                let hint = "可在启动页配置 npm 源（如 https://registry.npmmirror.com）";
                emit(
                    app,
                    HarnessEvent::Installing {
                        message: format!("{} 安装失败（将继续）：{e}\n{hint}", plugin.id),
                    },
                );
            }
        }
    }
    let _ = write_bootstrap_marker(&installed_ids);
}

fn spawn_harness(node: &str, bin_js: &PathBuf, port: u16) -> Result<Child, String> {
    let mut command = Command::new(node);
    command
        .arg(bin_js)
        .args(["web", "--port", &port.to_string(), "--no-open"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    if let Ok(path) = std::env::var("PATH") {
        command.env("PATH", path);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    command
        .spawn()
        .map_err(|e| format!("启动 dsh 失败 / failed to spawn dsh: {e}"))
}

fn extract_url(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let idx = lower.find("http://").or_else(|| lower.find("https://"))?;
    let rest = &line[idx..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | ']' | '"' | '\''))
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['/', ',', ';']).to_string();
    if url.contains("127.0.0.1") || url.contains("localhost") {
        Some(url)
    } else {
        None
    }
}

/// TCP open is not enough — wait until HTTP responds with a real page body
/// to avoid navigating into a blank/white webview.
fn http_page_ready(url: &str) -> bool {
    // Do not follow redirects: the token URL answers 303, and following it
    // drops the token and yields a bare 401 that never looks "ready".
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_millis(400))
        .timeout_read(Duration::from_secs(2))
        .redirects(0)
        .build();
    match agent.get(url).call() {
        Ok(resp) => {
            let status = resp.status();
            // Token handshake often returns 303 with empty body (Set-Cookie / Location).
            // Untokenized local dsh answers 401 — not navigable.
            if url.contains("token=") && (200..400).contains(&status) {
                return true;
            }
            if status == 401 || status == 403 {
                return false;
            }
            if !(200..400).contains(&status) {
                return false;
            }
            let body = resp.into_string().unwrap_or_default();
            body.len() > 32
                && (body.contains('<') || body.contains('{') || body.contains("dsh"))
        }
        Err(_) => false
    }
}

fn drain_pipes(child: &mut Child) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel::<String>();
    if let Some(stdout) = child.stdout.take() {
        let tx_out = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().flatten() {
                let _ = tx_out.send(line);
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().flatten() {
                let _ = tx.send(line);
            }
        });
    }
    rx
}

fn wait_until_ready(child: &mut Child, port: u16, deadline: Instant) -> Result<String, String> {
    let fallback = format!("http://127.0.0.1:{port}");
    let mut discovered: Option<String> = None;
    let lines = drain_pipes(child);

    while Instant::now() < deadline {
        while let Ok(line) = lines.try_recv() {
            if let Some(url) = extract_url(&line) {
                discovered = Some(url);
            }
        }

        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("dsh 进程提前退出 / exited early: {status}"));
        }

        // Prefer the stdout URL (includes ?token=). Bare port often returns 401.
        // IMPORTANT: never HTTP-probe a tokenized URL — the token is one-time and
        // probing it consumes auth before the webview can open it.
        if let Some(candidate) = discovered.clone() {
            if candidate.contains("token=") {
                // Brief settle only — do not HTTP-probe (one-time token).
                thread::sleep(Duration::from_millis(100));
                return Ok(candidate);
            }
            if http_page_ready(&candidate) {
                return Ok(candidate);
            }
        } else if http_page_ready(&fallback) {
            // Rare: server serves UI without token.
            return Ok(fallback.clone());
        }

        thread::sleep(POLL_INTERVAL);
    }

    Err(format!(
        "等待 dsh 就绪超时（{}s）。/ Timed out waiting for dsh after {}s.",
        READY_TIMEOUT.as_secs(),
        READY_TIMEOUT.as_secs()
    ))
}


pub fn start_harness(app: AppHandle, manager: Arc<HarnessManager>) {
    thread::spawn(move || {
        manager.stop();
        ensure_path_for_gui();

        emit(
            &app,
            HarnessEvent::Checking {
                message: "检查运行时…".into(),
            },
        );

        let app_for_progress = app.clone();
        let (node, bin_js) = match ensure_managed_runtime(move |installing, msg| {
            if installing {
                emit(
                    &app_for_progress,
                    HarnessEvent::Installing { message: msg },
                );
            } else {
                emit(
                    &app_for_progress,
                    HarnessEvent::Checking { message: msg },
                );
            }
        }) {
            Ok(v) => v,
            Err(e) => {
                emit(&app, HarnessEvent::Error { message: e });
                return;
            }
        };

        // Local .npmrc writes only — cheap; leave on path so plugin install / npm
        // child processes see the user's chosen registry.
        let _ = crate::settings::apply_npm_registry_files();
        // Skips plugins already present in web profile package.json or bootstrap marker.
        ensure_default_plugins(&app, &node, &bin_js);

        let port = match find_free_port() {
            Ok(p) => p,
            Err(e) => {
                emit(
                    &app,
                    HarnessEvent::Error {
                        message: format!("无法分配本地端口 / cannot allocate port: {e}"),
                    },
                );
                return;
            }
        };

        emit(
            &app,
            HarnessEvent::Starting {
                message: format!("正在启动 dsh（端口 {port}）…"),
            },
        );

        let mut child = match spawn_harness(&node, &bin_js, port) {
            Ok(c) => c,
            Err(e) => {
                emit(&app, HarnessEvent::Error { message: e });
                return;
            }
        };

        // npm view hits the registry with no timeout — never block spawn/UI on it.
        // Run after spawn so the splash can still show UpdateAvailable while waiting.
        let app_for_update = app.clone();
        thread::spawn(move || {
            if let Ok((current, latest, available)) = check_dsh_update() {
                if available && current != latest {
                    emit(
                        &app_for_update,
                        HarnessEvent::UpdateAvailable { current, latest },
                    );
                }
            }
        });

        let deadline = Instant::now() + READY_TIMEOUT;
        let url = match wait_until_ready(&mut child, port, deadline) {
            Ok(u) => u,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                emit(&app, HarnessEvent::Error { message: e });
                return;
            }
        };

        {
            let mut guard = manager.inner.lock().expect("harness lock");
            guard.child = Some(child);
            guard.url = Some(url.clone());
            guard.port = Some(port);
        }

        emit(
            &app,
            HarnessEvent::Starting {
                message: "正在打开界面…".into(),
            },
        );
        emit(&app, HarnessEvent::Ready { url: url.clone() });

        let app_open = app.clone();
        let url_open = url.clone();
        if let Err(e) = app.run_on_main_thread(move || {
            if let Err(err) = open_dsh_window(&app_open, &url_open) {
                emit(
                    &app_open,
                    HarnessEvent::Error {
                        message: format!("打开 Web UI 失败 / open window failed: {err}"),
                    },
                );
            }
        }) {
            emit(
                &app,
                HarnessEvent::Error {
                    message: format!("无法在主线程打开窗口 / main thread schedule failed: {e}"),
                },
            );
        }
    });
}

/// Open dsh in a webview whose *first* document is the token URL.
/// Navigating from the asset:// splash is cross-site, so SameSite=Strict
/// auth cookies never stick and the UI shows "authentication required".
fn open_dsh_window(app: &AppHandle, url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    // Close prior harness window if restarting.
    if let Some(w) = app.get_webview_window("harness") {
        let _ = w.close();
    }
    WebviewWindowBuilder::new(app, "harness", WebviewUrl::External(parsed))
        .title("DSH Desktop")
        .inner_size(1280.0, 840.0)
        .min_inner_size(960.0, 640.0)
        .focused(true)
        .build()
        .map_err(|e| e.to_string())?;
    if let Some(splash) = app.get_webview_window("main") {
        let _ = splash.close();
    }
    Ok(())
}

pub fn cmd_check_dsh_update() -> Result<serde_json::Value, String> {
    let (current, latest, update_available) = check_dsh_update()?;
    Ok(serde_json::json!({
        "current": current,
        "latest": latest,
        "updateAvailable": update_available,
    }))
}

pub fn cmd_update_dsh_runtime() -> Result<serde_json::Value, String> {
    let installed = update_managed_runtime(Some("latest"))?;
    Ok(serde_json::json!({
        "installed": installed,
    }))
}

pub fn cmd_runtime_info() -> Result<serde_json::Value, String> {
    Ok(serde_json::json!({
        "runtimeDir": crate::runtime::runtime_dir()?.to_string_lossy(),
        "version": crate::runtime::read_installed_version(),
        "bin": crate::runtime::runtime_bin_js()?.to_string_lossy(),
        "ready": crate::runtime::runtime_bin_js().map(|p| p.is_file()).unwrap_or(false),
    }))
}
