//! 本地 HTTP API。前端（Next.js）与桌面壳都通过 127.0.0.1:8787/api 访问。

use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

use crate::models::*;
use crate::state::{Shared, ZhihuOpenSettings};
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

pub(crate) type R = Result<Json<Value>, ApiError>;

pub fn router(state: Shared) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/session", get(get_session))
        .route("/session/onboarding/finish", post(finish_onboarding))
        .route("/open-url", post(open_external))
        .route("/session/oauth/start", post(crate::oauth::start))
        .route("/session/oauth/poll", post(crate::oauth::poll))
        .route("/session/oauth/cancel", post(crate::oauth::cancel))
        .route("/session/logout", post(crate::oauth::logout))
        .route("/session/refresh", post(crate::oauth::refresh))
        .route("/profile", get(get_profile).put(put_profile))
        .route("/profile/analyze", post(analyze_profile))
        .route("/sources", get(get_sources).put(put_sources))
        .route("/sources/discover", post(discover_sources))
        .route("/questions", get(list_questions))
        .route("/questions/metrics/sync", post(crate::creator::sync_question_metrics))
        .route("/answers", get(crate::creator::list_answers))
        .route("/answers/sync", post(crate::creator::sync_answers))
        .route("/answers/{id}", get(crate::creator::get_answer))
        .route("/questions/import", post(import_question))
        .route("/questions/discover", post(discover_questions))
        .route("/questions/sync", post(sync_now))
        .route("/questions/sync/recent", post(sync_recent_now))
        .route("/questions/{id}", get(get_question))
        .route(
            "/questions/{id}/research",
            get(research_status).post(research_question),
        )
        .route("/questions/{id}/status", post(set_status))
        .route("/questions/{id}/answered", post(mark_answered))
        .route("/questions/{id}/dismiss", post(dismiss_question))
        .route("/questions/{id}/restore", post(restore_question))
        .route("/drafts/{qid}", get(get_draft).put(put_draft))
        .route("/drafts/{qid}/direction", axum::routing::put(put_direction))
        .route("/drafts/{qid}/reset", post(reset_draft))
        .route("/images/{qid}", get(list_images).post(search_images))
        .route(
            "/images/{qid}/import",
            post(import_image).layer(DefaultBodyLimit::max(8_010_000)),
        )
        .route("/images/{qid}/{name}", get(get_image).delete(delete_image))
        .route("/model", get(get_model).put(put_model))
        .route("/model/catalog", get(model_catalog))
        .route("/model/test", post(test_model))
        .route("/model/list", post(list_models))
        .route("/settings", get(get_settings).put(put_settings))
        .route(
            "/settings/zhihu-open",
            get(get_zhihu_open).put(put_zhihu_open).delete(delete_zhihu_open),
        )
        .route("/settings/zhihu-open/test", post(test_zhihu_open))
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
        db: if db_ok {
            "sqlite ok".into()
        } else {
            "sqlite error".into()
        },
        data_dir: st.data_dir.to_string_lossy().to_string(),
        uptime_seconds: (chrono::Utc::now() - st.started_at).num_seconds().max(0),
    })))
}

// ---------------- 会话 ----------------

async fn get_session(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let row = db.conn.query_row(
        "SELECT zhihu_id, name, url_token, avatar_url, headline, answer_count, articles_count,
         follower_count, oauth_expires_at, updated_at, cookie<>'' FROM active_account",
        [],
        |r| Ok((ZhihuUser {
            id: r.get(0)?,
            name: r.get(1)?,
            url_token: r.get(2)?,
            avatar_url: r.get(3)?,
            headline: r.get(4)?,
            answer_count: r.get(5)?,
            articles_count: r.get(6)?,
            follower_count: r.get(7)?,
        }, r.get::<_, i64>(8)?, r.get::<_, String>(9)?, r.get::<_, bool>(10)?)),
    ).optional()?;
    let logged_in = row.is_some();
    let (user, expires_at, last_sync_at, can_sync_notifications) = match row {
        Some((user, expires, updated, can_sync)) => (
            Some(user),
            chrono::DateTime::from_timestamp(expires, 0).map(|time| time.to_rfc3339()),
            Some(updated),
            can_sync,
        ),
        None => (None, None, None, false),
    };

    let can_sync_answers = can_sync_notifications || user.as_ref()
        .is_some_and(|account| ZhihuOpenSettings::load(&db).is_ok_and(|settings| settings.profile_enabled(&account.id)));
    Ok(Json(json!(Session {
        logged_in,
        onboarding_required: logged_in && tasks::onboarding_required(&db.conn)?,
        user,
        expires_at,
        last_sync_at,
        can_sync_notifications,
        can_sync_answers,
    })))
}

async fn finish_onboarding(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    db.conn.execute(
        "INSERT OR IGNORE INTO onboarding_finished SELECT zhihu_id FROM active_account", [],
    )?;
    Ok(Json(json!({"ok": true})))
}

/// 用系统默认浏览器打开链接。
/// 桌面壳是 Tauri 的 WebView2，页面里的 `<a target="_blank">` 与 window.open()
/// 都不会真的唤起浏览器，所以必须交给内核用系统命令打开。
pub(crate) fn open_in_default_browser(url: &str) -> anyhow::Result<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        anyhow::bail!("只允许打开 http/https 链接");
    }
    if url.len() > 2048 || url.chars().any(|c| c.is_control()) {
        anyhow::bail!("链接格式不合法");
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // 优先走 url.dll，它按协议交给默认浏览器，不经过 cmd 解析，& 之类字符不会被截断
        let first = std::process::Command::new("rundll32.exe")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
        if first.is_ok() {
            return Ok(());
        }
        std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| anyhow::anyhow!("无法唤起默认浏览器：{e}"))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| anyhow::anyhow!("无法唤起默认浏览器：{e}"))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(url)
            .spawn()
            .map_err(|e| anyhow::anyhow!("无法唤起默认浏览器：{e}"))?;
    }
    Ok(())
}

/// 前端所有外链都走这里：交给系统默认浏览器，而不是在 WebView 里开新窗口。
async fn open_external(Json(body): Json<Value>) -> R {
    let url = body
        .get("url")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if url.is_empty() {
        return Err(ApiError(anyhow::anyhow!("缺少 url 参数")));
    }
    open_in_default_browser(&url)?;
    Ok(Json(json!({ "ok": true, "url": url })))
}

// ---------------- 画像 ----------------

async fn get_profile(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    let (status, summary, evidence, sample, updated, style_json): (
        String,
        String,
        String,
        i64,
        Option<String>,
        String,
    ) = db
        .conn
        .query_row(
            "SELECT status, summary, evidence, sample_size, updated_at, style FROM profile_meta WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap_or((
            "empty".into(),
            "".into(),
            "[]".into(),
            0,
            None,
            "{}".into(),
        ));

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
        style: serde_json::from_str::<ProfileStyle>(&style_json).unwrap_or_default(),
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
            return Ok(Json(
                json!({ "ok": true, "message": "画像分析已经在进行中", "jobId": id }),
            ));
        }
    }
    let job_id = tasks::job_start(&st, "profile", None, "读取历史回答").await;
    {
        let db = st.db.lock().await;
        let _ = db
            .conn
            .execute("UPDATE profile_meta SET status='running' WHERE id=1", []);
    }
    let st2 = st.clone();
    let job_id2 = job_id.clone();
    tokio::spawn(async move {
        if let Err(e) = tasks::run_profile_job(&st2, job_id2).await {
            tracing::warn!("画像分析失败: {e}");
        }
    });
    Ok(Json(
        json!({ "ok": true, "message": "已开始分析历史内容", "jobId": job_id }),
    ))
}

async fn put_profile(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let terms = body
        .get("terms")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let summary = body.get("summary").and_then(|x| x.as_str()).unwrap_or("");
    let style = body.get("style").cloned().unwrap_or(Value::Null);
    {
        let mut db = st.db.lock().await;
        let tx = db.conn.transaction()?;
        tx.execute("DELETE FROM profile_terms", [])?;
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
            tx.execute(
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
            )?;
        }
        if style.is_object() {
            tx.execute(
                "UPDATE profile_meta SET summary=?1, style=?2, updated_at=?3 WHERE id=1",
                params![summary, style.to_string(), now_iso()],
            )?;
        } else {
            tx.execute(
                "UPDATE profile_meta SET summary=?1, updated_at=?2 WHERE id=1",
                params![summary, now_iso()],
            )?;
        }
        tasks::refresh_inbox_fit(&tx)?;
        tx.commit()?;
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
    Ok(Json(
        json!({ "ok": true, "message": "正在按领域检索权威平台" }),
    ))
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
        fit_evaluated: r.get(23)?,
        status: r.get(10)?,
        bucket: r.get(11)?,
        answer_count: r.get(12)?,
        follower_count: r.get(13)?,
        metrics: r.get::<_, Option<String>>(24)?.and_then(|text| serde_json::from_str(&text).ok()),
        metrics_error: r.get(25)?,
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
    (SELECT COUNT(*) FROM drafts d WHERE d.question_id = q.id AND trim(d.markdown)<>'') AS has_draft,
    (SELECT COUNT(*) FROM images i WHERE i.question_id = q.id) AS image_count,
    COALESCE((SELECT d.word_count FROM drafts d WHERE d.question_id = q.id), 0) AS word_count,
    COALESCE((SELECT json_array_length(d.doubts) FROM drafts d WHERE d.question_id = q.id), 0) AS doubt_count,
    q.fit_evaluated, q.metrics, q.metrics_error
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
        where_parts.push(format!(
            "(q.title LIKE ?{0} OR q.excerpt LIKE ?{0})",
            args.len() + 1
        ));
        args.push(format!("%{}%", k));
    }
    // 默认不展示已回答/已弃用
    if qp.get("status").is_none() {
        where_parts.push("q.status IN ('inbox','researching','pending_review')".into());
    }
    where_parts.push(format!("(q.status NOT IN ('inbox','researching','pending_review') OR NOT {})", tasks::IS_ANSWERED));

    let sql = format!(
        "{Q_SELECT}{} ORDER BY (q.source='imported') DESC, CASE WHEN q.source='imported' THEN q.synced_at END DESC, CASE q.status WHEN 'pending_review' THEN 0 WHEN 'researching' THEN 1 ELSE 2 END, q.fit DESC, q.synced_at DESC LIMIT 500",
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
            .query_map([], |r| {
                Ok(json!({ "domain": r.get::<_, String>(0)?, "count": r.get::<_, i64>(1)? }))
            })?
            .filter_map(|x| x.ok())
            .collect::<Vec<Value>>();
        rows
    };

    let metrics_job = crate::creator::running_metrics_job(&db.conn)?;
    Ok(Json(json!({ "questions": list, "domains": domains, "metricsJobId": metrics_job })))
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
    crate::tasks::account_client(&st).await?.2.require_web_session()?;
    let job_id = tasks::start_sync(&st, None, false).await;
    Ok(Json(
        json!({ "ok": true, "jobId": job_id, "message": "开始同步知乎通知" }),
    ))
}

async fn sync_recent_now(State(st): State<Shared>, body: Option<Json<Value>>) -> R {
    crate::tasks::account_client(&st).await?.2.require_web_session()?;
    let select_only = body.as_ref().and_then(|body| body.get("selectOnly")).and_then(Value::as_bool).unwrap_or(false);
    let job_id = tasks::start_sync(&st, Some(3), select_only).await;
    Ok(Json(
        json!({ "ok": true, "jobId": job_id, "message": "开始同步最近 3 天的邀请与推荐" }),
    ))
}

async fn import_question(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let url = body.get("url").and_then(Value::as_str).unwrap_or_default();
    let results = tasks::import_questions(&st, url).await?;
    Ok(Json(json!({"ok": true, "results": results})))
}

async fn discover_questions(State(st): State<Shared>) -> R {
    let job = tasks::start_question_discovery(&st).await?;
    Ok(Json(json!({"ok": true, "jobId": job})))
}

async fn research_question(State(st): State<Shared>, Path(id): Path<String>, body: Option<Json<Value>>) -> R {
    let direction = body.as_ref().and_then(|body| body.get("direction")).and_then(Value::as_str);
    let job = tasks::start_research(&st, &id, direction, false).await?;
    Ok(Json(json!({ "ok": true, "jobId": job })))
}

async fn research_status(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let db = st.db.lock().await;
    let entry = db
        .conn
        .query_row(
            "SELECT id, stage, progress, message, state, started_at, finished_at, verify_started_ms, verify_seconds
         FROM jobs WHERE kind='research' AND question_id=?1 ORDER BY rowid DESC LIMIT 1",
            params![id],
            |row| {
                let job = Job {
                    id: row.get(0)?,
                    kind: "research".into(),
                    question_id: Some(id.clone()),
                    stage: row.get(1)?,
                    progress: row.get(2)?,
                    message: row.get(3)?,
                    state: row.get(4)?,
                    started_at: row.get(5)?,
                    finished_at: row.get(6)?,
                };
                let timing = row.get::<_, Option<i64>>(7)?.zip(row.get::<_, Option<i64>>(8)?);
                Ok((job, timing))
            },
        )
        .optional()?;
    let checkpoint = db
        .conn
        .query_row(
            "SELECT drafted FROM research_checkpoints WHERE question_id=?1",
            params![id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?;
    let verification = entry.as_ref().filter(|(job, _)| job.state == "running")
        .and_then(|(_, timing)| *timing)
        .map(|(started_ms, seconds)| json!({"startedMs": started_ms, "limitSeconds": seconds}));
    Ok(Json(json!({ "job": entry.map(|(job, _)| job), "verification": verification,
        "resumable": checkpoint.is_some(), "draftSaved": checkpoint == Some(true) })))
}

async fn set_status(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> R {
    let status = body
        .get("status")
        .and_then(|x| x.as_str())
        .unwrap_or("inbox");
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
        ensure_not_researching(&db, &id)?;
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
        db.conn.execute(
            "INSERT OR IGNORE INTO answered_questions SELECT a.zhihu_id, q.question_id
             FROM active_account a JOIN questions q ON q.id=?1
             WHERE q.question_id<>''", params![id],
        )?;
        db.conn.execute(
            "UPDATE questions SET status='answered', bucket='archived' WHERE id=?1",
            params![id],
        )?;
        db.log_event(
            "answered",
            Some(&id),
            "标记为已回答",
            &format!("释放 {} KB", freed / 1024),
        );
        freed
    };
    Ok(Json(json!({ "ok": true, "freedBytes": freed })))
}

async fn dismiss_question(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let freed = {
        let mut db = st.db.lock().await;
        ensure_not_researching(&db, &id)?;
        let freed = db.purge_images(&id).unwrap_or(0);
        let tx = db.conn.transaction()?;
        tx.execute("DELETE FROM drafts WHERE question_id=?1", params![id])?;
        tx.execute("DELETE FROM author_directions WHERE question_id=?1", params![id])?;
        tx.execute("DELETE FROM research_checkpoints WHERE question_id=?1", params![id])?;
        tx.execute(
            "UPDATE questions SET status='dismissed', bucket='archived' WHERE id=?1",
            params![id],
        )?;
        tx.commit()?;
        db.log_event("dismiss", Some(&id), "弃用该问题", "");
        freed
    };
    Ok(Json(json!({ "ok": true, "freedBytes": freed })))
}

async fn restore_question(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let db = st.db.lock().await;
    let changed = db.conn.execute(
        "UPDATE questions SET status='inbox', bucket='queue' WHERE id=?1 AND status='dismissed'",
        params![id],
    )?;
    if changed == 0 {
        return Err(anyhow::anyhow!("只有已弃用的问题可以恢复。").into());
    }
    db.log_event("restore", Some(&id), "恢复问题到待解答", "");
    Ok(Json(json!({ "ok": true })))
}

fn ensure_not_researching(db: &crate::db::Db, id: &str) -> anyhow::Result<()> {
    let status: String = db.conn.query_row(
        "SELECT status FROM questions WHERE id=?1",
        params![id],
        |row| row.get(0),
    )?;
    anyhow::ensure!(
        status != "researching",
        "任务仍在进行，完成后再归档或弃用。"
    );
    db.conn.execute(
        "DELETE FROM research_checkpoints WHERE question_id=?1",
        params![id],
    )?;
    Ok(())
}

// ---------------- 草稿与图片 ----------------

async fn get_draft(State(st): State<Shared>, Path(qid): Path<String>) -> R {
    let db = st.db.lock().await;
    let row = db
        .conn
        .query_row(
            "SELECT markdown, doubts, citations, model, word_count, updated_at,
                    COALESCE(warnings, '[]')
             FROM drafts WHERE question_id=?1",
            params![qid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            },
        )
        .ok();

    let images = load_images(&db.conn, &qid)?;

    let direction = db.conn.query_row("SELECT text FROM author_directions WHERE question_id=?1",
        params![qid], |row| row.get::<_, String>(0)).optional()?.unwrap_or_default();
    let mut draft = match row {
        Some((md, doubts, cites, model, wc, upd, warns)) => json!(Draft {
            question_id: qid,
            markdown: md,
            doubts: serde_json::from_str(&doubts).unwrap_or_default(),
            citations: serde_json::from_str(&cites).unwrap_or_default(),
            images,
            warnings: serde_json::from_str(&warns).unwrap_or_default(),
            word_count: wc,
            updated_at: upd,
            model,
        }),
        None => json!({
            "questionId": qid, "markdown": "", "doubts": [], "citations": [],
            "images": images, "warnings": [], "wordCount": 0, "updatedAt": "", "model": ""
        }),
    };
    draft["direction"] = json!(direction);
    Ok(Json(draft))
}

fn load_images(conn: &rusqlite::Connection, qid: &str) -> rusqlite::Result<Vec<ImageAsset>> {
    let mut stmt = conn
        .prepare("SELECT id, question_id, filename, source_url, source_name, caption, bytes, created_at FROM images WHERE question_id=?1 ORDER BY created_at")?;
    let rows = stmt.query_map(params![qid], |r| {
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
    })?;
    rows.collect()
}

async fn list_images(State(st): State<Shared>, Path(qid): Path<String>) -> R {
    let db = st.db.lock().await;
    Ok(Json(json!(load_images(&db.conn, &qid)?)))
}

async fn illustration_question(st: &Shared, qid: &str) -> Result<Value, ApiError> {
    let db = st.db.lock().await;
    let question = db
        .conn
        .query_row(
            "SELECT title, excerpt, tags FROM questions WHERE id=?1 AND status IN ('inbox','pending_review')",
            params![qid],
            |row| {
                let tags = serde_json::from_str::<Vec<String>>(&row.get::<_, String>(2)?).unwrap_or_default();
                Ok(json!({"title": row.get::<_, String>(0)?, "excerpt": row.get::<_, String>(1)?, "tags": tags}))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow::anyhow!("当前问题不能补图，请等待拟稿完成或检查归档状态。"))?;
    Ok(question)
}

async fn search_images(
    State(st): State<Shared>,
    Path(qid): Path<String>,
    Json(body): Json<Value>,
) -> R {
    let question = illustration_question(&st, &qid).await?;
    let source_only = body["fromSources"].as_bool() == Some(true);
    let (maximum, sources, model, pages) = {
        let db = st.db.lock().await;
        let mut pages = Vec::new();
        if source_only {
            if let Some((citations, doubts)) = db.conn.query_row(
                "SELECT citations, doubts FROM drafts WHERE question_id=?1", params![qid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            ).optional()? {
                let citations: Vec<Citation> = serde_json::from_str(&citations).unwrap_or_default();
                let doubts: Vec<Doubt> = serde_json::from_str(&doubts).unwrap_or_default();
                let links = citations.into_iter().map(|c| (c.url, c.source_name))
                    .chain(doubts.into_iter().flat_map(|d| d.evidence.into_iter().map(|e| (e.url, e.source_name))));
                let mut seen = std::collections::HashSet::new();
                for (url, name) in links {
                    if url.len() <= 2048 && (url.starts_with("https://") || url.starts_with("http://")) && seen.insert(url.clone()) {
                        pages.push(json!({"url": url, "sourceName": name}));
                    }
                    if pages.len() >= 8 { break; }
                }
            }
        }
        (
            tasks::load_settings(&db).image_max,
            tasks::research_sources(&db)?,
            tasks::load_model(&db),
            pages,
        )
    };
    let raw_query = body["query"].as_str().unwrap_or_default().trim();
    let query: String = raw_query.chars().take(110).collect();
    let public_page = if raw_query.starts_with("https://") && raw_query.len() <= 2048 {
        raw_query
    } else {
        ""
    };
    let payload = json!({"question": question, "supplement": true, "publicPage": public_page,
        "sources": sources, "model": model, "sourceOnly": source_only, "sourcePages": pages,
        "settings": {"imageMax": if maximum > 0 { maximum.min(8) } else { 4 }},
        "imageQueries": if query.is_empty() { vec![] } else { vec![query] }});
    let (added, _) = tasks::collect_question_images(&st, &qid, payload, false, true).await?;
    Ok(Json(json!({"added": added})))
}

async fn import_image(
    State(st): State<Shared>,
    Path(qid): Path<String>,
    Json(body): Json<Value>,
) -> R {
    illustration_question(&st, &qid).await?;
    let image = body["imageData"]
        .as_str()
        .filter(|image| !image.is_empty() && image.len() <= 8_000_000)
        .ok_or_else(|| anyhow::anyhow!("请选择不超过 6 MB 的图片文件。"))?;
    let url = body["sourceUrl"].as_str().unwrap_or_default().trim();
    if !url.is_empty()
        && (url.len() > 2048
            || !reqwest::Url::parse(url).is_ok_and(|link| {
                matches!(link.scheme(), "http" | "https")
                    && link.host().is_some()
                    && link.username().is_empty()
                    && link.password().is_none()
            }))
    {
        return Err(anyhow::anyhow!("请填写有效的 http 或 https 来源链接，或留空。").into());
    }
    let caption: String = body["caption"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .chars()
        .take(160)
        .collect();
    let payload = json!({"imageData": image, "sourceUrl": url, "caption": caption});
    let (added, _) = tasks::collect_question_images(&st, &qid, payload, true, true).await?;
    Ok(Json(json!({"added": added})))
}

async fn put_draft(
    State(st): State<Shared>,
    Path(qid): Path<String>,
    Json(body): Json<Value>,
) -> R {
    let md = body
        .get("markdown")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("缺少正文"))?;
    let wc = md.chars().filter(|c| !c.is_whitespace()).count() as i64;
    let db = st.db.lock().await;
    let editable = db.conn.query_row(
        "SELECT status IN ('inbox','pending_review') OR
         (status='researching' AND EXISTS(SELECT 1 FROM drafts WHERE question_id=?1)) FROM questions WHERE id=?1",
        params![qid], |row| row.get::<_, bool>(0),
    ).optional()?.unwrap_or(false);
    if !editable {
        return Err(anyhow::anyhow!("当前问题不能编辑正文，请等待初稿保存或检查归档状态。").into());
    }
    db.conn.execute(
        "INSERT INTO drafts (question_id, markdown, doubts, citations, model, word_count, updated_at)
         VALUES (?1,?2,'[]','[]','manual',?3,?4)
         ON CONFLICT(question_id) DO UPDATE SET markdown=?2, word_count=?3, updated_at=?4",
        params![qid, md, wc, now_iso()],
    )?;
    Ok(Json(json!({ "ok": true, "wordCount": wc })))
}

async fn put_direction(State(st): State<Shared>, Path(qid): Path<String>, Json(body): Json<Value>) -> R {
    let direction = body.get("direction").and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("缺少答题思路"))?;
    let mut db = st.db.lock().await;
    let tx = db.conn.transaction()?;
    let editable: bool = tx.query_row(
        "SELECT status='inbox' AND NOT EXISTS(SELECT 1 FROM drafts WHERE question_id=?1 AND trim(markdown)<>'')
         FROM questions WHERE id=?1", params![qid], |row| row.get(0),
    ).optional()?.unwrap_or(false);
    if !editable {
        return Err(anyhow::anyhow!("当前不能修改拟稿前的答题思路").into());
    }
    tasks::save_direction(&tx, &qid, direction)?;
    tx.commit()?;
    Ok(Json(json!({"ok": true})))
}

async fn reset_draft(State(st): State<Shared>, Path(qid): Path<String>) -> R {
    let mut db = st.db.lock().await;
    let tx = db.conn.transaction()?;
    let changed = tx.execute(
        "UPDATE questions SET status='inbox' WHERE id=?1 AND status='pending_review'
         AND NOT EXISTS (SELECT 1 FROM jobs WHERE question_id=?1 AND kind='research' AND state='running')",
        params![qid],
    )?;
    if changed == 0 {
        return Err(anyhow::anyhow!("只有已结束拟稿、等待批阅的问题可以清空稿件。").into());
    }
    tx.execute("DELETE FROM drafts WHERE question_id=?1", params![qid])?;
    tx.execute("DELETE FROM author_directions WHERE question_id=?1", params![qid])?;
    tx.execute(
        "DELETE FROM research_checkpoints WHERE question_id=?1",
        params![qid],
    )?;
    tx.commit()?;
    db.log_event("draft_reset", Some(&qid), "清空稿件，等待重新拟稿", "");
    Ok(Json(json!({ "ok": true })))
}

async fn get_image(
    State(st): State<Shared>,
    Path((qid, name)): Path<(String, String)>,
) -> Response {
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
        "svg" => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported image").into_response(),
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
        return Err(ApiError(anyhow::anyhow!(
            "接口地址必须使用 http:// 或 https://"
        )));
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
        format!(
            "{}••••{}",
            &m.api_key[..4],
            &m.api_key[m.api_key.len() - 4..]
        )
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
    let model = require_model_text(
        body.get("model").and_then(|x| x.as_str()).unwrap_or(""),
        "模型名",
    )?;
    let key_in = body.get("apiKey").and_then(|x| x.as_str()).unwrap_or("");
    validate_model_payload(&provider, &base_url, &model)?;
    let temp = body
        .get("temperature")
        .and_then(|x| x.as_f64())
        .unwrap_or(0.3)
        .clamp(0.0, 1.0);
    let ctx_in = body.get("contextLimit").and_then(|x| x.as_i64());
    let out_in = body.get("outputLimit").and_then(|x| x.as_i64());
    let db = st.db.lock().await;
    let (cur_ctx, cur_out): (i64, i64) = db
        .conn
        .query_row(
            "SELECT context_limit, output_limit FROM model_config WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((0, 0));
    // 0 表示「未知」，允许保留；有值时必须落在合理区间，避免手滑写错把请求撑爆
    let context_limit = ctx_in.unwrap_or(cur_ctx).clamp(0, 20_000_000);
    let output_limit = out_in.unwrap_or(cur_out).clamp(0, 2_000_000);
    // 传空字符串表示「不修改 key」
    if key_in.is_empty() || key_in.contains('•') {
        let _ = db.conn.execute(
            "UPDATE model_config SET provider=?1, base_url=?2, model=?3, temperature=?4,
                    context_limit=?5, output_limit=?6 WHERE id=1",
            params![provider, base_url, model, temp, context_limit, output_limit],
        );
    } else {
        let _ = db.conn.execute(
            "UPDATE model_config SET provider=?1, base_url=?2, model=?3, api_key=?4, temperature=?5,
                    context_limit=?6, output_limit=?7 WHERE id=1",
            params![provider, base_url, model, key_in.trim(), temp, context_limit, output_limit],
        );
    }
    db.log_event(
        "model",
        None,
        "更新模型设置",
        &format!("{provider} / {model}"),
    );
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

fn zhihu_open_status(db: &crate::db::Db, settings: &ZhihuOpenSettings) -> R {
    let account_id = zhihu_open_account(db)?;
    Ok(Json(json!({
        "configured": !settings.secret.is_empty(),
        "enabled": settings.enabled,
        "profileEnabled": settings.profile_enabled(&account_id),
    })))
}

fn zhihu_open_account(db: &crate::db::Db) -> rusqlite::Result<String> {
    Ok(db.conn.query_row(
        "SELECT zhihu_id FROM active_account",
        [], |row| row.get(0),
    ).optional()?.unwrap_or_default())
}

fn validate_zhihu_open_secret(secret: &str) -> anyhow::Result<&str> {
    let secret = secret.trim();
    anyhow::ensure!(
        !secret.is_empty() && secret.len() <= 4096 && secret.bytes().all(|byte| byte.is_ascii_graphic()),
        "请输入原始 Access Secret，不要加 Bearer 或空格"
    );
    Ok(secret)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZhihuOpenUpdate {
    enabled: bool,
    access_secret: Option<String>,
    profile_account_id: Option<String>,
}

async fn get_zhihu_open(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    zhihu_open_status(&db, &ZhihuOpenSettings::load(&db)?)
}

async fn put_zhihu_open(State(st): State<Shared>, Json(body): Json<ZhihuOpenUpdate>) -> R {
    let db = st.db.lock().await;
    let mut settings = ZhihuOpenSettings::load(&db)?;
    if let Some(secret) = body.access_secret.as_deref() {
        settings.secret = validate_zhihu_open_secret(secret)?.to_owned();
        settings.account_id = body.profile_account_id.unwrap_or_default();
        if !settings.account_id.is_empty() && settings.account_id != zhihu_open_account(&db)? {
            return Err(anyhow::anyhow!("知乎账号已切换，请确认当前账号后重新保存").into());
        }
    }
    if body.enabled && settings.secret.is_empty() {
        return Err(anyhow::anyhow!("请先填写 Access Secret").into());
    }
    settings.enabled = body.enabled;
    db.conn.execute(
        "INSERT OR REPLACE INTO zhihu_open_settings (id, access_secret, enabled, account_id) VALUES (1,?1,?2,?3)",
        params![settings.secret, settings.enabled, settings.account_id],
    )?;
    zhihu_open_status(&db, &settings)
}

async fn delete_zhihu_open(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    db.conn.execute("DELETE FROM zhihu_open_settings WHERE id=1", [])?;
    zhihu_open_status(&db, &ZhihuOpenSettings::default())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZhihuOpenTest {
    access_secret: Option<String>,
}

async fn test_zhihu_open(State(st): State<Shared>, Json(body): Json<ZhihuOpenTest>) -> R {
    let secret = if let Some(secret) = body.access_secret {
        secret
    } else {
        ZhihuOpenSettings::load(&*st.db.lock().await)?.secret
    };
    let secret = validate_zhihu_open_secret(&secret)?;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        st.agent.post("/v1/zhihu-open/test", &json!({ "zhihuAccessSecret": secret })),
    ).await.map_err(|_| anyhow::anyhow!("知乎接口测试超时，请稍后重试"))??;
    Ok(Json(result))
}

async fn get_settings(State(st): State<Shared>) -> R {
    let db = st.db.lock().await;
    Ok(Json(json!(tasks::load_settings(&db))))
}

async fn put_settings(State(st): State<Shared>, Json(body): Json<Value>) -> R {
    let db = st.db.lock().await;
    let cur = tasks::load_settings(&db);
    let g = |k: &str, d: &str| {
        body.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or(d)
            .to_string()
    };
    let mut times = body
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
    times.sort();
    times.dedup();
    let pick_ratio = body
        .get("pickRatio")
        .and_then(|x| x.as_f64())
        .unwrap_or(cur.pick_ratio)
        .clamp(0.1, 0.9);
    let draft_min = body
        .get("draftMin")
        .and_then(|x| x.as_i64())
        .unwrap_or(cur.draft_min)
        .clamp(100, 5000);
    let draft_max = body
        .get("draftMax")
        .and_then(|x| x.as_i64())
        .unwrap_or(cur.draft_max)
        .clamp(draft_min, 5000);
    let image_min = body
        .get("imageMin")
        .and_then(|x| x.as_i64())
        .unwrap_or(cur.image_min)
        .clamp(1, 20);
    let image_max = body
        .get("imageMax")
        .and_then(|x| x.as_i64())
        .unwrap_or(cur.image_max)
        .clamp(image_min, 20);

    let writing_mode = g("writingMode", &cur.writing_mode);
    if !matches!(writing_mode.as_str(), "automatic" | "subjective") {
        return Err(anyhow::anyhow!("不支持的拟稿模式").into());
    }
    db.conn.execute(
        "UPDATE settings SET cron_times=?1, pick_ratio=?2, draft_min=?3, draft_max=?4,
         image_min=?5, image_max=?6, auto_sync=?7, minimize_to_tray=?8, auto_launch=?9, sign_mode=?10,
         verify_timeout_seconds=?11, writing_mode=?12 WHERE id=1",
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
            body.get("verifyTimeoutSeconds").and_then(Value::as_i64).unwrap_or(cur.verify_timeout_seconds).clamp(30, 180),
            writing_mode,
        ],
    )?;
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
    let inbox = count(&format!("SELECT COUNT(*) FROM questions q WHERE status='inbox' AND NOT {}", tasks::IS_ANSWERED));
    let pending = count(&format!("SELECT COUNT(*) FROM questions q WHERE status='pending_review' AND NOT {}", tasks::IS_ANSWERED));
    let answered = count("SELECT COUNT(*) FROM questions WHERE status='answered'");
    let dismissed = count("SELECT COUNT(*) FROM questions WHERE status='dismissed'");
    let drafts = count("SELECT COUNT(*) FROM drafts");
    let images = count("SELECT COUNT(*) FROM images");
    let bytes = count("SELECT COALESCE(SUM(bytes),0) FROM images");
    let avg_fit: f64 = db
        .conn
        .query_row("SELECT COALESCE(AVG(fit),0) FROM questions", [], |r| {
            r.get(0)
        })
        .unwrap_or(0.0);
    let answers_30d = count("SELECT COUNT(*) FROM questions WHERE status='answered' AND synced_at > date('now','-30 day')");
    let activity: Option<(i64, String)> = db.conn.query_row(
        "SELECT stats.votes, stats.end_date FROM creator_account_activity stats
         JOIN active_account a ON a.zhihu_id=stats.account_id",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional()?;
    let answer_votes: Option<i64> = db.conn.query_row(
        "SELECT SUM(json_extract(c.metrics,'$.votes')) FROM creator_answers c
         JOIN active_account a ON a.zhihu_id=c.account_id",
        [], |row| row.get(0),
    )?;
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
        voteups_30d: activity.as_ref().map(|(votes, _)| *votes),
        voteups_through: activity.map(|(_, end)| end),
        synced_answer_voteups: answer_votes,
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
