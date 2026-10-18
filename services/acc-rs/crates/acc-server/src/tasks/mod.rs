//! 后台任务：过期上传清理、配额对账、每日备份。

pub mod backup;
pub mod cleanup;
pub mod reconcile;
pub mod s3;

use crate::state::AppState;
use tokio::task::JoinHandle;

/// 启动全部后台任务；返回句柄便于测试中取消。
pub fn spawn_all(state: AppState) -> Vec<JoinHandle<()>> {
    vec![
        cleanup::spawn(state.clone()),
        reconcile::spawn(state.clone()),
        backup::spawn(state),
    ]
}
