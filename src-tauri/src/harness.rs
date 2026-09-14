use std::io::{BufRead, BufReader};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

const DEFAULT_PACKAGE: &str = "@deepseek-ai/dsh";
const READY_TIMEOUT: Duration = Duration::from_secs(120);
const POLL_INTERVAL: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HarnessEvent {
    Checking { message: String },
    Starting { message: String },
    Ready { url: String },
    Error { message: String },
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

fn find_free_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    drop(listener);
    Ok(port)
}

fn which(bin: &str) -> Option<String> {
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

fn resolve_launcher() -> Result<(String, Vec<String>), String> {
    if which("dsh").is_some() {
        return Ok(("dsh".into(), vec!["web".into()]));
    }

    let npx = which("npx").ok_or_else(|| {
        "未找到 Node.js / npx。请先安装 Node.js ≥ 22.19，然后重试。\n\
         Node.js / npx not found. Install Node.js ≥ 22.19 and retry."
            .to_string()
    })?;

    Ok((
        npx,
        vec![
            "--yes".into(),
            DEFAULT_PACKAGE.into(),
            "web".into(),
        ],
    ))
}

fn spawn_harness(port: u16) -> Result<Child, String> {
    let (program, mut args) = resolve_launcher()?;
    args.push("--port".into());
    args.push(port.to_string());

    let mut command = Command::new(&program);
    command
        .args(&args)
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
        .map_err(|e| format!("启动 dsh 失败 / failed to spawn dsh ({program}): {e}"))
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

fn http_ready(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("127.0.0.1");
    let port = parsed.port_or_known_default().unwrap_or(80);
    let addr: SocketAddr = if host == "localhost" || host == "127.0.0.1" {
        SocketAddr::from(([127, 0, 0, 1], port))
    } else {
        match format!("{host}:{port}").parse() {
            Ok(a) => a,
            Err(_) => return false,
        }
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok()
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

        let candidate = discovered.clone().unwrap_or_else(|| fallback.clone());
        if http_ready(&candidate) {
            return Ok(candidate);
        }

        thread::sleep(POLL_INTERVAL);
    }

    Err(format!(
        "等待 dsh 就绪超时（{}s）。/ Timed out waiting for dsh after {}s.",
        READY_TIMEOUT.as_secs(),
        READY_TIMEOUT.as_secs()
    ))
}

fn navigate_main(app: &AppHandle, url: &str) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window missing".to_string())?;
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    window.navigate(parsed).map_err(|e| e.to_string())
}

pub fn start_harness(app: AppHandle, manager: Arc<HarnessManager>) {
    thread::spawn(move || {
        manager.stop();

        emit(
            &app,
            HarnessEvent::Checking {
                message: "检查 Node.js / dsh…".into(),
            },
        );

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
                message: format!("正在启动 DeepSeek Harness（端口 {port}）…"),
            },
        );

        let mut child = match spawn_harness(port) {
            Ok(c) => c,
            Err(e) => {
                emit(&app, HarnessEvent::Error { message: e });
                return;
            }
        };

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

        emit(&app, HarnessEvent::Ready { url: url.clone() });

        if let Err(e) = navigate_main(&app, &url) {
            emit(
                &app,
                HarnessEvent::Error {
                    message: format!("打开 Web UI 失败 / navigate failed: {e}"),
                },
            );
        }
    });
}
