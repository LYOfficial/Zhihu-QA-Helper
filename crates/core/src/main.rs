//! 知乎答主助手 · 核心服务
//!
//! 启动顺序：日志 → 数据目录/数据库 → Python 智能体 → 定时器 → 本地 HTTP 服务。
//! 桌面壳（Tauri）或浏览器都指向 http://127.0.0.1:8787 。

mod api;
mod creator;
mod db;
mod models;
mod scheduler;
mod state;
mod tasks;
mod zhihu;

use anyhow::Result;
use axum::http::{header, HeaderValue, Method};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing_subscriber::EnvFilter;

struct AgentGuard(Option<Child>);
impl Drop for AgentGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0.as_mut() {
            let _ = c.kill();
        }
    }
}

/// 给智能体挑一个空闲端口。
/// 固定端口会在「强杀内核 → python 变孤儿」时被旧进程占着，
/// 新内核就会连到旧代码上（表现为接口 404，很难查）。所以默认动态分配。
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(8788)
}

fn find_python() -> Option<String> {
    for cand in ["python", "python3", "py"] {
        if Command::new(cand)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(cand.to_string());
        }
    }
    None
}

fn spawn_agent(
    port: u16,
    data_dir: &std::path::Path,
    runtime: Option<&std::path::Path>,
) -> AgentGuard {
    let mut cmd = if let Some(runtime) = runtime {
        Command::new(runtime.join("agent").join("zhihu-qa-agent.exe"))
    } else {
        let Some(python) = find_python() else {
            tracing::warn!("系统里找不到 Python，无法启动开发版智能体");
            return AgentGuard(None);
        };
        let mut command = Command::new(python);
        command.arg("agents/main.py");
        command
    };
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("logs").join("agent.log"))
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());
    cmd.arg("--port")
        .arg(port.to_string())
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--parent-pid")
        .arg(std::process::id().to_string())
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        Ok(child) => {
            tracing::info!("Python 智能体已启动（pid {}，端口 {port}）", child.id());
            AgentGuard(Some(child))
        }
        Err(e) => {
            tracing::warn!("Python 智能体启动失败：{e}");
            AgentGuard(None)
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenv_lite();

    let data_dir = db::resolve_data_dir();
    std::fs::create_dir_all(data_dir.join("logs")).ok();

    // 日志：控制台 + 文件
    let file_appender = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("logs").join("core.log"))
        .ok();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,zhihu_qa_core=debug"));

    if let Some(f) = file_appender {
        use tracing_subscriber::prelude::*;
        let shared = SharedFile(std::sync::Arc::new(std::sync::Mutex::new(f)));
        let layer_file = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || shared.clone());
        tracing_subscriber::registry()
            .with(filter)
            .with(layer_file)
            .with(tracing_subscriber::fmt::layer())
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }

    let port: u16 = std::env::var("ZHIHU_QA_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8787);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await.map_err(|error| {
        tracing::error!(%addr, %error, "内核端口无法监听");
        error
    })?;
    let runtime = std::env::var_os("ZHIHU_QA_RUNTIME").map(PathBuf::from);
    let desktop_child = std::env::var_os("ZHIHU_QA_DESKTOP_CHILD").is_some();
    let _data_lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(data_dir.join("core.lock"))?;
    if let Err(error) = _data_lock.try_lock() {
        tracing::error!(%error, "数据目录已被另一内核占用，请从托盘退出另一实例");
        anyhow::bail!("数据目录已被另一内核占用");
    }
    if runtime.is_some() && port != 8787 && legacy_uses_data(&data_dir).await {
        tracing::error!("旧版内核仍在使用此数据目录，请从托盘退出旧版");
        anyhow::bail!("旧版内核仍在使用此数据目录");
    }
    let web_out = runtime
        .as_ref()
        .map(|path| path.join("web"))
        .unwrap_or_else(|| PathBuf::from("apps/web/.build"));
    let index = web_out.join("index.html");
    if runtime.is_some() && !index.is_file() {
        tracing::error!(path = %index.display(), "缺少安装版前端，请重新安装");
        anyhow::bail!("缺少安装版前端");
    }
    let agent_port: u16 = std::env::var("ZHIHU_QA_AGENT_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(free_port);

    let database = db::Db::open(&data_dir)?;
    let state = state::AppState::new(database, data_dir.clone(), agent_port);

    let _agent = spawn_agent(agent_port, &data_dir, runtime.as_deref());
    tasks::recover_stale_jobs(&state).await;
    scheduler::spawn(state.clone());
    let bootstrap_state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        tasks::bootstrap_if_empty(&bootstrap_state).await;
    });

    // 静态前端：优先 apps/web/.build（next export 的产物），否则给一个提示页
    let mut app = api::router(state.clone()).layer(
        CorsLayer::new()
            .allow_origin([
                "http://localhost:31873".parse::<HeaderValue>().unwrap(),
                "http://127.0.0.1:31873".parse::<HeaderValue>().unwrap(),
                "http://tauri.localhost".parse::<HeaderValue>().unwrap(),
            ])
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(tower_http::cors::Any),
    );

    if index.exists() {
        if desktop_child {
            app = app.route(
                "/__desktop_ready",
                axum::routing::get(|| async { std::process::id().to_string() }),
            );
        }
        app = app.fallback_service(ServeDir::new(&web_out).fallback(ServeFile::new(&index)));
        tracing::info!("静态前端：{}", web_out.display());
    } else {
        app = app.fallback(|| async {
            (
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                r#"<!doctype html><meta charset="utf-8">
<title>知乎答主助手 · 内核已启动</title>
<body style="font:15px/1.7 'Microsoft YaHei UI',sans-serif;background:#f7f4ed;color:#1b1a17;padding:48px;max-width:640px;margin:auto">
<h1 style="font-family:KaiTi,serif;font-weight:400">内核已启动</h1>
<p>后端运行在 <code>127.0.0.1:8787</code>，但还没找到前端产物。</p>
<p>开发模式请另开终端执行 <code>npm run dev:web</code> 并访问 <code>localhost:31873</code>；<br>
打包模式请先执行 <code>npm run build:web</code> 生成 <code>apps/web/.build</code>。</p>
</body>"#,
            )
        });
    }

    tracing::info!("知乎答主助手 已就绪 → http://{addr}");
    tracing::info!("数据目录：{}", data_dir.display());

    if desktop_child {
        let (close, closed) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = std::io::stdin().read(&mut [0]);
            let _ = close.send(());
        });
        tokio::select! {
            result = axum::serve(listener, app) => result?,
            _ = closed => {},
        }
    } else {
        axum::serve(listener, app).await?;
    }
    Ok(())
}

async fn legacy_uses_data(data_dir: &std::path::Path) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    else {
        return false;
    };
    let Ok(response) = client.get("http://127.0.0.1:8787/api/health").send().await else {
        return false;
    };
    let Ok(health) = response.json::<serde_json::Value>().await else {
        return false;
    };
    let Some(old_dir) = health["dataDir"].as_str() else {
        return false;
    };
    match (
        data_dir.canonicalize(),
        std::path::Path::new(old_dir).canonicalize(),
    ) {
        (Ok(current), Ok(previous)) if cfg!(windows) => current
            .to_string_lossy()
            .eq_ignore_ascii_case(&previous.to_string_lossy()),
        (Ok(current), Ok(previous)) => current == previous,
        _ => false,
    }
}

fn dotenv_lite() -> Option<()> {
    let txt = std::fs::read_to_string(".env").ok()?;
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if std::env::var(k).is_err() {
                std::env::set_var(k, v.trim());
            }
        }
    }
    Some(())
}

#[derive(Clone)]
struct SharedFile(std::sync::Arc<std::sync::Mutex<std::fs::File>>);
impl Write for SharedFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut f) => f.write(buf),
            Err(_) => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock() {
            Ok(mut f) => f.flush(),
            Err(_) => Ok(()),
        }
    }
}
