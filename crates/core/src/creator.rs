use anyhow::{anyhow, ensure, Result};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api::R,
    models::now_iso,
    state::{zhihu_client_from_db, Shared, ZhihuOpenSettings},
    tasks::{account_client, account_identity, job_end, job_start, job_step, IS_ANSWERED},
    zhihu::{
        as_str, count_field, is_content_id, parse_shared_link, strip_html, SharedLink, ZhihuClient,
    },
};

const ANSWER_SYNC: &str = "creator_answers";
const QUESTION_SYNC: &str = "question_metrics";
const ANSWER_PAGE_SIZE: i64 = 20;
const QUESTION_REFRESH_LIMIT: usize = 500;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AnswerMetrics {
    reads: Option<i64>,
    votes: Option<i64>,
    comments: Option<i64>,
    favorites: Option<i64>,
    likes: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreatorAnswer {
    id: String,
    question_id: String,
    title: String,
    excerpt: String,
    created_time: Option<i64>,
    updated_time: Option<i64>,
    metrics: AnswerMetrics,
}

fn parse_answer(row: &Value) -> Result<CreatorAnswer> {
    let data = &row["data"];
    let id = as_str(data, &["id"]);
    let question_id = as_str(data, &["question_id"]);
    let title = as_str(data, &["title"]);
    ensure!(
        [&id, &question_id]
            .iter()
            .all(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit())),
        "知乎创作中心返回了无法识别的回答记录"
    );
    let reaction = &row["reaction"];
    Ok(CreatorAnswer {
        id,
        question_id,
        title: if title.trim().is_empty() {
            "知乎回答".to_owned()
        } else {
            title
        },
        excerpt: strip_html(&as_str(data, &["excerpt"])),
        created_time: count_field(data, &["created_time"]).filter(|time| *time > 0),
        updated_time: count_field(data, &["updated_time"]).filter(|time| *time > 0),
        metrics: AnswerMetrics {
            reads: count_field(reaction, &["read_count"]),
            votes: count_field(reaction, &["vote_up_count", "voteup_count"]),
            comments: count_field(reaction, &["comment_count"]),
            favorites: count_field(reaction, &["collect_count", "favlists_count"]),
            likes: count_field(reaction, &["like_count"]),
        },
    })
}

fn running_job(conn: &Connection, kind: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT id FROM jobs WHERE kind=?1 AND state='running' ORDER BY started_at DESC LIMIT 1",
        [kind],
        |row| row.get(0),
    )
    .optional()
}

#[derive(Deserialize)]
pub(crate) struct AnswerQuery {
    offset: Option<i64>,
    q: Option<String>,
}

pub(crate) async fn list_answers(State(st): State<Shared>, Query(query): Query<AnswerQuery>) -> R {
    let db = st.db.lock().await;
    let (account, _) = account_identity(&db.conn)?;
    let q = query
        .q
        .as_deref()
        .unwrap_or("")
        .trim()
        .chars()
        .take(200)
        .collect::<String>();
    let offset = query.offset.unwrap_or(0).max(0);
    let filter =
        "FROM creator_answers WHERE account_id=?1 AND (instr(title, ?2)>0 OR instr(excerpt, ?2)>0)";
    let total: i64 = db.conn.query_row(
        &format!("SELECT COUNT(*) {filter}"),
        params![account, q],
        |row| row.get(0),
    )?;
    let mut stmt = db.conn.prepare(&format!(
        "SELECT answer_id, question_id, title, excerpt, created_time, updated_time, metrics {filter}
         ORDER BY created_time DESC, answer_id DESC LIMIT ?3 OFFSET ?4"
    ))?;
    let answers = stmt
        .query_map(params![account, q, ANSWER_PAGE_SIZE, offset], |row| {
            let metrics: String = row.get(6)?;
            let metrics = serde_json::from_str(&metrics).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    6,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?;
            Ok(CreatorAnswer {
                id: row.get(0)?,
                question_id: row.get(1)?,
                title: row.get(2)?,
                excerpt: row.get(3)?,
                created_time: row.get(4)?,
                updated_time: row.get(5)?,
                metrics,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let synced_at: Option<String> = db
        .conn
        .query_row(
            "SELECT synced_at FROM creator_answer_sync WHERE account_id=?1",
            [account],
            |row| row.get(0),
        )
        .optional()?;
    Ok(Json(json!({
        "answers": answers, "total": total, "pageSize": ANSWER_PAGE_SIZE, "syncedAt": synced_at,
        "jobId": running_job(&db.conn, ANSWER_SYNC)?,
    })))
}

pub(crate) async fn sync_answers(State(st): State<Shared>) -> R {
    let job = start_answer_sync(&st, None).await?;
    Ok(Json(json!({ "jobId": job })))
}

pub(crate) async fn start_answer_sync(st: &Shared, expected: Option<&str>) -> Result<String> {
    let _lock = st.creator_jobs_lock.lock().await;
    let (account, client, access_secret) = {
        let db = st.db.lock().await;
        let (account, _) = account_identity(&db.conn)?;
        ensure!(
            expected.is_none_or(|expected| expected == account),
            "知乎账号已切换"
        );
        if let Some(job) = running_job(&db.conn, ANSWER_SYNC)? {
            return Ok(job);
        }
        let settings = ZhihuOpenSettings::load(&db)?;
        let access_secret = settings
            .profile_enabled(&account)
            .then_some(settings.secret);
        (account, zhihu_client_from_db(&db)?, access_secret)
    };
    let job = job_start(st, ANSWER_SYNC, None, "正在读取知乎作答").await;
    let state = st.clone();
    let job_id = job.clone();
    tokio::spawn(async move {
        let result = match client.require_web_session() {
            Ok(()) => sync_answers_inner(&state, &job_id, &account, &client).await,
            Err(error) => Err(error),
        };
        let result = match (result, access_secret.as_deref()) {
            (Err(_), Some(secret)) => sync_open_answers(&state, &job_id, &account, secret).await,
            (Err(_), None) if client.cookie.trim().is_empty() => Err(anyhow!(
                "请在设置中启用当前账号的知乎 Access Secret，以同步历史作答"
            )),
            (result, _) => result,
        };
        if let Some(secret) = access_secret {
            if let Err(error) = refresh_account_activity(&state, &account, &secret).await {
                tracing::debug!("创作统计更新未完成：{error}");
            }
        }
        match result {
            Ok(count) => {
                job_end(
                    &state,
                    &job_id,
                    true,
                    &format!("已同步 {count} 条回答及可用互动数据"),
                )
                .await
            }
            Err(error) => job_end(&state, &job_id, false, &format!("同步回答失败：{error}")).await,
        }
    });
    Ok(job)
}

async fn sync_answers_inner(
    st: &Shared,
    job: &str,
    account: &str,
    client: &ZhihuClient,
) -> Result<usize> {
    let mut offset = Some(0);
    let mut answers = std::collections::HashMap::new();
    while let Some(current) = offset {
        let (rows, next) = client.creator_answers_page(current).await?;
        for row in &rows {
            let answer = parse_answer(row)?;
            answers.entry(answer.id.clone()).or_insert(answer);
        }
        {
            let db = st.db.lock().await;
            ensure!(
                account_identity(&db.conn)?.0 == account,
                "知乎账号已切换，请重新同步"
            );
        }
        let progress = 0.1 + 0.8 * (answers.len() as f64 / (answers.len() as f64 + 100.0));
        job_step(
            st,
            job,
            "同步作答",
            progress,
            &format!("已读取 {} 条回答", answers.len()),
        )
        .await;
        offset = next;
    }
    let mut db = st.db.lock().await;
    ensure!(
        account_identity(&db.conn)?.0 == account,
        "知乎账号已切换，请重新同步"
    );
    save_answers(&mut db.conn, account, answers.values(), true)?;
    Ok(answers.len())
}

fn parse_open_answer(row: &Value) -> Result<CreatorAnswer> {
    let url = reqwest::Url::parse(row["Url"].as_str().unwrap_or_default())?;
    let link = parse_shared_link(url.as_str())?;
    let path = url.path().trim_end_matches('/');
    let id = path
        .rsplit('/')
        .next()
        .filter(|id| is_content_id(id))
        .ok_or_else(|| anyhow!("知乎没有返回有效的回答编号"))?;
    ensure!(
        row["ContentType"] == "answer" && path.contains("/answer/"),
        "开放平台返回了非回答内容"
    );
    Ok(CreatorAnswer {
        id: id.to_string(),
        question_id: match link {
            SharedLink::Question(id) => id,
            SharedLink::Answer(_) => String::new(),
        },
        title: row["Title"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or("知乎回答")
            .to_owned(),
        excerpt: strip_html(row["Summary"].as_str().unwrap_or_default()),
        created_time: count_field(row, &["CreatedAt"]).filter(|time| *time > 0),
        updated_time: None,
        metrics: AnswerMetrics {
            reads: None,
            votes: count_field(row, &["LikeCount"]),
            comments: count_field(row, &["CommentCount"]),
            favorites: count_field(row, &["FavoriteCount"]),
            likes: None,
        },
    })
}

async fn sync_open_answers(st: &Shared, job: &str, account: &str, secret: &str) -> Result<usize> {
    let mut offset = 0u64;
    let mut answers = std::collections::HashMap::new();
    for _ in 0..50 {
        let page = crate::zhihu_open::get(
            "user/contents",
            secret,
            &[
                ("ContentType", "answer"),
                ("Limit", "50"),
                ("Offset", &offset.to_string()),
            ],
        )
        .await?;
        let rows = page["Items"]
            .as_array()
            .ok_or_else(|| anyhow!("知乎没有返回回答列表"))?;
        for row in rows {
            let answer = parse_open_answer(row)?;
            answers.entry(answer.id.clone()).or_insert(answer);
        }
        {
            let db = st.db.lock().await;
            ensure!(account_identity(&db.conn)?.0 == account, "知乎账号已切换");
        }
        job_step(
            st,
            job,
            "同步作答",
            0.1 + 0.8 * (answers.len() as f64 / (answers.len() as f64 + 100.0)),
            &format!("已读取 {} 条回答", answers.len()),
        )
        .await;
        if page["Paging"]["IsEnd"] == true {
            let mut db = st.db.lock().await;
            ensure!(account_identity(&db.conn)?.0 == account, "知乎账号已切换");
            save_answers(&mut db.conn, account, answers.values(), false)?;
            return Ok(answers.len());
        }
        offset = page["Paging"]["NextOffset"]
            .as_str()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|next| *next > offset && !rows.is_empty())
            .ok_or_else(|| anyhow!("知乎回答分页异常"))?;
    }
    anyhow::bail!("知乎回答分页超过本次同步上限")
}

async fn refresh_account_activity(st: &Shared, account: &str, secret: &str) -> Result<()> {
    let end = chrono::Local::now().date_naive();
    let start = (end - chrono::Duration::days(29)).to_string();
    let end = end.to_string();
    let stats = crate::zhihu_open::get(
        "user/creator_account_stats",
        secret,
        &[
            ("ContentType", "all"),
            ("StartDate", &start),
            ("EndDate", &end),
        ],
    )
    .await?;
    let votes = count_field(&stats["Metrics"], &["IncreasedUpvoteCount"])
        .ok_or_else(|| anyhow!("知乎没有返回该时间段的新增赞同"))?;
    let db = st.db.lock().await;
    ensure!(account_identity(&db.conn)?.0 == account, "知乎账号已切换");
    db.conn.execute(
        "INSERT OR REPLACE INTO creator_account_activity (account_id, end_date, votes) VALUES (?1,?2,?3)",
        params![account, end, votes],
    )?;
    Ok(())
}

fn save_answers<'a>(
    conn: &mut Connection,
    account: &str,
    answers: impl IntoIterator<Item = &'a CreatorAnswer>,
    complete: bool,
) -> Result<()> {
    let tx = conn.transaction()?;
    let synced = now_iso();
    {
        let mut insert = tx.prepare(
            "INSERT INTO creator_answers (account_id, answer_id, question_id, title, excerpt, created_time, updated_time, metrics, synced_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(account_id, answer_id) DO UPDATE SET
                question_id=COALESCE(NULLIF(excluded.question_id,''),creator_answers.question_id), title=excluded.title,
                excerpt=excluded.excerpt, created_time=COALESCE(excluded.created_time,creator_answers.created_time),
                updated_time=COALESCE(excluded.updated_time,creator_answers.updated_time),
                metrics=json_object(
                    'reads', COALESCE(json_extract(excluded.metrics,'$.reads'),json_extract(creator_answers.metrics,'$.reads')),
                    'votes', COALESCE(json_extract(excluded.metrics,'$.votes'),json_extract(creator_answers.metrics,'$.votes')),
                    'comments', COALESCE(json_extract(excluded.metrics,'$.comments'),json_extract(creator_answers.metrics,'$.comments')),
                    'favorites', COALESCE(json_extract(excluded.metrics,'$.favorites'),json_extract(creator_answers.metrics,'$.favorites')),
                    'likes', COALESCE(json_extract(excluded.metrics,'$.likes'),json_extract(creator_answers.metrics,'$.likes'))
                ), synced_at=excluded.synced_at,
                content=CASE WHEN excluded.updated_time IS NULL OR excluded.updated_time IS creator_answers.updated_time THEN creator_answers.content END",
        )?;
        let mut remember = tx.prepare(
            "INSERT OR IGNORE INTO answered_questions SELECT account_id, question_id
            FROM creator_answers WHERE account_id=?1 AND answer_id=?2 AND question_id<>''",
        )?;
        for answer in answers {
            insert.execute(params![
                account,
                answer.id,
                answer.question_id,
                answer.title,
                answer.excerpt,
                answer.created_time,
                answer.updated_time,
                serde_json::to_string(&answer.metrics)?,
                synced
            ])?;
            remember.execute(params![account, answer.id])?;
        }
    }
    if complete {
        tx.execute(
            "DELETE FROM creator_answers WHERE account_id=?1 AND synced_at<>?2",
            params![account, synced],
        )?;
    }
    tx.execute(
        "INSERT OR REPLACE INTO creator_answer_sync VALUES (?1,?2)",
        params![account, synced],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) async fn get_answer(State(st): State<Shared>, Path(id): Path<String>) -> R {
    let (account, updated, client, secret) = {
        let db = st.db.lock().await;
        let (account, _) = account_identity(&db.conn)?;
        let (content, truncated, updated): (Option<String>, bool, Option<i64>) = db.conn.query_row(
            "SELECT content, truncated, updated_time FROM creator_answers WHERE account_id=?1 AND answer_id=?2",
            params![account, id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?.ok_or_else(|| anyhow!("本账号没有这条回答记录，请先同步作答"))?;
        if let Some(content) = content {
            return Ok(Json(json!({ "content": content, "truncated": truncated })));
        }
        let settings = ZhihuOpenSettings::load(&db)?;
        let secret = settings
            .profile_enabled(&account)
            .then_some(settings.secret);
        (account, updated, zhihu_client_from_db(&db)?, secret)
    };
    if !is_content_id(&id) {
        return Err(anyhow!("无效的回答编号").into());
    }
    let (content, truncated) = match (secret, client.creator_answer_content(&id).await) {
        (_, Ok(value)) => value,
        (Some(secret), Err(_)) => {
            let url = format!("https://www.zhihu.com/answer/{id}");
            let data =
                crate::zhihu_open::get("user/content_detail", &secret, &[("ContentUrl", &url)])
                    .await?;
            let body = data["Body"]
                .as_str()
                .filter(|body| !body.is_empty())
                .ok_or_else(|| anyhow!("知乎未提供回答正文"))?;
            (body.to_owned(), false)
        }
        (None, Err(error)) => return Err(error.into()),
    };
    let db = st.db.lock().await;
    if account_identity(&db.conn)?.0 != account {
        return Err(anyhow!("知乎账号已切换，请重新打开回答").into());
    }
    let saved = db.conn.execute(
        "UPDATE creator_answers SET content=?3, truncated=?4 WHERE account_id=?1 AND answer_id=?2 AND updated_time IS ?5",
        params![account, id, content, truncated, updated],
    )?;
    if saved != 1 {
        return Err(anyhow!("回答已在同步中更新，请重新打开").into());
    }
    Ok(Json(json!({ "content": content, "truncated": truncated })))
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct QuestionMetricsRequest {
    missing_only: bool,
}

struct QuestionTarget {
    local_ids: Vec<String>,
    question_id: String,
}

pub(crate) fn running_metrics_job(conn: &Connection) -> rusqlite::Result<Option<String>> {
    running_job(conn, QUESTION_SYNC)
}

fn question_targets(conn: &Connection, missing_only: bool) -> Result<Vec<QuestionTarget>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT q.id, q.question_id, q.url FROM questions q
         WHERE q.status IN ('inbox','researching','pending_review') AND NOT {IS_ANSWERED}
           AND (NOT ?1 OR (
             (json_extract(q.metrics, '$.views') IS NULL OR json_extract(q.metrics, '$.answers') IS NULL
                OR json_extract(q.metrics, '$.followers') IS NULL)
             AND (q.metrics_attempted_at='' OR datetime(q.metrics_attempted_at)<=datetime('now','-30 minutes'))
           ))
         ORDER BY (q.metrics IS NOT NULL), q.metrics_attempted_at, q.synced_at DESC"
    ))?;
    let rows = stmt.query_map([missing_only], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut positions = std::collections::HashMap::<String, usize>::new();
    let mut targets: Vec<QuestionTarget> = Vec::new();
    for row in rows {
        let (local_id, id, url) = row?;
        let question_id = match parse_shared_link(&url) {
            Ok(SharedLink::Question(id)) => id,
            _ if is_content_id(&id) => id,
            _ => continue,
        };
        if let Some(&index) = positions.get(&question_id) {
            targets[index].local_ids.push(local_id);
        } else if targets.len() < QUESTION_REFRESH_LIMIT {
            positions.insert(question_id.clone(), targets.len());
            targets.push(QuestionTarget {
                local_ids: vec![local_id],
                question_id,
            });
        }
    }
    Ok(targets)
}

pub(crate) async fn sync_question_metrics(
    State(st): State<Shared>,
    body: Option<Json<QuestionMetricsRequest>>,
) -> R {
    let missing_only = body.is_some_and(|Json(body)| body.missing_only);
    Ok(Json(
        json!({ "jobId": start_question_metrics(&st, missing_only).await? }),
    ))
}

pub(crate) async fn start_question_metrics(
    st: &Shared,
    missing_only: bool,
) -> Result<Option<String>> {
    let _lock = st.creator_jobs_lock.lock().await;
    let (account, _, client) = account_client(st).await?;
    let questions = {
        let db = st.db.lock().await;
        if let Some(job) = running_metrics_job(&db.conn)? {
            return Ok(Some(job));
        }
        question_targets(&db.conn, missing_only)?
    };
    if questions.is_empty() {
        return Ok(None);
    }
    let job = job_start(
        st,
        QUESTION_SYNC,
        None,
        &format!("正在查询 {} 道待答题的原题数据", questions.len()),
    )
    .await;
    let state = st.clone();
    let job_id = job.clone();
    tokio::spawn(async move {
        match refresh_question_metrics(&state, &job_id, &account, &client, &questions).await {
            Ok(message) => job_end(&state, &job_id, true, &message).await,
            Err(error) => job_end(&state, &job_id, false, &error.to_string()).await,
        }
    });
    Ok(Some(job))
}

async fn refresh_question_metrics(
    st: &Shared,
    job: &str,
    account: &str,
    client: &ZhihuClient,
    targets: &[QuestionTarget],
) -> Result<String> {
    let mut updated = 0;
    let mut attempted = 0;
    let mut last_error = None;
    let mut consecutive_failures = 0;
    for group in targets.chunks(4) {
        let mut batch = tokio::task::JoinSet::new();
        for target in group {
            let id = target.question_id.clone();
            let local_ids = serde_json::to_string(&target.local_ids)?;
            let client = client.clone();
            batch.spawn(async move {
                let result = client.question_details(&id, "inbox").await;
                (id, local_ids, result)
            });
        }
        let mut transient_failures = 0;
        while let Some(result) = batch.join_next().await {
            attempted += 1;
            let (question_id, local_ids, result) = result?;
            let (metrics, error) = match result {
                Ok((question, _, _)) => (
                    question.metrics.filter(|m| {
                        m.views.is_some() && m.answers.is_some() && m.followers.is_some()
                    }),
                    "知乎未返回完整的浏览、回答或关注数据",
                ),
                Err(error) => {
                    let message = error.to_string();
                    tracing::debug!("读取知乎问题数据失败：{error}");
                    let reason =
                        if message.starts_with("HTTP 404") || message.starts_with("HTTP 410") {
                            "知乎原问题暂不可访问"
                        } else {
                            transient_failures += 1;
                            "暂时无法读取知乎原题，可稍后重试"
                        };
                    (None, reason)
                }
            };
            let db = st.db.lock().await;
            ensure!(
                account_identity(&db.conn)?.0 == account,
                "知乎账号已切换，请重新刷新"
            );
            if let Some(metrics) = metrics {
                db.conn.execute(
                    "UPDATE questions SET question_id=?1, metrics=?3, answer_count=?4, follower_count=?5,
                        metrics_attempted_at=?6, metrics_error=''
                     WHERE question_id=?1 OR id IN (SELECT value FROM json_each(?2))",
                    params![question_id, local_ids, serde_json::to_string(&metrics)?, metrics.answers, metrics.followers, metrics.synced_at],
                )?;
                updated += 1;
            } else {
                db.conn.execute(
                    "UPDATE questions SET metrics_attempted_at=?3, metrics_error=?4
                     WHERE question_id=?1 OR id IN (SELECT value FROM json_each(?2))",
                    params![question_id, local_ids, now_iso(), error],
                )?;
                last_error = Some(error);
            }
        }
        job_step(
            st,
            job,
            "刷新机会数据",
            (attempted as f64 / targets.len() as f64).min(0.99),
            &format!(
                "已检查 {attempted}/{} 道题，已更新 {updated} 道",
                targets.len()
            ),
        )
        .await;
        consecutive_failures = if transient_failures == group.len() {
            consecutive_failures + 1
        } else {
            0
        };
        if consecutive_failures >= 3 {
            break;
        }
    }
    ensure!(
        updated > 0,
        "未能更新问题数据：{}",
        last_error.unwrap_or("知乎未返回问题数据")
    );
    Ok(if updated == targets.len() {
        format!("已更新 {updated} 个问题的机会数据")
    } else {
        format!(
            "已更新 {updated} 个问题，{} 个未更新，可稍后重试",
            targets.len() - updated
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_metrics_use_original_links_and_skip_recent_failures() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(crate::db::SCHEMA)?;
        conn.execute_batch(
            "CREATE TABLE answered_questions (account_id TEXT, question_id TEXT);
             INSERT INTO questions (id,title,question_id,url,metrics,metrics_attempted_at,status) VALUES
              ('old','旧题','','https://www.zhihu.com/question/123',NULL,'','pending_review'),
              ('alias','旧题副本','999','https://www.zhihu.com/question/123',NULL,'','inbox'),
              ('failed','访问失败','456','',NULL,datetime('now'),'inbox'),
              ('complete','已有数据','789','','{\"views\":0,\"followers\":0,\"answers\":0}','','inbox'),
              ('archived','已弃用','987','',NULL,'','dismissed');",
        )?;
        let missing = question_targets(&conn, true)?;
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].question_id, "123");
        assert_eq!(missing[0].local_ids.len(), 2);
        assert_eq!(question_targets(&conn, false)?.len(), 3);
        for id in 1000..1070 {
            conn.execute(
                "INSERT INTO questions (id,title,question_id) VALUES (?1,'旧题',?1)",
                [id.to_string()],
            )?;
        }
        assert_eq!(question_targets(&conn, true)?.len(), 71);
        Ok(())
    }

    #[test]
    fn creator_metrics_keep_zero_distinct_from_missing() -> Result<()> {
        let answer = parse_answer(&json!({
            "data": {"id": "12", "question_id": "123", "title": "问题", "excerpt": "<b>摘要</b>"},
            "reaction": {"read_count": 800, "vote_up_count": "21", "comment_count": 0, "collect_count": 7}
        }))?;
        assert_eq!(answer.excerpt, "摘要");
        assert_eq!(answer.metrics.reads, Some(800));
        assert_eq!(answer.metrics.votes, Some(21));
        assert_eq!(answer.metrics.comments, Some(0));
        assert_eq!(answer.metrics.favorites, Some(7));
        assert_eq!(answer.metrics.likes, None);
        assert!(parse_answer(&json!({"data": {"id": "12"}})).is_err());
        Ok(())
    }
}
