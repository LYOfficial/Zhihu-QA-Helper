//! 定时器按用户设定的时间点拉取知乎通知，并在当天去重。

use chrono::{Local, NaiveDateTime, Timelike};
use std::collections::HashSet;
use tokio::time::{sleep, Duration};

use crate::state::Shared;
use crate::tasks;

pub fn spawn(state: Shared) {
    tokio::spawn(async move {
        let mut fired: HashSet<String> = HashSet::new();
        loop {
            let now = Local::now();
            let times = {
                let db = state.db.lock().await;
                tasks::load_settings(&db).cron_times
            };
            let auto_sync = {
                let db = state.db.lock().await;
                let can_sync = db.conn.query_row(
                    "SELECT cookie<>'' FROM active_account", [], |row| row.get::<_, bool>(0)
                ).unwrap_or(false);
                can_sync && tasks::load_settings(&db).auto_sync && !tasks::onboarding_required(&db.conn).unwrap_or(true)
            };

            let today = now.format("%Y-%m-%d").to_string();
            for t in &times {
                let slot = format!("{today} {t}");
                if fired.contains(&slot) {
                    continue;
                }
                if let Ok(naive) = NaiveDateTime::parse_from_str(&slot, "%Y-%m-%d %H:%M") {
                    let target = naive.and_local_timezone(Local).single();
                    if let Some(target) = target {
                        // 命中当前分钟（容忍 59 秒调度漂移）
                        if now.timestamp() / 60 == target.timestamp() / 60
                            && now.minute() == target.minute()
                        {
                            fired.insert(slot.clone());
                            if auto_sync {
                                tracing::info!(target: "scheduler", "定时同步触发：{slot}");
                                let st = state.clone();
                                tokio::spawn(async move {
                                    match tasks::run_sync(&st).await {
                                        Ok(r) => tracing::info!("定时同步完成：{}", r.message),
                                        Err(e) => tracing::warn!("定时同步失败：{e}"),
                                    }
                                });
                            }
                        }
                    }
                }
            }

            // 跨天清理去重集合
            if now.hour() == 0 && now.minute() == 0 {
                fired.retain(|s| s.starts_with(&today));
            }

            sleep(Duration::from_secs(20)).await;
        }
    });
}
