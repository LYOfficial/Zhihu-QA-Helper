//! 后台任务：同步通知、生成初稿、分析画像、发现权威平台。
//! 所有任务都会写一行 job 记录，前端据此显示进度。

use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::models::*;
use crate::state::{AppState, ZhihuOpenSettings};
use crate::zhihu::{answer_question_id, parse_shared_link, strip_html, RawQuestion, SharedLink, ZhihuClient};

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
    job_stage(state, id, stage, progress, msg, None).await;
}

async fn job_stage(state: &AppState, id: &str, stage: &str, progress: f64, msg: &str, verify_seconds: Option<i64>) {
    let db = state.db.lock().await;
    let _ = db.conn.execute(
        "UPDATE jobs SET stage=?1, progress=?2, message=?3, verify_started_ms=?4, verify_seconds=?5 WHERE id=?6",
        params![stage, progress, msg, verify_seconds.map(|_| chrono::Utc::now().timestamp_millis()), verify_seconds, id],
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

pub const IS_ANSWERED: &str = "EXISTS(SELECT 1 FROM answered_questions h JOIN active_account a ON a.zhihu_id=h.account_id
    WHERE h.question_id=q.question_id)";

pub fn onboarding_required(conn: &rusqlite::Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM active_account a WHERE NOT EXISTS(SELECT 1 FROM onboarding_finished WHERE account_id=a.zhihu_id))", [], |row| row.get(0),
    )
}

pub(crate) fn account_identity(conn: &rusqlite::Connection) -> Result<(String, String)> {
    conn.query_row("SELECT zhihu_id, url_token FROM active_account",
        [], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| anyhow::anyhow!("尚未登录知乎账号"))
}

pub(crate) async fn account_client(state: &AppState) -> Result<(String, String, ZhihuClient)> {
    let db = state.db.lock().await;
    let (account_id, token) = account_identity(&db.conn)?;
    Ok((account_id, token, crate::state::zhihu_client_from_db(&db)?))
}

fn remember_answers(conn: &rusqlite::Connection, account_id: &str, answers: &[Value]) -> Result<usize> {
    let mut insert = conn.prepare("INSERT OR IGNORE INTO answered_questions VALUES (?1,?2)")?;
    let mut added = 0;
    for id in answers.iter().map(answer_question_id).filter(|id| !id.is_empty()) {
        added += insert.execute(params![account_id, id])?;
    }
    Ok(added)
}

async fn refresh_answer_history(state: &AppState, client: &ZhihuClient, account_id: &str, token: &str) -> Result<bool> {
    let _lock = state.answer_history_lock.lock().await;
    anyhow::ensure!(!token.is_empty(), "无法识别答主主页");
    let mut backfill = {
        let db = state.db.lock().await;
        db.conn.query_row("SELECT next_offset FROM answer_history_progress WHERE account_id=?1",
            [account_id], |row| row.get::<_, i64>(0)).optional()?.unwrap_or(0)
    };
    let mut newest = Some(0);
    for turn in 0..10 {
        let head = newest.is_some() && (turn < 5 || backfill < 0);
        let offset = if head { newest.unwrap_or_default() } else if backfill >= 0 { backfill as usize } else { break };
        let (answers, end) = client.answer_history_page(token, offset).await?;
        anyhow::ensure!(answers.iter().all(|answer| !answer_question_id(answer).is_empty()), "回答记录缺少问题编号");
        let mut db = state.db.lock().await;
        let tx = db.conn.transaction()?;
        let added = remember_answers(&tx, account_id, &answers)?;
        let next = (offset + answers.len()) as i64;
        if head {
            newest = if end || (backfill != 0 && added == 0) { None } else { Some(next as usize) };
        }
        if offset as i64 == backfill || (head && end) {
            backfill = if end { -1 } else { next };
            tx.execute("INSERT OR REPLACE INTO answer_history_progress VALUES (?1,?2)", params![account_id, backfill])?;
        }
        tx.commit()?;
        if backfill < 0 && newest.is_none() {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn sync_answer_history(state: &AppState, client: &ZhihuClient, account_id: &str, token: &str) -> &'static str {
    match tokio::time::timeout(std::time::Duration::from_secs(35), refresh_answer_history(state, client, account_id, token)).await {
        Ok(Ok(true)) => "",
        Ok(Ok(false)) => "；已排除已记录的回答，剩余历史会在下次同步继续补齐",
        result => {
            tracing::warn!("回答历史同步未完成：{result:?}");
            "；未能同步全部回答记录，已使用本机已知记录排除"
        }
    }
}

fn load_terms(conn: &rusqlite::Connection) -> Result<Vec<ProfileTerm>> {
    let mut stmt = conn.prepare(
        "SELECT id, kind, label, weight, keywords, locked, note FROM profile_terms ORDER BY ord, weight DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ProfileTerm {
            id: row.get(0)?,
            kind: row.get(1)?,
            label: row.get(2)?,
            weight: row.get(3)?,
            keywords: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
            locked: row.get::<_, i64>(5)? != 0,
            note: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn prioritize(
    conn: &rusqlite::Connection,
    candidates: &mut [(String, f64)],
) -> Result<Vec<String>> {
    candidates.sort_by(|left, right| right.1.total_cmp(&left.1));
    let limit = (candidates.len() * 2).div_ceil(5);
    let mut selected = Vec::new();
    let mut update = conn.prepare("UPDATE questions SET bucket='selected' WHERE id=?1")?;
    for (id, _) in candidates
        .iter()
        .take(limit)
        .take_while(|(_, fit)| *fit > 0.0)
    {
        update.execute([id])?;
        selected.push(id.clone());
    }
    Ok(selected)
}

fn is_answered(conn: &rusqlite::Connection, account_id: &str, id: &str) -> Result<bool> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM answered_questions WHERE account_id=?1 AND question_id=?2)",
        params![account_id, id], |row| row.get(0))?)
}

fn existing_question(conn: &rusqlite::Connection, id: &str) -> Result<Option<(String, String)>> {
    Ok(conn.query_row("SELECT id, status FROM questions WHERE question_id=?1 LIMIT 1", [id],
        |row| Ok((row.get(0)?, row.get(1)?))).optional()?)
}

fn insert_question(conn: &rusqlite::Connection, raw: &RawQuestion, created: i64, terms: &[ProfileTerm]) -> Result<(String, f64)> {
    let (domain, sub, fit) = classify(&raw.title, &raw.tags, terms);
    let id = Uuid::new_v4().to_string();
    let synced = now_iso();
    let created = chrono::DateTime::from_timestamp(created, 0).filter(|_| created > 0)
        .map(|time| time.to_rfc3339()).unwrap_or_else(|| synced.clone());
    conn.execute(
        "INSERT INTO questions (id, zhihu_id, question_id, source, title, excerpt, url, domain, sub_domain,
         fit, status, bucket, answer_count, follower_count, asker_name, asker_headline, tags, created_at, synced_at, fit_evaluated, metrics)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'inbox','queue',?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![id, raw.zhihu_id, raw.question_id, raw.source, raw.title, raw.excerpt, raw.url, domain, sub, fit,
            raw.answer_count, raw.follower_count, raw.asker_name, raw.asker_headline,
            serde_json::to_string(&raw.tags)?, created, synced, !terms.is_empty(),
            raw.metrics.as_ref().map(serde_json::to_string).transpose()?],
    )?;
    Ok((id, fit))
}

fn share_urls(text: &str) -> Vec<&str> {
    let mut seen = std::collections::HashSet::new();
    text.split(|character: char| character.is_whitespace() || "[]()<>\"'`，。；（）【】".contains(character))
        .filter(|part| part.starts_with("https://") || part.starts_with("http://"))
        .filter(|part| seen.insert(*part)).collect()
}

pub async fn import_questions(state: &AppState, text: &str) -> Result<Vec<Value>> {
    anyhow::ensure!(text.len() <= 64000, "分享文本过长");
    let normalized = text.replace("\\:", ":").replace("\\.", ".");
    let urls = share_urls(&normalized);
    anyhow::ensure!(!urls.is_empty() && urls.len() <= 10, "请粘贴 1–10 条知乎分享链接");
    let (account_id, _, client) = account_client(state).await?;
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for url in urls {
        let result = tokio::time::timeout(std::time::Duration::from_secs(20), async {
            let question_id = match parse_shared_link(url)? {
                SharedLink::Question(id) => id,
                SharedLink::Answer(id) => client.question_from_answer(&id).await?,
            };
            if seen.contains(&question_id) {
                return Ok(None);
            }
            {
                let db = state.db.lock().await;
                anyhow::ensure!(account_identity(&db.conn)?.0 == account_id, "知乎账号已切换，请重试");
                if let Some(id) = pin_existing(&db.conn, &account_id, &question_id)? {
                    seen.insert(question_id);
                    return Ok(Some(id));
                }
            }
            let (question, created, _) = client.question_details(&question_id, "imported").await?;
            let db = state.db.lock().await;
            anyhow::ensure!(account_identity(&db.conn)?.0 == account_id, "知乎账号已切换，请重试");
            if let Some(id) = pin_existing(&db.conn, &account_id, &question_id)? {
                seen.insert(question_id);
                return Ok(Some(id));
            }
            let terms = load_terms(&db.conn)?;
            let (id, _) = insert_question(&db.conn, &question, created, &terms)?;
            seen.insert(question_id);
            Ok(Some(id))
        }).await;
        let result: Result<Option<String>> = result.unwrap_or_else(|_| Err(anyhow::anyhow!("读取知乎问题超时")));
        results.push(match result {
            Ok(Some(id)) => json!({"url": url, "id": id}),
            Ok(None) => continue,
            Err(error) => json!({"url": url, "error": error.to_string()}),
        });
    }
    Ok(results)
}

fn pin_existing(conn: &rusqlite::Connection, account_id: &str, question_id: &str) -> Result<Option<String>> {
    anyhow::ensure!(!is_answered(conn, account_id, question_id)?, "你已回答过该问题");
    let Some((id, status)) = existing_question(conn, question_id)? else { return Ok(None) };
    anyhow::ensure!(status != "answered", "该问题已标记为已回答");
    anyhow::ensure!(status != "dismissed", "该问题已弃用，请从「已弃用」恢复后再导入");
    conn.execute("UPDATE questions SET source='imported', synced_at=?2 WHERE id=?1", params![id, now_iso()])?;
    Ok(Some(id))
}

pub async fn start_question_discovery(state: &crate::state::Shared) -> Result<String> {
    let _lock = state.discovery_lock.lock().await;
    let (account_id, token, terms, secret, client) = {
        let db = state.db.lock().await;
        if let Some(id) = db.conn.query_row("SELECT id FROM jobs WHERE kind='discover_questions' AND state='running' LIMIT 1",
            [], |row| row.get(0)).optional()? { return Ok(id); }
        let (account_id, token) = account_identity(&db.conn)?;
        let terms = load_terms(&db.conn)?;
        anyhow::ensure!(!terms.is_empty(), "请先分析领域画像或添加擅长的领域，再主动找题");
        let open = ZhihuOpenSettings::load(&db)?;
        (account_id, token, terms, if open.enabled { open.secret } else { String::new() }, crate::state::zhihu_client_from_db(&db)?)
    };
    let job = job_start(state, "discover_questions", None, "正在准备按领域找题").await;
    let job_id = job.clone();
    let state = state.clone();
    tokio::spawn(async move {
        match discover_questions_inner(&state, &client, &job_id, &account_id, &token, terms, secret).await {
            Ok(message) => job_end(&state, &job_id, true, &message).await,
            Err(error) => job_end(&state, &job_id, false, &error.to_string()).await,
        }
    });
    Ok(job)
}

async fn discover_questions_inner(state: &AppState, client: &ZhihuClient, job: &str, account_id: &str, token: &str, mut terms: Vec<ProfileTerm>, secret: String) -> Result<String> {
    job_step(state, job, "筛选已答", 0.1, "更新本机回答记录").await;
    let note = sync_answer_history(state, client, account_id, token).await;
    terms.sort_by(|left, right| right.weight.total_cmp(&left.weight));
    let topics: Vec<_> = terms.iter().filter(|term| !term.label.trim().is_empty()).take(4).map(|term| term.label.clone()).collect();
    job_step(state, job, "检索知乎", 0.25, "按画像检索近期问题").await;
    let payload = json!({"topics": topics, "zhihuAccessSecret": secret});
    let (native, found) = tokio::join!(search_native(client, topics), agent_stage(state, "/v1/questions/discover", &payload, 30));
    anyhow::ensure!(native.is_ok() || found.is_ok(), "暂时无法检索知乎问题，请检查网络或稍后重试");
    let external: Vec<_> = found.unwrap_or(Value::Null)["urls"].as_array().into_iter().flatten().filter_map(Value::as_str)
        .filter_map(|url| match parse_shared_link(url) { Ok(SharedLink::Question(id)) => Some(id), _ => None }).collect();
    let native = native.unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut checks = tokio::task::JoinSet::new();
    let mut ids = Vec::new();
    {
        let db = state.db.lock().await;
        for index in 0..native.len().max(external.len()) {
            for id in [native.get(index), external.get(index)].into_iter().flatten() {
                if seen.insert(id.clone()) && !is_answered(&db.conn, account_id, id)? && existing_question(&db.conn, id)?.is_none() {
                    ids.push(id.clone());
                }
            }
        }
    }
    job_step(state, job, "评估候选", 0.52, "核对发问时间、关注度和领域契合度").await;
    let now = chrono::Utc::now().timestamp();
    let mut ranked = Vec::new();
    let mut readable = 0;
    let candidates = ids.len();
    for group in ids.chunks(4).take(5) {
        for id in group {
            let client = client.clone();
            let id = id.clone();
            checks.spawn(async move { tokio::time::timeout(std::time::Duration::from_secs(9), client.question_details(&id, "discovered")).await });
        }
        while let Some(outcome) = checks.join_next().await {
            let Ok(Ok(Ok((question, created, views)))) = outcome else { continue };
            readable += 1;
            if let Some(score) = discovery_score(&question, created, views, now, &terms) {
                ranked.push((score, question, created));
            }
        }
        if ranked.len() >= 12 { break; }
    }
    anyhow::ensure!(candidates == 0 || readable > 0, "找到了候选链接，但知乎未返回问题详情，请稍后重试");
    ranked.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.question_id.cmp(&right.1.question_id)));
    let mut db = state.db.lock().await;
    let tx = db.conn.transaction()?;
    anyhow::ensure!(account_identity(&tx)?.0 == account_id, "知乎账号已切换，请重试");
    let mut added = 0;
    for (_, question, created) in ranked.into_iter().take(8) {
        if !is_answered(&tx, account_id, &question.question_id)? && existing_question(&tx, &question.question_id)?.is_none() {
            insert_question(&tx, &question, created, &terms)?;
            added += 1;
        }
    }
    tx.commit()?;
    Ok(if added == 0 { format!("没有发现新的合适问题，可调整领域关键词后重试{note}") }
        else { format!("已加入 {added} 个近期相关问题，不会自动拟稿{note}") })
}

async fn search_native(client: &ZhihuClient, topics: Vec<String>) -> Result<Vec<String>> {
    let mut searches = tokio::task::JoinSet::new();
    for (index, topic) in topics.into_iter().enumerate() {
        let client = client.clone();
        searches.spawn(async move { (index, tokio::time::timeout(std::time::Duration::from_secs(8), client.search_recent_questions(&topic)).await) });
    }
    let mut groups = Vec::new();
    while let Some(result) = searches.join_next().await {
        if let Ok((index, Ok(Ok(ids)))) = result { groups.push((index, ids)); }
    }
    anyhow::ensure!(!groups.is_empty(), "知乎站内检索暂不可用");
    groups.sort_by_key(|(index, _)| *index);
    Ok((0..groups.iter().map(|(_, ids)| ids.len()).max().unwrap_or(0))
        .flat_map(|position| groups.iter().filter_map(move |(_, ids)| ids.get(position).cloned())).collect())
}

fn discovery_score(question: &RawQuestion, created: i64, views: i64, now: i64, terms: &[ProfileTerm]) -> Option<f64> {
    let age = now.checked_sub(created)?;
    if !(0..=90 * 86400).contains(&age) { return None; }
    let (_, _, fit) = classify(&question.title, &question.tags, terms);
    let activity = question.follower_count.max(0) as f64 + 2.0 * question.answer_count.max(0) as f64
        + views.max(0) as f64 / 100.0;
    (fit > 0.0 && activity > 0.0).then(|| fit * 3.0 + activity.ln_1p() / 5.0 - age as f64 / (90.0 * 86400.0))
}

#[cfg(test)]
mod question_tests {
    use super::*;

    #[test]
    fn discovery_ranks_relevant_recent_questions_with_real_activity() {
        let terms = [ProfileTerm {
            id: "rail".into(), kind: "primary".into(), label: "铁路".into(), weight: 1.0,
            keywords: vec!["信号".into()], locked: false, note: String::new(),
        }];
        let mut question = RawQuestion {
            question_id: "123".into(), zhihu_id: "123".into(), title: "铁路信号怎么设置？".into(),
            source: "discovered".into(), excerpt: String::new(), url: String::new(), asker_name: String::new(),
            asker_headline: String::new(), answer_count: 0, follower_count: 0, tags: Vec::new(), metrics: None,
        };
        let now = 1_800_000_000;
        let created = now - 86_400;
        assert!(discovery_score(&question, created, 0, now, &terms).is_none());
        assert!(discovery_score(&question, now - 91 * 86_400, 100, now, &terms).is_none());
        let score = discovery_score(&question, created, 100, now, &terms).unwrap();
        assert!(discovery_score(&question, created, 500, now, &terms).unwrap() > score);
        assert!(discovery_score(&question, created - 86_400, 100, now, &terms).unwrap() < score);
        question.title = "如何给水彩配色？".into();
        assert!(discovery_score(&question, created, 1_000_000, now, &terms).is_none());
    }
}

pub(crate) fn refresh_inbox_fit(conn: &rusqlite::Connection) -> Result<()> {
    let terms = load_terms(conn)?;
    let mut select = conn.prepare(&format!(
        "SELECT id, title, tags, source FROM questions q
         WHERE status='inbox' AND bucket IN ('queue', 'selected')
         AND NOT {IS_ANSWERED}
         ORDER BY synced_at DESC, id",
    ))?;
    let questions = select
        .query_map([], |row| {
            let tags =
                serde_json::from_str::<Vec<String>>(&row.get::<_, String>(2)?).unwrap_or_default();
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, tags, row.get::<_, String>(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut update = conn.prepare(
        "UPDATE questions SET domain=?2, sub_domain=?3, fit=?4, fit_evaluated=?5, bucket='queue'
         WHERE id=?1",
    )?;
    let mut candidates = Vec::with_capacity(questions.len());
    for (id, title, tags, source) in questions {
        let (domain, sub, fit) = classify(&title, &tags, &terms);
        update.execute(params![id, domain, sub, fit, !terms.is_empty()])?;
        if !matches!(source.as_str(), "imported" | "discovered") { candidates.push((id, fit)); }
    }
    prioritize(conn, &mut candidates)?;
    Ok(())
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

pub async fn start_sync(state: &crate::state::Shared, days: Option<i64>, select_only: bool) -> String {
    let message = if days.is_some() {
        "正在准备最近 3 天通知"
    } else {
        "正在连接知乎"
    };
    let job = job_start(state, "sync", None, message).await;
    let state = state.clone();
    let job_id = job.clone();
    tokio::spawn(async move {
        let result = sync_inner(&state, &job_id, days, select_only, None).await;
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

pub async fn sync_after_login(state: &crate::state::Shared, expected: Option<&str>) {
    let (account, session, client, secret) = {
        let mut last = state.login_sync_session.lock().await;
        let db = state.db.lock().await;
        let Ok((account, session)) = db.conn.query_row(
            "SELECT zhihu_id, oauth_session FROM active_account", [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ) else { return; };
        if *last == session || expected.is_some_and(|expected| expected != session) {
            return;
        }
        let Ok(client) = crate::state::zhihu_client_from_db(&db) else { return; };
        let settings = ZhihuOpenSettings::load(&db).unwrap_or_default();
        let secret = settings.profile_enabled(&account).then_some(settings.secret);
        last.clone_from(&session);
        (account, session, client, secret)
    };
    if let Err(error) = crate::creator::start_answer_sync(state, Some(&account)).await {
        tracing::debug!("登录后未启动回答同步：{error}");
    }
    let job = job_start(state, "sync", None, "登录后同步知乎问题").await;
    let result = async {
        if client.require_web_session().is_ok() {
            if let Ok(report) = sync_inner(state, &job, Some(3), true, Some(&session)).await {
                return Ok(report.message);
            }
        }
        let official_error = if let Some(secret) = secret {
            match sync_recommended_questions(state, &job, &session, &account, &secret).await {
                Ok(message) => return Ok(message),
                Err(error) => Some(error),
            }
        } else { None };
        let terms = {
            let db = state.db.lock().await;
            ensure_current_session(&db.conn, &session)?;
            load_terms(&db.conn)?
        };
        if terms.is_empty() {
            return Err(official_error.unwrap_or_else(|| anyhow::anyhow!("请配置当前账号的知乎 Access Secret，或添加擅长领域后获取公开推荐")));
        }
        let (_, token, _) = account_client(state).await?;
        discover_questions_inner(state, &client, &job, &account, &token, terms, String::new()).await
    }.await;
    match result {
        Ok(message) => job_end(state, &job, true, &message).await,
        Err(error) => job_end(state, &job, false, &format!("同步问题未完成：{error}")).await,
    }
}

fn ensure_current_session(conn: &rusqlite::Connection, session: &str) -> Result<()> {
    let active: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM active_account WHERE oauth_session=?1)",
        [session], |row| row.get(0))?;
    anyhow::ensure!(active, "知乎账号已切换");
    Ok(())
}

async fn sync_recommended_questions(state: &AppState, job: &str, session: &str, account: &str, secret: &str) -> Result<String> {
    job_step(state, job, "同步推荐", 0.4, "正在读取知乎推荐问题").await;
    let result = crate::zhihu_open::get("user/question_recommendations", secret, &[("Count", "10")]).await?;
    let items = result["Items"].as_array().ok_or_else(|| anyhow::anyhow!("知乎没有返回推荐列表"))?;
    let mut db = state.db.lock().await;
    let tx = db.conn.transaction()?;
    ensure_current_session(&tx, session)?;
    let terms = load_terms(&tx)?;
    let mut added = 0;
    for item in items {
        let Some(url) = item["Url"].as_str() else { continue; };
        let Ok(SharedLink::Question(id)) = parse_shared_link(url) else { continue; };
        let title = strip_html(item["Title"].as_str().unwrap_or_default());
        if title.is_empty() || is_answered(&tx, account, &id)? || existing_question(&tx, &id)?.is_some() {
            continue;
        }
        let raw = RawQuestion {
            zhihu_id: id.clone(), question_id: id.clone(), title,
            url: format!("https://www.zhihu.com/question/{id}"), source: "discovered".into(),
            excerpt: String::new(), asker_name: String::new(), asker_headline: String::new(),
            answer_count: 0, follower_count: 0, tags: vec![], metrics: None,
        };
        insert_question(&tx, &raw, 0, &terms)?;
        added += 1;
    }
    tx.commit()?;
    Ok(format!("已同步知乎推荐，新增 {added} 个问题"))
}

async fn run_sync_window(state: &crate::state::Shared, days: Option<i64>) -> Result<SyncReport> {
    let job = job_start(state, "sync", None, "正在连接知乎").await;
    let result = sync_inner(state, &job, days, false, None).await;
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
    select_only: bool,
    expected_session: Option<&str>,
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
    let (account_id, token, client) = account_client(state).await?;
    if let Some(session) = expected_session {
        let db = state.db.lock().await;
        ensure_current_session(&db.conn, session)?;
    }
    client.require_web_session()?;
    let batch = match days {
        Some(days) => client.fetch_invites_since(days).await?,
        None => client.fetch_invites(20).await?,
    };
    let received = batch.received;
    let raws = batch.questions;
    job_step(state, job, "排除已答", 0.32, "正在更新本机答题记录").await;
    let history_note = sync_answer_history(state, &client, &account_id, &token).await;

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
    let (added, selected_ids) = {
        let mut db = state.db.lock().await;
        let tx = db.conn.transaction()?;
        if let Some(session) = expected_session {
            ensure_current_session(&tx, session)?;
        }
        anyhow::ensure!(account_identity(&tx)?.0 == account_id, "知乎账号已切换，请重试");
        let terms = load_terms(&tx)?;
        let mut inserted = Vec::new();
        for rq in &raws {
            if !is_answered(&tx, &account_id, &rq.question_id)? && existing_question(&tx, &rq.question_id)?.is_none() {
                inserted.push(insert_question(&tx, rq, 0, &terms)?);
            }
        }
        let selected_ids = prioritize(&tx, &mut inserted)?;
        tx.execute(
            "UPDATE account SET updated_at = ?1 WHERE id = 1",
            params![now_iso()],
        )?;
        tx.commit()?;
        (inserted.len(), selected_ids)
    };

    if let Err(error) = crate::creator::start_question_metrics(state, true).await {
        tracing::debug!("机会数据同步未启动：{error}");
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
        message: (if added == 0 {
            format!(
                "读取 {received} 条通知，识别 {} 条邀请/推荐，没有新的问题",
                raws.len()
            )
        } else {
            format!(
                "读取 {received} 条通知，新增 {added} 个问题，{} 个进入重点队列",
                selected_ids.len()
            )
        }) + history_note,
    };

    if !select_only {
        let auto_draft = {
            let db = state.db.lock().await;
            account_identity(&db.conn).is_ok_and(|(id, _)| id == account_id)
                && load_settings(&db).writing_mode == "automatic" && !onboarding_required(&db.conn)?
        };
        if auto_draft {
            for id in selected_ids {
                let st = state.clone();
                tokio::spawn(async move { let _ = start_research(&st, &id, None, true).await; });
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
        }
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
    let (url_token, account_id, open_settings, client) = {
        let db = state.db.lock().await;
        let (account_id, token) = account_identity(&db.conn)?;
        (token, account_id, ZhihuOpenSettings::load(&db)?, crate::state::zhihu_client_from_db(&db)?)
    };

    job_step(state, job, "获取创作样本", 0.16, "正在读取账号的公开创作").await;
    let (answers, articles) = if client.require_web_session().is_ok() && !url_token.is_empty() {
        let (answers, articles) = tokio::join!(client.my_answers(&url_token, 3), client.my_articles(&url_token, 2));
        let answers = answers.unwrap_or_default();
        remember_answers(&state.db.lock().await.conn, &account_id, &answers)?;
        (answers, articles.unwrap_or_default())
    } else {
        (Vec::new(), Vec::new())
    };
    let mut samples: Vec<Value> = answers
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

    let official = crate::oauth::profile_samples(state, &account_id).await;
    let official_error = official.as_ref().err().map(ToString::to_string);
    if let Ok(official) = official {
        samples.extend(official);
    }
    if open_settings.profile_enabled(&account_id) {
        job_step(state, job, "整理样本", 0.46, "补充知乎开放平台公开创作摘要").await;
        let payload = json!({ "zhihuAccessSecret": open_settings.secret });
        if let Ok(official) = agent_stage(state, "/v1/zhihu-open/profile", &payload, 14).await {
            let mut seen = samples.iter().map(|sample| (
                sample["kind"].as_str().unwrap_or_default().to_owned(),
                sample["title"].as_str().unwrap_or_default().trim().to_owned(),
            )).collect::<std::collections::HashSet<_>>();
            for (kind, limit) in [("answer", 12), ("article", 8)] {
                if let Some(items) = official[kind].as_array() {
                    samples.extend(items.iter().filter(|sample| {
                        let title = sample["title"].as_str().unwrap_or_default().trim();
                        seen.insert((kind.to_owned(), title.to_owned()))
                    }).take(limit).cloned());
                }
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    samples.retain(|sample| {
        seen.insert((sample["kind"].as_str().unwrap_or_default().to_owned(),
                     sample["title"].as_str().unwrap_or_default().trim().to_owned()))
    });
    let sample = samples.len();
    if sample == 0 {
        if let Some(error) = official_error {
            anyhow::bail!("未获取到公开创作样本：{error}");
        }
    }
    anyhow::ensure!(sample > 0, "未获取到公开创作样本，请确认账号有公开内容且已授权读取");

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
    let style = resp.get("style").cloned().unwrap_or_else(|| json!({}));

    job_step(state, job, "更新契合度", 0.84, "正在按新画像重算待阅问题").await;
    {
        let mut db = state.db.lock().await;
        let tx = db.conn.transaction()?;
        let current_account: String =
            tx.query_row("SELECT zhihu_id FROM active_account", [], |row| {
                row.get(0)
            })?;
        anyhow::ensure!(
            current_account == account_id && !account_id.is_empty(),
            "分析期间账号已切换，未保存文风样本"
        );
        // 用户手动锁定的词条保留
        let locked: Vec<String> = {
            let mut stmt = tx
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
        tx.execute("DELETE FROM profile_terms WHERE locked = 0", [])?;

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
            tx.execute(
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
            )?;
        }

        tx.execute(
            "UPDATE profile_meta SET status='ready', summary=?1, evidence=?2, sample_size=?3, style=?4, updated_at=?5 WHERE id=1",
            params![
                summary,
                serde_json::to_string(&evidence).unwrap_or_else(|_| "[]".into()),
                sample as i64,
                style.to_string(),
                now_iso()
            ],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO writing_samples (account_id, samples) VALUES (?1, ?2)",
            params![
                account_id,
                resp.get("samples")
                    .cloned()
                    .unwrap_or(json!([]))
                    .to_string()
            ],
        )?;
        refresh_inbox_fit(&tx)?;
        tx.commit()?;
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
    let terms = load_terms(&state.db.lock().await.conn)?;
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
pub fn save_direction(conn: &rusqlite::Connection, id: &str, text: &str) -> Result<()> {
    anyhow::ensure!(text.chars().count() <= 6000, "答题思路不能超过 6000 字");
    let previous: Option<String> = conn.query_row("SELECT text FROM author_directions WHERE question_id=?1", [id], |row| row.get(0)).optional()?;
    if previous.as_deref().unwrap_or_default() != text {
        conn.execute("INSERT INTO author_directions VALUES (?1,?2) ON CONFLICT(question_id) DO UPDATE SET text=excluded.text", params![id, text])?;
        conn.execute("DELETE FROM research_checkpoints WHERE question_id=?1 AND drafted=0", [id])?;
    }
    Ok(())
}

pub async fn start_research(state: &crate::state::Shared, question_id: &str, direction: Option<&str>, automatic: bool) -> Result<String> {
    let job = {
        let mut db = state.db.lock().await;
        let settings = load_settings(&db);
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
        let (status, bucket, source, answered): (String, String, String, bool) = tx
            .query_row(
                &format!("SELECT status, bucket, source, {IS_ANSWERED} FROM questions q WHERE id=?1"),
                params![question_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|_| anyhow::anyhow!("问题不存在"))?;
        let checkpoint = tx
            .query_row(
                "SELECT drafted FROM research_checkpoints WHERE question_id=?1",
                params![question_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
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
            !has_draft || checkpoint == Some(true),
            "已有正文。为避免覆盖手工改动，不会重新生成。"
        );
        anyhow::ensure!(!answered, "该问题已回答，无需再次拟稿");
        let suggestion = bucket == "selected" && !matches!(source.as_str(), "imported" | "discovered");
        let saved_direction: String = tx.query_row("SELECT text FROM author_directions WHERE question_id=?1", [question_id],
            |row| row.get(0)).optional()?.unwrap_or_default();
        if automatic {
            anyhow::ensure!(settings.writing_mode == "automatic" && suggestion && !onboarding_required(&tx)?
                && saved_direction.trim().is_empty() && checkpoint.is_none(), "该问题等待作者确认后再拟稿");
        }
        if !has_draft {
            let text = direction.unwrap_or(&saved_direction);
            anyhow::ensure!(settings.writing_mode != "subjective" || !suggestion || !text.trim().is_empty(),
                "主观模式下，请先写一段自己的判断和解答方向");
            if let Some(text) = direction { save_direction(&tx, question_id, text)?; }
        }
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
    let mut research_payload;
    let payload = if matches!(path, "/v1/research/collect" | "/v1/research/qualify" | "/v1/research/review") {
        let settings = ZhihuOpenSettings::load(&*state.db.lock().await)?;
        if settings.enabled && !settings.secret.is_empty() {
            research_payload = payload.clone();
            research_payload["zhihuAccessSecret"] = json!(settings.secret);
            &research_payload
        } else {
            payload
        }
    } else {
        payload
    };
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
) -> Result<(usize, Vec<String>)> {
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
        65,
    )
    .await?;
    let mut db = state.db.lock().await;
    let tx = db.conn.transaction()?;
    let mut added = 0;
    let mut suggested = Vec::new();
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
        let inserted = tx.execute(
            "INSERT INTO images (id, question_id, filename, source_url, source_name, caption, bytes, created_at)
             SELECT ?1,?2,?3,?4,?5,?6,?7,?8 WHERE NOT EXISTS
             (SELECT 1 FROM images WHERE question_id=?2 AND filename=?3)",
            params![format!("i_{}", Uuid::new_v4().simple()), qid, filename,
                image["sourceUrl"].as_str().unwrap_or_default(), image["sourceName"].as_str().unwrap_or_default(),
                image["caption"].as_str().unwrap_or_default(), metadata.len() as i64, now_iso()],
        )?;
        added += inserted;
        if inserted > 0 && image["autoInsert"] == true && suggested.len() < 2 {
            suggested.push(filename.to_string());
        }
    }
    tx.commit()?;
    Ok((added, suggested))
}

fn append_draft_illustrations(markdown: &str, filenames: &[String]) -> String {
    if filenames.is_empty() {
        return markdown.to_string();
    }
    let images = filenames
        .iter()
        .map(|filename| format!("[IMG:{filename}]"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let heading = "\n\n### 参考资料\n";
    match markdown.rsplit_once(heading) {
        Some((body, references)) => format!("{body}\n\n{images}{heading}{references}"),
        None => format!("{markdown}\n\n{images}"),
    }
}

async fn research_inner(state: &AppState, qid: &str, job: &str) -> Result<()> {
    let (question, style, sources, model, settings) = {
        let db = state.db.lock().await;
        let q = db
            .conn
            .query_row(
                "SELECT id, zhihu_id, question_id, source, title, excerpt, url, domain, sub_domain, fit,
                        status, bucket, answer_count, follower_count, asker_name, asker_headline, tags, created_at, synced_at, fit_evaluated
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
                        fit_evaluated: r.get(19)?,
                        status: r.get(10)?,
                        bucket: r.get(11)?,
                        answer_count: r.get(12)?,
                        follower_count: r.get(13)?,
                        metrics: None,
                        metrics_error: String::new(),
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

        let style: Value = db
            .conn
            .query_row("SELECT style FROM profile_meta WHERE id=1", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
            .filter(|v| v.is_object())
            .unwrap_or_else(|| json!({}));

        let src = research_sources(&db)?;

        (q, style, src, load_model(&db), load_settings(&db))
    };

    let img_dir = state.data_dir.join("images").join(crate::db::sanitize(qid));
    let (checkpoint, direction) = {
        let db = state.db.lock().await;
        let checkpoint = db
            .conn
            .query_row(
                "SELECT material, drafted FROM research_checkpoints WHERE question_id=?1",
                params![qid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()?;
        (
            checkpoint,
            db.conn.query_row("SELECT text FROM author_directions WHERE question_id=?1", [qid],
                |row| row.get::<_, String>(0)).optional()?.unwrap_or_default(),
        )
    };
    let mut payload = json!({
        "question": question,
        "direction": direction,
        "profile": { "style": style },
        "sources": sources,
        "model": model,
        "settings": settings,
        "imagesDir": img_dir.to_string_lossy(),
    });
    let (mut material, drafted) = match checkpoint {
        Some((material, drafted)) => (serde_json::from_str(&material)?, drafted),
        None => {
            job_step(state, job, "取材", 0.18, "搜索资料并筛选可引用正文").await;
            let material = agent_stage(state, "/v1/research/collect", &payload, 55).await?;
            let db = state.db.lock().await;
            db.conn.execute(
                "INSERT INTO research_checkpoints (question_id, material) VALUES (?1,?2)",
                params![qid, material.to_string()],
            )?;
            (material, false)
        }
    };
    if !drafted {
        if material.get("findings").is_none() {
            job_stage(
                state,
                job,
                "查证",
                0.35,
                "逐项核对问题前提，补查缺失或冲突的证据",
                Some(settings.verify_timeout_seconds),
            )
            .await;
            payload["material"] = material.clone();
            match agent_stage(state, "/v1/research/qualify", &payload, settings.verify_timeout_seconds as u64).await {
                Ok(result) => material = result,
                Err(error) => {
                    let mut warnings = material["warnings"].as_array().cloned().unwrap_or_default();
                    warnings.push(json!(format!("前置核查未完成：{error} 初稿将按已取得的资料限定结论。")));
                    material["warnings"] = json!(warnings);
                }
            }
            state.db.lock().await.conn.execute(
                "UPDATE research_checkpoints SET material=?2 WHERE question_id=?1",
                params![qid, material.to_string()],
            )?;
        }
        payload["material"] = material.clone();
        job_step(
            state,
            job,
            "拟稿与找图",
            0.55,
            "根据问题和资料起草，再通读整理全文。配图优先取自原文",
        )
        .await;
        let (draft_result, images) = tokio::join!(
            agent_stage(state, "/v1/research/draft", &payload, 205),
            async {
                if material.get("autoImageFiles").is_some() || settings.image_max == 0 {
                    return Ok(None);
                }
                collect_question_images(state, qid, payload.clone(), false, false)
                    .await
                    .map(Some)
            },
        );
        let mut image_warning = None;
        match images {
            Ok(Some((added, suggested))) => {
                material["autoImageFiles"] = json!(suggested);
                if added == 0 {
                    image_warning = Some("没有取得可用图片；可从引用与核查来源重新抓取，或换词补搜。");
                } else if suggested.is_empty() {
                    image_warning = Some("备选图已保存；来源或画面仍需目测，未自动插入正文。");
                }
            }
            Err(_) => image_warning = Some("配图搜索未完成，初稿仍可编辑；可在写作台重新搜索。"),
            Ok(None) => {}
        }
        if let Some(message) = image_warning {
            if let Some(warnings) = material["warnings"].as_array_mut() {
                warnings.push(json!(message));
            }
        }
        state.db.lock().await.conn.execute(
            "UPDATE research_checkpoints SET material=?2 WHERE question_id=?1",
            params![qid, material.to_string()],
        )?;
        let mut response = draft_result?;
        if let (Some(message), Some(warnings)) =
            (image_warning, response["warnings"].as_array_mut())
        {
            warnings.push(json!(message));
        }
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
        let suggested: Vec<String> =
            serde_json::from_value(material["autoImageFiles"].clone()).unwrap_or_default();
        let markdown = append_draft_illustrations(markdown, &suggested);
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
    payload["material"] = material;

    let raw_doubts: String = state.db.lock().await.conn.query_row(
        "SELECT doubts FROM drafts WHERE question_id=?1",
        params![qid],
        |row| row.get(0),
    )?;
    payload["doubts"] = serde_json::from_str(&raw_doubts)?;
    job_stage(
        state,
        job,
        "补充核查",
        0.76,
        "正文已保存并可编辑；正在联网复核剩余说法",
        Some(settings.verify_timeout_seconds),
    )
    .await;
    let review = agent_stage(state, "/v1/research/review", &payload, settings.verify_timeout_seconds as u64).await;
    let mut db = state.db.lock().await;
    let tx = db.conn.transaction()?;
    if let Ok(review) = &review {
        tx.execute(
            "UPDATE drafts SET doubts=?2 WHERE question_id=?1",
            params![qid, review["doubts"].to_string()],
        )?;
    }
    if review.is_ok() {
        tx.execute(
            "DELETE FROM research_checkpoints WHERE question_id=?1",
            params![qid],
        )?;
    }
    tx.commit()?;
    if let Err(error) = review {
        anyhow::bail!("正文已保存，核查未完成：{error}");
    }
    Ok(())
}

// ---------------- 配置读取 ----------------

pub fn research_sources(db: &crate::db::Db) -> Result<Vec<Source>> {
    let mut statement = db.conn.prepare(
        "SELECT id, domain, name, base_url, kind, trust, enabled, notes, use_for, discovered_by, ok FROM sources ORDER BY trust DESC, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(Source {
            id: row.get(0)?,
            domain: row.get(1)?,
            name: row.get(2)?,
            base_url: row.get(3)?,
            kind: row.get(4)?,
            trust: row.get(5)?,
            enabled: row.get::<_, i64>(6)? != 0,
            notes: row.get(7)?,
            use_for: serde_json::from_str(&row.get::<_, String>(8)?).unwrap_or_default(),
            discovered_by: row.get(9)?,
            ok: row.get::<_, i64>(10)? != 0,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn load_settings(db: &crate::db::Db) -> Settings {
    db.conn
        .query_row(
            "SELECT cron_times, pick_ratio, draft_min, draft_max, image_min, image_max,
                    auto_sync, minimize_to_tray, auto_launch, sign_mode, verify_timeout_seconds, writing_mode FROM settings WHERE id=1",
            [],
            |r| {
                Ok(Settings {
                    writing_mode: r.get(11)?,
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
                    verify_timeout_seconds: r.get::<_, i64>(10)?.clamp(30, 180),
                    data_dir: db.data_dir.to_string_lossy().to_string(),
                })
            },
        )
        .unwrap_or(Settings {
            writing_mode: "automatic".into(),
            cron_times: vec!["07:00".into(), "15:00".into(), "20:00".into()],
            pick_ratio: 0.4,
            draft_min: 500,
            draft_max: 1000,
            image_min: 3,
            image_max: 10,
            verify_timeout_seconds: 180,
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
