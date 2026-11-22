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
  updated_at TEXT,
  style      TEXT NOT NULL DEFAULT '{}'
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
  fit_evaluated INTEGER NOT NULL DEFAULT 0,
  status        TEXT NOT NULL DEFAULT 'inbox',    -- inbox|researching|pending_review|answered|dismissed
  bucket        TEXT NOT NULL DEFAULT 'queue',    -- queue|selected|archived
  answer_count  INTEGER NOT NULL DEFAULT 0,
  follower_count INTEGER NOT NULL DEFAULT 0,
  asker_name    TEXT NOT NULL DEFAULT '',
  metrics       TEXT,
  metrics_attempted_at TEXT NOT NULL DEFAULT '',
  metrics_error TEXT NOT NULL DEFAULT '',
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
  sign_mode     TEXT NOT NULL DEFAULT 'auto',
  verify_timeout_seconds INTEGER NOT NULL DEFAULT 180
);

CREATE TABLE IF NOT EXISTS zhihu_open_settings (
  id            INTEGER PRIMARY KEY CHECK (id = 1),
  access_secret TEXT NOT NULL,
  enabled       INTEGER NOT NULL,
  account_id    TEXT NOT NULL
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
  finished_at TEXT,
  verify_started_ms INTEGER,
  verify_seconds INTEGER
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

CREATE TABLE IF NOT EXISTS research_checkpoints (
  question_id TEXT PRIMARY KEY REFERENCES questions(id) ON DELETE CASCADE,
  material TEXT NOT NULL,
  drafted INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS writing_samples (
  account_id TEXT PRIMARY KEY,
  samples TEXT NOT NULL
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
        migrate(&conn)?;
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
        self.conn
            .execute("INSERT OR IGNORE INTO model_config (id) VALUES (1)", [])?;
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
        let safe_domain = sanitize(if domain.is_empty() {
            "未分类"
        } else {
            domain
        });
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

/// 轻量迁移：只做加法，不删不改，老库升级不会丢数据。
fn migrate(conn: &Connection) -> Result<()> {
    let has_column = |table: &str, column: &str| -> bool {
        let sql = format!("PRAGMA table_info({table})");
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let found = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map(|rows| rows.filter_map(|x| x.ok()).any(|name| name == column))
            .unwrap_or(false);
        found
    };
    if !has_column("drafts", "warnings") {
        conn.execute(
            "ALTER TABLE drafts ADD COLUMN warnings TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    if !has_column("settings", "verify_timeout_seconds") {
        conn.execute("ALTER TABLE settings ADD COLUMN verify_timeout_seconds INTEGER NOT NULL DEFAULT 180", [])?;
    }
    if !has_column("jobs", "verify_started_ms") {
        conn.execute("ALTER TABLE jobs ADD COLUMN verify_started_ms INTEGER", [])?;
    }
    if !has_column("jobs", "verify_seconds") {
        conn.execute("ALTER TABLE jobs ADD COLUMN verify_seconds INTEGER", [])?;
    }
    if !has_column("model_config", "context_limit") {
        conn.execute(
            "ALTER TABLE model_config ADD COLUMN context_limit INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !has_column("model_config", "output_limit") {
        conn.execute(
            "ALTER TABLE model_config ADD COLUMN output_limit INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !has_column("profile_meta", "style") {
        conn.execute(
            "ALTER TABLE profile_meta ADD COLUMN style TEXT NOT NULL DEFAULT '{}'",
            [],
        )?;
    }
    if !has_column("settings", "writing_mode") {
        conn.execute("ALTER TABLE settings ADD COLUMN writing_mode TEXT NOT NULL DEFAULT 'automatic'", [])?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS author_directions (
            question_id TEXT PRIMARY KEY REFERENCES questions(id) ON DELETE CASCADE,
            text TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS answered_questions (
            account_id TEXT NOT NULL, question_id TEXT NOT NULL,
            PRIMARY KEY (account_id, question_id)
         );
         CREATE TABLE IF NOT EXISTS answer_history_progress (
            account_id TEXT PRIMARY KEY, next_offset INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS onboarding_finished (account_id TEXT PRIMARY KEY);
         CREATE INDEX IF NOT EXISTS idx_q_zhihu_question ON questions(question_id);",
    )?;
    if !has_column("questions", "fit_evaluated") {
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "ALTER TABLE questions ADD COLUMN fit_evaluated INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        tx.execute("UPDATE questions SET fit_evaluated=1 WHERE fit>0", [])?;
        crate::tasks::refresh_inbox_fit(&tx)?;
        tx.commit()?;
    }
    if !has_column("questions", "metrics") {
        conn.execute("ALTER TABLE questions ADD COLUMN metrics TEXT", [])?;
    }
    if !has_column("questions", "metrics_attempted_at") {
        conn.execute("ALTER TABLE questions ADD COLUMN metrics_attempted_at TEXT NOT NULL DEFAULT ''", [])?;
    }
    if !has_column("questions", "metrics_error") {
        conn.execute("ALTER TABLE questions ADD COLUMN metrics_error TEXT NOT NULL DEFAULT ''", [])?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS creator_answers (
            account_id TEXT NOT NULL,
            answer_id TEXT NOT NULL,
            question_id TEXT NOT NULL,
            title TEXT NOT NULL,
            excerpt TEXT NOT NULL,
            created_time INTEGER,
            updated_time INTEGER,
            metrics TEXT NOT NULL,
            synced_at TEXT NOT NULL,
            content TEXT,
            truncated INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (account_id, answer_id)
         );
         CREATE TABLE IF NOT EXISTS creator_answer_sync (
            account_id TEXT PRIMARY KEY,
            synced_at TEXT NOT NULL
         );",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_inbox_is_rescored_without_touching_drafts_or_status() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(&SCHEMA.replace("  fit_evaluated INTEGER NOT NULL DEFAULT 0,\n", ""))?;
        conn.execute_batch(
            "INSERT INTO profile_terms (id, kind, label, weight, keywords)
             VALUES ('primary', 'primary', 'Minecraft 原版机制解析', 0.9, '[\"我的世界\",\"基岩版\"]');
             INSERT INTO questions (id, title, domain, fit, status, bucket)
             VALUES ('match', '我的世界基岩版配装', '未分类', 0, 'inbox', 'queue'),
                    ('miss', '如何选显示器', '未分类', 0, 'inbox', 'queue'),
                    ('draft', '我的世界基岩版', '原有领域', 0.8, 'pending_review', 'selected'),
                    ('archive', '我的世界基岩版', '原有领域', 0.7, 'answered', 'archived');
             INSERT INTO drafts (question_id, markdown) VALUES ('draft', '手动编辑的原稿');",
        )?;
        let question = |id: &str| {
            conn.query_row(
                "SELECT fit, fit_evaluated, domain, status, bucket FROM questions WHERE id=?1",
                [id],
                |row| {
                    Ok((
                        row.get::<_, f64>(0)?,
                        row.get::<_, bool>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
        };
        migrate(&conn)?;
        let matched = question("match")?;
        assert!((matched.0 - 0.6366666666666667).abs() < 1e-12);
        assert!(matched.1);
        assert_eq!(matched.2, "Minecraft 原版机制解析");
        assert_eq!(matched.3, "inbox");
        assert_eq!(matched.4, "selected");
        assert_eq!(
            question("miss")?,
            (0.0, true, "未分类".into(), "inbox".into(), "queue".into())
        );

        conn.execute("DELETE FROM profile_terms", [])?;
        migrate(&conn)?;
        assert_eq!(question("match")?, matched);
        crate::tasks::refresh_inbox_fit(&conn)?;
        assert_eq!(
            question("match")?,
            (0.0, false, "未分类".into(), "inbox".into(), "queue".into())
        );
        assert_eq!(
            question("draft")?,
            (
                0.8,
                true,
                "原有领域".into(),
                "pending_review".into(),
                "selected".into()
            )
        );
        assert_eq!(
            question("archive")?,
            (
                0.7,
                true,
                "原有领域".into(),
                "answered".into(),
                "archived".into()
            )
        );
        assert_eq!(
            conn.query_row(
                "SELECT markdown FROM drafts WHERE question_id='draft'",
                [],
                |row| row.get::<_, String>(0)
            )?,
            "手动编辑的原稿"
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM jobs", [], |row| row.get::<_, i64>(0))?,
            0
        );
        Ok(())
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
