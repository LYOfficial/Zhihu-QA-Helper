//! 知乎网页客户端：通知拉取、公开内容查询。
//!
//! 协议参考 docs/zhihu-apis.md（整理自知乎++ / zhurl 的公开接口索引）。
//! 注意：知乎风控参数（x-zse-96）会随版本变动，本项目把它做成可切换策略，
//! 签名失败时自动降级为「仅 Cookie」请求，并在 UI 里如实提示。

use anyhow::{anyhow, Result};
use reqwest::Client;
use serde_json::Value;
use std::time::Duration;

use crate::models::{now_iso, QuestionMetrics};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const ZSE93: &str = "101_3_3.0";
const ANDROID_API_VERSION: &str = "3.0.93";
const ANDROID_APP_VERSION: &str = "10.100.0";
const ANDROID_UA: &str = "com.zhihu.android/Futureve/10.100.0 Mozilla/5.0 (Linux; Android 12; sdk_gphone64_arm64 Build/SE1A.220630.001.A1; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/57.0.1000.10 Mobile Safari/537.36";
const ANDROID_APP_ZA: &str = "OS=Android&Release=12&Model=sdk_gphone64_arm64&VersionName=10.100.0&VersionCode=30008&Product=com.zhihu.android&Width=1440&Height=2952&Installer=Market&DeviceType=AndroidPhone&Brand=google";

#[derive(Clone, Copy, PartialEq)]
pub enum SignMode {
    /// 尝试签名；401/403 时自动用不带签名的请求重试
    Auto,
    /// 始终签名
    Always,
    /// 从不签名（部分接口接受）
    Off,
}

impl SignMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "always" => SignMode::Always,
            "off" => SignMode::Off,
            _ => SignMode::Auto,
        }
    }
}

#[derive(Clone)]
pub struct ZhihuClient {
    http: Client,
    pub cookie: String,
    pub sign_mode: SignMode,
}

impl ZhihuClient {
    pub fn new(cookie: String, sign_mode: SignMode) -> Result<Self> {
        let http = Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(25))
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            http,
            cookie,
            sign_mode,
        })
    }

    fn xsrf(&self) -> Option<String> {
        self.cookie
            .split(';')
            .filter_map(|s| s.trim().split_once('='))
            .find(|(k, _)| *k == "_xsrf")
            .map(|(_, v)| v.to_string())
    }

    pub fn d_c0(&self) -> String {
        self.cookie
            .split(';')
            .filter_map(|s| s.trim().split_once('='))
            .find(|(k, _)| *k == "d_c0")
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    }

    async fn send(&self, method: reqwest::Method, url: &str, signed: bool) -> Result<Value> {
        let signed = signed && !self.cookie.is_empty() && match self.sign_mode {
            SignMode::Always => true,
            SignMode::Auto => !self.d_c0().is_empty(),
            SignMode::Off => false,
        };
        match self.send_web(method.clone(), url, signed).await {
            Err(error)
                if self.sign_mode == SignMode::Auto && signed && error.retry_without_sign =>
            {
                tracing::warn!(target: "zhihu", "签名请求被知乎拒绝，改用仅 Cookie 重试：{}", error.message);
                self.send_web(method, url, false)
                    .await
                    .map_err(|error| anyhow!(error.message))
            }
            result => result.map_err(|error| anyhow!(error.message)),
        }
    }

    async fn send_web(
        &self,
        method: reqwest::Method,
        url: &str,
        signed: bool,
    ) -> std::result::Result<Value, RequestError> {
        if signed && self.d_c0().is_empty() {
            return Err(RequestError::fatal(
                "旧版网页会话缺少签名所需的字段",
            ));
        }
        let path = request_path(url);
        let mut req = self
            .http
            .request(method, url)
            .header("Cookie", &self.cookie)
            .header("x-requested-with", "fetch")
            .header("Referer", "https://www.zhihu.com/")
            .header("Origin", "https://www.zhihu.com")
            .header("Accept", "application/json, text/plain, */*");
        if let Some(xsrf) = self.xsrf() {
            req = req.header("x-xsrftoken", xsrf);
        }
        if signed {
            for (name, value) in zhihu_sign::sign_zhihu_request(&path, &self.d_c0(), None) {
                req = req.header(name, value);
            }
        } else {
            req = req.header("x-zse-93", ZSE93);
        }
        decode_response(req.send().await.map_err(RequestError::from)?).await
    }

    /// 移动端通知接口不使用 Web 的 x-zse-96，请求头与 zhurl --android 保持一致。
    async fn send_mobile(&self, url: &str) -> Result<Value> {
        let req = self
            .http
            .get(url)
            .header("User-Agent", ANDROID_UA)
            .header("Cookie", &self.cookie)
            .header("Accept", "application/json, text/plain, */*")
            .header("x-page-id", "132")
            .header("x-api-version", ANDROID_API_VERSION)
            .header("x-app-version", ANDROID_APP_VERSION)
            .header("x-app-za", ANDROID_APP_ZA)
            .header("x-app-bundleid", "com.zhihu.android")
            .header("x-app-flavor", "zhihuwap64")
            .header("x-app-build", "release")
            .header("x-network-type", "3G");
        let response = req
            .send()
            .await
            .map_err(|error| anyhow!(error.to_string()))?;
        decode_response(response)
            .await
            .map_err(|error| anyhow!(error.message))
    }

    pub fn require_web_session(&self) -> Result<()> {
        anyhow::ensure!(!self.cookie.trim().is_empty(), "知乎 OAuth 暂未开放通知和创作中心接口。可以手动导入问题或使用问题发现。");
        Ok(())
    }

    // ---------- 通知 → 问题 ----------

    /// 拉取独立邀请时间线及三类 Web 通知里的「邀请回答」与「推荐回答」。
    pub async fn fetch_invites(&self, limit: u32) -> Result<NotificationBatch> {
        self.fetch_invites_window(limit.clamp(1, 20), None).await
    }

    /// 拉取指定天数内的邀请与推荐，首次登录时用于补齐历史通知。
    pub async fn fetch_invites_since(&self, days: i64) -> Result<NotificationBatch> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days.max(1));
        self.fetch_invites_window(20, Some(cutoff.timestamp()))
            .await
    }

    async fn fetch_invites_window(
        &self,
        limit: u32,
        cutoff: Option<i64>,
    ) -> Result<NotificationBatch> {
        let mut out: Vec<RawQuestion> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut received = 0usize;
        let mut succeeded_sources = 0usize;
        let mut errors = Vec::new();

        for kind in ["invite", "default", "follow", "vote_thank"] {
            let mobile = kind == "invite";
            let pages = if cutoff.is_some() { 100 } else { 1 };
            let mut next_url = if mobile {
                format!("https://api.zhihu.com/notifications/v3/timeline/entry/invite?invite_with_time_slice=1&limit={limit}")
            } else {
                format!("https://www.zhihu.com/api/v4/notifications/v2/{kind}?limit={limit}")
            };
            let mut visited_urls = std::collections::HashSet::new();
            let mut source_received = 0usize;
            let mut source_candidates = 0usize;
            let mut source_succeeded = false;

            for page in 0..pages {
                if !visited_urls.insert(next_url.clone()) {
                    tracing::warn!(target: "zhihu", "通知源 {kind} 的分页游标重复，停止该来源翻页");
                    break;
                }
                let url = next_url.clone();
                let response = if mobile {
                    self.send_mobile(&url).await
                } else {
                    self.send(reqwest::Method::GET, &url, true).await
                };
                let v = match response {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(target: "zhihu", "通知源 {kind} 第 {} 页拉取失败: {e}", page + 1);
                        errors.push(format!("{kind}: {e}"));
                        break;
                    }
                };
                let Some(items) = v.get("data").and_then(|d| d.as_array()) else {
                    let message = format!(
                        "通知源 {kind} 的响应缺少 data 数组：{}",
                        slim(&v.to_string(), 180)
                    );
                    tracing::warn!(target: "zhihu", "{message}");
                    errors.push(message);
                    break;
                };
                source_succeeded = true;
                received += items.len();
                source_received += items.len();
                if items.is_empty() {
                    break;
                }

                let mut dated_items = 0usize;
                let mut old_items = 0usize;
                for item in items {
                    if let Some(cutoff) = cutoff {
                        if let Some(ts) = notification_timestamp(item) {
                            dated_items += 1;
                            if ts < cutoff {
                                old_items += 1;
                                continue;
                            }
                        }
                    }
                    if let Some(rq) = parse_notification(item, mobile) {
                        source_candidates += 1;
                        if seen.insert(rq.question_id.clone()) {
                            out.push(rq);
                        }
                    }
                }
                if cutoff.is_some() && dated_items > 0 && old_items == dated_items {
                    break;
                }
                if cutoff.is_none() {
                    break;
                }

                let paging = v.get("paging");
                let is_end = paging
                    .and_then(|p| p.get("is_end"))
                    .and_then(|x| x.as_bool())
                    .unwrap_or(true);
                let next = paging
                    .and_then(|p| p.get("next"))
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.replacen("http://", "https://", 1));
                if is_end {
                    break;
                }
                match next {
                    Some(url) => next_url = url,
                    None => {
                        tracing::warn!(target: "zhihu", "通知源 {kind} 未到底但缺少 paging.next，停止该来源翻页");
                        break;
                    }
                }
            }

            if source_succeeded {
                succeeded_sources += 1;
            }
            tracing::info!(target: "zhihu", "通知源 {kind} 共读取 {source_received} 条，识别 {source_candidates} 条邀请/推荐候选");
        }

        if succeeded_sources == 0 {
            return Err(anyhow!("所有通知源均拉取失败：{}", errors.join("；")));
        }
        if out.is_empty() && !errors.is_empty() {
            return Err(anyhow!(
                "部分通知源读取失败，且没有识别到邀请/推荐候选：{}",
                errors.join("；")
            ));
        }
        Ok(NotificationBatch {
            questions: out,
            received,
        })
    }

    pub async fn my_answers(&self, url_token: &str, pages: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for page in 0..pages {
            match self.my_answers_page(url_token, page).await {
                Ok(data) => {
                    let len = data.len();
                    out.extend(data);
                    if len < 20 {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(target: "zhihu", "历史回答第 {page} 页失败: {e}");
                    break;
                }
            }
        }
        Ok(out)
    }

    pub async fn answer_history_page(&self, url_token: &str, offset: usize) -> Result<(Vec<Value>, bool)> {
        let mut url = reqwest::Url::parse("https://www.zhihu.com/api/v4/members")?;
        url.path_segments_mut().map_err(|_| anyhow::anyhow!("无效的知乎地址"))?.push(url_token).push("answers");
        url.query_pairs_mut().append_pair("include", "data[*].question.id")
            .append_pair("limit", "20").append_pair("offset", &offset.to_string()).append_pair("sort_by", "created");
        let data = self.send(reqwest::Method::GET, url.as_str(), true).await?;
        let entries = data["data"].as_array().ok_or_else(|| anyhow::anyhow!("没有收到回答记录"))?;
        let done = data.pointer("/paging/is_end").and_then(Value::as_bool).unwrap_or(entries.len() < 20);
        Ok((entries.clone(), done || entries.is_empty()))
    }

    pub async fn question_from_answer(&self, id: &str) -> Result<String> {
        let url = format!("https://www.zhihu.com/api/v4/answers/{id}?include=question.id");
        let data = self.send(reqwest::Method::GET, &url, true).await?;
        let question_id = answer_question_id(&data);
        anyhow::ensure!(!question_id.is_empty(), "无法从该回答定位原问题");
        Ok(question_id)
    }

    pub async fn creator_answers_page(&self, offset: usize) -> Result<(Vec<Value>, Option<usize>)> {
        self.require_web_session()?;
        let url = format!("https://www.zhihu.com/api/v4/creators/creations/v2/answer?start=0&end=0&limit=20&offset={offset}&need_co_creation=1&sort_type=created");
        let response = self.send(reqwest::Method::GET, &url, true).await?;
        let rows = response["data"].as_array().ok_or_else(|| anyhow!("知乎创作中心没有返回回答列表"))?;
        let paging = &response["paging"];
        let end = paging["is_end"].as_bool().ok_or_else(|| anyhow!("知乎创作中心没有返回分页状态"))?;
        if end {
            return Ok((rows.clone(), None));
        }
        let next = paging["next"].as_str()
            .and_then(|url| reqwest::Url::parse(url).ok())
            .and_then(|url| url.query_pairs().find(|(key, _)| key == "offset").and_then(|(_, value)| value.parse().ok()));
        anyhow::ensure!(!rows.is_empty() && next.is_some_and(|next| next > offset), "知乎回答列表分页异常");
        Ok((rows.clone(), next))
    }

    pub async fn creator_answer_content(&self, id: &str) -> Result<(String, bool)> {
        anyhow::ensure!(is_content_id(id), "无效的回答编号");
        let url = format!("https://www.zhihu.com/api/v4/answers/{id}?include=content,content_need_truncated");
        let data = self.send(reqwest::Method::GET, &url, true).await?;
        anyhow::ensure!(as_str(&data, &["id"]) == id, "知乎没有返回对应的回答");
        let content = data["content"].as_str().filter(|content| !content.trim().is_empty())
            .ok_or_else(|| anyhow!("知乎未提供回答正文"))?;
        Ok((content.to_owned(), data["content_need_truncated"].as_bool().unwrap_or(false)))
    }

    pub async fn search_recent_questions(&self, topic: &str) -> Result<Vec<String>> {
        let mut url = reqwest::Url::parse("https://www.zhihu.com/api/v4/search_v3")?;
        url.query_pairs_mut().extend_pairs([
            ("q", topic), ("t", "general"), ("gk_version", "gz-gaokao"), ("search_source", "Filter"),
            ("limit", "12"), ("sort", "created_time"), ("time_interval", "three_months"), ("include", "data[*].object,type"),
        ]);
        let data = self.send(reqwest::Method::GET, url.as_str(), true).await?;
        Ok(search_question_ids(&data))
    }

    pub async fn question_details(&self, id: &str, source: &str) -> Result<(RawQuestion, i64, i64)> {
        anyhow::ensure!(is_content_id(id), "无效的知乎问题编号");
        let url = format!("https://www.zhihu.com/api/v4/questions/{id}?include=detail,excerpt,created,created_time,answer_count,follower_count,visit_count,read_count,topics,author");
        let data = self.send(reqwest::Method::GET, &url, true).await?;
        let title = strip_html(data["title"].as_str().unwrap_or_default());
        anyhow::ensure!(as_str(&data, &["id"]) == id && !title.trim().is_empty(), "知乎没有返回可读取的问题标题");
        let metrics = question_metrics(&data);
        let created = metrics.created_time.unwrap_or(0);
        let views = metrics.views.unwrap_or(0);
        let tags = data["topics"].as_array().into_iter().flatten()
            .filter_map(|topic| topic["name"].as_str().map(str::to_owned)).collect();
        Ok((RawQuestion {
            zhihu_id: id.to_owned(), question_id: id.to_owned(), source: source.to_owned(), title,
            url: format!("https://www.zhihu.com/question/{id}"),
            excerpt: slim(&strip_html(&first_text([as_str(&data, &["detail"]), as_str(&data, &["excerpt"])])), 4000),
            asker_name: as_str(&data, &["author", "name"]),
            asker_headline: as_str(&data, &["author", "headline"]),
            answer_count: metrics.answers.unwrap_or(0),
            follower_count: metrics.followers.unwrap_or(0),
            tags,
            metrics: Some(metrics),
        }, created, views))
    }

    pub async fn my_answers_page(&self, url_token: &str, page: usize) -> Result<Vec<Value>> {
        let offset = page * 20;
        let url = format!(
            "https://www.zhihu.com/api/v4/members/{url_token}/answers?include=data[*].content,excerpt,voteup_count,comment_count,question.title&limit=20&offset={offset}&sort_by=voteups"
        );
        let v = self.send(reqwest::Method::GET, &url, true).await?;
        Ok(v.get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default())
    }

    pub async fn my_articles(&self, url_token: &str, pages: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for page in 0..pages {
            match self.my_articles_page(url_token, page).await {
                Ok(data) => {
                    let len = data.len();
                    out.extend(data);
                    if len < 20 {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!(target: "zhihu", "历史文章第 {page} 页失败: {e}");
                    break;
                }
            }
        }
        Ok(out)
    }

    pub async fn my_articles_page(&self, url_token: &str, page: usize) -> Result<Vec<Value>> {
        let offset = page * 20;
        let url = format!(
            "https://www.zhihu.com/api/v4/members/{url_token}/articles?include=data[*].content,excerpt,voteup_count,comment_count&limit=20&offset={offset}&sort_by=created"
        );
        let v = self.send(reqwest::Method::GET, &url, true).await?;
        Ok(v.get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default())
    }
}

#[derive(Debug)]
struct RequestError {
    message: String,
    retry_without_sign: bool,
}

impl RequestError {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_without_sign: false,
        }
    }
}

impl From<reqwest::Error> for RequestError {
    fn from(error: reqwest::Error) -> Self {
        Self::fatal(format!("请求知乎失败：{error}"))
    }
}

async fn decode_response(resp: reqwest::Response) -> std::result::Result<Value, RequestError> {
    let status = resp.status();
    let text = resp.text().await.map_err(RequestError::from)?;
    if !status.is_success() {
        return Err(RequestError {
            message: format!("HTTP {status}：知乎拒绝了请求：{}", slim(&text, 200)),
            retry_without_sign: matches!(status.as_u16(), 401 | 403 | 404),
        });
    }
    let value: Value = serde_json::from_str(&text).map_err(|_| {
        RequestError::fatal(format!(
            "知乎返回 HTTP {status}，但正文不是 JSON（可能是登录页或风控页）：{}",
            slim(&text, 160)
        ))
    })?;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("知乎接口返回错误");
        return Err(RequestError::fatal(format!(
            "{message}：{}",
            slim(&error.to_string(), 180)
        )));
    }
    Ok(value)
}

fn request_path(url: &str) -> String {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let after_scheme = without_fragment
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(without_fragment);
    let path_start = after_scheme.find(['/', '?']).unwrap_or(after_scheme.len());
    let path = &after_scheme[path_start..];
    if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    }
}

#[derive(Debug)]
pub struct NotificationBatch {
    pub questions: Vec<RawQuestion>,
    pub received: usize,
}

fn notification_timestamp(item: &Value) -> Option<i64> {
    for key in [
        "create_time",
        "created_time",
        "created_at",
        "created",
        "createdTime",
        "time",
    ] {
        let Some(value) = item.get(key) else { continue };
        if let Some(n) = value.as_i64() {
            return Some(if n > 10_000_000_000 { n / 1000 } else { n });
        }
        if let Some(s) = value.as_str() {
            if let Ok(n) = s.parse::<i64>() {
                return Some(if n > 10_000_000_000 { n / 1000 } else { n });
            }
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
                return Some(dt.timestamp());
            }
        }
    }
    item.get("content")
        .and_then(|content| notification_timestamp(content))
}

pub fn answer_question_id(answer: &Value) -> String {
    let id = as_str(answer, &["question", "id"]);
    if is_content_id(&id) { id } else { String::new() }
}

fn search_question_ids(search: &Value) -> Vec<String> {
    search["data"].as_array().into_iter().flatten()
        .filter_map(|row| {
            let object = &row["object"];
            let id = match object["type"].as_str() {
                Some("question") => as_str(object, &["id"]),
                Some("answer") => first_text([answer_question_id(object), as_str(object, &["question_id"])]),
                _ => return None,
            };
            is_content_id(&id).then_some(id)
        }).collect()
}

pub(crate) fn is_content_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Debug, PartialEq)]
pub enum SharedLink {
    Question(String),
    Answer(String),
}

pub fn parse_shared_link(raw: &str) -> Result<SharedLink> {
    let url = reqwest::Url::parse(raw).map_err(|_| anyhow::anyhow!("不是有效的知乎链接"))?;
    anyhow::ensure!(matches!(url.scheme(), "http" | "https") && url.username().is_empty()
        && url.password().is_none() && url.port().is_none(), "不是有效的知乎链接");
    if url.host_str() == Some("zhuanlan.zhihu.com") {
        anyhow::bail!("专栏文章不是问题，不能导入收件箱");
    }
    anyhow::ensure!(matches!(url.host_str(), Some("www.zhihu.com" | "zhihu.com" | "m.zhihu.com")), "只支持知乎问题或回答链接");
    let segments = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        ["question", id] if is_content_id(id) => Ok(SharedLink::Question((*id).to_owned())),
        ["question", id, "answer", answer] if is_content_id(id) && is_content_id(answer) => Ok(SharedLink::Question((*id).to_owned())),
        ["answer", id] if is_content_id(id) => Ok(SharedLink::Answer((*id).to_owned())),
        _ => anyhow::bail!("只支持知乎问题或回答链接"),
    }
}

#[derive(Debug, Clone)]
pub struct RawQuestion {
    pub zhihu_id: String,
    pub question_id: String,
    pub title: String,
    pub excerpt: String,
    pub url: String,
    pub source: String,
    pub asker_name: String,
    pub asker_headline: String,
    pub answer_count: i64,
    pub follower_count: i64,
    pub tags: Vec<String>,
    pub metrics: Option<QuestionMetrics>,
}

pub(crate) fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .trim()
        .to_string()
}

pub(crate) fn as_str(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for k in path {
        cur = match cur.get(*k) {
            Some(x) => x,
            None => return String::new(),
        };
    }
    match cur {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

pub(crate) fn count_field(data: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|key| {
        let value = data.get(*key)?;
        value.as_i64().or_else(|| value.as_str()?.parse().ok()).filter(|count| *count >= 0)
    })
}

fn question_metrics(data: &Value) -> QuestionMetrics {
    QuestionMetrics {
        views: count_field(data, &["visit_count", "read_count", "view_count"]),
        answers: count_field(data, &["answer_count", "answerCount"]),
        followers: count_field(data, &["follower_count", "follow_num", "followNum", "follow_count"]),
        created_time: notification_timestamp(data).filter(|time| *time > 0),
        synced_at: now_iso(),
    }
}

fn first_text(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default()
}

fn question_id_from_text(text: &str) -> String {
    let Some(pos) = text.find("/question/") else {
        return String::new();
    };
    text[pos + 10..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect()
}

fn question_id_from_links(objects: &[&Value]) -> String {
    for object in objects {
        for key in ["url", "link", "target_link", "targetLink", "my_answer_url"] {
            let qid = question_id_from_text(&as_str(object, &[key]));
            if !qid.is_empty() {
                return qid;
            }
        }
    }
    String::new()
}

/// 同时兼容 Web v2 与 Android v3 通知结构；独立 invite 时间线中的条目都按邀请处理。
fn parse_notification(item: &Value, assume_invite: bool) -> Option<RawQuestion> {
    let content = item.get("content").unwrap_or(&Value::Null);
    let target_source = item.get("target_source").unwrap_or(&Value::Null);
    let raw_text = first_text([
        as_str(content, &["text"]),
        as_str(content, &["title"]),
        as_str(content, &["sub_title"]),
        as_str(content, &["subTitle"]),
        as_str(content, &["target", "text"]),
        as_str(target_source, &["full_text"]),
        as_str(target_source, &["fullText"]),
    ]);
    let text = strip_html(&raw_text);
    let verb = as_str(content, &["verb"]);

    let is_invite = assume_invite
        || verb.to_lowercase().contains("invite")
        || text.contains("邀请你回答")
        || text.contains("邀请回答");
    let is_recommend = verb.to_lowercase().contains("recommend")
        || text.contains("推荐回答")
        || text.contains("为你推荐");
    if !is_invite && !is_recommend {
        return None;
    }

    let mut subject = content.get("subject").cloned().unwrap_or(Value::Null);
    if !subject.is_object() {
        subject = item.get("target").cloned().unwrap_or(Value::Null);
    }
    let question = if as_str(&subject, &["type"]) == "answer" {
        subject.get("question").cloned().unwrap_or(Value::Null)
    } else {
        subject.clone()
    };

    let mut qid = first_text([
        as_str(&question, &["id"]),
        as_str(&subject, &["question", "id"]),
        as_str(target_source, &["object_id"]),
        as_str(target_source, &["objectId"]),
    ]);
    if qid.is_empty() {
        qid = question_id_from_links(&[
            &question,
            &subject,
            content.get("target").unwrap_or(&Value::Null),
            item.get("target").unwrap_or(&Value::Null),
            target_source,
            content,
        ]);
    }
    if qid.is_empty() {
        qid = question_id_from_text(&raw_text);
    }
    if qid.is_empty() {
        return None;
    }

    let mut title = first_text([
        as_str(&question, &["title"]),
        as_str(&subject, &["title"]),
        as_str(item, &["target", "title"]),
        as_str(content, &["target", "text"]),
        as_str(content, &["title"]),
        as_str(target_source, &["top_text"]),
        as_str(target_source, &["topText"]),
    ]);
    title = strip_html(&title);
    if title.is_empty() {
        title = text
            .split(['，', ',', '「', '」'])
            .find(|s| s.chars().count() > 6)
            .unwrap_or("（未能解析问题标题）")
            .trim()
            .to_string();
    }

    let asker = content
        .get("actor")
        .cloned()
        .or_else(|| item.get("actor").cloned())
        .or_else(|| {
            content
                .get("actors")
                .and_then(|a| a.as_array())
                .and_then(|a| a.first())
                .cloned()
        })
        .or_else(|| item.pointer("/head/author").cloned())
        .unwrap_or(Value::Null);

    let tags = question
        .get("topics")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    t.get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    let excerpt = first_text([
        as_str(&question, &["excerpt"]),
        as_str(target_source, &["sub_text"]),
        as_str(target_source, &["subText"]),
    ]);
    let zhihu_id = first_text([
        as_str(item, &["id"]),
        as_str(item, &["unique_id"]),
        as_str(item, &["uniqueId"]),
    ]);

    let metrics = question_metrics(&question);
    Some(RawQuestion {
        zhihu_id,
        question_id: qid.clone(),
        title,
        excerpt: strip_html(&excerpt),
        url: format!("https://www.zhihu.com/question/{qid}"),
        source: if is_invite {
            "invite".into()
        } else {
            "recommend".into()
        },
        asker_name: as_str(&asker, &["name"]),
        asker_headline: as_str(&asker, &["headline"]),
        answer_count: metrics.answers.unwrap_or(0),
        follower_count: metrics.followers.unwrap_or(0),
        metrics: metrics.views.is_some().then_some(metrics),
        tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_supported_share_links_and_rejects_other_content() {
        for link in ["https://www.zhihu.com/question/123456789", "https://www.zhihu.com/question/123456789/answer/987654321?share=1#ref"] {
            assert_eq!(parse_shared_link(link).unwrap(), SharedLink::Question("123456789".into()));
        }
        assert_eq!(parse_shared_link("https://www.zhihu.com/answer/987654321").unwrap(), SharedLink::Answer("987654321".into()));
        assert!(parse_shared_link("https://zhuanlan.zhihu.com/p/123456789").unwrap_err().to_string().contains("专栏"));
        assert!(parse_shared_link("https://www.zhihu.com.evil.test/question/123456789").is_err());
        assert!(parse_shared_link("https://www.zhihu.com/question/123/answer/missing").is_err());
        assert_eq!(answer_question_id(&json!({"question": {"id": 123456789}})), "123456789");
    }

    #[test]
    fn search_results_use_original_question_ids_not_answer_or_article_ids() {
        assert_eq!(search_question_ids(&json!({"data": [
            {"object": {"type": "question", "id": 41}},
            {"object": {"type": "answer", "id": 90, "question": {"id": "42"}}},
            {"object": {"type": "article", "id": 43}},
            {"object": {"type": "answer", "id": 12}},
        ]})), vec!["41", "42"]);
    }

    #[test]
    fn parses_web_invite_with_numeric_id_and_create_time() {
        let item = json!({
            "id": "web-1",
            "create_time": 1_790_000_000,
            "content": {
                "verb": "INVITE_ANSWER",
                "actors": [{"name": "邀请人", "headline": "简介"}],
                "target": {
                    "text": "如何理解分布式系统？",
                    "link": "https://www.zhihu.com/question/123456789"
                }
            }
        });
        let parsed = parse_notification(&item, false).expect("Web 邀请应可解析");
        assert_eq!(parsed.question_id, "123456789");
        assert_eq!(parsed.title, "如何理解分布式系统？");
        assert_eq!(parsed.asker_name, "邀请人");
        assert_eq!(notification_timestamp(&item), Some(1_790_000_000));
    }

    #[test]
    fn parses_mobile_invite_timeline_item() {
        let item = json!({
            "uniqueId": "mobile-1",
            "created": 1_790_000_000_000i64,
            "content": {"title": "移动端邀请问题", "subTitle": "邀请你回答"},
            "target": {
                "id": "987654321",
                "type": "question",
                "title": "移动端邀请问题",
                "followNum": 42
            },
            "head": {"author": {"name": "移动端邀请人", "headline": "作者简介"}}
        });
        let parsed = parse_notification(&item, true).expect("移动端邀请应可解析");
        assert_eq!(parsed.zhihu_id, "mobile-1");
        assert_eq!(parsed.question_id, "987654321");
        assert_eq!(parsed.source, "invite");
        assert_eq!(parsed.follower_count, 42);
        assert_eq!(parsed.asker_name, "移动端邀请人");
        assert_eq!(notification_timestamp(&item), Some(1_790_000_000));
    }

    #[test]
    fn ignores_non_invite_notifications() {
        let item = json!({
            "content": {"verb": "ANSWER_VOTE_UP", "text": "有人赞了你的回答"},
            "target": {"id": 123, "type": "question", "title": "不应入库"}
        });
        assert!(parse_notification(&item, false).is_none());
    }

    #[test]
    fn signing_path_keeps_query_and_removes_fragment() {
        assert_eq!(
            request_path("https://www.zhihu.com/api/v4/me?include=foo#ignored"),
            "/api/v4/me?include=foo"
        );
    }
}

fn slim(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    t
}
