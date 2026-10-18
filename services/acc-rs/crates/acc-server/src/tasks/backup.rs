//! 每日备份：Mongo dump + saves 归档 → MinIO，保留 30 天。

use crate::state::AppState;
use crate::tasks::s3::S3Client;
use chrono::{Datelike, Local, NaiveTime, TimeZone, Timelike};
use serde::Serialize;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupStatus {
    pub enabled: bool,
    pub running: bool,
    pub last_run_at: Option<i64>,
    pub last_ok_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_object: Option<String>,
    pub last_duration_ms: Option<i64>,
}

/// 启动每日备份任务（本地时间 `backup.at`，默认 03:00）。
pub fn spawn(state: AppState) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        {
            let mut status = state.backup.write().await;
            status.enabled = state.cfg.backup.enabled;
        }
        if !state.cfg.backup.enabled {
            info!("备份任务未启用");
            return;
        }

        loop {
            let Some(wait) = until_next(&state.cfg.backup.at) else {
                error!(at = %state.cfg.backup.at, "backup.at 格式非法，备份任务退出");
                return;
            };
            info!(seconds = wait.as_secs(), "距离下次备份");

            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = state.shutdown.cancelled() => {
                    info!("备份任务退出");
                    return;
                }
            }

            if let Err(err) = run_once(&state).await {
                error!(error = %err, "备份失败");
                let mut status = state.backup.write().await;
                status.last_error = Some(err.to_string());
                status.last_run_at = Some(crate::util::now_ms());
            }
        }
    })
}

fn until_next(at: &str) -> Option<std::time::Duration> {
    let time = NaiveTime::parse_from_str(at.trim(), "%H:%M").ok()?;
    let now = Local::now();
    let mut target = Local
        .with_ymd_and_hms(now.year(), now.month(), now.day(), time.hour(), time.minute(), 0)
        .single()?;
    if target <= now {
        target += chrono::Duration::days(1);
    }
    (target - now).to_std().ok()
}

/// 立即执行一次备份；返回上传对象前缀。
pub async fn run_once(state: &AppState) -> anyhow::Result<Option<String>> {
    if !state.cfg.backup.enabled {
        anyhow::bail!("备份未启用");
    }
    let lock_key = "lock:backup";
    let owner = uuid::Uuid::new_v4().to_string();
    if !state.cache.set_nx(lock_key, &owner, 3600).await {
        info!("已有备份在运行，跳过本次");
        return Ok(None);
    }

    let started = std::time::Instant::now();
    {
        let mut status = state.backup.write().await;
        status.running = true;
        status.last_run_at = Some(crate::util::now_ms());
        status.last_error = None;
    }

    let result = do_backup(state).await;

    {
        let mut status = state.backup.write().await;
        status.running = false;
        status.last_duration_ms = Some(started.elapsed().as_millis() as i64);
        match &result {
            Ok(Some(object)) => {
                status.last_ok_at = Some(crate::util::now_ms());
                status.last_object = Some(object.clone());
            }
            Ok(None) => {}
            Err(err) => status.last_error = Some(err.to_string()),
        }
    }
    state.cache.release_lock(lock_key, &owner).await;

    match &result {
        Ok(object) => {
            info!(?object, "备份完成");
            Ok(object.clone())
        }
        Err(err) => Err(anyhow::anyhow!(err.to_string())),
    }
}

async fn do_backup(state: &AppState) -> anyhow::Result<Option<String>> {
    let cfg = &state.cfg.backup;
    if cfg.minio.access_key.is_empty() || cfg.minio.secret_key.is_empty() {
        anyhow::bail!("MinIO 凭据未配置");
    }

    let work = cfg.work_dir.clone();
    tokio::fs::create_dir_all(&work).await?;
    let date = Local::now().format("%Y-%m-%d").to_string();

    // 1) Mongo dump
    let dump_path = work.join(format!("mongo-{date}.archive.gz"));
    let mut cmd = Command::new("mongodump");
    cmd.arg(format!("--uri={}", state.cfg.database.uri))
        .arg(format!("--archive={}", dump_path.display()))
        .arg("--gzip")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    if !state.cfg.database.db.is_empty() {
        cmd.arg(format!("--db={}", state.cfg.database.db));
    }
    run_command(cmd, "mongodump").await?;

    // 2) saves 归档（含 .versions）
    let saves_dir = &state.cfg.storage.saves_dir;
    let parent = saves_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("saves_dir 缺少父目录"))?;
    let name = saves_dir
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| anyhow::anyhow!("saves_dir 非法"))?;

    let zstd_path = work.join(format!("saves-{date}.tar.zst"));
    let mut tar = Command::new("tar");
    tar.arg("--zstd")
        .arg("-cf")
        .arg(&zstd_path)
        .arg("-C")
        .arg(parent)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let (archive_path, archive_name) = match run_command(tar, "tar --zstd").await {
        Ok(_) => (zstd_path, format!("saves/{date}.tar.zst")),
        Err(err) => {
            warn!(error = %err, "zstd 归档失败，回退 gzip");
            let gz_path = work.join(format!("saves-{date}.tar.gz"));
            let mut tar = Command::new("tar");
            tar.arg("-czf")
                .arg(&gz_path)
                .arg("-C")
                .arg(parent)
                .arg(name)
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            run_command(tar, "tar -czf").await?;
            (gz_path, format!("saves/{date}.tar.gz"))
        }
    };

    // 3) 上传
    let client = S3Client::new(
        &cfg.minio.endpoint,
        &cfg.minio.access_key,
        &cfg.minio.secret_key,
        &cfg.minio.region,
        &cfg.bucket,
    )?;
    client
        .put_file(&format!("mongo/{date}.archive.gz"), &dump_path)
        .await?;
    client.put_file(&archive_name, &archive_path).await?;

    // 4) 清理过期对象
    if let Err(err) = prune(&client, cfg.retention_days).await {
        warn!(error = %err, "清理过期备份失败");
    }

    // 5) 删除本地临时文件
    let _ = tokio::fs::remove_file(&dump_path).await;
    let _ = tokio::fs::remove_file(&archive_path).await;

    Ok(Some(format!("mongo/{date}.archive.gz, {archive_name}")))
}

async fn prune(client: &S3Client, retention_days: i64) -> anyhow::Result<()> {
    let cutoff = (Local::now() - chrono::Duration::days(retention_days.max(1)))
        .format("%Y-%m-%d")
        .to_string();

    for prefix in ["mongo/", "saves/"] {
        for (key, _) in client.list(prefix).await? {
            // 文件名以 YYYY-MM-DD 开头
            let name = key.rsplit('/').next().unwrap_or("");
            if name.len() >= 10 {
                let date = &name[..10];
                if date < cutoff.as_str() {
                    if let Err(err) = client.delete(&key).await {
                        warn!(key = %key, error = %err, "删除过期备份失败");
                    } else {
                        info!(key = %key, "已删除过期备份");
                    }
                }
            }
        }
    }
    Ok(())
}

async fn run_command(mut cmd: Command, label: &str) -> anyhow::Result<()> {
    let output = cmd
        .output()
        .await
        .map_err(|err| anyhow::anyhow!("执行 {label} 失败（命令不存在？）: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{label} 退出码 {:?}: {stderr}", output.status.code());
    }
    Ok(())
}

/// 恢复说明（供 `acc-cli backup restore` 引用）。
pub fn restore_hint(date: &str, saves_dir: &Path) -> String {
    format!(
        "停服后执行：\n  mongorestore --uri=<uri> --archive=<mongo-{date}.archive.gz> --gzip --drop\n  tar --zstd -xf <saves-{date}.tar.zst> -C {}\n然后重启服务。",
        saves_dir.display()
    )
}
