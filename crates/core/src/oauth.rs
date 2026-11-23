use std::time::{Duration, Instant};

use anyhow::{anyhow, ensure, Result};
use axum::{extract::State, Json};
use rusqlite::{params, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::{
    api::{open_in_default_browser, R},
    models::{now_iso, ZhihuUser},
    state::{AppState, Shared},
};

const SERVICE: &str = "https://zh.ijune.cn/oauth/zhihu";
const TTL: Duration = Duration::from_secs(300);

pub struct Client {
    http: reqwest::Client,
    pending: Mutex<Option<Pending>>,
}

#[derive(Deserialize)]
struct Ticket {
    url: String,
    state: String,
    secret: String,
}

struct Pending {
    ticket: Ticket,
    deadline: Instant,
}

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum Check {
    Pending,
    Exchanging,
    Expired,
    Cancelled,
    Failed,
    Complete(Authorized),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Authorized {
    session: String,
    user: ZhihuUser,
    expires_at: i64,
}

impl Client {
    pub fn new() -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("OAuth HTTP client"),
            pending: Mutex::new(None),
        }
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: Value) -> Result<Option<T>> {
        let timeout = if path == "profile" {
            Duration::from_secs(30)
        } else {
            Duration::from_secs(10)
        };
        let response = self
            .http
            .post(format!("{SERVICE}/{path}"))
            .timeout(timeout)
            .json(&body)
            .send()
            .await
            .map_err(|_| anyhow!("无法连接知乎授权服务，请检查网络后重试"))?;
        match response.status() {
            reqwest::StatusCode::UNAUTHORIZED => Ok(None),
            reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::NOT_IMPLEMENTED => {
                Err(anyhow!("知乎授权服务尚未上线，请联系服务管理员"))
            }
            reqwest::StatusCode::SERVICE_UNAVAILABLE => {
                let detail = response
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|body| body["error"].as_str().map(str::to_owned));
                Err(anyhow!(
                    "{}",
                    detail.as_deref().unwrap_or("知乎授权服务暂不可用")
                ))
            }
            status if status.is_success() => response
                .json()
                .await
                .map(Some)
                .map_err(|_| anyhow!("知乎授权服务响应格式异常")),
            _ => Err(anyhow!("知乎授权服务请求失败，请稍后重试")),
        }
    }
}

pub(crate) async fn start(State(st): State<Shared>, Json(_): Json<Value>) -> R {
    let mut pending = st.oauth.pending.lock().await;
    *pending = None;
    if crate::tasks::account_identity(&st.db.lock().await.conn).is_ok() {
        return Err(anyhow!("请先退出当前知乎账号").into());
    }
    let ticket: Ticket = st
        .oauth
        .post("start", json!({}))
        .await?
        .ok_or_else(|| anyhow!("无法发起知乎授权"))?;
    let url = reqwest::Url::parse(&ticket.url)?;
    if url.scheme() != "https"
        || url.host_str() != Some("zh.ijune.cn")
        || url.port().is_some()
        || url.path() != "/oauth/zhihu/launch"
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(anyhow!("知乎授权链接无效").into());
    }
    open_in_default_browser(url.as_str())?;
    *pending = Some(Pending {
        ticket,
        deadline: Instant::now() + TTL,
    });
    Ok(Json(json!({"ok": true})))
}

pub(crate) async fn poll(State(st): State<Shared>, Json(_): Json<Value>) -> R {
    let mut pending = st.oauth.pending.lock().await;
    let Some(flow) = pending
        .as_ref()
        .filter(|flow| flow.deadline > Instant::now())
    else {
        *pending = None;
        return Ok(Json(json!({"status": "expired"})));
    };
    let result = st
        .oauth
        .post(
            "poll",
            json!({
                "state": flow.ticket.state, "secret": flow.ticket.secret
            }),
        )
        .await?
        .unwrap_or(Check::Expired);
    let status = match result {
        Check::Pending | Check::Exchanging => "pending",
        Check::Expired => "expired",
        Check::Cancelled => "cancelled",
        Check::Failed => "failed",
        Check::Complete(auth) => {
            *pending = None;
            let session = auth.session.clone();
            persist(&st, auth).await?;
            drop(pending);
            tokio::spawn(async move {
                crate::tasks::sync_after_login(&st, Some(&session)).await;
            });
            return Ok(Json(json!({"status": "complete"})));
        }
    };
    if status != "pending" {
        *pending = None;
    }
    Ok(Json(json!({"status": status})))
}

pub(crate) async fn cancel(State(st): State<Shared>, Json(_): Json<Value>) -> R {
    let flow = st.oauth.pending.lock().await.take();
    if let Some(flow) = flow {
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            st.oauth.post::<Value>(
                "cancel",
                json!({
                    "state": flow.ticket.state, "secret": flow.ticket.secret
                }),
            ),
        )
        .await;
    }
    Ok(Json(json!({"ok": true})))
}

async fn persist(st: &AppState, auth: Authorized) -> Result<()> {
    ensure!(
        !auth.session.is_empty()
            && !auth.user.id.is_empty()
            && !auth.user.name.is_empty()
            && auth.expires_at > chrono::Utc::now().timestamp(),
        "知乎授权已过期或未返回账号信息"
    );
    let db = st.db.lock().await;
    let user = auth.user;
    db.conn.execute(
        "INSERT INTO account (id, zhihu_id, name, url_token, avatar_url, headline, answer_count,
         articles_count, follower_count, oauth_session, oauth_expires_at, updated_at)
         VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(id) DO UPDATE SET cookie=CASE WHEN account.zhihu_id=excluded.zhihu_id THEN account.cookie ELSE '' END,
         zhihu_id=excluded.zhihu_id, name=excluded.name,
         url_token=CASE WHEN account.zhihu_id=excluded.zhihu_id AND excluded.url_token='' THEN account.url_token ELSE excluded.url_token END,
         avatar_url=excluded.avatar_url,
         headline=excluded.headline, answer_count=excluded.answer_count, articles_count=excluded.articles_count,
         follower_count=excluded.follower_count, oauth_session=excluded.oauth_session,
         oauth_expires_at=excluded.oauth_expires_at, updated_at=excluded.updated_at",
        params![user.id, user.name, user.url_token, user.avatar_url, user.headline, user.answer_count,
            user.articles_count, user.follower_count, auth.session, auth.expires_at, now_iso()],
    )?;
    db.log_event("login", None, "知乎 OAuth 登录成功", &user.name);
    Ok(())
}

pub(crate) async fn logout(State(st): State<Shared>, Json(_): Json<Value>) -> R {
    let session: Option<String> = {
        let mut pending = st.oauth.pending.lock().await;
        *pending = None;
        let db = st.db.lock().await;
        let session = db
            .conn
            .query_row(
                "SELECT oauth_session FROM account WHERE id=1 AND oauth_session<>''",
                [],
                |row| row.get(0),
            )
            .optional()?;
        db.conn.execute(
            "UPDATE account SET oauth_session='', oauth_expires_at=0, cookie='' WHERE id=1",
            [],
        )?;
        db.log_event("logout", None, "退出登录", "");
        session
    };
    if let Some(session) = session {
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            st.oauth
                .post::<Value>("logout", json!({ "session":session })),
        )
        .await;
    }
    Ok(Json(json!({"ok": true})))
}

async fn active_session(st: &AppState) -> Result<(String, String)> {
    Ok(st
        .db
        .lock()
        .await
        .conn
        .query_row(
            "SELECT zhihu_id, oauth_session FROM active_account",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow!("知乎授权已过期，请重新登录"))?)
}

pub(crate) async fn refresh(State(st): State<Shared>, Json(_): Json<Value>) -> R {
    let (id, session) = active_session(&st).await?;
    let data: Option<Value> = st
        .oauth
        .post("session", json!({"session": session}))
        .await?;
    let db = st.db.lock().await;
    let Some(data) = data else {
        db.conn.execute(
            "UPDATE account SET oauth_session='', oauth_expires_at=0 WHERE oauth_session=?1",
            [&session],
        )?;
        return Err(anyhow!("知乎授权已失效，请重新登录").into());
    };
    let user: ZhihuUser = serde_json::from_value(data["user"].clone())?;
    if user.id != id {
        return Err(anyhow!("知乎账号信息校验失败").into());
    }
    let updated = db.conn.execute(
        "UPDATE account SET name=?1, url_token=COALESCE(NULLIF(?2,''), url_token), avatar_url=?3, headline=?4, answer_count=?5,
         articles_count=?6, follower_count=?7, updated_at=?8
         WHERE oauth_session=?9 AND oauth_expires_at > unixepoch()",
        params![
            user.name,
            user.url_token,
            user.avatar_url,
            user.headline,
            user.answer_count,
            user.articles_count,
            user.follower_count,
            now_iso(),
            session
        ],
    )?;
    if updated == 0 {
        return Err(anyhow!("知乎账号已切换，请重试").into());
    }
    Ok(Json(json!(user)))
}

pub(crate) async fn profile_samples(st: &AppState, id: &str) -> Result<Vec<Value>> {
    let (active, session) = active_session(st).await?;
    ensure!(active == id, "知乎账号已切换，请重试");
    let result: Value = st
        .oauth
        .post("profile", json!({"session": session}))
        .await?
        .ok_or_else(|| anyhow!("知乎授权已失效，请重新登录"))?;
    result["samples"]
        .as_array()
        .cloned()
        .ok_or_else(|| anyhow!("知乎公开创作样本格式异常"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn oauth_identity_activates_only_matching_legacy_cookie() -> Result<()> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch(crate::db::SCHEMA)?;
        conn.execute("INSERT INTO account (id, zhihu_id, name, cookie) VALUES (1, 'legacy', '旧账号', 'fixture')", [])?;
        assert!(crate::tasks::account_identity(&conn).is_err());
        let st = AppState::new(
            crate::db::Db {
                conn,
                data_dir: Default::default(),
            },
            Default::default(),
            0,
        );
        for (id, expected_cookie) in [("legacy", "fixture"), ("other", "")] {
            let reply = json!({
                "status": "complete", "session": "fixture-session",
                "expiresAt": chrono::Utc::now().timestamp() + 300,
                "user": { "id": id, "name": "测试答主" }
            });
            let Check::Complete(authorized) = serde_json::from_value(reply)? else {
                anyhow::bail!("missing authorized session");
            };
            persist(&st, authorized).await?;
            let db = st.db.lock().await;
            assert_eq!(crate::tasks::account_identity(&db.conn)?.0, id);
            let cookie: String =
                db.conn
                    .query_row("SELECT cookie FROM active_account", [], |row| row.get(0))?;
            assert_eq!(cookie, expected_cookie);
        }
        let db = st.db.lock().await;
        db.conn
            .execute("UPDATE account SET oauth_expires_at=0", [])?;
        assert!(crate::tasks::account_identity(&db.conn).is_err());
        Ok(())
    }
}
