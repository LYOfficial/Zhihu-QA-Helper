//! 本地 HTTP API。前端（Next.js）与桌面壳都通过 127.0.0.1:8787/api 访问。

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::*;
use crate::state::{zhihu_client, Shared};
use crate::tasks;

pub struct ApiError(anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("{}", self.0), "ok": false })),
        )
            .into_response()
    }
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}

type R = Result<Json<Value>, ApiError>;

pub fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/session", get(get_session))
        .route("/session/qrcode", post(create_qrcode))
        .route("/session/qrcode/{token}", get(poll_qrcode))
        .route("/session/cookie", post(login_with_cookie))
        .route("/session/logout", post(logout))
        .route("/login/browser", post(browser_login_request).get(browser_login_pending))
        .route("/login/browser/done", post(browser_login_done))
        .route("/session/open-browser", post(open_login_in_browser))
        .route("/session/refresh", post(refresh_me))
        .route("/profile", get(get_profile).put(put_profile))
        .route("/profile/analyze", post(analyze_profile))
        .route("/sources", get(get_sources).put(put_sources))
        .route("/sources/discover", post(discover_sources))
        .route("/questions", get(list_questions))
        .route("/questions/sync", post(sync_now))
        .route("/questions/{id}", get(get_question))
        .route("/questions/{id}/research", post(research_question))
        .route("/questions/{id}/status", post(set_status))
        .route("/questions/{id}/answered", post(mark_answered))
        .route("/questions/{id}/dismiss", post(dismiss_question))
        .route("/drafts/{qid}", get(get_draft).put(put_draft))
        .route("/images/{qid}/{name}", get(get_image).delete(delete_image))
        .route("/model", get(get_model).put(put_model))
        .route("/model/catalog", get(model_catalog))
        .route("/model/test", post(test_model))
        .route("/model/list", post(list_models))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/events", get(list_events))
        .route("/stats/overview", get(stats_overview))
        .route("/stats/series", get(stats_series))
        .with_state(state);
    Router::new().nest("/api", api)
}

// ---------------- 基础 ----------------

async fn health(State(st): State<Shared>) -> R {
    let agent = st.agent.alive().await;
    let db_ok = st
        .db
        .lock()
        .await
        .conn
        .query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
        .is_ok();
    Ok(Json(json!(Health {
        ok: db_ok,
        version: st.version.clone(),
        agent,
        db: if db_ok { "sqlite ok".into() } else { "sqlite error".into() },
        data_dir: st.data_dir.to_string_lossy().to_string(),
        uptime_seconds: (chrono::Utc::now() - st.started_at).num_seconds().max(0),
    })))
}

// ---------------- 会话 ----------------

async fn get_session(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let row: Option<(String, String, String, String, String, i64, i64, i64, String, Option<String>)> = db
        .conn
        .query_row(
            "SELECT zhihu_id, name, url_token, avatar_url, headline, answer_count, articles_count, follower_count, cookie, expires_at FROM account WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?)),
        )
        .ok();
    let sign_mode = db
        .conn
        .query_row("SELECT sign_mode FROM settings WHERE id=1", [], |r| r.get::<_, String>(0))
        .unwrap_or_else(|_| "auto".into());
    let last_sync: Option<String> = db
        .conn
        .query_row("SELECT updated_at FROM account WHERE id=1", [], |r| r.get(0))
        .ok()
        .filter(|s: &String| !s.is_empty());

    let (logged_in, user, expires_at) = match row {
        Some((id, name, tok, avatar, headline, ac, arc, fc, cookie, exp)) => (
            !cookie.trim().is_empty() && !name.is_empty(),
            if name.is_empty() {
                None
            } else {
                Some(ZhihuUser {
                    id,
                    name,
                    url_token: tok,
                    avatar_url: avatar,
                    headline,
                    answer_count: ac,
                    articles_count: arc,
                    follower_count: fc,
                })
            },
            exp,
        ),
        None => (false, None, None),
    };

    Ok(Json(json!(Session {
        logged_in,
        user,
        expires_at,
        last_sync_at: last_sync,
        sign_mode,
    })))
}

async fn create_qrcode(State(st): State<Shared>) -> R {
    let sign_mode = {
        let db = st.db.lock().await;
        db.conn
            .query_row("SELECT sign_mode FROM settings WHERE id=1", [], |r| r.get::<_, String>(0))
            .unwrap_or_else(|_| "auto".into())
    };
    let mut client = crate::zhihu::ZhihuClient::new(
        String::new(),
        crate::zhihu::SignMode::from_str(&sign_mode),
    )?;
    let (token, link, expires_secs) = client.qr_create().await?;
    // 轮询要沿用同一份匿名会话（含 d_c0），否则知乎认不出这张码
    *st.qr_cookie.lock().await = client.cookie.clone();
    let svg = crate::zhihu::make_qr_svg(&link).unwrap_or_default();
    Ok(Json(json!(QrTicket {
        token,
        link,
        svg,
        expires_at: expires_secs * 1000,
    })))
}

/// 把登录态落库（扫码登录与 Cookie 登录共用）
async fn persist_account(st: &Shared, cookie: &str, user: &ZhihuUser) {
    let db = st.db.lock().await;
    let _ = db.conn.execute(
        "INSERT INTO account (id, zhihu_id, name, url_token, avatar_url, headline, answer_count, articles_count, follower_count, cookie, expires_at, updated_at)
         VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,?10)
         ON CONFLICT(id) DO UPDATE SET zhihu_id=?1, name=?2, url_token=?3, avatar_url=?4, headline=?5,
         answer_count=?6, articles_count=?7, follower_count=?8, cookie=?9, updated_at=?10",
        params![
            user.id, user.name, user.url_token, user.avatar_url, user.headline,
            user.answer_count, user.articles_count, user.follower_count, cookie, now_iso()
        ],
    );
    db.log_event("login", None, "登录成功", &user.name);
    let should_bootstrap = db
        .conn
        .query_row("SELECT COUNT(*) FROM questions", [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        == 0;
    drop(db);
    if should_bootstrap {
        let state = st.clone();
        tokio::spawn(async move {
            if let Err(e) = tasks::run_sync_recent(&state, 3).await {
                tracing::warn!("首次登录补齐最近 3 天通知失败：{e}");
            }
        });
    }
}

async fn poll_qrcode(State(st): State<Shared>, Path(token): Path<String>) -> R {
    let cookie = st.qr_cookie.lock().await.clone();
    let client = crate::zhihu::ZhihuClient::new(cookie, crate::zhihu::SignMode::Off)?;
    let (state_str, cb, note) = client.qr_poll(&token).await?;

    // 风控拦截：不要再让界面干等，如实告诉用户换一条路
    if state_str == "blocked" {
        return Ok(Json(json!(QrState {
            status: "blocked".into(),
            user: None,
            message: note,
        })));
    }

    if state_str == "confirmed" {
        let mut c = client;
        let Some(raw) = cb else {
            return Ok(Json(json!(QrState {
                status: "error".into(),
                user: None,
                message: "扫码已确认但未拿到登录凭证".into(),
            })));
        };
        c.merge_cookie(&raw);
        match c.me().await {
            Ok(user) => {
                persist_account(&st, &c.cookie, &user).await;
                return Ok(Json(json!(QrState {
                    status: "confirmed".into(),
                    user: Some(user),
                    message: "登录成功".into(),
                })));
            }
            Err(e) => {
                return Ok(Json(json!(QrState {
                    status: "error".into(),
                    user: None,
                    message: format!("扫码已确认，但获取账号失败：{e}"),
                })));
            }
        }
    }

    Ok(Json(json!(QrState {
        status: state_str,
        user: None,
        message: note,
    })))
}

/// 手工粘贴 Cookie 的兜底登录：用户从浏览器 DevTools 里复制 z_c0 即可。
/// 这是知乎改协议时唯一必然可用的入口，所以做成正式能力而不是隐藏开关。
fn normalize_cookie(raw: &str) -> String {
    let s = raw
        .trim()
        .trim_start_matches("Cookie:")
        .trim_start_matches("cookie:")
        .replace(['\n', '\r', '\t'], " ")
        .trim()
        .to_string();
    if s.contains('=') {
        return s;
    }
    // 只粘贴了 z_c0 的值
    if s.len() > 12 && !s.contains(' ') {
        return format!("z_c0={s}");
    }
    s
}

async fn login_with_cookie(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let raw = body.get("cookie").and_then(|x| x.as_str()).unwrap_or("");
    let cookie = normalize_cookie(raw);
    if cookie.len() < 12 {
        return Err(ApiError(anyhow::anyhow!(
            "Cookie 太短了，至少要包含 z_c0=... 这一项"
        )));
    }
    let sign_mode = {
        let db = st.db.lock().await;
        db.conn
            .query_row("SELECT sign_mode FROM settings WHERE id=1", [], |r| r.get::<_, String>(0))
            .unwrap_or_else(|_| "auto".into())
    };
    let client = crate::zhihu::ZhihuClient::new(
        cookie.clone(),
        crate::zhihu::SignMode::from_str(&sign_mode),
    )?;
    let user = client.me().await.map_err(|e| {
        ApiError(anyhow::anyhow!(
            "这份 Cookie 无效或已过期（{e}）。请确认复制的是 z_c0，且没有漏掉等号后面那段。"
        ))
    })?;
    persist_account(&st, &client.cookie, &user).await;
    Ok(Json(json!({ "ok": true, "user": user })))
}

/// 用系统默认浏览器打开知乎登录页。
/// 这条路走的是用户自己的浏览器会话，软件读不到它的 Cookie，
/// 所以登录后需要用户把 z_c0 复制回来；好在只需要一次。
async fn open_login_in_browser() -> R {
    let url = "https://www.zhihu.com/signin";
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(url).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
    Ok(Json(json!({ "ok": true, "url": url })))
}

// 桌面壳每隔一两秒问一次：要不要开登录窗口
async fn browser_login_request(State(st): State<Shared>) -> R {
    *st.browser_login_pending.lock().await = true;
    Ok(Json(json!({ "ok": true })))
}
async fn browser_login_pending(State(st): State<Shared>) -> R {
    let pending = *st.browser_login_pending.lock().await;
    Ok(Json(json!({ "pending": pending })))
}
async fn browser_login_done(State(st): State<Shared>) -> R {
    *st.browser_login_pending.lock().await = false;
    Ok(Json(json!({ "ok": true })))
}

async fn logout(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let _ = db.conn.execute(
        "UPDATE account SET cookie='', name='', zhihu_id='' WHERE id=1",
        [],
    );
    db.log_event("logout", None, "退出登录", "");
    Ok(Json(json!({ "ok": true })))
}

async fn refresh_me(State(st): State<Shared>) -> R {
    let client = zhihu_client(&st).await?;
    let user = client.me().await?;
    let db = st.db.lock().await;
    let _ = db.conn.execute(
        "UPDATE account SET name=?1, url_token=?2, avatar_url=?3, headline=?4, answer_count=?5, articles_count=?6, follower_count=?7, updated_at=?8 WHERE id=1",
        params![user.name, user.url_token, user.avatar_url, user.headline, user.answer_count, user.articles_count, user.follower_count, now_iso()],
    );
    let _ = db.conn.execute("UPDATE account SET cookie=?1 WHERE id=1", params![client.cookie]);
    Ok(Json(json!(user)))
}

// ---------------- 画像 ----------------

async fn get_profile(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let (status, summary, evidence, sample, updated): (String, String, String, i64, Option<String>) = db
        .conn
        .query_row(
            "SELECT status, summary, evidence, sample_size, updated_at FROM profile_meta WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap_or(("empty".into(), "".into(), "[]".into(), 0, None));

    let mut stmt = db.conn.prepare(
        "SELECT id, kind, label, weight, keywords, locked, note FROM profile_terms ORDER BY ord, weight DESC",
    )?;
    let terms: Vec<ProfileTerm> = stmt
        .query_map([], |r| {
            Ok(ProfileTerm {
                id: r.get(0)?,
                kind: r.get(1)?,
                label: r.get(2)?,
                weight: r.get(3)?,
                keywords: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                locked: r.get::<_, i64>(5)? != 0,
                note: r.get(6)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();

    Ok(Json(json!(Profile {
        status,
        summary,
        updated_at: updated,
        terms,
        evidence: serde_json::from_str(&evidence).unwrap_or_default(),
        sample_size: sample,
    })))
}

async fn analyze_profile(State(st): State<Shared>) -> R {
    {
        let db = st.db.lock().await;
        let running: Option<String> = db
            .conn
            .query_row(
                "SELECT id FROM jobs WHERE kind='profile' AND state='running' ORDER BY started_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .ok();
        if let Some(id) = running {
            return Ok(Json(json!({ "ok": true, "message": "画像分析已经在进行中", "jobId": id })));
        }
    }
    let job_id = tasks::job_start(&st, "profile", None, "读取历史回答").await;
    {
        let db = st.db.lock().await;
        let _ = db.conn.execute("UPDATE profile_meta SET status='running' WHERE id=1", []);
    }
    let st2 = st.clone();
    let job_id2 = job_id.clone();
    tokio::spawn(async move {
        if let Err(e) = tasks::run_profile_job(&st2, job_id2).await {
            tracing::warn!("画像分析失败: {e}");
        }
    });
    Ok(Json(json!({ "ok": true, "message": "已开始分析历史内容", "jobId": job_id })))
}

async fn put_profile(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let terms = body.get("terms").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    let summary = body.get("summary").and_then(|x| x.as_str()).unwrap_or("");
    {
        let db = st.db.lock().await;
        let _ = db.conn.execute("DELETE FROM profile_terms", []);
        for (i, t) in terms.iter().enumerate() {
            let label = t.get("label").and_then(|x| x.as_str()).unwrap_or("").trim();
            if label.is_empty() {
                continue;
            }
            let id = t
                .get("id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("t_{}", Uuid::new_v4().simple()));
            let _ = db.conn.execute(
                "INSERT INTO profile_terms (id, kind, label, weight, keywords, locked, note, ord)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    id,
                    t.get("kind").and_then(|x| x.as_str()).unwrap_or("primary"),
                    label,
                    t.get("weight").and_then(|x| x.as_f64()).unwrap_or(0.6),
                    t.get("keywords").cloned().unwrap_or(json!([])).to_string(),
                    t.get("locked").and_then(|x| x.as_bool()).unwrap_or(false) as i64,
                    t.get("note").and_then(|x| x.as_str()).unwrap_or(""),
                    i as i64
                ],
            );
        }
        let _ = db.conn.execute(
            "UPDATE profile_meta SET summary=?1, updated_at=?2 WHERE id=1",
            params![summary, now_iso()],
        );
        db.log_event("profile", None, "手动修订领域词条", "");
    }
    get_profile(State(st)).await
}

// ---------------- 权威平台 ----------------

async fn get_sources(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let mut stmt = db.conn.prepare(
        "SELECT id, domain, name, base_url, kind, trust, enabled, notes, use_for, discovered_by, ok FROM sources ORDER BY domain, trust DESC",
    )?;
    let list: Vec<Source> = stmt
        .query_map([], |r| {
            Ok(Source {
                id: r.get(0)?,
                domain: r.get(1)?,
                name: r.get(2)?,
                base_url: r.get(3)?,
                kind: r.get(4)?,
                trust: r.get(5)?,
                enabled: r.get::<_, i64>(6)? != 0,
                notes: r.get(7)?,
                use_for: serde_json::from_str(&r.get::<_, String>(8)?).unwrap_or_default(),
                discovered_by: r.get(9)?,
                ok: r.get::<_, i64>(10)? != 0,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Json(json!({ "sources": list })))
}

async fn put_sources(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    if let Some(list) = body.get("sources").and_then(|x| x.as_array()) {
        let db = st.db.lock().await;
        for s in list {
            let id = s.get("id").and_then(|x| x.as_str()).unwrap_or("");
            if id.is_empty() {
                continue;
            }
            let _ = db.conn.execute(
                "UPDATE sources SET enabled=?1, trust=?2, use_for=?3, notes=?4 WHERE id=?5",
                params![
                    s.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true) as i64,
                    s.get("trust").and_then(|x| x.as_i64()).unwrap_or(3),
                    s.get("useFor").cloned().unwrap_or(json!([])).to_string(),
                    s.get("notes").and_then(|x| x.as_str()).unwrap_or(""),
                    id
                ],
            );
        }
    }
    get_sources(State(st)).await
}

async fn discover_sources(State(st): State<Shared>) -> R {
    let st2 = st.clone();
    tokio::spawn(async move {
        let _ = tasks::run_discover_sources(&st2).await;
    });
    Ok(Json(json!({ "ok": true, "message": "正在按领域检索权威平台" })))
}

// ---------------- 问题 ----------------

fn row_to_question(r: &rusqlite::Row<'_>) -> rusqlite::Result<Question> {
    Ok(Question {
        id: r.get(0)?,
        zhihu_id: r.get(1)?,
        question_id: r.get(2)?,
        source: r.get(3)?,
        title: r.get(4)?,
        excerpt: r.get(5)?,
        url: r.get(6)?,
        domain: r.get(7)?,
        sub_domain: r.get(8)?,
        fit: r.get(9)?,
        status: r.get(10)?,
        bucket: r.get(11)?,
        answer_count: r.get(12)?,
        follower_count: r.get(13)?,
        asker_name: r.get(14)?,
        asker_headline: r.get(15)?,
        tags: serde_json::from_str(&r.get::<_, String>(16)?).unwrap_or_default(),
        created_at: r.get(17)?,
        synced_at: r.get(18)?,
        has_draft: r.get::<_, i64>(19)? > 0,
        image_count: r.get(20)?,
        word_count: r.get(21)?,
        doubt_count: r.get(22)?,
    })
}

const Q_SELECT: &str = "SELECT q.id, q.zhihu_id, q.question_id, q.source, q.title, q.excerpt, q.url,
    q.domain, q.sub_domain, q.fit, q.status, q.bucket, q.answer_count, q.follower_count,
    q.asker_name, q.asker_headline, q.tags, q.created_at, q.synced_at,
    (SELECT COUNT(*) FROM drafts d WHERE d.question_id = q.id) AS has_draft,
    (SELECT COUNT(*) FROM images i WHERE i.question_id = q.id) AS image_count,
    COALESCE((SELECT d.word_count FROM drafts d WHERE d.question_id = q.id), 0) AS word_count,
    COALESCE((SELECT json_array_length(d.doubts) FROM drafts d WHERE d.question_id = q.id), 0) AS doubt_count
    FROM questions q";

async fn list_questions(State(st): State<Shared>, Query(qp): Query<HashMap<String, String>>) -> R {
    let mut where_parts: Vec<String> = Vec::new();
    let mut args: Vec<String> = Vec::new();

    if let Some(s) = qp.get("status").filter(|s| !s.is_empty() && *s != "all") {
        where_parts.push(format!("q.status = ?{}", args.len() + 1));
        args.push(s.clone());
    }
    if let Some(d) = qp.get("domain").filter(|s| !s.is_empty() && *s != "all") {
        where_parts.push(format!("q.domain = ?{}", args.len() + 1));
        args.push(d.clone());
    }
    if let Some(b) = qp.get("bucket").filter(|s| !s.is_empty() && *s != "all") {
        where_parts.push(format!("q.bucket = ?{}", args.len() + 1));
        args.push(b.clone());
    }
    if let Some(k) = qp.get("q").filter(|s| !s.trim().is_empty()) {
        where_parts.push(format!("(q.title LIKE ?{0} OR q.excerpt LIKE ?{0})", args.len() + 1));
        args.push(format!("%{}%", k));
    }
    // 默认不展示已回答/已弃用
    if qp.get("status").is_none() {
        where_parts.push("q.status IN ('inbox','researching','pending_review')".into());
    }

    let sql = format!(
        "{Q_SELECT}{} ORDER BY CASE q.status WHEN 'pending_review' THEN 0 WHEN 'researching' THEN 1 ELSE 2 END, q.fit DESC, q.synced_at DESC LIMIT 500",
        if where_parts.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", where_parts.join(" AND "))
        }
    );

    let db = st.db.lock().await;
    let mut stmt = db.conn.prepare(&sql)?;
    let params_ref: Vec<&dyn rusqlite::ToSql> =
        args.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let list: Vec<Question> = stmt
        .query_map(params_ref.as_slice(), |r| row_to_question(r))?
        .filter_map(|x| x.ok())
        .collect();

    let domains = {
        let mut dstmt = db
            .conn
            .prepare("SELECT domain, COUNT(*) FROM questions GROUP BY domain ORDER BY 2 DESC")?;
        let rows = dstmt
            .query_map([], |r| Ok(json!({ "domain": r.get::<_, String>(0)?, "count": r.get::<_, i64>(1)? })))?
            .filter_map(|x| x.ok())
            .collect::<Vec<Value>>();
        rows
    };

    Ok(Json(json!({ "questions": list, "domains": domains })))
}

async fn get_question(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let db = st.db.lock().await;
    let sql = format!("{Q_SELECT} WHERE q.id = ?1");
    let q = db
        .conn
        .query_row(&sql, params![id], |r| row_to_question(r))
        .map_err(|_| ApiError(anyhow::anyhow!("问题不存在")))?;
    Ok(Json(json!(q)))
}

async fn sync_now(State(st): State<Shared>) -> R {
    let st2 = st.clone();
    tokio::spawn(async move {
        let _ = tasks::run_sync(&st2).await;
    });
    Ok(Json(json!({ "ok": true, "message": "开始同步知乎通知" })))
}

async fn research_question(State(st): State<Shared>, Path(id): Path<String>) -> R {
    {
        let db = st.db.lock().await;
        let _ = db.conn.execute(
            "UPDATE questions SET status='researching' WHERE id=?1",
            params![id],
        );
    }
    let st2 = st.clone();
    let qid = id.clone();
    tokio::spawn(async move {
        if let Err(e) = tasks::run_research(&st2, &qid).await {
            tracing::warn!("出稿失败: {e}");
        }
    });
    Ok(Json(json!({ "ok": true, "message": "智能体已开始工作" })))
}

async fn set_status(State(st): State<Shared>, Path(id): Path<String>, Json(body): Json<Value>) -> R {
    let status = body.get("status").and_then(|x| x.as_str()).unwrap_or("inbox");
    let db = st.db.lock().await;
    let _ = db.conn.execute(
        "UPDATE questions SET status=?1 WHERE id=?2",
        params![status, id],
    );
    Ok(Json(json!({ "ok": true })))
}

async fn mark_answered(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let freed = {
        let db = st.db.lock().await;
        // 归档正文到领域目录，再清掉本地图片
        let row: Option<(String, String, String)> = db
            .conn
            .query_row(
                "SELECT q.domain, q.title, COALESCE(d.markdown,'') FROM questions q LEFT JOIN drafts d ON d.question_id=q.id WHERE q.id=?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok();
        if let Some((domain, title, md)) = row {
            if !md.trim().is_empty() {
                let _ = db.archive_draft(&domain, &title, &md);
            }
        }
        let freed = db.purge_images(&id).unwrap_or(0);
        let _ = db.conn.execute(
            "UPDATE questions SET status='answered', bucket='archived' WHERE id=?1",
            params![id],
        );
        db.log_event("answered", Some(&id), "标记为已回答", &format!("释放 {} KB", freed / 1024));
        freed
    };
    Ok(Json(json!({ "ok": true, "freedBytes": freed })))
}

async fn dismiss_question(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let freed = {
        let db = st.db.lock().await;
        let freed = db.purge_images(&id).unwrap_or(0);
        let _ = db.conn.execute("DELETE FROM drafts WHERE question_id=?1", params![id]);
        let _ = db.conn.execute(
            "UPDATE questions SET status='dismissed', bucket='archived' WHERE id=?1",
            params![id],
        );
        db.log_event("dismiss", Some(&id), "弃用该问题", "");
        freed
    };
    Ok(Json(json!({ "ok": true, "freedBytes": freed })))
}

// ---------------- 草稿与图片 ----------------

async fn get_draft(State(st): State<Shared>, Path(qid): Path<String>) -> R {
    let db = st.db.lock().await;
    let row = db
        .conn
        .query_row(
            "SELECT markdown, doubts, citations, model, word_count, updated_at FROM drafts WHERE question_id=?1",
            params![qid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .ok();

    let mut stmt = db
        .conn
        .prepare("SELECT id, question_id, filename, source_url, source_name, caption, bytes, created_at FROM images WHERE question_id=?1 ORDER BY created_at")?;
    let images: Vec<ImageAsset> = stmt
        .query_map(params![qid], |r| {
            let filename: String = r.get(2)?;
            let qid: String = r.get(1)?;
            Ok(ImageAsset {
                id: r.get(0)?,
                question_id: qid.clone(),
                url: format!("/api/images/{}/{}", qid, filename),
                filename,
                source_url: r.get(3)?,
                source_name: r.get(4)?,
                caption: r.get(5)?,
                bytes: r.get(6)?,
                created_at: r.get(7)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();

    match row {
        Some((md, doubts, cites, model, wc, upd)) => Ok(Json(json!(Draft {
            question_id: qid,
            markdown: md,
            doubts: serde_json::from_str(&doubts).unwrap_or_default(),
            citations: serde_json::from_str(&cites).unwrap_or_default(),
            images,
            word_count: wc,
            updated_at: upd,
            model,
        }))),
        None => Ok(Json(json!({
            "questionId": qid, "markdown": "", "doubts": [], "citations": [],
            "images": images, "wordCount": 0, "updatedAt": "", "model": ""
        }))),
    }
}

async fn put_draft(State(st): State<Shared>, Path(qid): Path<String>, Json(body): Json<Value>) -> R {
    let md = body.get("markdown").and_then(|x| x.as_str()).unwrap_or("");
    let wc = md.chars().filter(|c| !c.is_whitespace()).count() as i64;
    let db = st.db.lock().await;
    let _ = db.conn.execute(
        "INSERT INTO drafts (question_id, markdown, doubts, citations, model, word_count, updated_at)
         VALUES (?1,?2,'[]','[]','manual',?3,?4)
         ON CONFLICT(question_id) DO UPDATE SET markdown=?2, word_count=?3, updated_at=?4",
        params![qid, md, wc, now_iso()],
    );
    Ok(Json(json!({ "ok": true, "wordCount": wc })))
}

async fn get_image(State(st): State<Shared>, Path((qid, name)): Path<(String, String)>) -> Response {
    // 防目录穿越
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return (StatusCode::BAD_REQUEST, "bad name").into_response();
    }
    let path = st
        .data_dir
        .join("images")
        .join(crate::db::sanitize(&qid))
        .join(&name);
    let Ok(bytes) = std::fs::read(&path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, mime)], bytes).into_response()
}

async fn delete_image(State(st): State<Shared>, Path((qid, name)): Path<(String, String)>) -> R {
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err(ApiError(anyhow::anyhow!("非法文件名")));
    }
    let path = st
        .data_dir
        .join("images")
        .join(crate::db::sanitize(&qid))
        .join(&name);
    let _ = std::fs::remove_file(path);
    let db = st.db.lock().await;
    let _ = db.conn.execute(
        "DELETE FROM images WHERE question_id=?1 AND filename=?2",
        params![qid, name],
    );
    Ok(Json(json!({ "ok": true })))
}

// ---------------- 模型 ----------------

fn require_model_text(value: &str, label: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(ApiError(anyhow::anyhow!("{label}不能为空")));
    }
    Ok(value.to_string())
}

fn require_model_url(value: &str) -> Result<String, ApiError> {
    let value = require_model_text(value, "接口地址")?;
    if value.chars().any(char::is_whitespace)
        || !(value.starts_with("http://") || value.starts_with("https://"))
        || value.len() <= 8
    {
        return Err(ApiError(anyhow::anyhow!("接口地址必须使用 http:// 或 https://")));
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn validate_model_payload(provider: &str, base_url: &str, model: &str) -> Result<(), ApiError> {
    require_model_text(provider, "模型平台")?;
    require_model_url(base_url)?;
    require_model_text(model, "模型名")?;
    Ok(())
}

async fn get_model(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let mut m = tasks::load_model(&db);
    // 不回传完整 key，只给掩码
    let masked = if m.api_key.len() > 8 {
        format!("{}••••{}", &m.api_key[..4], &m.api_key[m.api_key.len() - 4..])
    } else if m.api_key.is_empty() {
        String::new()
    } else {
        "••••".into()
    };
    m.api_key = masked;
    Ok(Json(json!(m)))
}

async fn model_catalog(State(st): State<Shared>) -> R {
    let resp = st.agent.get("/v1/catalog").await?;
    if resp.get("ok").and_then(|x| x.as_bool()) == Some(false) {
        let message = resp
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("模型目录同步失败");
        return Err(ApiError(anyhow::anyhow!(message.to_string())));
    }
    Ok(Json(resp))
}

async fn put_model(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let provider = require_model_text(
        body.get("provider").and_then(|x| x.as_str()).unwrap_or(""),
        "模型平台",
    )?;
    let base_url = require_model_url(body.get("baseUrl").and_then(|x| x.as_str()).unwrap_or(""))?;
    let model = require_model_text(body.get("model").and_then(|x| x.as_str()).unwrap_or(""), "模型名")?;
    let key_in = body.get("apiKey").and_then(|x| x.as_str()).unwrap_or("");
    validate_model_payload(&provider, &base_url, &model)?;
    let temp = body
        .get("temperature")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.3)
        .clamp(0.0, 1.0);
    let db = st.db.lock().await;
    // 传空字符串表示「不修改 key」
    if key_in.is_empty() || key_in.contains('•') {
        let _ = db.conn.execute(
            "UPDATE model_config SET provider=?1, base_url=?2, model=?3, temperature=?4 WHERE id=1",
            params![provider, base_url, model, temp],
        );
    } else {
        let _ = db.conn.execute(
            "UPDATE model_config SET provider=?1, base_url=?2, model=?3, api_key=?4, temperature=?5 WHERE id=1",
            params![provider, base_url, model, key_in.trim(), temp],
        );
    }
    db.log_event("model", None, "更新模型设置", &format!("{provider} / {model}"));
    drop(db);
    get_model(State(st)).await
}

/// 拉取该 Key 在这个平台上真正可用的模型清单（转发给智能体）。
/// 内置清单只是快捷方式，接口才是真相。
async fn list_models(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let db = st.db.lock().await;
    let saved = tasks::load_model(&db);
    drop(db);
    let key_in = body.get("apiKey").and_then(|x| x.as_str()).unwrap_or("");
    let api_key = if key_in.is_empty() || key_in.contains('•') {
        saved.api_key
    } else {
        key_in.to_string()
    };
    let payload = json!({
        "provider": body.get("provider").and_then(|x| x.as_str()).unwrap_or(&saved.provider),
        "baseUrl": body.get("baseUrl").and_then(|x| x.as_str()).unwrap_or(&saved.base_url),
        "model": body.get("model").and_then(|x| x.as_str()).unwrap_or(&saved.model),
        "apiKey": api_key,
    });
    validate_model_payload(
        payload["provider"].as_str().unwrap_or(""),
        payload["baseUrl"].as_str().unwrap_or(""),
        payload["model"].as_str().unwrap_or(""),
    )?;
    Ok(Json(st.agent.post("/v1/models", &payload).await?))
}

async fn test_model(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let db = st.db.lock().await;
    let saved = tasks::load_model(&db);
    drop(db);
    let key_in = body.get("apiKey").and_then(|x| x.as_str()).unwrap_or("");
    let api_key = if key_in.is_empty() || key_in.contains('•') {
        saved.api_key
    } else {
        key_in.to_string()
    };
    let payload = json!({
        "provider": body.get("provider").and_then(|x| x.as_str()).unwrap_or(&saved.provider),
        "baseUrl": body.get("baseUrl").and_then(|x| x.as_str()).unwrap_or(&saved.base_url),
        "model": body.get("model").and_then(|x| x.as_str()).unwrap_or(&saved.model),
        "apiKey": api_key,
    });
    validate_model_payload(
        payload["provider"].as_str().unwrap_or(""),
        payload["baseUrl"].as_str().unwrap_or(""),
        payload["model"].as_str().unwrap_or(""),
    )?;
    let resp = st.agent.post("/v1/test", &payload).await?;
    Ok(Json(resp))
}

// ---------------- 设置 ----------------

async fn get_settings(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    Ok(Json(json!(tasks::load_settings(&db))))
}

async fn put_settings(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let db = st.db.lock().await;
    let cur = tasks::load_settings(&db);
    let g = |k: &str, d: &str| body.get(k).and_then(|x| x.as_str()).unwrap_or(d).to_string();
    let times = body
        .get("cronTimes")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let value = x.as_str()?.trim();
                    let (hour, minute) = value.split_once(':')?;
                    let hour: u8 = hour.parse().ok()?;
                    let minute: u8 = minute.parse().ok()?;
                    (hour < 24 && minute < 60).then(|| format!("{hour:02}:{minute:02}"))
                })
                .collect::<Vec<String>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or(cur.cron_times.clone());
    let pick_ratio = body.get("pickRatio").and_then(|x| x.as_f64()).unwrap_or(cur.pick_ratio).clamp(0.1, 0.9);
    let draft_min = body.get("draftMin").and_then(|x| x.as_i64()).unwrap_or(cur.draft_min).clamp(100, 5000);
    let draft_max = body.get("draftMax").and_then(|x| x.as_i64()).unwrap_or(cur.draft_max).clamp(draft_min, 5000);
    let image_min = body.get("imageMin").and_then(|x| x.as_i64()).unwrap_or(cur.image_min).clamp(1, 20);
    let image_max = body.get("imageMax").and_then(|x| x.as_i64()).unwrap_or(cur.image_max).clamp(image_min, 20);

    let _ = db.conn.execute(
        "UPDATE settings SET cron_times=?1, pick_ratio=?2, draft_min=?3, draft_max=?4,
         image_min=?5, image_max=?6, auto_sync=?7, minimize_to_tray=?8, auto_launch=?9, sign_mode=?10 WHERE id=1",
        params![
            serde_json::to_string(&times).unwrap_or_else(|_| "[]".into()),
            pick_ratio,
            draft_min,
            draft_max,
            image_min,
            image_max,
            body.get("autoSync").and_then(|x| x.as_bool()).unwrap_or(cur.auto_sync) as i64,
            body.get("minimizeToTray").and_then(|x| x.as_bool()).unwrap_or(cur.minimize_to_tray) as i64,
            body.get("autoLaunch").and_then(|x| x.as_bool()).unwrap_or(cur.auto_launch) as i64,
            if g("signMode", &cur.sign_mode).is_empty() { cur.sign_mode } else { g("signMode", &cur.sign_mode) },
        ],
    );
    Ok(Json(json!(tasks::load_settings(&db))))
}

// ---------------- job / event / stats ----------------

async fn list_jobs(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let mut stmt = db.conn.prepare(
        "SELECT id, kind, question_id, stage, progress, message, state, started_at, finished_at
         FROM jobs ORDER BY started_at DESC LIMIT 30",
    )?;
    let list: Vec<Job> = stmt
        .query_map([], |r| {
            Ok(Job {
                id: r.get(0)?,
                kind: r.get(1)?,
                question_id: r.get(2)?,
                stage: r.get(3)?,
                progress: r.get(4)?,
                message: r.get(5)?,
                state: r.get(6)?,
                started_at: r.get(7)?,
                finished_at: r.get(8)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Json(json!({ "jobs": list })))
}

async fn get_job(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let db = st.db.lock().await;
    let j = db
        .conn
        .query_row(
            "SELECT id, kind, question_id, stage, progress, message, state, started_at, finished_at FROM jobs WHERE id=?1",
            params![id],
            |r| {
                Ok(Job {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    question_id: r.get(2)?,
                    stage: r.get(3)?,
                    progress: r.get(4)?,
                    message: r.get(5)?,
                    state: r.get(6)?,
                    started_at: r.get(7)?,
                    finished_at: r.get(8)?,
                })
            },
        )
        .map_err(|_| ApiError(anyhow::anyhow!("任务不存在")))?;
    Ok(Json(json!(j)))
}

async fn list_events(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let mut stmt = db.conn.prepare(
        "SELECT id, kind, question_id, title, detail, created_at FROM events ORDER BY id DESC LIMIT 60",
    )?;
    let list: Vec<Event> = stmt
        .query_map([], |r| {
            Ok(Event {
                id: r.get(0)?,
                kind: r.get(1)?,
                question_id: r.get(2)?,
                title: r.get(3)?,
                detail: r.get(4)?,
                created_at: r.get(5)?,
            })
        })?
        .filter_map(|x| x.ok())
        .collect();
    Ok(Json(json!({ "events": list })))
}

async fn stats_overview(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let count = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    let total = count("SELECT COUNT(*) FROM questions");
    let inbox = count("SELECT COUNT(*) FROM questions WHERE status='inbox'");
    let pending = count("SELECT COUNT(*) FROM questions WHERE status='pending_review'");
    let answered = count("SELECT COUNT(*) FROM questions WHERE status='answered'");
    let dismissed = count("SELECT COUNT(*) FROM questions WHERE status='dismissed'");
    let drafts = count("SELECT COUNT(*) FROM drafts");
    let images = count("SELECT COUNT(*) FROM images");
    let bytes = count("SELECT COALESCE(SUM(bytes),0) FROM images");
    let avg_fit: f64 = db
        .conn
        .query_row("SELECT COALESCE(AVG(fit),0) FROM questions", [], |r| r.get(0))
        .unwrap_or(0.0);
    let answers_30d = count("SELECT COUNT(*) FROM questions WHERE status='answered' AND synced_at > date('now','-30 day')");
    let v: i64 = db
        .conn
        .query_row("SELECT COALESCE(SUM(voteups),0) FROM stats_snapshots WHERE date > date('now','-30 day')", [], |r| r.get(0))
        .unwrap_or(0);
    let c: i64 = db
        .conn
        .query_row("SELECT COALESCE(SUM(comments),0) FROM stats_snapshots WHERE date > date('now','-30 day')", [], |r| r.get(0))
        .unwrap_or(0);
    let hit = if total > 0 {
        answered as f64 / total as f64
    } else {
        0.0
    };

    Ok(Json(json!(StatsOverview {
        total_questions: total,
        inbox,
        pending_review: pending,
        answered,
        dismissed,
        drafts,
        images,
        images_bytes: bytes,
        answers_30d,
        voteups_30d: v,
        new_comments_30d: c,
        avg_fit,
        hit_rate: hit,
        saved_minutes: drafts * 25,
    })))
}

async fn stats_series(State(st): State<Shared>, Query(qp): Query<HashMap<String, String>>) -> R {
    let days: i64 = qp.get("days").and_then(|s| s.parse().ok()).unwrap_or(14);
    let db = st.db.lock().await;
    let mut out: Vec<StatsPoint> = Vec::new();
    for i in (0..days).rev() {
        let date = (chrono::Local::now() - chrono::Duration::days(i))
            .format("%Y-%m-%d")
            .to_string();
        let collected: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM questions WHERE substr(synced_at,1,10)=?1",
                params![date],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let answered: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind='answered' AND substr(created_at,1,10)=?1",
                params![date],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let (v, c): (i64, i64) = db
            .conn
            .query_row(
                "SELECT COALESCE(voteups,0), COALESCE(comments,0) FROM stats_snapshots WHERE date=?1",
                params![date],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap_or((0, 0));
        out.push(StatsPoint {
            date,
            collected,
            answered,
            voteups: v,
            comments: c,
        });
    }
    Ok(Json(json!({ "series": out })))
}
