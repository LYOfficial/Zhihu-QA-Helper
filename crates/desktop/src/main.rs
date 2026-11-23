//! 知乎答主助手 · Windows 桌面壳
//!
//! 职责很薄但关键：拉起内核进程、开窗、驻留托盘。
//! 界面本身由内核（127.0.0.1:8787）托管，所以「关掉窗口」不等于「退出软件」——
//! 关窗只是收进托盘，定时值守继续跑。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const INSTALLED_PORT: u16 = 18787;

fn runtime_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let runtime = exe.parent()?.join("runtime");
    (cfg!(feature = "installed-runtime") || runtime.exists()).then_some(runtime)
}

fn core_port() -> u16 {
    if runtime_dir().is_some() {
        return INSTALLED_PORT;
    }
    std::env::var("ZHIHU_QA_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(8787)
}

struct CoreProcess(Mutex<Option<Child>>);

#[cfg(windows)]
fn acquire_single_instance() -> Option<isize> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let name: Vec<u16> = if core_port() == 8787 {
        "Local\\ZhihuQAHelperDesktop\0".to_string()
    } else {
        format!("Local\\ZhihuQAHelperDesktop-{}\0", core_port())
    }
    .encode_utf16()
    .collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return None;
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(handle) };
        return None;
    }
    Some(handle as isize)
}

#[cfg(not(windows))]
fn acquire_single_instance() -> Option<()> {
    Some(())
}

fn find_core_binary() -> Option<PathBuf> {
    let exe_name = if cfg!(windows) {
        "zhihu-qa-core.exe"
    } else {
        "zhihu-qa-core"
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(exe_name));
            candidates.push(dir.join("..").join(exe_name));
        }
    }
    candidates.push(PathBuf::from("target/release").join(exe_name));
    candidates.push(PathBuf::from("target/debug").join(exe_name));
    candidates.into_iter().find(|p| p.exists())
}

fn spawn_core(runtime: Option<&Path>) -> std::io::Result<Child> {
    let path = match runtime {
        Some(path) => path.join("zhihu-qa-core.exe"),
        None => find_core_binary().ok_or(std::io::ErrorKind::NotFound)?,
    };
    let mut cmd = Command::new(path);
    if let Some(runtime) = runtime {
        cmd.current_dir(runtime)
            .env("ZHIHU_QA_RUNTIME", runtime)
            .env("ZHIHU_QA_PORT", INSTALLED_PORT.to_string())
            .env_remove("ZHIHU_QA_AGENT_PORT")
            .env("ZHIHU_QA_DESKTOP_CHILD", "1")
            .stdin(Stdio::piped());
    }
    if cfg!(debug_assertions) {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
}

fn wait_core(timeout: Duration, mut child: Option<&mut Child>) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], core_port()));
    let started = Instant::now();
    while started.elapsed() < timeout {
        if let Some(child) = child.as_mut() {
            if !matches!(child.try_wait(), Ok(None)) {
                return false;
            }
            let pid = child.id().to_string();
            if core_get("/__desktop_ready").is_some_and(|response| {
                response.split_whitespace().nth(1) == Some("200")
                    && response
                        .split_once("\r\n\r\n")
                        .is_some_and(|(_, body)| body == pid)
            }) {
                return true;
            }
        } else if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(180));
    }
    false
}

fn startup_error(message: &str) {
    eprintln!("[desktop] {message}");
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR};
        let text: Vec<u16> = message.encode_utf16().chain([0]).collect();
        let title: Vec<u16> = "知乎答主助手无法启动".encode_utf16().chain([0]).collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                title.as_ptr(),
                MB_ICONERROR,
            )
        };
    }
}

// ---------------- 与内核通信（只走本机回环，不引额外依赖） ----------------

fn core_get(path: &str) -> Option<String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], core_port()));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    Some(out)
}

/// 托盘菜单直接触发一次同步（内核没在跑时静默失败，不弹窗打扰）
fn trigger_sync() {
    if let Ok(mut s) = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], core_port())),
        Duration::from_millis(600),
    ) {
        let _ = s.set_write_timeout(Some(Duration::from_secs(3)));
        let req = "POST /api/questions/sync HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        let _ = s.write_all(req.as_bytes());
        let mut buf = [0u8; 64];
        let _ = s.read(&mut buf);
    }
}

fn main() {
    let Some(_instance_guard) = acquire_single_instance() else {
        eprintln!("[desktop] 已有知乎答主助手实例在运行，忽略本次重复启动");
        return;
    };
    let runtime = runtime_dir();
    let mut child = if let Some(runtime) = &runtime {
        if [
            "web/index.html",
            "agent/zhihu-qa-agent.exe",
            "zhihu-qa-core.exe",
        ]
        .iter()
        .any(|file| !runtime.join(file).is_file())
        {
            startup_error("安装文件不完整。请重新运行安装包；不要删除现有数据目录。");
            return;
        }
        if TcpStream::connect_timeout(
            &SocketAddr::from(([127, 0, 0, 1], INSTALLED_PORT)),
            Duration::from_millis(250),
        )
        .is_ok()
        {
            startup_error("本机端口 18787 已被占用。请退出旧的知乎答主助手或释放该端口后重试。");
            return;
        }
        match spawn_core(Some(runtime)) {
            Ok(child) => Some(child),
            Err(error) => {
                startup_error(&format!("无法启动安装版内核。请重新安装后重试。\n{error}"));
                return;
            }
        }
    } else if wait_core(Duration::from_millis(120), None) {
        None
    } else {
        spawn_core(None).ok()
    };
    let ok = wait_core(
        Duration::from_secs(25),
        if runtime.is_some() {
            child.as_mut()
        } else {
            None
        },
    );
    if !ok && runtime.is_some() {
        if let Some(mut child) = child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        startup_error("安装版内核未能就绪。请先在托盘退出旧版再试；仍失败时，查看数据目录下的 logs\\core.log（默认 %APPDATA%\\ZhihuQAHelper）。");
        return;
    }

    tauri::Builder::default()
        .manage(CoreProcess(Mutex::new(child)))
        .setup(move |app| {
            // 窗口在运行时创建：开发时指向 Next dev server（可热更新），
            // 生产时指向内核托管的静态页。两者同源策略不同，不能用同一个 URL。
            let dev_url = runtime
                .is_none()
                .then(|| std::env::var("ZHIHU_QA_DEV_WEB"))
                .and_then(Result::ok)
                .filter(|v| !v.trim().is_empty());
            let target = dev_url
                .clone()
                .unwrap_or_else(|| format!("http://127.0.0.1:{}/", core_port()));
            let url = tauri::Url::parse(&target)?;
            let logged_in = core_get("/api/session")
                .and_then(|response| {
                    let (headers, body) = response.split_once("\r\n\r\n")?;
                    headers.starts_with("HTTP/1.1 200").then_some(())?;
                    serde_json::from_str::<serde_json::Value>(body).ok()
                })
                .and_then(|session| session["loggedIn"].as_bool())
                .unwrap_or(false);
            let (width, height, min_width, min_height) = if logged_in {
                (1280.0, 840.0, 820.0, 580.0)
            } else {
                (480.0, 520.0, 360.0, 430.0)
            };

            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("知乎答主助手")
                .inner_size(width, height)
                .min_inner_size(min_width, min_height)
                .maximizable(logged_in)
                .decorations(false)
                .transparent(true)
                .shadow(false)
                .center()
                .visible(false)
                .build()?;
            let _ = window.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
            if dev_url.is_some() {
                eprintln!("[desktop] 开发模式：窗口加载 {target}");
            }

            let show = MenuItem::with_id(app, "show", "打开案头", true, None::<&str>)?;
            let sync = MenuItem::with_id(app, "sync", "立即同步知乎", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出（停止值守）", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &sync, &quit])?;

            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("知乎答主助手 · 值守中")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => {
                        if let Some(state) = app.try_state::<CoreProcess>() {
                            if let Some(mut c) = state.0.lock().unwrap().take() {
                                if c.stdin.take().is_none() {
                                    let _ = c.kill();
                                }
                            }
                        }
                        app.exit(0);
                    }
                    "sync" => trigger_sync(),
                    _ => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.unminimize();
                            let _ = w.set_focus();
                        }
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            if w.is_visible().unwrap_or(false) {
                                let _ = w.hide();
                            } else {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let _ = window.show();
            let _ = window.set_focus();


            if !ok {
                // 内核没起来：窗口会显示内核自带的提示页，这里只留一条日志
                eprintln!("[desktop] 内核未在 25 秒内就绪，请检查 target/debug/zhihu-qa-core.exe");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 关窗 = 收进托盘，值守不中断
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("启动桌面壳失败");
}
