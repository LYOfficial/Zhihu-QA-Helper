//! 知乎客户端：二维码登录、通知拉取、历史内容抓取。
//!
//! 协议参考 docs/zhihu-apis.md（整理自知乎++ / zhurl 的公开接口索引）。
//! 注意：知乎风控参数（x-zse-96）会随版本变动，本项目把它做成可切换策略，
//! 签名失败时自动降级为「仅 Cookie」请求，并在 UI 里如实提示。

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use md5::{Digest, Md5};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

use crate::models::{ZhihuUser};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const ZSE93: &str = "101_3_3.0";

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

pub struct ZhihuClient {
    http: Client,
    pub cookie: String,
    pub sign_mode: SignMode,
}

/// 从 `z_c0=xxx; Path=/; Domain=.zhihu.com` 这类串里抽出 k=v 对。
fn extract_cookie_pairs(raw: &str) -> Vec<(String, String)> {
    raw.split(';')
        .filter_map(|seg| {
            let seg = seg.trim();
            if seg.is_empty() || seg.contains("Path=") || seg.contains("Domain=") {
                return None;
            }
            let (k, v) = seg.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .filter(|(k, _)| !k.is_empty())
        .collect()
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

    /// 把响应里的 Set-Cookie 合并进本地 jar（只取 name=value，忽略属性）
    fn absorb(&mut self, resp: &reqwest::Response) {
        let parts: Vec<String> = resp
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(|raw| {
                let first = raw.split(';').next()?.trim();
                if first.contains('=') {
                    Some(first.to_string())
                } else {
                    None
                }
            })
            .collect();
        if parts.is_empty() {
            return;
        }
        for pair in parts {
            let (k, v) = match pair.split_once('=') {
                Some(x) => x,
                None => continue,
            };
            let mut pairs: Vec<(String, String)> = self
                .cookie
                .split(';')
                .filter_map(|s| s.trim().split_once('='))
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .filter(|(a, _)| !a.is_empty())
                .collect();
            match pairs.iter_mut().find(|(a, _)| a == k) {
                Some(slot) => slot.1 = v.to_string(),
                None => pairs.push((k.to_string(), v.to_string())),
            }
            self.cookie = pairs
                .into_iter()
                .map(|(a, b)| format!("{a}={b}"))
                .collect::<Vec<_>>()
                .join("; ");
        }
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

    /// 合并新 cookie（登录回调返回的只包含 z_c0 等少量字段）。
    pub fn merge_cookie(&mut self, raw: &str) {
        let mut pairs: Vec<(String, String)> = self
            .cookie
            .split(';')
            .filter_map(|s| s.trim().split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        for (k, v) in extract_cookie_pairs(raw) {
            if let Some(slot) = pairs.iter_mut().find(|(pk, _)| *pk == k) {
                slot.1 = v;
            } else {
                pairs.push((k, v));
            }
        }
        self.cookie = pairs
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
    }

    /// x-zse-96 v2：`2.0_` + base64(md5("101_3_3.0+{path}+{d_c0}"))
    /// 知乎换算法时这里会失效，届时 `sign_mode=off` 仍可跑通不需要签名的接口。
    fn signed_zse96(&self, path_with_query: &str) -> String {
        let raw = format!("{ZSE93}+{path_with_query}+{}", self.d_c0());
        let mut h = Md5::new();
        h.update(raw.as_bytes());
        let digest = h.finalize();
        format!("2.0_{}", B64.encode(digest))
    }

    async fn send(&self, method: reqwest::Method, url: &str, signed: bool) -> Result<Value> {
        let path = url
            .splitn(2, "www.zhihu.com")
            .nth(1)
            .or_else(|| url.splitn(2, "api.zhihu.com").nth(1))
            .unwrap_or("/");
        let path = path.splitn(2, '#').next().unwrap_or(path);

        let mut req = self
            .http
            .request(method.clone(), url)
            .header("Cookie", &self.cookie)
            .header("x-requested-with", "fetch")
            .header("x-zse-93", ZSE93)
            .header("Referer", "https://www.zhihu.com/")
            .header("Accept", "application/json, text/plain, */*");

        if signed {
            req = req.header("x-zse-96", self.signed_zse96(path));
        }

        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if status == 404 || status == 403 || status == 401 {
            if self.sign_mode == SignMode::Auto && signed {
                // 降级重试：不带签名
                let mut req2 = self
                    .http
                    .request(method, url)
                    .header("Cookie", &self.cookie)
                    .header("x-requested-with", "fetch")
                    .header("Referer", "https://www.zhihu.com/");
                req2 = req2.header("Accept", "application/json");
                let r2 = req2.send().await?;
                let s2 = r2.status();
                let t2 = r2.text().await.unwrap_or_default();
                if s2.is_success() {
                    return Ok(serde_json::from_str(&t2)?);
                }
                return Err(anyhow!(
                    "HTTP {s2}：知乎拒绝了这次请求（风控签名可能已失效，或登录态过期）"
                ));
            }
            return Err(anyhow!("HTTP {status}：知乎拒绝了这次请求"));
        }
        if !status.is_success() {
            return Err(anyhow!("HTTP {status}: {}", slim(&text, 200)));
        }
        Ok(serde_json::from_str(&text).unwrap_or(json!({})))
    }

    // ---------- 登录 ----------

    /// 二维码登录的完整握手（顺序不能变，少一步知乎就打回）：
    ///   ① GET  /signin                    建匿名会话（_xsrf / BEC）
    ///   ② POST /udid                      设备预热
    ///   ③ GET  /api/v3/oauth/captcha/v2   关键：这一步才会下发 d_c0
    ///   ④ POST /api/v3/account/api/login/qrcode
    /// 没有 d_c0 的二维码是"无设备绑定"的，手机扫了也确认不下来。
    pub async fn qr_create(&mut self) -> Result<(String, String, i64)> {
        let signin = self
            .http
            .get("https://www.zhihu.com/signin")
            .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
            .header("Accept-Language", "zh-CN,zh;q=0.9")
            .header("Referer", "https://www.zhihu.com/")
            .send()
            .await?;
        self.absorb(&signin);

        let udid = self
            .http
            .post("https://www.zhihu.com/udid")
            .header("Content-Type", "application/json;charset=UTF-8")
            .header("x-requested-with", "fetch")
            .header("Cookie", &self.cookie)
            .header("Referer", "https://www.zhihu.com/signin")
            .body("{}")
            .send()
            .await;
        if let Ok(r) = udid {
            self.absorb(&r);
        }

        // ③ 设备指纹：d_c0 由这个接口下发
        let cap = self
            .http
            .get("https://www.zhihu.com/api/v3/oauth/captcha/v2?type=captcha_sign_in")
            .header("Cookie", &self.cookie)
            .header("x-requested-with", "fetch")
            .header("Referer", "https://www.zhihu.com/signin")
            .send()
            .await;
        if let Ok(r) = cap {
            self.absorb(&r);
        }
        if self.d_c0().is_empty() {
            tracing::warn!(target: "zhihu", "captcha 未下发 d_c0，登录可能无法确认");
        }

        // ④ 建码
        let mut req = self
            .http
            .post("https://www.zhihu.com/api/v3/account/api/login/qrcode")
            .header("Content-Type", "application/json;charset=UTF-8")
            .header("x-requested-with", "fetch")
            .header("Origin", "https://www.zhihu.com")
            .header("Cookie", &self.cookie)
            .header("Referer", "https://www.zhihu.com/signin");
        if let Some(t) = self.xsrf() {
            req = req.header("x-xsrftoken", t);
        }
        let resp = req.body("{}").send().await?;
        let status = resp.status();
        self.absorb(&resp);
        let v: Value = resp.json().await.unwrap_or(json!({}));
        if !status.is_success() {
            return Err(anyhow!(
                "知乎拒绝了建码请求（HTTP {status}）：{}",
                slim(&v.to_string(), 160)
            ));
        }
        let token = v
            .get("token")
            .and_then(|x| x.as_str())
            .ok_or_else(|| anyhow!("二维码响应缺少 token 字段"))?
            .to_string();
        // 知乎返回的 link 尾部会带一段 "?/api/login/qrcode"（网关把原始路径当查询串追加了），
        // 直接编码进二维码会让 App 解析不稳；这里只留规范路径。
        let raw_link = v
            .get("link")
            .and_then(|x| x.as_str())
            .unwrap_or("https://www.zhihu.com/signin");
        let link = raw_link.split('?').next().unwrap_or(raw_link).to_string();
        // expires_at 是秒级时间戳
        let expires_at = v
            .get("expires_at")
            .and_then(|x| x.as_i64())
            .unwrap_or_else(|| chrono::Utc::now().timestamp() + 300);
        Ok((token, link, expires_at))
    }

    /// 轮询扫码状态。返回 (状态, 登录 cookie, 给用户看的说明)。
    /// 状态取值：waiting / scanned / confirmed / expired / blocked
    pub async fn qr_poll(&self, token: &str) -> Result<(String, Option<String>, String)> {
        let url =
            format!("https://www.zhihu.com/api/v3/account/api/login/qrcode/{token}/scan_info");
        let resp = self
            .http
            .get(&url)
            .header("Cookie", &self.cookie)
            .header("x-zse-93", ZSE93)
            .header("x-requested-with", "fetch")
            .header("Accept", "application/json, text/plain, */*")
            .header("Origin", "https://www.zhihu.com")
            .header("Referer", "https://www.zhihu.com/signin")
            .send()
            .await?;

        let code = resp.status();
        let v: Value = resp.json().await.unwrap_or(json!({}));

        // 403 / code 40352：知乎要求人机验证，扫码流程在本机被风控拦住了
        let err_code = v.pointer("/error/code").and_then(|x| x.as_i64()).unwrap_or(0);
        if code == 403 || err_code == 40352 {
            let msg = v
                .pointer("/error/message")
                .and_then(|x| x.as_str())
                .unwrap_or("知乎要求人机验证，扫码登录已被风控拦截");
            return Ok(("blocked".to_string(), None, msg.to_string()));
        }
        if !code.is_success() {
            return Ok((
                "blocked".to_string(),
                None,
                format!("轮询返回 HTTP {code}"),
            ));
        }

        let login_status = v
            .get("login_status")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let status_num = v.get("status").and_then(|x| x.as_i64()).unwrap_or(0);
        let cookie = v
            .get("cookie")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let state = match login_status.as_str() {
            "CONFIRMED" => "confirmed",
            "SCANNED" => "scanned",
            "EXPIRED" => "expired",
            _ => {
                if status_num == 1 {
                    "scanned"
                } else if status_num >= 2 {
                    "confirmed"
                } else {
                    "waiting"
                }
            }
        };
        let note = match state {
            "scanned" => "已扫码，请在手机上点「确认登录」",
            "expired" => "二维码已过期，请换一张",
            "confirmed" => "登录成功",
            _ => "等待扫码",
        };
        Ok((state.to_string(), cookie, note.to_string()))
    }

    pub async fn me(&self) -> Result<ZhihuUser> {
        let v = self
            .send(
                reqwest::Method::GET,
                "https://www.zhihu.com/api/v4/me?include=answer_count,articles_count,follower_count",
                true,
            )
            .await?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let n = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        if s("name").is_empty() {
            return Err(anyhow!("登录态无效：拿不到账号信息"));
        }
        Ok(ZhihuUser {
            id: s("id"),
            name: s("name"),
            url_token: s("url_token"),
            avatar_url: s("avatar_url"),
            headline: s("headline"),
            answer_count: n("answer_count"),
            articles_count: n("articles_count"),
            follower_count: n("follower_count"),
        })
    }

    // ---------- 通知 → 问题 ----------

    /// 拉取三类通知里的「邀请回答」与「推荐回答」。
    pub async fn fetch_invites(&self, limit: u32) -> Result<Vec<RawQuestion>> {
        self.fetch_invites_window(limit, None).await
    }

    /// 拉取指定天数内的邀请与推荐，首次登录时用于补齐历史通知。
    pub async fn fetch_invites_since(&self, days: i64) -> Result<Vec<RawQuestion>> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(days.max(1));
        self.fetch_invites_window(20, Some(cutoff.timestamp())).await
    }

    async fn fetch_invites_window(&self, limit: u32, cutoff: Option<i64>) -> Result<Vec<RawQuestion>> {
        let mut out: Vec<RawQuestion> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for kind in ["default", "follow", "vote_thank"] {
            let pages = if cutoff.is_some() { 12 } else { 1 };
            for page in 0..pages {
                let offset = page * limit as usize;
                let url = format!(
                    "https://www.zhihu.com/api/v4/notifications/v2/{kind}?limit={limit}&offset={offset}"
                );
                let v = match self.send(reqwest::Method::GET, &url, true).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(target: "zhihu", "通知源 {kind} 第 {page} 页拉取失败: {e}");
                        break;
                    }
                };
                let items = v.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default();
                if items.is_empty() {
                    break;
                }
                let mut old_items = 0;
                for item in &items {
                    if let Some(cutoff) = cutoff {
                        if let Some(ts) = notification_timestamp(item) {
                            if ts < cutoff {
                                old_items += 1;
                                continue;
                            }
                        }
                    }
                    if let Some(rq) = parse_notification(item) {
                        if seen.insert(rq.question_id.clone()) {
                            out.push(rq);
                        }
                    }
                }
                if cutoff.is_some() && old_items >= items.len().saturating_div(2) {
                    break;
                }
                if items.len() < limit as usize {
                    break;
                }
            }
        }
        Ok(out)
    }

    pub async fn my_answers(&self, url_token: &str, pages: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for page in 0..pages {
            match self.my_answers_page(url_token, page).await {
                Ok(data) => {
                    let len = data.len();
                    out.extend(data);
                    if len < 20 { break; }
                }
                Err(e) => { tracing::warn!(target: "zhihu", "历史回答第 {page} 页失败: {e}"); break; }
            }
        }
        Ok(out)
    }

    pub async fn my_answers_page(&self, url_token: &str, page: usize) -> Result<Vec<Value>> {
        let offset = page * 20;
        let url = format!(
            "https://www.zhihu.com/api/v4/members/{url_token}/answers?include=data[*].content,excerpt,voteup_count,comment_count,question.title&limit=20&offset={offset}&sort_by=voteups"
        );
        let v = self.send(reqwest::Method::GET, &url, true).await?;
        Ok(v.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default())
    }

    pub async fn my_articles(&self, url_token: &str, pages: usize) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        for page in 0..pages {
            match self.my_articles_page(url_token, page).await {
                Ok(data) => {
                    let len = data.len();
                    out.extend(data);
                    if len < 20 { break; }
                }
                Err(e) => { tracing::warn!(target: "zhihu", "历史文章第 {page} 页失败: {e}"); break; }
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
        Ok(v.get("data").and_then(|d| d.as_array()).cloned().unwrap_or_default())
    }
}

fn notification_timestamp(item: &Value) -> Option<i64> {
    for key in ["created_time", "created_at", "created", "time"] {
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
}

fn strip_html(s: &str) -> String {
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
    out.replace("&nbsp;", " ").replace("&quot;", "\"").trim().to_string()
}

fn as_str(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for k in path {
        cur = match cur.get(*k) {
            Some(x) => x,
            None => return String::new(),
        };
    }
    cur.as_str().unwrap_or("").to_string()
}

/// 通知条目结构在不同版本间有差异，这里做容错解析：
/// 优先取 content.subject（问题对象），其次 target，再其次从文案里找 question id。
fn parse_notification(item: &Value) -> Option<RawQuestion> {
    let content = item.get("content").unwrap_or(&Value::Null);
    let text = strip_html(&as_str(content, &["text"]));
    let verb = as_str(content, &["verb"]);

    // 只保留「邀请回答 / 推荐回答」类通知
    let is_invite = verb.to_lowercase().contains("invite")
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
    // subject 可能是 answer，其 question 字段才是问题
    let question = if as_str(&subject, &["type"]) == "answer" {
        subject.get("question").cloned().unwrap_or(Value::Null)
    } else {
        subject.clone()
    };

    let mut qid = as_str(&question, &["id"]);
    if qid.is_empty() {
        qid = as_str(&subject, &["question", "id"]);
    }
    // 兜底：从 https://www.zhihu.com/question/123456 里抠
    if qid.is_empty() {
        if let Some(pos) = text.find("/question/") {
            let tail: String = text[pos + 10..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            qid = tail;
        }
    }
    if qid.is_empty() {
        return None;
    }

    let mut title = as_str(&question, &["title"]);
    if title.is_empty() {
        title = as_str(&subject, &["title"]);
    }
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
        .unwrap_or(Value::Null);

    let tags = question
        .get("topics")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    Some(RawQuestion {
        zhihu_id: as_str(item, &["id"]),
        question_id: qid.clone(),
        title,
        excerpt: strip_html(&as_str(&question, &["excerpt"])),
        url: format!("https://www.zhihu.com/question/{qid}"),
        source: if is_invite { "invite".into() } else { "recommend".into() },
        asker_name: as_str(&asker, &["name"]),
        asker_headline: as_str(&asker, &["headline"]),
        answer_count: question.get("answer_count").and_then(|x| x.as_i64()).unwrap_or(0),
        follower_count: question.get("follower_count").and_then(|x| x.as_i64()).unwrap_or(0),
        tags,
    })
}

fn slim(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    t
}

pub fn make_qr_svg(link: &str) -> Result<String> {
    use qrcode::render::svg;
    let code = qrcode::QrCode::new(link.as_bytes())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(236, 236)
        .quiet_zone(true)
        .dark_color(svg::Color("#1b1a17"))
        .light_color(svg::Color("#ffffff"))
        .build())
}
