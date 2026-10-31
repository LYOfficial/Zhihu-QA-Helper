use crate::db::Db;
use crate::zhihu::ZhihuClient;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppState {
    pub db: Mutex<Db>,
    pub agent: AgentBridge,
    pub data_dir: PathBuf,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub version: String,
    /// 本次扫码流程中建立起来的匿名会话，轮询时必须沿用同一份
    pub qr_cookie: Mutex<String>,
    /// 界面请求「用应用内浏览器登录」的标记，由桌面壳轮询后打开登录窗口
    pub browser_login_pending: Mutex<bool>,
    /// 拟稿并发名额：同时最多两份，避免多份稿子抢同一个模型 Key
    pub research_slots: tokio::sync::Semaphore,
}

pub type Shared = Arc<AppState>;

impl AppState {
    pub fn new(db: Db, data_dir: PathBuf, agent_port: u16) -> Shared {
        Arc::new(AppState {
            db: Mutex::new(db),
            agent: AgentBridge::new(agent_port),
            data_dir,
            started_at: chrono::Utc::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            qr_cookie: Mutex::new(String::new()),
            browser_login_pending: Mutex::new(false),
            research_slots: tokio::sync::Semaphore::new(2),
        })
    }
}

/// 与 Python 智能体进程通信。Python 侧只用标准库起一个 http server。
#[derive(Clone)]
pub struct AgentBridge {
    pub port: u16,
    pub http: reqwest::Client,
}

impl AgentBridge {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_default(),
        }
    }

    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub async fn alive(&self) -> bool {
        matches!(
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                self.http.get(format!("{}/health", self.base())).send()
            )
            .await,
            Ok(Ok(r)) if r.status().is_success()
        )
    }

    pub async fn post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let resp = self
            .http
            .post(format!("{}{}", self.base(), path))
            .json(body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("连不上本地智能体（Python 服务未启动？）：{e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "智能体返回 HTTP {status}：{}",
                text.chars().take(400).collect::<String>()
            ));
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn get(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self
            .http
            .get(format!("{}{}", self.base(), path))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("连不上本地智能体（Python 服务未启动？）：{e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "智能体返回 HTTP {status}：{}",
                text.chars().take(400).collect::<String>()
            ));
        }
        Ok(serde_json::from_str(&text)?)
    }
}

/// 组一个带当前登录态的知乎客户端
pub async fn zhihu_client(state: &AppState) -> anyhow::Result<ZhihuClient> {
    let (cookie, sign_mode) = {
        let db = state.db.lock().await;
        let cookie: String = db
            .conn
            .query_row("SELECT cookie FROM account WHERE id = 1", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap_or_default();
        let mode: String = db
            .conn
            .query_row("SELECT sign_mode FROM settings WHERE id = 1", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap_or_else(|_| "auto".into());
        (cookie, mode)
    };
    if cookie.trim().is_empty() {
        anyhow::bail!("尚未登录知乎账号");
    }
    ZhihuClient::new(cookie, crate::zhihu::SignMode::from_str(&sign_mode))
}
