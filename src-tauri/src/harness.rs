use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::collections::VecDeque;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::runtime::{
    check_dsh_update, ensure_managed_runtime, ensure_path_for_gui, update_managed_runtime,
};
use crate::settings::{self, load_settings};
use crate::shim;

const READY_TIMEOUT: Duration = Duration::from_secs(120);
const PLUGIN_INSTALL_TIMEOUT: Duration = Duration::from_secs(240);
const POLL_INTERVAL: Duration = Duration::from_millis(400);
const MAX_CRASH_RETRIES: u32 = 3;
const CRASH_BACKOFF_BASE: Duration = Duration::from_secs(2);

struct DefaultPlugin {
    id: &'static str,
    install_spec: &'static str,
    detect_needles: &'static [&'static str],
    label: &'static str,
    description: &'static str,
}

const DEFAULT_PLUGINS: &[DefaultPlugin] = &[
    DefaultPlugin {
        id: "dshmarket",
        install_spec: "dshmarket",
        detect_needles: &["dshmarket"],
        label: "插件市场",
        description: "dshmarket — 应用内插件市场",
    },
    DefaultPlugin {
        id: "loopx",
        install_spec: "github:huangruiteng/loopx",
        detect_needles: &["loopx", "dsh-loopx", "dsh-loopx-plugin"],
        label: "LoopX",
        description: "长任务 Goal / Todo / 配额控制面",
    },
    DefaultPlugin {
        id: "dsh-chat-import",
        install_spec: "dsh-chat-import",
        detect_needles: &["dsh-chat-import"],
        label: "对话导入",
        description: "dsh-chat-import — 导入外部对话",
    },
    DefaultPlugin {
        id: "dsh-llm-capabilities",
        install_spec: "dsh-llm-capabilities",
        detect_needles: &["dsh-llm-capabilities"],
        label: "LLM 能力",
        description: "dsh-llm-capabilities — 模型能力探测",
    },
];

pub fn default_plugin_ids() -> Vec<&'static str> {
    DEFAULT_PLUGINS.iter().map(|p| p.id).collect()
}

pub fn plugin_catalog_json() -> serde_json::Value {
    let plugins: Vec<serde_json::Value> = DEFAULT_PLUGINS
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "label": p.label,
                "description": p.description,
                "installSpec": p.install_spec,
                "recommended": true,
            })
        })
        .collect();
    serde_json::json!({ "plugins": plugins })
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HarnessEvent {
    Checking { message: String },
    Installing { message: String },
    Starting { message: String },
    Ready { url: String },
    Error { message: String },
    UpdateAvailable { current: String, latest: String },
    NeedsWizard {
        plugins: Vec<serde_json::Value>,
        message: String,
    },
    Crashed {
        message: String,
        attempt: u32,
        will_retry: bool,
    },
    Restarting {
        message: String,
        attempt: u32,
    },
}

pub struct HarnessManager {
    inner: Mutex<HarnessInner>,
}

struct HarnessInner {
    child: Option<Child>,
    url: Option<String>,
    port: Option<u16>,
    /// When true, exit is intentional (stop / quit / manual restart).
    stopping: bool,
    generation: u64,
    crash_retries: u32,
}

impl HarnessManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HarnessInner {
                child: None,
                url: None,
                port: None,
                stopping: false,
                generation: 0,
                crash_retries: 0,
            }),
        }
    }

    pub fn stop(&self) {
        let mut guard = self.inner.lock().expect("harness lock");
        guard.stopping = true;
        guard.generation = guard.generation.wrapping_add(1);
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

    fn begin_start(&self) -> u64 {
        let mut guard = self.inner.lock().expect("harness lock");
        guard.stopping = true;
        if let Some(mut child) = guard.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        guard.url = None;
        guard.port = None;
        guard.generation = guard.generation.wrapping_add(1);
        let gen = guard.generation;
        guard.stopping = false;
        gen
    }

    fn reset_crash_retries(&self) {
        self.inner.lock().expect("harness lock").crash_retries = 0;
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

fn selected_plugins_to_install() -> Vec<&'static DefaultPlugin> {
    let selected = settings::selected_plugin_ids();
    let ids: Vec<String> = if selected.is_empty() {
        // Wizard completed with empty selection — install nothing.
        // If somehow selected is empty but wizard not run, fall back to all.
        if load_settings().wizard_completed {
            Vec::new()
        } else {
            DEFAULT_PLUGINS.iter().map(|p| p.id.to_string()).collect()
        }
    } else {
        selected
    };
    DEFAULT_PLUGINS
        .iter()
        .filter(|p| ids.iter().any(|id| id == p.id))
        .collect()
}

fn ensure_selected_plugins(app: &AppHandle, node: &str, bin_js: &PathBuf) {
    let wanted = selected_plugins_to_install();
    if wanted.is_empty() {
        return;
    }

    let missing: Vec<&DefaultPlugin> = wanted
        .iter()
        .copied()
        .filter(|p| !is_plugin_installed(p))
        .collect();
    if missing.is_empty() {
        return;
    }

    let mut installed_ids: Vec<&str> = wanted
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
        Err(_) => false,
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

fn push_recent(recent: &mut VecDeque<String>, line: String) {
    const CAP: usize = 40;
    if recent.len() >= CAP {
        recent.pop_front();
    }
    recent.push_back(line);
}

fn format_recent_tail(recent: &VecDeque<String>) -> String {
    let n = recent.len().min(30);
    if n == 0 {
        return String::new();
    }
    let start = recent.len() - n;
    let body: Vec<&str> = recent.iter().skip(start).map(|s| s.as_str()).collect();
    format!("\n--- 最近输出 / recent output ---\n{}", body.join("\n"))
}

fn drain_remaining_lines(lines: &mpsc::Receiver<String>, recent: &mut VecDeque<String>) {
    let drain_deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < drain_deadline {
        match lines.try_recv() {
            Ok(line) => push_recent(recent, line),
            Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(40)),
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
}

fn wait_until_ready(child: &mut Child, port: u16, deadline: Instant) -> Result<String, String> {
    let fallback = format!("http://127.0.0.1:{port}");
    let mut discovered: Option<String> = None;
    let lines = drain_pipes(child);
    let mut recent: VecDeque<String> = VecDeque::with_capacity(40);

    while Instant::now() < deadline {
        while let Ok(line) = lines.try_recv() {
            if let Some(url) = extract_url(&line) {
                discovered = Some(url);
            }
            push_recent(&mut recent, line);
        }

        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            // Brief wait to drain remaining stdout/stderr so splash shows real cause.
            drain_remaining_lines(&lines, &mut recent);
            let detail = format_recent_tail(&recent);
            return Err(format!("dsh 进程提前退出 / exited early: {status}{detail}"));
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

    drain_remaining_lines(&lines, &mut recent);
    let detail = format_recent_tail(&recent);
    Err(format!(
        "等待 dsh 就绪超时（{}s）。/ Timed out waiting for dsh after {}s.{detail}",
        READY_TIMEOUT.as_secs(),
        READY_TIMEOUT.as_secs()
    ))
}

fn supervise_after_ready(app: AppHandle, manager: Arc<HarnessManager>, generation: u64) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_millis(750));
            let exit_status = {
                let mut guard = manager.inner.lock().expect("harness lock");
                if guard.generation != generation || guard.stopping {
                    return;
                }
                match guard.child.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => {
                            guard.child = None;
                            Some(status)
                        }
                        Ok(None) => None,
                        Err(_) => None,
                    },
                    None => return,
                }
            };

            let Some(status) = exit_status else {
                continue;
            };

            let (attempt, will_retry) = {
                let mut guard = manager.inner.lock().expect("harness lock");
                if guard.generation != generation || guard.stopping {
                    return;
                }
                guard.crash_retries += 1;
                let attempt = guard.crash_retries;
                (attempt, attempt <= MAX_CRASH_RETRIES)
            };

            emit(
                &app,
                HarnessEvent::Crashed {
                    message: format!("dsh 意外退出：{status}"),
                    attempt,
                    will_retry,
                },
            );

            let _ = app.run_on_main_thread({
                let app = app.clone();
                move || {
                    show_splash_window(&app);
                }
            });

            if !will_retry {
                emit(
                    &app,
                    HarnessEvent::Error {
                        message: format!(
                            "dsh 连续崩溃 {MAX_CRASH_RETRIES} 次，已停止自动重启。可点击「重启 dsh」。"
                        ),
                    },
                );
                return;
            }

            let backoff = CRASH_BACKOFF_BASE * attempt;
            emit(
                &app,
                HarnessEvent::Restarting {
                    message: format!(
                        "正在自动重启 dsh（第 {attempt}/{MAX_CRASH_RETRIES} 次）…"
                    ),
                    attempt,
                },
            );
            thread::sleep(backoff);

            {
                let guard = manager.inner.lock().expect("harness lock");
                if guard.generation != generation || guard.stopping {
                    return;
                }
            }

            start_harness_inner(app.clone(), manager.clone(), false);
            return;
        }
    });
}

/// Show / focus the splash (main) window for settings or error UI.
pub fn show_splash_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let _ = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("DSH Desktop")
        .inner_size(1280.0, 840.0)
        .min_inner_size(960.0, 640.0)
        .focused(true)
        .build();
}

pub fn focus_main_or_harness(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("harness") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    show_splash_window(app);
}

pub fn start_harness(app: AppHandle, manager: Arc<HarnessManager>) {
    start_harness_inner(app, manager, true);
}

fn start_harness_inner(app: AppHandle, manager: Arc<HarnessManager>, reset_retries: bool) {
    thread::spawn(move || {
        let generation = manager.begin_start();
        if reset_retries {
            manager.reset_crash_retries();
        }

        settings::migrate_wizard_if_needed(&default_plugin_ids());

        let settings = load_settings();
        if !settings.wizard_completed {
            let catalog = plugin_catalog_json();
            let plugins = catalog
                .get("plugins")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([]));
            let plugins_vec = plugins.as_array().cloned().unwrap_or_default();
            emit(
                &app,
                HarnessEvent::NeedsWizard {
                    plugins: plugins_vec,
                    message: "首次启动：请选择要安装的推荐插件".into(),
                },
            );
            return;
        }

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
        // Install missing plugins from the *selected* wizard set only.
        ensure_selected_plugins(&app, &node, &bin_js);

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

        {
            let guard = manager.inner.lock().expect("harness lock");
            if guard.generation != generation || guard.stopping {
                return;
            }
        }

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
            if guard.generation != generation || guard.stopping {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            guard.child = Some(child);
            guard.url = Some(url.clone());
            guard.port = Some(port);
        }

        // PATH shim if enabled in settings.
        shim::maybe_install_shim_after_ready();

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

        supervise_after_ready(app, manager, generation);
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
    // Hide splash instead of destroying it — Desktop settings / restart UI stay available.
    if let Some(splash) = app.get_webview_window("main") {
        let _ = splash.hide();
    }
    Ok(())
}


const CORE_BUNDLES: &[&str] = &[
    "@deepseek-ai/dsh-base",
    "@deepseek-ai/dsh-web-app",
];

fn is_core_bundle(name: &str) -> bool {
    CORE_BUNDLES
        .iter()
        .any(|c| c.eq_ignore_ascii_case(name))
}

fn core_bundles_json_array() -> Vec<serde_json::Value> {
    CORE_BUNDLES
        .iter()
        .map(|s| serde_json::Value::String((*s).to_string()))
        .collect()
}

/// Ensure `dsh.profile` object exists; return mutable reference to `bundles` array.
fn ensure_profile_bundles(value: &mut serde_json::Value) -> &mut Vec<serde_json::Value> {
    let root = value.as_object_mut().expect("package.json root object");
    let dsh = root
        .entry("dsh")
        .or_insert_with(|| serde_json::json!({}));
    let dsh_obj = dsh.as_object_mut().expect("dsh object");
    let profile = dsh_obj
        .entry("profile")
        .or_insert_with(|| serde_json::json!({}));
    let profile_obj = profile.as_object_mut().expect("dsh.profile object");
    let bundles = profile_obj
        .entry("bundles")
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));
    if !bundles.is_array() {
        *bundles = serde_json::Value::Array(Vec::new());
    }
    bundles.as_array_mut().expect("bundles array")
}

fn bundle_name_matches(entry: &str, name: &str, matching_ids: &[String]) -> bool {
    if is_core_bundle(entry) {
        return false;
    }
    entry.eq_ignore_ascii_case(name)
        || matching_ids.iter().any(|id| {
            entry.eq_ignore_ascii_case(id)
                || id.eq_ignore_ascii_case(entry)
                || entry
                    .to_ascii_lowercase()
                    .contains(&id.to_ascii_lowercase())
                || id
                    .to_ascii_lowercase()
                    .contains(&entry.to_ascii_lowercase())
        })
}

/// Strip matching non-core entries from `dsh.profile.bundles` (and `dsh.profile.plugins` if present).
fn drop_from_profile_plugin_lists(
    value: &mut serde_json::Value,
    name: &str,
    matching_ids: &[String],
) -> Vec<String> {
    let mut removed: Vec<String> = Vec::new();
    let Some(root) = value.as_object_mut() else {
        return removed;
    };
    let Some(dsh) = root.get_mut("dsh").and_then(|v| v.as_object_mut()) else {
        return removed;
    };
    let Some(profile) = dsh.get_mut("profile").and_then(|v| v.as_object_mut()) else {
        return removed;
    };
    for key in ["bundles", "plugins"] {
        let Some(arr) = profile.get_mut(key).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        let before = arr.len();
        arr.retain(|item| {
            let Some(s) = item.as_str() else {
                return true;
            };
            if bundle_name_matches(s, name, matching_ids) {
                removed.push(s.to_string());
                false
            } else {
                true
            }
        });
        let _ = before;
    }
    removed.sort();
    removed.dedup();
    removed
}

/// Keep only core bundles in profile plugin lists; return cleared non-core names.
fn retain_core_only_profile_lists(value: &mut serde_json::Value) -> Vec<String> {
    let mut cleared: Vec<String> = Vec::new();
    let bundles = ensure_profile_bundles(value);
    let kept: Vec<serde_json::Value> = bundles
        .iter()
        .filter_map(|item| {
            let s = item.as_str()?;
            if is_core_bundle(s) {
                Some(serde_json::Value::String(s.to_string()))
            } else {
                cleared.push(s.to_string());
                None
            }
        })
        .collect();
    // Always ensure both core bundles are present.
    let mut final_list = core_bundles_json_array();
    for item in kept {
        let s = item.as_str().unwrap_or("");
        if !final_list.iter().any(|c| c.as_str() == Some(s)) {
            final_list.push(item);
        }
    }
    *ensure_profile_bundles(value) = final_list;

    if let Some(profile) = value
        .pointer_mut("/dsh/profile")
        .and_then(|v| v.as_object_mut())
    {
        if let Some(arr) = profile.get_mut("plugins").and_then(|v| v.as_array_mut()) {
            arr.retain(|item| {
                let Some(s) = item.as_str() else {
                    return true;
                };
                if is_core_bundle(s) {
                    true
                } else {
                    cleared.push(s.to_string());
                    false
                }
            });
        }
    }
    cleared.sort();
    cleared.dedup();
    cleared
}

fn plugin_ids_matching_dep(name: &str) -> Vec<String> {
    let mut ids = vec![name.to_string()];
    for p in DEFAULT_PLUGINS {
        let matches = p.id.eq_ignore_ascii_case(name)
            || p.install_spec.eq_ignore_ascii_case(name)
            || p.detect_needles
                .iter()
                .any(|n| name.eq_ignore_ascii_case(n) || name.to_ascii_lowercase().contains(&n.to_ascii_lowercase()));
        if matches {
            ids.push(p.id.to_string());
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn drop_ids_from_bootstrap(ids: &[String]) -> Result<(), String> {
    let Some(path) = bootstrap_marker_path() else {
        return Ok(());
    };
    if !path.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let remaining: Vec<String> = value
        .get("plugins")
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .filter(|id| {
                    !ids.iter().any(|drop| {
                        drop.eq_ignore_ascii_case(id)
                            || id.to_ascii_lowercase().contains(&drop.to_ascii_lowercase())
                            || drop.to_ascii_lowercase().contains(&id.to_ascii_lowercase())
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "plugins".into(),
            serde_json::Value::Array(
                remaining
                    .into_iter()
                    .map(serde_json::Value::String)
                    .collect(),
            ),
        );
    }
    let body = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(&path, format!("{body}\n")).map_err(|e| e.to_string())
}

fn remove_one_plugin(node: &str, bin_js: &PathBuf, name: &str) -> Result<(), String> {
    let args = vec![
        bin_js.to_string_lossy().to_string(),
        "plugin".into(),
        "--profile".into(),
        "web".into(),
        "remove".into(),
        name.into(),
    ];
    let node = node.to_string();
    let handle = thread::spawn(move || run_command(&node, &args));
    let started = Instant::now();
    loop {
        if handle.is_finished() {
            break;
        }
        if started.elapsed() > PLUGIN_INSTALL_TIMEOUT {
            return Err(format!("卸载超时 / remove timed out: {name}"));
        }
        thread::sleep(Duration::from_millis(200));
    }
    handle
        .join()
        .map_err(|_| "remove thread panicked".to_string())?
}

pub fn cmd_list_installed_plugins() -> Result<serde_json::Value, String> {
    use std::collections::BTreeMap;
    let mut by_id: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    if let Some(pkg) = web_profile_package_json() {
        if let Ok(text) = std::fs::read_to_string(&pkg) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(deps) = value.get("dependencies").and_then(|d| d.as_object()) {
                    for (id, ver) in deps {
                        let version = ver.as_str().map(|s| s.to_string());
                        let core = is_core_bundle(id);
                        by_id.insert(
                            id.clone(),
                            serde_json::json!({
                                "id": id,
                                "version": version,
                                "core": core,
                                "removable": !core,
                                "source": "dependencies",
                            }),
                        );
                    }
                }
                // Also surface dsh.profile.bundles (what dsh actually loads).
                if let Some(arr) = value.pointer("/dsh/profile/bundles").and_then(|v| v.as_array()) {
                    for item in arr {
                        let Some(id) = item.as_str() else { continue };
                        let core = is_core_bundle(id);
                        by_id
                            .entry(id.to_string())
                            .and_modify(|existing| {
                                existing
                                    .as_object_mut()
                                    .map(|o| o.insert("source".into(), serde_json::json!("both")));
                            })
                            .or_insert_with(|| {
                                serde_json::json!({
                                    "id": id,
                                    "version": serde_json::Value::Null,
                                    "core": core,
                                    "removable": !core,
                                    "source": "bundles",
                                })
                            });
                    }
                }
            }
        }
    }
    let plugins: Vec<serde_json::Value> = by_id.into_values().collect();
    Ok(serde_json::json!({ "plugins": plugins }))
}

/// Drop matching non-core deps from ~/.dsh/profiles/web/package.json.
/// Returns the dependency keys that were removed. Does not require pnpm/CLI.
fn drop_dep_from_web_package_json(name: &str) -> Result<Vec<String>, String> {
    let Some(pkg_path) = web_profile_package_json() else {
        return Ok(Vec::new());
    };
    if !pkg_path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&pkg_path).map_err(|e| e.to_string())?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let matching_ids = plugin_ids_matching_dep(name);
    let mut removed: Vec<String> = Vec::new();
    if let Some(deps) = value
        .get_mut("dependencies")
        .and_then(|d| d.as_object_mut())
    {
        let keys: Vec<String> = deps.keys().cloned().collect();
        for key in keys {
            if is_core_bundle(&key) {
                continue;
            }
            let hit = key.eq_ignore_ascii_case(name)
                || matching_ids.iter().any(|id| {
                    key.eq_ignore_ascii_case(id)
                        || id.eq_ignore_ascii_case(&key)
                        || key
                            .to_ascii_lowercase()
                            .contains(&id.to_ascii_lowercase())
                        || id
                            .to_ascii_lowercase()
                            .contains(&key.to_ascii_lowercase())
                });
            if hit {
                deps.remove(&key);
                removed.push(key);
            }
        }
    }
    let from_bundles = drop_from_profile_plugin_lists(&mut value, name, &matching_ids);
    removed.extend(from_bundles);
    removed.sort();
    removed.dedup();
    if !removed.is_empty() {
        let body = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
        std::fs::write(&pkg_path, format!("{body}\n")).map_err(|e| e.to_string())?;
    }
    Ok(removed)
}

pub fn cmd_remove_plugin(name: String) -> Result<serde_json::Value, String> {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err("插件名为空".into());
    }
    if is_core_bundle(&trimmed) {
        return Err(format!("核心组件不可卸载：{trimmed}"));
    }

    // Primary path: edit package.json directly (no pnpm required).
    let removed_deps = drop_dep_from_web_package_json(&trimmed)?;

    let matching = plugin_ids_matching_dep(&trimmed);
    let mut cleared_ids = matching.clone();
    cleared_ids.extend(removed_deps.iter().cloned());
    cleared_ids.sort();
    cleared_ids.dedup();

    let _ = drop_ids_from_bootstrap(&cleared_ids);
    crate::settings::remove_from_selected_plugins(&cleared_ids)?;

    // Best-effort CLI `plugin remove` — optional; must not fail the command
    // if package.json / bootstrap / selectedPlugins were already updated.
    let mut cli_ok = false;
    let mut cli_error: Option<String> = None;
    ensure_path_for_gui();
    let _ = crate::settings::apply_npm_registry_files();
    match ensure_managed_runtime(|_, _| {}) {
        Ok((node, bin_js)) => match remove_one_plugin(&node, &bin_js, &trimmed) {
            Ok(()) => cli_ok = true,
            Err(e) => cli_error = Some(e),
        },
        Err(e) => cli_error = Some(e),
    }

    Ok(serde_json::json!({
        "ok": true,
        "removed": trimmed,
        "removedDeps": removed_deps,
        "clearedIds": cleared_ids,
        "cliOk": cli_ok,
        "cliError": cli_error,
    }))
}

/// Nuclear recovery: backup web profile package.json, clear non-core dependencies
/// and non-core `dsh.profile.bundles` so the next launch will not load third-party
/// plugins. Also clears selectedPlugins and bootstrap marker plugin list.
pub fn cmd_safe_disable_all_plugins() -> Result<serde_json::Value, String> {
    let pkg_path = web_profile_package_json().ok_or_else(|| "cannot resolve home dir".to_string())?;
    if !pkg_path.is_file() {
        return Ok(serde_json::json!({
            "ok": true,
            "message": "web profile package.json 不存在，无需禁用",
            "cleared": [],
        }));
    }

    let text = std::fs::read_to_string(&pkg_path).map_err(|e| e.to_string())?;
    let bak = pkg_path.with_file_name(format!(
        "package.json.dsh-desktop-bak.{}",
        chrono_like_now()
    ));
    std::fs::write(&bak, &text).map_err(|e| e.to_string())?;

    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut cleared: Vec<String> = Vec::new();
    if let Some(deps) = value
        .get_mut("dependencies")
        .and_then(|d| d.as_object_mut())
    {
        let keys: Vec<String> = deps.keys().cloned().collect();
        for key in keys {
            if is_core_bundle(&key) {
                continue;
            }
            deps.remove(&key);
            cleared.push(key);
        }
    }
    // Also strip non-core entries from dsh.profile.bundles (what dsh loads).
    let cleared_bundles = retain_core_only_profile_lists(&mut value);
    cleared.extend(cleared_bundles.iter().cloned());
    cleared.sort();
    cleared.dedup();

    let body = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    std::fs::write(&pkg_path, format!("{body}\n")).map_err(|e| e.to_string())?;

    // Clear selection / bootstrap so wizard does not reinstall on next launch.
    let matching: Vec<String> = {
        let mut all = cleared.clone();
        for name in &cleared {
            all.extend(plugin_ids_matching_dep(name));
        }
        all.sort();
        all.dedup();
        all
    };
    let _ = drop_ids_from_bootstrap(&matching);
    crate::settings::remove_from_selected_plugins(&matching)?;
    // Also wipe selectedPlugins entirely for nuclear recovery.
    crate::settings::clear_selected_plugins()?;

    Ok(serde_json::json!({
        "ok": true,
        "backup": bak.to_string_lossy(),
        "cleared": cleared,
        "clearedBundles": cleared_bundles,
        "coreBundles": CORE_BUNDLES,
    }))
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

pub fn cmd_plugin_catalog() -> Result<serde_json::Value, String> {
    Ok(plugin_catalog_json())
}
