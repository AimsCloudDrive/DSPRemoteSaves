//! 每 60 秒清理过期上传会话：释放预留配额、删会话、删临时分片。

use crate::repo;
use crate::state::AppState;
use tracing::{info, warn};

pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(err) = run_once(&state).await {
                        warn!(error = %err, "清理任务失败");
                    }
                }
                _ = state.shutdown.cancelled() => {
                    info!("清理任务退出");
                    return;
                }
            }
        }
    })
}

pub async fn run_once(state: &AppState) -> anyhow::Result<usize> {
    let expired = repo::expired_upload_sessions(&state.db, 200).await?;
    let mut removed = 0;
    for session in expired {
        // 先删会话，避免重复释放预留
        repo::delete_upload_session(&state.db, &session.id).await?;
        repo::release_reservation(&state.db, session.user_id, session.file_size).await?;
        state.storage.remove_temp_dir(&session.id).await;
        removed += 1;
    }
    if removed > 0 {
        info!(removed, "已清理过期上传会话");
    }
    Ok(removed)
}
