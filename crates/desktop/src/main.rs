//! 知乎答主助手 · Windows 桌面壳
//!
//! 职责很薄但关键：拉起内核进程、开窗、驻留托盘。
//! 界面本身由内核（127.0.0.1:8787）托管，所以「关掉窗口」不等于「退出软件」——
//! 关窗只是收进托盘，定时值守继续跑。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const PORT: u16 = 8787;

struct CoreProcess(Mutex<Option<Child>>);

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

fn spawn_core() -> Option<Child> {
    // 已经有一个在跑就不重复起
    if TcpStream::connect_timeout(
        &"127.0.0.1:8787".parse::<SocketAddr>().ok()?,
        Duration::from_millis(250),
    )
    .is_ok()
    {
        return None;
    }
    let path = find_core_binary()?;
    let mut cmd = Command::new(path);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn().ok()
}

fn wait_core(timeout: Duration) -> bool {
    let addr: SocketAddr = format!("127.0.0.1:{PORT}").parse().unwrap();
    let started = Instant::now();
    while started.elapsed() < timeout {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(180));
    }
    false
}


// ---------------- 与内核通信（只走本机回环，不引额外依赖） ----------------

fn core_post(path: &str, body: &str) -> Option<String> {
    let addr: SocketAddr = format!("127.0.0.1:{PORT}").parse().ok()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.as_bytes().len(),
        body
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    Some(out)
}

fn core_get(path: &str) -> Option<String> {
    let addr: SocketAddr = format!("127.0.0.1:{PORT}").parse().ok()?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(800)).ok()?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let req = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).ok()?;
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    Some(out)
}

/// 知乎 Cookie 都在 zhihu.com 域下；拿到 z_c0 就说明登录成功
fn harvest_cookie(app: &tauri::AppHandle) -> Option<String> {
    let win = app.get_webview_window("login")?;
    let url = tauri::Url::parse("https://www.zhihu.com").ok()?;
    let cookies = win.cookies_for_url(url).ok()?;
    let mut parts: Vec<String> = Vec::new();
    let mut has_zc0 = false;
    for c in cookies {
        let name = c.name().to_string();
        if name == "z_c0" {
            has_zc0 = true;
        }
        parts.push(format!("{}={}", name, c.value()));
    }
    if !has_zc0 {
        return None;
    }
    Some(parts.join("; "))
}

/// 打开知乎登录窗口，登录成功后把 Cookie 交给内核并关窗
fn open_login_window(app: tauri::AppHandle) {
    // 窗口必须在主线程创建，否则 Tauri 会 panic
    let app_main = app.clone();
    let _ = app.run_on_main_thread(move || {
        if app_main.get_webview_window("login").is_none() {
            if let Ok(url) = tauri::Url::parse("https://www.zhihu.com/signin") {
                let _ = WebviewWindowBuilder::new(
                    &app_main,
                    "login",
                    WebviewUrl::External(url),
                )
                .title("登录知乎 · 完成后本窗口会自动关闭")
                .inner_size(560.0, 780.0)
                .center()
                .build();
            }
        }
    });

    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(900);
        loop {
            std::thread::sleep(Duration::from_millis(900));
            if Instant::now() > deadline || app.get_webview_window("login").is_none() {
                break;
            }
            if let Some(cookie) = harvest_cookie(&app) {
                let body = serde_json::json!({ "cookie": cookie }).to_string();
                eprintln!("[desktop] 已从登录窗口取到知乎 Cookie，提交内核");
                let _ = core_post("/api/session/cookie", &body);
                let _ = core_post("/api/login/browser/done", "{}");
                if let Some(w) = app.get_webview_window("login") {
                    let _ = w.close();
                }
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.eval("window.location.reload()");
                    let _ = main.set_focus();
                }
                break;
            }
        }
    });
}

/// 轮询内核：界面点了「内置浏览器登录」就开窗
fn watch_browser_login(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(1200));
        if let Some(resp) = core_get("/api/login/browser") {
            if resp.contains("\"pending\":true") {
                open_login_window(app.clone());
                let _ = core_post("/api/login/browser/done", "{}");
                std::thread::sleep(Duration::from_secs(2));
            }
        }
    });
}

/// 托盘菜单直接触发一次同步（内核没在跑时静默失败，不弹窗打扰）
fn trigger_sync() {
    if let Ok(mut s) = TcpStream::connect_timeout(
        &format!("127.0.0.1:{PORT}").parse().unwrap(),
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
    let child = spawn_core();
    let ok = wait_core(Duration::from_secs(25));

    tauri::Builder::default()
        .manage(CoreProcess(Mutex::new(child)))
        .setup(move |app| {
            // 窗口在运行时创建：开发时指向 Next dev server（可热更新），
            // 生产时指向内核托管的静态页。两者同源策略不同，不能用同一个 URL。
            let dev_url = std::env::var("ZHIHU_QA_DEV_WEB")
                .ok()
                .filter(|v| !v.trim().is_empty());
            let target = dev_url
                .clone()
                .unwrap_or_else(|| format!("http://127.0.0.1:{PORT}/"));
            let url = tauri::Url::parse(&target)?;

            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
                .title("知乎答主助手")
                .inner_size(1280.0, 840.0)
                .min_inner_size(1060.0, 700.0)
                .center()
                .visible(false)
                .build()?;
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
                                let _ = c.kill();
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

            watch_browser_login(app.handle().clone());

            if !ok {
                // 内核没起来：窗口会显示内核自带的提示页，这里只留一条日志
                eprintln!("[desktop] 内核未在 25 秒内就绪，请检查 target/debug/zhihu-qa-core.exe");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 关窗 = 收进托盘，值守不中断
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("启动桌面壳失败");
}
