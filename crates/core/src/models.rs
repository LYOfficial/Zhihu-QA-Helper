//! 数据模型：与前端 `apps/web/lib/types.ts` 一一对应（serde 统一输出 camelCase）。

use serde::{Deserialize, Serialize};

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn now_iso() -> String {
    now()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ZhihuUser {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub url_token: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub headline: String,
    #[serde(default)]
    pub answer_count: i64,
    #[serde(default)]
    pub articles_count: i64,
    #[serde(default)]
    pub follower_count: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub logged_in: bool,
    pub user: Option<ZhihuUser>,
    pub expires_at: Option<String>,
    pub last_sync_at: Option<String>,
    pub sign_mode: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QrTicket {
    pub token: String,
    pub link: String,
    pub svg: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QrState {
    pub status: String, // waiting | scanned | confirmed | expired
    pub user: Option<ZhihuUser>,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTerm {
    pub id: String,
    pub kind: String, // primary | sub | adjacent
    pub label: String,
    pub weight: f64,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub locked: bool,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub status: String, // empty | running | ready
    pub summary: String,
    pub updated_at: Option<String>,
    pub terms: Vec<ProfileTerm>,
    pub evidence: Vec<String>,
    pub sample_size: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub id: String,
    pub domain: String,
    pub name: String,
    pub base_url: String,
    pub kind: String, // wiki | community | official | media | dataset | forum
    pub trust: i64,   // 1..5
    pub enabled: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub use_for: Vec<String>, // fact | image | data
    #[serde(default)]
    pub discovered_by: String,
    #[serde(default)]
    pub ok: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Question {
    pub id: String,
    pub zhihu_id: String,
    pub question_id: String,
    pub source: String, // invite | recommend
    pub title: String,
    pub excerpt: String,
    pub url: String,
    pub domain: String,
    pub sub_domain: String,
    pub fit: f64,
    pub status: String, // inbox | researching | pending_review | answered | dismissed
    pub bucket: String, // queue | selected | archived
    pub answer_count: i64,
    pub follower_count: i64,
    pub asker_name: String,
    pub asker_headline: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub synced_at: String,
    #[serde(default)]
    pub has_draft: bool,
    #[serde(default)]
    pub image_count: i64,
    #[serde(default)]
    pub word_count: i64,
    #[serde(default)]
    pub doubt_count: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub label: String,
    pub url: String,
    #[serde(default)]
    pub source_name: String,
    #[serde(default)]
    pub snippet: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Doubt {
    pub text: String,
    pub status: String, // open | verified | corrected | rejected
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ImageAsset {
    pub id: String,
    pub question_id: String,
    pub filename: String,
    pub url: String, // 本地可访问路径
    pub source_url: String,
    pub source_name: String,
    pub caption: String,
    pub bytes: i64,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub question_id: String,
    pub markdown: String,
    pub doubts: Vec<Doubt>,
    pub citations: Vec<Citation>,
    pub images: Vec<ImageAsset>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub word_count: i64,
    pub updated_at: String,
    pub model: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub api_key: String,
    #[serde(default)]
    pub has_key: bool,
    pub temperature: f64,
    #[serde(default)]
    pub verified_at: Option<String>,
    /// 上下文窗口（tokens），来自 models.dev 的 limit.context
    #[serde(default)]
    pub context_limit: i64,
    /// 单次最大输出（tokens），来自 models.dev 的 limit.output
    #[serde(default)]
    pub output_limit: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub cron_times: Vec<String>,
    pub pick_ratio: f64,
    pub draft_min: i64,
    pub draft_max: i64,
    pub image_min: i64,
    pub image_max: i64,
    pub auto_sync: bool,
    pub minimize_to_tray: bool,
    pub auto_launch: bool,
    pub sign_mode: String,
    pub data_dir: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub question_id: Option<String>,
    pub stage: String,
    pub progress: f64,
    pub message: String,
    pub state: String, // running | done | failed
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: i64,
    pub kind: String,
    pub question_id: Option<String>,
    pub title: String,
    pub detail: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatsOverview {
    pub total_questions: i64,
    pub inbox: i64,
    pub pending_review: i64,
    pub answered: i64,
    pub dismissed: i64,
    pub drafts: i64,
    pub images: i64,
    pub images_bytes: i64,
    pub answers_30d: i64,
    pub voteups_30d: i64,
    pub new_comments_30d: i64,
    pub avg_fit: f64,
    pub hit_rate: f64,
    pub saved_minutes: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StatsPoint {
    pub date: String,
    pub collected: i64,
    pub answered: i64,
    pub voteups: i64,
    pub comments: i64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    pub ok: bool,
    pub version: String,
    pub agent: bool,
    pub db: String,
    pub data_dir: String,
    pub uptime_seconds: i64,
}
