use crate::db::Db;
use crate::zhihu::ZhihuClient;
use rusqlite::OptionalExtension;
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
    pub oauth: crate::oauth::Client,
    /// 拟稿并发名额：同时最多两份，避免多份稿子抢同一个模型 Key
    pub research_slots: tokio::sync::Semaphore,
    pub image_slots: tokio::sync::Semaphore,
    pub answer_history_lock: Mutex<()>,
    pub creator_jobs_lock: Mutex<()>,
    pub discovery_lock: Mutex<()>,
}

pub type Shared = Arc<AppState>;

#[derive(Default)]
pub struct ZhihuOpenSettings {
    pub secret: String,
    pub enabled: bool,
    pub account_id: String,
}

impl ZhihuOpenSettings {
    pub fn load(db: &Db) -> rusqlite::Result<Self> {
        Ok(db.conn.query_row(
            "SELECT access_secret, enabled, account_id FROM zhihu_open_settings WHERE id=1",
            [],
            |row| Ok(Self { secret: row.get(0)?, enabled: row.get(1)?, account_id: row.get(2)? }),
        ).optional()?.unwrap_or_default())
    }

    pub fn profile_enabled(&self, account_id: &str) -> bool {
        self.enabled && !self.secret.is_empty() && !account_id.is_empty() && self.account_id == account_id
    }
}

impl AppState {
    pub fn new(db: Db, data_dir: PathBuf, agent_port: u16) -> Shared {
        Arc::new(AppState {
            db: Mutex::new(db),
            agent: AgentBridge::new(agent_port),
            data_dir,
            started_at: chrono::Utc::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            oauth: crate::oauth::Client::new(),
            research_slots: tokio::sync::Semaphore::new(2),
            image_slots: tokio::sync::Semaphore::new(1),
            answer_history_lock: Mutex::new(()),
            creator_jobs_lock: Mutex::new(()),
            discovery_lock: Mutex::new(()),
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

pub fn zhihu_client_from_db(db: &Db) -> anyhow::Result<ZhihuClient> {
    let (cookie, sign_mode) = {
        let cookie: String = db
            .conn
            .query_row("SELECT cookie FROM active_account", [], |r| {
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
    ZhihuClient::new(cookie, crate::zhihu::SignMode::from_str(&sign_mode))
}
