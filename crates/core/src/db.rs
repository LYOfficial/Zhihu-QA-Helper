//! SQLite 持久化层。所有表在启动时幂等创建。

use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS account (
  id            INTEGER PRIMARY KEY CHECK (id = 1),
  zhihu_id      TEXT NOT NULL DEFAULT '',
  name          TEXT NOT NULL DEFAULT '',
  url_token     TEXT NOT NULL DEFAULT '',
  avatar_url    TEXT NOT NULL DEFAULT '',
  headline      TEXT NOT NULL DEFAULT '',
  answer_count  INTEGER NOT NULL DEFAULT 0,
  articles_count INTEGER NOT NULL DEFAULT 0,
  follower_count INTEGER NOT NULL DEFAULT 0,
  cookie        TEXT NOT NULL DEFAULT '',
  expires_at    TEXT,
  updated_at    TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS profile_meta (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  status     TEXT NOT NULL DEFAULT 'empty',
  summary    TEXT NOT NULL DEFAULT '',
  evidence   TEXT NOT NULL DEFAULT '[]',
  sample_size INTEGER NOT NULL DEFAULT 0,
  updated_at TEXT
);

CREATE TABLE IF NOT EXISTS profile_terms (
  id       TEXT PRIMARY KEY,
  kind     TEXT NOT NULL,           -- primary | sub | adjacent
  label    TEXT NOT NULL,
  weight   REAL NOT NULL DEFAULT 0.5,
  keywords TEXT NOT NULL DEFAULT '[]',
  locked   INTEGER NOT NULL DEFAULT 0,
  note     TEXT NOT NULL DEFAULT '',
  ord      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS sources (
  id           TEXT PRIMARY KEY,
  domain       TEXT NOT NULL,
  name         TEXT NOT NULL,
  base_url     TEXT NOT NULL,
  kind         TEXT NOT NULL,
  trust        INTEGER NOT NULL DEFAULT 3,
  enabled      INTEGER NOT NULL DEFAULT 1,
  notes        TEXT NOT NULL DEFAULT '',
  use_for      TEXT NOT NULL DEFAULT '[]',
  discovered_by TEXT NOT NULL DEFAULT 'llm',
  ok           INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE IF NOT EXISTS questions (
  id            TEXT PRIMARY KEY,
  zhihu_id      TEXT NOT NULL DEFAULT '',
  question_id   TEXT NOT NULL DEFAULT '',
  source        TEXT NOT NULL DEFAULT 'invite',   -- invite | recommend
  title         TEXT NOT NULL,
  excerpt       TEXT NOT NULL DEFAULT '',
  url           TEXT NOT NULL DEFAULT '',
  domain        TEXT NOT NULL DEFAULT '',
  sub_domain    TEXT NOT NULL DEFAULT '',
  fit           REAL NOT NULL DEFAULT 0,
  status        TEXT NOT NULL DEFAULT 'inbox',    -- inbox|researching|pending_review|answered|dismissed
  bucket        TEXT NOT NULL DEFAULT 'queue',    -- queue|selected|archived
  answer_count  INTEGER NOT NULL DEFAULT 0,
  follower_count INTEGER NOT NULL DEFAULT 0,
  asker_name    TEXT NOT NULL DEFAULT '',
  asker_headline TEXT NOT NULL DEFAULT '',
  tags          TEXT NOT NULL DEFAULT '[]',
  created_at    TEXT NOT NULL DEFAULT '',
  synced_at     TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_q_status ON questions(status);
CREATE INDEX IF NOT EXISTS idx_q_domain ON questions(domain);

CREATE TABLE IF NOT EXISTS drafts (
  question_id TEXT PRIMARY KEY REFERENCES questions(id) ON DELETE CASCADE,
  markdown    TEXT NOT NULL DEFAULT '',
  doubts      TEXT NOT NULL DEFAULT '[]',
  citations   TEXT NOT NULL DEFAULT '[]',
  model       TEXT NOT NULL DEFAULT '',
  word_count  INTEGER NOT NULL DEFAULT 0,
  updated_at  TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS images (
  id          TEXT PRIMARY KEY,
  question_id TEXT NOT NULL REFERENCES questions(id) ON DELETE CASCADE,
  filename    TEXT NOT NULL,
  source_url  TEXT NOT NULL DEFAULT '',
  source_name TEXT NOT NULL DEFAULT '',
  caption     TEXT NOT NULL DEFAULT '',
  bytes       INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_img_q ON images(question_id);

CREATE TABLE IF NOT EXISTS model_config (
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  provider    TEXT NOT NULL DEFAULT 'deepseek',
  base_url    TEXT NOT NULL DEFAULT 'https://api.deepseek.com/v1',
  model       TEXT NOT NULL DEFAULT 'deepseek-flash',
  api_key     TEXT NOT NULL DEFAULT '',
  temperature REAL NOT NULL DEFAULT 0.3,
  verified_at TEXT
);

CREATE TABLE IF NOT EXISTS settings (
  id            INTEGER PRIMARY KEY CHECK (id = 1),
  cron_times    TEXT NOT NULL DEFAULT '["07:00","15:00","20:00"]',
  pick_ratio    REAL NOT NULL DEFAULT 0.4,
  draft_min     INTEGER NOT NULL DEFAULT 500,
  draft_max     INTEGER NOT NULL DEFAULT 1000,
  image_min     INTEGER NOT NULL DEFAULT 3,
  image_max     INTEGER NOT NULL DEFAULT 10,
  auto_sync     INTEGER NOT NULL DEFAULT 1,
  minimize_to_tray INTEGER NOT NULL DEFAULT 1,
  auto_launch   INTEGER NOT NULL DEFAULT 0,
  sign_mode     TEXT NOT NULL DEFAULT 'auto'
);

CREATE TABLE IF NOT EXISTS jobs (
  id          TEXT PRIMARY KEY,
  kind        TEXT NOT NULL,
  question_id TEXT,
  stage       TEXT NOT NULL DEFAULT '',
  progress    REAL NOT NULL DEFAULT 0,
  message     TEXT NOT NULL DEFAULT '',
  state       TEXT NOT NULL DEFAULT 'running',
  started_at  TEXT NOT NULL DEFAULT '',
  finished_at TEXT
);

CREATE TABLE IF NOT EXISTS events (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  kind        TEXT NOT NULL,
  question_id TEXT,
  title       TEXT NOT NULL DEFAULT '',
  detail      TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_ev_time ON events(created_at DESC);

CREATE TABLE IF NOT EXISTS stats_snapshots (
  date      TEXT PRIMARY KEY,
  answered  INTEGER NOT NULL DEFAULT 0,
  voteups   INTEGER NOT NULL DEFAULT 0,
  comments  INTEGER NOT NULL DEFAULT 0,
  followers INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS research_cache (
  key        TEXT PRIMARY KEY,
  payload    TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT ''
);
"#;

pub struct Db {
    pub conn: Connection,
    pub data_dir: PathBuf,
}

pub fn resolve_data_dir() -> PathBuf {
    if let Ok(p) = std::env::var("ZHIHU_QA_DATA") {
        if !p.trim().is_empty() {
            return PathBuf::from(p);
        }
    }
    // 开发态：仓库根目录 ./data；打包后：%APPDATA%\ZhihuQAHelper
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if cwd.join("package.json").exists() || cwd.join("Cargo.toml").exists() {
        return cwd.join("data");
    }
    dirs::data_dir()
        .unwrap_or_else(|| cwd.clone())
        .join("ZhihuQAHelper")
}

impl Db {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        std::fs::create_dir_all(dir.join("images"))?;
        std::fs::create_dir_all(dir.join("logs"))?;
        let conn = Connection::open(dir.join("zhihu-qa.db"))?;
        conn.execute_batch(SCHEMA)?;
        let db = Db {
            conn,
            data_dir: dir.to_path_buf(),
        };
        db.seed_defaults()?;
        Ok(db)
    }

    /// 只建必要的默认行（设置 / 模型配置 / 画像元信息），不写任何业务内容
    fn seed_defaults(&self) -> Result<()> {
        self.conn
            .execute("INSERT OR IGNORE INTO settings (id) VALUES (1)", [])?;
        self.conn.execute(
            "INSERT OR IGNORE INTO model_config (id) VALUES (1)",
            [],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO profile_meta (id, status) VALUES (1, 'empty')",
            [],
        )?;
        Ok(())
    }

    pub fn log_event(&self, kind: &str, qid: Option<&str>, title: &str, detail: &str) {
        let _ = self.conn.execute(
            "INSERT INTO events (kind, question_id, title, detail, created_at) VALUES (?1,?2,?3,?4,?5)",
            params![kind, qid, title, detail, crate::models::now_iso()],
        );
    }

    /// 归档目录：`data/domains/<领域>/<问题标题>.md`
    pub fn archive_draft(&self, domain: &str, title: &str, markdown: &str) -> Result<PathBuf> {
        let safe_domain = sanitize(if domain.is_empty() { "未分类" } else { domain });
        let dir = self.data_dir.join("domains").join(safe_domain);
        std::fs::create_dir_all(&dir)?;
        let name = format!("{}.md", sanitize(title));
        let path = dir.join(&name);
        let body = format!(
            "<!-- 知乎答主助手归档 · {} -->\n\n# {}\n\n{}",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            title,
            markdown
        );
        std::fs::write(&path, body)?;
        Ok(path)
    }

    /// 已回答/弃用后清理本地图片，释放磁盘
    pub fn purge_images(&self, question_id: &str) -> Result<u64> {
        let dir = self.data_dir.join("images").join(sanitize(question_id));
        let mut freed = 0u64;
        if let Ok(rows) = std::fs::read_dir(&dir) {
            for e in rows.flatten() {
                if let Ok(m) = e.metadata() {
                    freed += m.len();
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        self.conn.execute(
            "DELETE FROM images WHERE question_id = ?1",
            params![question_id],
        )?;
        Ok(freed)
    }
}

pub fn sanitize(s: &str) -> String {
    let s: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            '\n' | '\r' | '\t' => ' ',
            c => c,
        })
        .collect();
    let s = s.trim().trim_matches('.').to_string();
    let s: String = s.chars().take(80).collect();
    if s.is_empty() {
        "untitled".into()
    } else {
        s
    }
}
