//! 每 10 分钟对账：按 `save_versions` 聚合修正 `usedBytes`，按活跃上传会话修正 `reservedBytes`，
//! 并清理 DB 中不存在的孤儿临时目录。
//!
//! Mongo 单机没有多文档事务，这里用“幂等重算”代替事务。

use crate::repo;
use crate::state::AppState;
use mongodb::bson::doc;
use std::collections::HashMap;
use tracing::{info, warn};

pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(600));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(err) = run_once(&state).await {
                        warn!(error = %err, "配额对账失败");
                    }
                }
                _ = state.shutdown.cancelled() => {
                    info!("对账任务退出");
                    return;
                }
            }
        }
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ReconcileReport {
    pub users_checked: usize,
    pub users_fixed: usize,
    pub orphan_temp_removed: usize,
}

pub async fn run_once(state: &AppState) -> anyhow::Result<ReconcileReport> {
    let usage: HashMap<_, _> = repo::aggregate_usage(&state.db).await?.into_iter().collect();
    let sessions = repo::active_upload_sessions(&state.db).await?;
    let mut reserved: HashMap<_, i64> = HashMap::new();
    for session in sessions {
        *reserved.entry(session.user_id).or_insert(0) += session.file_size;
    }

    let users = repo::all_users(&state.db).await?;
    let mut report = ReconcileReport {
        users_checked: users.len(),
        ..Default::default()
    };

    for user in users {
        let used = *usage.get(&user.id).unwrap_or(&0);
        let reserved_now = *reserved.get(&user.id).unwrap_or(&0);
        if used != user.used_bytes || reserved_now != user.reserved_bytes {
            state
                .db
                .collection::<mongodb::bson::Document>("users")
                .update_one(
                    doc! { "_id": user.id },
                    doc! { "$set": { "usedBytes": used.max(0), "reservedBytes": reserved_now.max(0) } },
                )
                .await?;
            report.users_fixed += 1;
            info!(
                user = %user.user_name,
                from_used = user.used_bytes,
                to_used = used,
                from_reserved = user.reserved_bytes,
                to_reserved = reserved_now,
                "配额已对账"
            );
        }
    }

    report.orphan_temp_removed = state.storage.clean_orphan_temp(&state.db).await;
    Ok(report)
}
