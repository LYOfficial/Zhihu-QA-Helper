//! 后台任务：同步通知、生成初稿、分析画像、发现权威平台。
//! 所有任务都会写一行 job 记录，前端据此显示进度。

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::models::*;
use crate::state::{zhihu_client, AppState};

// ---------------- job 基础设施 ----------------

pub async fn job_start(state: &AppState, kind: &str, qid: Option<&str>, msg: &str) -> String {
    let id = Uuid::new_v4().to_string();
    let db = state.db.lock().await;
    let _ = db.conn.execute(
        "INSERT INTO jobs (id, kind, question_id, stage, progress, message, state, started_at)
         VALUES (?1,?2,?3,'准备',0.02,?4,'running',?5)",
        params![id, kind, qid, msg, now_iso()],
    );
    tracing::info!(target: "jobs", job_id = %id, kind, "任务开始：{msg}");
    db.log_event("job", qid, &format!("{kind} 任务开始"), msg);
    id
}

pub async fn job_step(state: &AppState, id: &str, stage: &str, progress: f64, msg: &str) {
    let db = state.db.lock().await;
    let _ = db.conn.execute(
        "UPDATE jobs SET stage=?1, progress=?2, message=?3 WHERE id=?4",
        params![stage, progress, msg, id],
    );
    tracing::info!(target: "jobs", job_id = %id, stage, progress, "任务进度：{msg}");
    db.log_event("job", None, &format!("任务阶段：{stage}"), msg);
}

pub async fn job_end(state: &AppState, id: &str, ok: bool, msg: &str) {
    let db = state.db.lock().await;
    let _ = db.conn.execute(
        "UPDATE jobs SET state=?1, message=?2, progress=1.0, finished_at=?3 WHERE id=?4",
        params![if ok { "done" } else { "failed" }, msg, now_iso(), id],
    );
    tracing::info!(target: "jobs", job_id = %id, state = if ok { "done" } else { "failed" }, "任务结束：{msg}");
    db.log_event("job", None, if ok { "任务完成" } else { "任务失败" }, msg);
}

/// 智能体出错时返回的是 200 + {ok:false, error}，这里把真实原因透出去，
/// 别让用户看到「没有返回正文」这种没用的提示。
fn agent_ok(resp: &Value) -> Result<()> {
    if resp.get("ok").and_then(|x| x.as_bool()) == Some(false) {
        let msg = resp
            .get("error")
            .and_then(|x| x.as_str())
            .unwrap_or("智能体执行失败");
        anyhow::bail!("{msg}");
    }
    Ok(())
}

// ---------------- 领域分类 ----------------

fn classify(title: &str, tags: &[String], terms: &[ProfileTerm]) -> (String, String, f64) {
    let hay = format!("{} {}", title, tags.join(" ")).to_lowercase();
    let mut best: (String, String, f64) = (String::new(), String::new(), 0.0);

    for t in terms {
        let mut hits: f64 = 0.0;
        if hay.contains(&t.label.to_lowercase()) {
            hits += 2.0;
        }
        for k in &t.keywords {
            let k = k.trim().to_lowercase();
            if !k.is_empty() && hay.contains(&k) {
                hits += 1.0;
            }
        }
        if hits <= 0.0 {
            continue;
        }
        let fit = (hits / 3.0).min(1.0) * (0.55 + 0.45 * t.weight.clamp(0.0, 1.0));
        let (domain, sub) = match t.kind.as_str() {
            "primary" => (t.label.clone(), String::new()),
            "sub" => (String::new(), t.label.clone()),
            _ => (String::new(), String::new()),
        };
        if fit > best.2 {
            best = (domain, sub, fit);
        }
    }

    if best.0.is_empty() && best.1.is_empty() {
        // 附属领域命中也记录，但领域留空让前端归到「未分类」
        if best.2 > 0.0 {
            return ("未分类".into(), best.1, best.2);
        }
        return ("未分类".into(), String::new(), 0.0);
    }
    // 若只命中子领域，主领域用第一个 primary 兜底
    if best.0.is_empty() {
        if let Some(p) = terms.iter().find(|t| t.kind == "primary") {
            return (p.label.clone(), best.1, best.2);
        }
        return ("未分类".into(), best.1, best.2);
    }
    (best.0, best.1, best.2)
}

async fn load_terms(state: &AppState) -> Vec<ProfileTerm> {
    let db = state.db.lock().await;
    let mut stmt = match db.conn.prepare(
        "SELECT id, kind, label, weight, keywords, locked, note FROM profile_terms ORDER BY ord, weight DESC",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = stmt
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
        })
        .ok();
    rows.map(|it| it.filter_map(|x| x.ok()).collect())
        .unwrap_or_default()
}

// ---------------- 同步通知 ----------------

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub fetched: usize,
    pub added: usize,
    pub selected: usize,
    pub by_domain: Vec<(String, i64)>,
    pub message: String,
}

pub async fn run_sync(state: &crate::state::Shared) -> Result<SyncReport> {
    run_sync_window(state, None).await
}

pub async fn run_sync_recent(state: &crate::state::Shared, days: i64) -> Result<SyncReport> {
    run_sync_window(state, Some(days)).await
}

pub async fn start_sync(state: &crate::state::Shared, days: Option<i64>) -> String {
    let message = if days.is_some() {
        "正在准备最近 3 天通知"
    } else {
        "正在连接知乎"
    };
    let job = job_start(state, "sync", None, message).await;
    let state = state.clone();
    let job_id = job.clone();
    tokio::spawn(async move {
        let result = sync_inner(&state, &job_id, days).await;
        match &result {
            Ok(report) => job_end(&state, &job_id, true, &report.message).await,
            Err(error) => {
                job_end(&state, &job_id, false, &format!("{error}")).await;
                tracing::error!(target: "sync", job_id = %job_id, "通知同步失败：{error:#}");
            }
        }
    });
    job
}

/// 内核被杀或重启后，库里会留下永远「running」的任务和永远「researching」的问题。
/// 启动时统一收尾，否则界面会一直转圈，用户以为智能体还在干活。
pub async fn recover_stale_jobs(state: &AppState) {
    let db = state.db.lock().await;
    let jobs = db
        .conn
        .execute(
            "UPDATE jobs SET state='failed', progress=1.0, message=?1, finished_at=?2
             WHERE state='running'",
            params!["应用已重启。重新开始时会从已保存的阶段继续。", now_iso()],
        )
        .unwrap_or(0);
    let questions = db
        .conn
        .execute(
            "UPDATE questions SET status=CASE WHEN EXISTS(SELECT 1 FROM drafts WHERE question_id=questions.id)
             THEN 'pending_review' ELSE 'inbox' END WHERE status='researching'",
            [],
        )
        .unwrap_or(0);
    if jobs > 0 || questions > 0 {
        tracing::warn!(jobs, questions, "已清理上次运行残留的进行中任务");
        db.log_event(
            "job",
            None,
            "清理中断任务",
            &format!("{jobs} 个任务标记失败，{questions} 个问题退回待阅"),
        );
    }
}

pub async fn bootstrap_if_empty(state: &crate::state::Shared) {
    let should_sync = {
        let db = state.db.lock().await;
        let logged_in = db
            .conn
            .query_row("SELECT cookie FROM account WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
            .map(|cookie| !cookie.trim().is_empty())
            .unwrap_or(false);
        let questions = db
            .conn
            .query_row("SELECT COUNT(*) FROM questions", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0);
        logged_in && questions == 0
    };
    if should_sync {
        if let Err(e) = run_sync_recent(state, 3).await {
            tracing::warn!("启动时补齐最近 3 天通知失败：{e}");
        }
    }
}

async fn run_sync_window(state: &crate::state::Shared, days: Option<i64>) -> Result<SyncReport> {
    let job = job_start(state, "sync", None, "正在连接知乎").await;
    let result = sync_inner(state, &job, days).await;
    match &result {
        Ok(r) => job_end(state, &job, true, &r.message).await,
        Err(e) => {
            job_end(state, &job, false, &format!("{e}")).await;
            tracing::error!(target: "sync", job_id = %job, "通知同步失败：{e:#}");
        }
    }
    result
}

async fn sync_inner(
    state: &crate::state::Shared,
    job: &str,
    days: Option<i64>,
) -> Result<SyncReport> {
    job_step(
        state,
        job,
        "拉取通知",
        0.12,
        if days.is_some() {
            "补齐最近 3 天通知"
        } else {
            "读取通知中心"
        },
    )
    .await;
    let client = zhihu_client(state).await?;
    let batch = match days {
        Some(days) => client.fetch_invites_since(days).await?,
        None => client.fetch_invites(20).await?,
    };
    let received = batch.received;
    let raws = batch.questions;

    job_step(
        state,
        job,
        "归类领域",
        0.45,
        &format!(
            "读取 {received} 条通知，识别 {} 条邀请/推荐候选",
            raws.len()
        ),
    )
    .await;
    let terms = load_terms(state).await;
    let mut added = 0usize;
    let mut inserted: Vec<(String, f64)> = Vec::new();

    {
        let db = state.db.lock().await;
        for rq in &raws {
            let exists: Option<String> = db
                .conn
                .query_row(
                    "SELECT id FROM questions WHERE question_id = ?1 LIMIT 1",
                    params![rq.question_id],
                    |r| r.get(0),
                )
                .ok();
            if exists.is_some() {
                continue;
            }
            let (domain, sub, fit) = classify(&rq.title, &rq.tags, &terms);
            let id = Uuid::new_v4().to_string();
            let ok = db.conn.execute(
                "INSERT INTO questions
                 (id, zhihu_id, question_id, source, title, excerpt, url, domain, sub_domain,
                  fit, status, bucket, answer_count, follower_count, asker_name, asker_headline,
                  tags, created_at, synced_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'inbox','queue',?11,?12,?13,?14,?15,?16,?16)",
                params![
                    id,
                    rq.zhihu_id,
                    rq.question_id,
                    rq.source,
                    rq.title,
                    rq.excerpt,
                    rq.url,
                    domain,
                    sub,
                    fit,
                    rq.answer_count,
                    rq.follower_count,
                    rq.asker_name,
                    rq.asker_headline,
                    serde_json::to_string(&rq.tags).unwrap_or_else(|_| "[]".into()),
                    now_iso(),
                ],
            );
            if ok.is_ok() {
                added += 1;
                inserted.push((id, fit));
            }
        }
    }

    // 挑「擅长系数最近」的前 40%
    inserted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let pick_n = ((inserted.len() as f64) * 0.4).ceil() as usize;
    let selected_ids: Vec<String> = inserted
        .iter()
        .take(pick_n)
        .filter(|(_, f)| *f > 0.0)
        .map(|(id, _)| id.clone())
        .collect();

    {
        let db = state.db.lock().await;
        for id in &selected_ids {
            let _ = db.conn.execute(
                "UPDATE questions SET bucket = 'selected' WHERE id = ?1",
                params![id],
            );
        }
        let _ = db.conn.execute(
            "UPDATE account SET updated_at = ?1 WHERE id = 1",
            params![now_iso()],
        );
    }

    job_step(state, job, "归档", 0.9, "写入领域目录").await;

    let by_domain = {
        let db = state.db.lock().await;
        let mut stmt = db
            .conn
            .prepare("SELECT domain, COUNT(*) FROM questions GROUP BY domain ORDER BY 2 DESC")
            .ok();
        stmt.as_mut()
            .and_then(|s| {
                s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .ok()
                    .map(|it| it.filter_map(|x| x.ok()).collect())
            })
            .unwrap_or_default()
    };

    {
        let db = state.db.lock().await;
        db.log_event(
            "sync",
            None,
            "完成一次通知同步",
            &format!(
                "读取 {received} 条通知，识别 {} 条候选，新增 {added} 条，进入重点队列 {} 条",
                raws.len(),
                selected_ids.len()
            ),
        );
    }

    let report = SyncReport {
        fetched: received,
        added,
        selected: selected_ids.len(),
        by_domain,
        message: if added == 0 {
            format!(
                "读取 {received} 条通知，识别 {} 条邀请/推荐，没有新的问题",
                raws.len()
            )
        } else {
            format!(
                "读取 {received} 条通知，新增 {added} 个问题，{} 个进入重点队列",
                selected_ids.len()
            )
        },
    };

    // 新进入重点队列的，自动开始研究
    for id in selected_ids {
        let st = state.clone();
        tokio::spawn(async move {
            let _ = start_research(&st, &id).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }

    Ok(report)
}

// ---------------- 画像分析 ----------------

pub async fn run_profile_job(state: &AppState, job: String) -> Result<()> {
    {
        let db = state.db.lock().await;
        let _ = db
            .conn
            .execute("UPDATE profile_meta SET status='running' WHERE id=1", []);
    }
    let r = profile_inner(state, &job).await;
    match &r {
        Ok(_) => job_end(state, &job, true, "领域画像已更新").await,
        Err(e) => {
            let db = state.db.lock().await;
            let _ = db.conn.execute(
                "UPDATE profile_meta SET status=CASE WHEN length(summary) > 0 THEN 'ready' ELSE 'empty' END WHERE id=1",
                [],
            );
            drop(db);
            job_end(state, &job, false, &format!("{e}")).await
        }
    }
    r
}

async fn profile_inner(state: &AppState, job: &str) -> Result<()> {
    let (url_token, account_id, cookie_empty) = {
        let db = state.db.lock().await;
        let tok: String = db
            .conn
            .query_row("SELECT url_token FROM account WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap_or_default();
        let ck: String = db
            .conn
            .query_row("SELECT cookie FROM account WHERE id = 1", [], |r| r.get(0))
            .unwrap_or_default();
        let account_id = db
            .conn
            .query_row("SELECT zhihu_id FROM account WHERE id=1", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
            .unwrap_or_default();
        (tok, account_id, ck.trim().is_empty())
    };
    if cookie_empty {
        anyhow::bail!("请先登录知乎账号");
    }

    job_step(state, job, "抓取回答", 0.16, "正在获取回答列表").await;
    let client = zhihu_client(state).await?;
    let answers = client.my_answers(&url_token, 3).await.unwrap_or_default();
    job_step(
        state,
        job,
        "抓取回答",
        0.28,
        &format!("已获取 {} 条回答", answers.len()),
    )
    .await;
    job_step(state, job, "抓取文章", 0.34, "正在获取文章列表").await;
    let articles = client.my_articles(&url_token, 2).await.unwrap_or_default();
    job_step(
        state,
        job,
        "整理样本",
        0.42,
        &format!("已获取 {} 篇文章", articles.len()),
    )
    .await;
    let sample = answers.len() + articles.len();
    if sample == 0 {
        anyhow::bail!("没有抓到历史内容，可能是登录态失效");
    }

    let samples: Vec<Value> = answers
        .iter()
        .map(|a| {
            json!({
                "kind": "answer",
                "title": a.pointer("/question/title").and_then(|x| x.as_str()).unwrap_or(""),
                "text": sample_text(a),
                "voteups": a.get("voteup_count").and_then(|x| x.as_i64()).unwrap_or(0),
            })
        })
        .chain(articles.iter().map(|a| {
            json!({
                "kind": "article",
                "title": a.get("title").and_then(|x| x.as_str()).unwrap_or(""),
                "text": sample_text(a),
                "voteups": a.get("voteup_count").and_then(|x| x.as_i64()).unwrap_or(0),
            })
        }))
        .collect();

    job_step(
        state,
        job,
        "整理样本",
        0.5,
        &format!("正在整理 {} 条历史内容", sample),
    )
    .await;
    job_step(state, job, "模型归纳", 0.58, "正在归纳主领域、边界与文风").await;

    let model = {
        let db = state.db.lock().await;
        load_model(&db)
    };
    let payload = json!({
        "samples": samples,
        "model": model,
        "display_name": { "name": "" },
    });
    let resp = state.agent.post("/v1/profile", &payload).await?;
    agent_ok(&resp)?;
    job_step(state, job, "提取词条", 0.76, "正在提取关键词与可迁移领域").await;

    let summary = resp
        .get("summary")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let evidence: Vec<String> = resp
        .get("evidence")
        .and_then(|x| x.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let terms = resp
        .get("terms")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    // 文风档案：拟稿时直接喂给写作模型，缺了也要落一个空对象，别让老库报错
    let style = resp.get("style").cloned().unwrap_or_else(|| json!({}));

    {
        let db = state.db.lock().await;
        let current_account: String =
            db.conn
                .query_row("SELECT zhihu_id FROM account WHERE id=1", [], |row| {
                    row.get(0)
                })?;
        anyhow::ensure!(
            current_account == account_id && !account_id.is_empty(),
            "分析期间账号已切换，未保存文风样本"
        );
        // 用户手动锁定的词条保留
        let locked: Vec<String> = {
            let mut stmt = db
                .conn
                .prepare("SELECT id FROM profile_terms WHERE locked = 1")
                .ok();
            stmt.as_mut()
                .and_then(|s| {
                    s.query_map([], |r| r.get::<_, String>(0))
                        .ok()
                        .map(|it| it.filter_map(|x| x.ok()).collect())
                })
                .unwrap_or_default()
        };
        let _ = db
            .conn
            .execute("DELETE FROM profile_terms WHERE locked = 0", []);

        for (i, t) in terms.iter().enumerate() {
            let label = t.get("label").and_then(|x| x.as_str()).unwrap_or("").trim();
            if label.is_empty() {
                continue;
            }
            let id = format!("t_{}", Uuid::new_v4().simple());
            if locked
                .iter()
                .any(|l| l.to_lowercase().contains(&label.to_lowercase()))
            {
                continue;
            }
            let kws = t.get("keywords").cloned().unwrap_or(json!([]));
            let _ = db.conn.execute(
                "INSERT INTO profile_terms (id, kind, label, weight, keywords, locked, note, ord)
                 VALUES (?1,?2,?3,?4,?5,0,?6,?7)",
                params![
                    id,
                    t.get("kind").and_then(|x| x.as_str()).unwrap_or("primary"),
                    label,
                    t.get("weight").and_then(|x| x.as_f64()).unwrap_or(0.6),
                    kws.to_string(),
                    t.get("note").and_then(|x| x.as_str()).unwrap_or(""),
                    i as i64
                ],
            );
        }

        let _ = db.conn.execute(
            "UPDATE profile_meta SET status='ready', summary=?1, evidence=?2, sample_size=?3, style=?4, updated_at=?5 WHERE id=1",
            params![
                summary,
                serde_json::to_string(&evidence).unwrap_or_else(|_| "[]".into()),
                sample as i64,
                style.to_string(),
                now_iso()
            ],
        );
        db.conn.execute(
            "INSERT OR REPLACE INTO writing_samples (account_id, samples) VALUES (?1, ?2)",
            params![
                account_id,
                resp.get("samples")
                    .cloned()
                    .unwrap_or(json!([]))
                    .to_string()
            ],
        )?;
        db.log_event(
            "profile",
            None,
            "更新领域画像与文风档案",
            &format!("基于 {sample} 篇内容"),
        );
    }

    // 画像有了，顺手让模型推荐权威平台
    job_step(state, job, "关联来源", 0.9, "正在关联对应的权威平台").await;
    let _ = run_discover_sources(state).await;
    Ok(())
}

fn sample_text(value: &Value) -> String {
    value
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .or_else(|| value.get("excerpt").and_then(Value::as_str))
        .unwrap_or_default()
        .chars()
        .take(32000)
        .collect()
}

// ---------------- 权威平台 ----------------

pub async fn run_discover_sources(state: &AppState) -> Result<()> {
    let job = job_start(state, "sources", None, "按领域检索权威平台").await;
    let r = discover_inner(state, &job).await;
    match &r {
        Ok(_) => job_end(state, &job, true, "权威平台已更新").await,
        Err(e) => job_end(state, &job, false, &format!("{e}")).await,
    }
    r
}

async fn discover_inner(state: &AppState, job: &str) -> Result<()> {
    let terms = load_terms(state).await;
    let primary: Vec<String> = terms
        .iter()
        .filter(|t| t.kind == "primary" || t.kind == "sub")
        .map(|t| t.label.clone())
        .collect();
    if primary.is_empty() {
        anyhow::bail!("领域画像还是空的，先做完画像再来");
    }
    job_step(
        state,
        job,
        "检索",
        0.4,
        &format!("为「{}」寻找权威来源", primary.join(" / ")),
    )
    .await;
    let model = {
        let db = state.db.lock().await;
        load_model(&db)
    };
    let resp = state
        .agent
        .post(
            "/v1/sources",
            &json!({ "domains": primary, "model": model }),
        )
        .await?;
    agent_ok(&resp)?;
    let items = resp
        .get("sources")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    {
        let db = state.db.lock().await;
        for s in &items {
            let name = s.get("name").and_then(|x| x.as_str()).unwrap_or("");
            if name.is_empty() {
                continue;
            }
            let existing: Option<String> = db
                .conn
                .query_row(
                    "SELECT id FROM sources WHERE name = ?1 AND domain = ?2",
                    params![name, s.get("domain").and_then(|x| x.as_str()).unwrap_or("")],
                    |r| r.get(0),
                )
                .ok();
            if existing.is_some() {
                continue;
            }
            let _ = db.conn.execute(
                "INSERT INTO sources (id, domain, name, base_url, kind, trust, enabled, notes, use_for, discovered_by, ok)
                 VALUES (?1,?2,?3,?4,?5,?6,1,?7,?8,'llm',1)",
                params![
                    format!("s_{}", Uuid::new_v4().simple()),
                    s.get("domain").and_then(|x| x.as_str()).unwrap_or(""),
                    name,
                    s.get("baseUrl").and_then(|x| x.as_str()).unwrap_or(""),
                    s.get("kind").and_then(|x| x.as_str()).unwrap_or("wiki"),
                    s.get("trust").and_then(|x| x.as_i64()).unwrap_or(3),
                    s.get("notes").and_then(|x| x.as_str()).unwrap_or(""),
                    s.get("useFor").cloned().unwrap_or(json!(["fact"])).to_string(),
                ],
            );
        }
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM sources", [], |r| r.get(0))
            .unwrap_or(0);
        db.log_event(
            "sources",
            None,
            "更新权威平台清单",
            &format!("共 {n} 个来源"),
        );
    }
    Ok(())
}

// ---------------- 出稿（核心流水线） ----------------

/// 同时最多跑两份稿子。三份以上一起打同一个模型 Key，既容易限流，
/// 也会让每一份都慢到让人以为卡死。
pub async fn start_research(state: &crate::state::Shared, question_id: &str) -> Result<String> {
    let job = {
        let mut db = state.db.lock().await;
        let tx = db.conn.transaction()?;
        if let Some(existing) = tx
            .query_row(
                "SELECT id FROM jobs WHERE kind='research' AND question_id=?1 AND state='running'",
                params![question_id],
                |row| row.get(0),
            )
            .optional()?
        {
            return Ok(existing);
        }
        let status: String = tx
            .query_row(
                "SELECT status FROM questions WHERE id=?1",
                params![question_id],
                |row| row.get(0),
            )
            .map_err(|_| anyhow::anyhow!("问题不存在"))?;
        let resume = tx
            .query_row(
                "SELECT 1 FROM research_checkpoints WHERE question_id=?1",
                params![question_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let has_draft = tx
            .query_row(
                "SELECT 1 FROM drafts WHERE question_id=?1 AND trim(markdown)<>''",
                params![question_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        anyhow::ensure!(
            matches!(status.as_str(), "inbox" | "pending_review"),
            "该问题已归档，不能拟稿"
        );
        anyhow::ensure!(
            !has_draft || resume,
            "已有正文。为避免覆盖手工改动，不会重新生成。"
        );
        let job = format!("j_{}", Uuid::new_v4().simple());
        tx.execute(
            "INSERT INTO jobs (id, kind, question_id, stage, message, started_at) VALUES (?1,'research',?2,'排队','等待拟稿名额',?3)",
            params![job, question_id, now_iso()],
        )?;
        tx.execute(
            "UPDATE questions SET status='researching' WHERE id=?1",
            params![question_id],
        )?;
        tx.commit()?;
        job
    };
    let job_id = job.clone();
    let qid = question_id.to_owned();
    let state = state.clone();
    tokio::spawn(async move {
        let Ok(_slot) = state.research_slots.acquire().await else {
            return;
        };
        let result = research_inner(&state, &qid, &job_id).await;
        let db = state.db.lock().await;
        let _ = db.conn.execute(
            "UPDATE questions SET status=CASE WHEN EXISTS(SELECT 1 FROM drafts WHERE question_id=?1)
             THEN 'pending_review' ELSE 'inbox' END WHERE id=?1 AND status='researching'",
            params![qid],
        );
        drop(db);
        match result {
            Ok(()) => job_end(&state, &job_id, true, "初稿和素材已保存，等待人工批阅").await,
            Err(error) => {
                job_end(
                    &state,
                    &job_id,
                    false,
                    &format!("{error} 已保存的阶段不会丢失。"),
                )
                .await
            }
        }
    });
    Ok(job)
}

async fn agent_stage(state: &AppState, path: &str, payload: &Value, seconds: u64) -> Result<Value> {
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(seconds),
        state.agent.post(path, payload),
    )
    .await
    .map_err(|_| anyhow::anyhow!("当前阶段超时，请重试或检查网络与模型配置。"))??;
    agent_ok(&response)?;
    Ok(response)
}

pub async fn collect_question_images(
    state: &AppState,
    qid: &str,
    mut payload: Value,
    import: bool,
    interactive: bool,
) -> Result<usize> {
    let _slot = if interactive {
        state
            .image_slots
            .try_acquire()
            .map_err(|_| anyhow::anyhow!("正在处理另一批配图，请稍后再试。"))?
    } else {
        state.image_slots.acquire().await?
    };
    let img_dir = state.data_dir.join("images").join(crate::db::sanitize(qid));
    let existing = {
        let db = state.db.lock().await;
        let mut stmt = db
            .conn
            .prepare("SELECT filename FROM images WHERE question_id=?1 ORDER BY created_at DESC")?;
        let rows = stmt.query_map(params![qid], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if existing.len() >= 80 {
        anyhow::bail!("备选图已达上限，请先整理已有图片。");
    }
    if payload["supplement"] == true {
        let maximum = payload["settings"]["imageMax"].as_u64().unwrap_or_default();
        payload["settings"]["imageMax"] = json!(maximum.min((80 - existing.len()) as u64));
    }
    payload["imagesDir"] = json!(img_dir);
    payload["existingImages"] = json!(existing);
    let result = agent_stage(
        state,
        if import {
            "/v1/research/images/import"
        } else {
            "/v1/research/images"
        },
        &payload,
        27,
    )
    .await?;
    let mut db = state.db.lock().await;
    let tx = db.conn.transaction()?;
    let mut added = 0;
    for image in result["images"].as_array().into_iter().flatten() {
        let filename = image["filename"].as_str().unwrap_or_default();
        if filename.len() != 36
            || !filename.as_bytes().ends_with(b".png")
            || !filename.as_bytes()[..32].iter().all(u8::is_ascii_hexdigit)
        {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(img_dir.join(filename)) else {
            continue;
        };
        added += tx.execute(
            "INSERT INTO images (id, question_id, filename, source_url, source_name, caption, bytes, created_at)
             SELECT ?1,?2,?3,?4,?5,?6,?7,?8 WHERE NOT EXISTS
             (SELECT 1 FROM images WHERE question_id=?2 AND filename=?3)",
            params![format!("i_{}", Uuid::new_v4().simple()), qid, filename,
                image["sourceUrl"].as_str().unwrap_or_default(), image["sourceName"].as_str().unwrap_or_default(),
                image["caption"].as_str().unwrap_or_default(), metadata.len() as i64, now_iso()],
        )?;
    }
    tx.commit()?;
    Ok(added)
}

async fn research_inner(state: &AppState, qid: &str, job: &str) -> Result<()> {
    let (question, terms, meta_summary, meta_style, sources, model, settings) = {
        let db = state.db.lock().await;
        let q = db
            .conn
            .query_row(
                "SELECT id, zhihu_id, question_id, source, title, excerpt, url, domain, sub_domain, fit,
                        status, bucket, answer_count, follower_count, asker_name, asker_headline, tags, created_at, synced_at
                 FROM questions WHERE id = ?1",
                params![qid],
                |r| {
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
                        has_draft: false,
                        image_count: 0,
                        word_count: 0,
                        doubt_count: 0,
                    })
                },
            )
            .map_err(|_| anyhow::anyhow!("问题不存在"))?;

        let summary: String = db
            .conn
            .query_row("SELECT summary FROM profile_meta WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap_or_default();

        let style: Value = db
            .conn
            .query_row("SELECT style FROM profile_meta WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(|| json!({}));

        let mut stmt = db.conn.prepare(
            "SELECT id, domain, name, base_url, kind, trust, enabled, notes, use_for, discovered_by, ok FROM sources WHERE enabled = 1",
        )?;
        let src: Vec<Source> = stmt
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
        drop(stmt);

        let s = load_settings(&db);
        let mut tstmt = db.conn.prepare(
            "SELECT id, kind, label, weight, keywords, locked, note FROM profile_terms ORDER BY ord",
        )?;
        let terms: Vec<ProfileTerm> = tstmt
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
        drop(tstmt);

        (q, terms, summary, style, src, load_model(&db), s)
    };

    let img_dir = state.data_dir.join("images").join(crate::db::sanitize(qid));
    let (checkpoint, samples) = {
        let db = state.db.lock().await;
        let checkpoint = db
            .conn
            .query_row(
                "SELECT material, drafted FROM research_checkpoints WHERE question_id=?1",
                params![qid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()?;
        let samples = db
            .conn
            .query_row(
                "SELECT samples FROM writing_samples WHERE account_id=(SELECT zhihu_id FROM account WHERE id=1 AND cookie<>'')",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_default();
        (
            checkpoint,
            serde_json::from_str::<Value>(&samples).unwrap_or(json!([])),
        )
    };
    let mut payload = json!({
        "question": question,
        "profile": { "summary": meta_summary, "terms": terms, "style": meta_style, "samples": samples },
        "sources": sources,
        "model": model,
        "settings": settings,
        "imagesDir": img_dir.to_string_lossy(),
    });
    let (material, drafted) = match checkpoint {
        Some((material, drafted)) => (serde_json::from_str(&material)?, drafted),
        None => {
            job_step(state, job, "取材", 0.18, "搜索资料并筛选可引用正文").await;
            let material = agent_stage(state, "/v1/research/collect", &payload, 50).await?;
            let db = state.db.lock().await;
            db.conn.execute(
                "INSERT INTO research_checkpoints (question_id, material) VALUES (?1,?2)",
                params![qid, material.to_string()],
            )?;
            (material, false)
        }
    };
    payload["material"] = material;
    if !drafted {
        job_step(state, job, "拟稿", 0.43, "资料已保存，正在生成正文").await;
        let response = agent_stage(state, "/v1/research/draft", &payload, 145).await?;
        let markdown = response
            .pointer("/draft/markdown")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        anyhow::ensure!(!markdown.is_empty(), "模型没有返回有效正文");
        let count = markdown
            .chars()
            .filter(|character| !character.is_whitespace())
            .count() as i64;
        let mut db = state.db.lock().await;
        let tx = db.conn.transaction()?;
        let written = tx.execute(
            "INSERT INTO drafts (question_id, markdown, doubts, citations, model, word_count, updated_at, warnings)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(question_id) DO UPDATE SET markdown=?2, doubts=?3, citations=?4, model=?5,
             word_count=?6, updated_at=?7, warnings=?8 WHERE trim(drafts.markdown)=''",
            params![qid, markdown, response["draft"]["doubts"].to_string(), response["draft"]["citations"].to_string(),
                model.model, count, now_iso(), response["warnings"].to_string()],
        )?;
        anyhow::ensure!(written == 1, "已有手工正文，未覆盖任何改动");
        tx.execute(
            "UPDATE research_checkpoints SET drafted=1 WHERE question_id=?1",
            params![qid],
        )?;
        tx.execute(
            "UPDATE questions SET bucket='selected' WHERE id=?1",
            params![qid],
        )?;
        tx.commit()?;
    }

    let raw_doubts: String = state.db.lock().await.conn.query_row(
        "SELECT doubts FROM drafts WHERE question_id=?1",
        params![qid],
        |row| row.get(0),
    )?;
    payload["doubts"] = serde_json::from_str(&raw_doubts)?;
    job_step(
        state,
        job,
        "补充核查与配图",
        0.76,
        "正文已保存并可编辑；正在补充核查与配图",
    )
    .await;
    let (review, images) = tokio::join!(
        agent_stage(state, "/v1/research/review", &payload, 42),
        collect_question_images(state, qid, payload.clone(), false, false),
    );
    let mut db = state.db.lock().await;
    let tx = db.conn.transaction()?;
    if let Ok(review) = &review {
        tx.execute(
            "UPDATE drafts SET doubts=?2 WHERE question_id=?1",
            params![qid, review["doubts"].to_string()],
        )?;
    }
    if review.is_ok() && images.is_ok() {
        tx.execute(
            "DELETE FROM research_checkpoints WHERE question_id=?1",
            params![qid],
        )?;
    }
    tx.commit()?;
    if let Err(error) = review {
        anyhow::bail!("正文已保存，核查未完成：{error}");
    }
    if let Err(error) = images {
        anyhow::bail!("正文已保存，配图未完成：{error}");
    }
    Ok(())
}

// ---------------- 配置读取 ----------------

pub fn load_settings(db: &crate::db::Db) -> Settings {
    db.conn
        .query_row(
            "SELECT cron_times, pick_ratio, draft_min, draft_max, image_min, image_max,
                    auto_sync, minimize_to_tray, auto_launch, sign_mode FROM settings WHERE id=1",
            [],
            |r| {
                Ok(Settings {
                    cron_times: serde_json::from_str(&r.get::<_, String>(0)?)
                        .unwrap_or_else(|_| vec!["07:00".into(), "15:00".into(), "20:00".into()]),
                    pick_ratio: r.get(1)?,
                    draft_min: r.get(2)?,
                    draft_max: r.get(3)?,
                    image_min: r.get(4)?,
                    image_max: r.get(5)?,
                    auto_sync: r.get::<_, i64>(6)? != 0,
                    minimize_to_tray: r.get::<_, i64>(7)? != 0,
                    auto_launch: r.get::<_, i64>(8)? != 0,
                    sign_mode: r.get(9)?,
                    data_dir: db.data_dir.to_string_lossy().to_string(),
                })
            },
        )
        .unwrap_or(Settings {
            cron_times: vec!["07:00".into(), "15:00".into(), "20:00".into()],
            pick_ratio: 0.4,
            draft_min: 500,
            draft_max: 1000,
            image_min: 3,
            image_max: 10,
            auto_sync: true,
            minimize_to_tray: true,
            auto_launch: false,
            sign_mode: "auto".into(),
            data_dir: db.data_dir.to_string_lossy().to_string(),
        })
}

pub fn load_model(db: &crate::db::Db) -> ModelConfig {
    let mut model = db.conn
        .query_row(
            "SELECT provider, base_url, model, api_key, temperature, verified_at, context_limit, output_limit FROM model_config WHERE id=1",
            [],
            |r| {
                let key: String = r.get(3)?;
                Ok(ModelConfig {
                    provider: r.get(0)?,
                    base_url: r.get(1)?,
                    model: r.get(2)?,
                    has_key: !key.trim().is_empty(),
                    api_key: key,
                    temperature: r.get(4)?,
                    verified_at: r.get(5)?,
                    context_limit: r.get(6)?,
                    output_limit: r.get(7)?,
                })
            },
        )
        .unwrap_or(ModelConfig {
            provider: "deepseek".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            model: "deepseek-flash".into(),
            api_key: String::new(),
            has_key: false,
            temperature: 0.3,
            verified_at: None,
            context_limit: 0,
            output_limit: 0,
        });
    if model.provider == "openrouter" {
        model.provider = "deepseek".into();
        model.base_url = "https://api.deepseek.com/v1".into();
        model.model = "deepseek-flash".into();
        model.api_key.clear();
        model.has_key = false;
        model.verified_at = None;
    }
    model
}
