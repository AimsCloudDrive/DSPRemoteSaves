//! DSP 云存档运维 CLI：创建管理员、配额对账、备份/恢复、密钥轮换。

use acc_server::auth::password::{ensure_strong, hash_password, Argon2Params};
use acc_server::config::Config;
use acc_server::repo::{self, NewUser, ROLE_ADMIN, ROLE_USER};
use acc_server::state::AppState;
use acc_server::tasks::{backup, reconcile, s3::S3Client};
use clap::{Parser, Subcommand};
use std::path::Path;

#[derive(Parser)]
#[command(name = "acc-cli", version, about = "DSP 云存档运维工具")]
struct Cli {
    /// 配置文件路径（默认取 ACC_CONFIG 或 ./config.toml）
    #[arg(long, global = true, env = "ACC_CONFIG", default_value = "./config.toml")]
    config: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 管理员账号
    Admin {
        #[command(subcommand)]
        action: AdminAction,
    },
    /// 立即执行一次配额对账
    Reconcile,
    /// 备份与恢复
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },
    /// SM2 密钥
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
    /// 列出用户
    Users {
        #[arg(long, default_value_t = 50)]
        limit: u64,
    },
}

#[derive(Subcommand)]
enum AdminAction {
    /// 创建管理员（首次部署用）
    Create {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
        #[arg(long)]
        email: Option<String>,
        /// 配额（字节），默认 1GB
        #[arg(long)]
        quota: Option<i64>,
        /// 创建为普通用户
        #[arg(long, default_value_t = false)]
        user: bool,
    },
    /// 修改用户密码（不使用密文，仅限本机运维）
    Passwd {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: String,
    },
}

#[derive(Subcommand)]
enum BackupAction {
    /// 立即备份到 MinIO
    Run,
    /// 从 MinIO 恢复（会覆盖当前数据，务必先停服）
    Restore {
        /// 日期，格式 YYYY-MM-DD
        #[arg(long)]
        date: String,
    },
    /// 仅打印恢复步骤
    Hint {
        #[arg(long)]
        date: String,
    },
}

#[derive(Subcommand)]
enum KeyAction {
    /// 轮换 SM2 密钥并清理超出宽限期的旧密钥
    Rotate,
    /// 查看当前 keyId
    Info,
}

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("错误: {err:#}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load_from(Path::new(&cli.config))?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .compact()
        .init();

    let (state, _mail_rx) = acc_server::bootstrap(cfg).await?;

    match cli.command {
        Command::Admin { action } => admin_action(&state, action).await,
        Command::Reconcile => {
            let report = reconcile::run_once(&state).await?;
            println!(
                "对账完成：检查 {} 个用户，修正 {} 个，清理孤儿目录 {} 个",
                report.users_checked, report.users_fixed, report.orphan_temp_removed
            );
            Ok(())
        }
        Command::Backup { action } => backup_action(&state, action).await,
        Command::Key { action } => key_action(&state, action).await,
        Command::Users { limit } => {
            let (users, total) = repo::list_users(&state.db, None, None, None, 0, limit).await?;
            println!("共 {total} 个用户，显示 {} 个：", users.len());
            for user in users {
                println!(
                    "{:<24} {:<6} {:<9} quota={:<12} used={}",
                    user.user_name, user.role, user.status, user.quota_bytes, user.used_bytes
                );
            }
            Ok(())
        }
    }
}

fn argon_params(state: &AppState) -> Argon2Params {
    Argon2Params {
        memory_kib: state.cfg.auth.argon2_memory_kib,
        iterations: state.cfg.auth.argon2_iterations,
        parallelism: state.cfg.auth.argon2_parallelism,
    }
}

async fn admin_action(state: &AppState, action: AdminAction) -> anyhow::Result<()> {
    match action {
        AdminAction::Create {
            username,
            password,
            email,
            quota,
            user: as_user,
        } => {
            let username = username.trim().to_string();
            if !acc_server::util::validate_user_name(&username) {
                anyhow::bail!("用户名非法：3~32 位字母/数字/下划线/中文");
            }
            let email = match email.as_deref() {
                Some(value) => Some(
                    acc_server::util::normalize_email(value)
                        .ok_or_else(|| anyhow::anyhow!("邮箱格式非法"))?,
                ),
                None => None,
            };
            ensure_strong(&password, &username, email.as_deref().unwrap_or(""))
                .map_err(|err| anyhow::anyhow!("密码强度不足: {err}"))?;

            if repo::find_user_by_name(&state.db, &username).await?.is_some() {
                anyhow::bail!("用户名已存在：{username}");
            }
            if let Some(email) = &email {
                if repo::find_user_by_email(&state.db, email).await?.is_some() {
                    anyhow::bail!("邮箱已被使用：{email}");
                }
            }

            let hash = hash_password(password, argon_params(state))
                .await
                .map_err(|err| anyhow::anyhow!("密码哈希失败: {err}"))?;
            let user = repo::create_user(
                &state.db,
                NewUser {
                    user_name: &username,
                    email: email.as_deref(),
                    password_hash: &hash,
                    role: if as_user { ROLE_USER } else { ROLE_ADMIN },
                    quota_bytes: quota.unwrap_or(state.cfg.quota.default_bytes),
                    locale: "zh-CN",
                    email_verified: email.is_some(),
                },
            )
            .await?;
            println!(
                "已创建 {}：{} ({})",
                if as_user { "用户" } else { "管理员" },
                user.user_name,
                user.id.to_hex()
            );
            Ok(())
        }
        AdminAction::Passwd { username, password } => {
            let user = repo::find_user_by_name(&state.db, &username)
                .await?
                .ok_or_else(|| anyhow::anyhow!("用户不存在：{username}"))?;
            ensure_strong(&password, &user.user_name, user.email.as_deref().unwrap_or(""))
                .map_err(|err| anyhow::anyhow!("密码强度不足: {err}"))?;
            let hash = hash_password(password, argon_params(state))
                .await
                .map_err(|err| anyhow::anyhow!("密码哈希失败: {err}"))?;
            repo::update_user(
                &state.db,
                user.id,
                mongodb::bson::doc! { "passwordHash": hash, "mustChangePassword": false },
            )
            .await?;
            repo::bump_token_version(&state.db, user.id, Some(false)).await?;
            repo::delete_user_sessions(&state.db, user.id).await?;
            state.invalidate_profile(user.id).await;
            println!("已重置 {} 的密码，所有会话已失效", user.user_name);
            Ok(())
        }
    }
}

async fn backup_action(state: &AppState, action: BackupAction) -> anyhow::Result<()> {
    match action {
        BackupAction::Run => {
            if !state.cfg.backup.enabled {
                anyhow::bail!("配置中 backup.enabled=false，无法执行备份");
            }
            match backup::run_once(state).await? {
                Some(object) => println!("备份完成：{object}"),
                None => println!("已有备份在运行，本次跳过"),
            }
            Ok(())
        }
        BackupAction::Hint { date } => {
            println!(
                "{}",
                backup::restore_hint(&date, &state.cfg.storage.saves_dir)
            );
            Ok(())
        }
        BackupAction::Restore { date } => {
            let cfg = &state.cfg.backup;
            let client = S3Client::new(
                &cfg.minio.endpoint,
                &cfg.minio.access_key,
                &cfg.minio.secret_key,
                &cfg.minio.region,
                &cfg.bucket,
            )?;

            let objects = client.list("mongo/").await?;
            let mongo_key = objects
                .iter()
                .map(|(key, _)| key)
                .find(|key| key.ends_with(&format!("{date}.archive.gz")))
                .ok_or_else(|| anyhow::anyhow!("未找到 {date} 的 Mongo 备份"))?
                .clone();

            let saves_objects = client.list("saves/").await?;
            let saves_key = saves_objects
                .iter()
                .map(|(key, _)| key)
                .find(|key| key.contains(&date))
                .ok_or_else(|| anyhow::anyhow!("未找到 {date} 的存档备份"))?
                .clone();

            let work = cfg.work_dir.join("restore");
            tokio::fs::create_dir_all(&work).await?;
            let mongo_file = work.join("restore.archive.gz");
            let saves_file = work.join("restore.archive");

            println!("下载 {mongo_key} ...");
            client.get_file(&mongo_key, &mongo_file).await?;
            println!("下载 {saves_key} ...");
            client.get_file(&saves_key, &saves_file).await?;

            println!("恢复 MongoDB（--drop）...");
            let status = tokio::process::Command::new("mongorestore")
                .arg(format!("--uri={}", state.cfg.database.uri))
                .arg(format!("--archive={}", mongo_file.display()))
                .arg("--gzip")
                .arg("--drop")
                .status()
                .await?;
            if !status.success() {
                anyhow::bail!("mongorestore 失败");
            }

            println!("解包存档到 {} ...", state.cfg.storage.saves_dir.display());
            let parent = state
                .cfg
                .storage
                .saves_dir
                .parent()
                .ok_or_else(|| anyhow::anyhow!("saves_dir 非法"))?;
            let mut tar = tokio::process::Command::new("tar");
            if saves_key.ends_with(".zst") {
                tar.arg("--zstd");
            } else {
                tar.arg("-z");
            }
            let status = tar
                .arg("-xf")
                .arg(&saves_file)
                .arg("-C")
                .arg(parent)
                .status()
                .await?;
            if !status.success() {
                anyhow::bail!("解包存档失败");
            }

            println!("恢复完成。请重启 acc-server，并核对用户/版本/文件。");
            println!(
                "{}",
                backup::restore_hint(&date, &state.cfg.storage.saves_dir)
            );
            Ok(())
        }
    }
}

async fn key_action(state: &AppState, action: KeyAction) -> anyhow::Result<()> {
    match action {
        KeyAction::Info => {
            let ring = state.keyring.read().await;
            let info = ring.public_key_info();
            println!("active keyId: {}", info.key_id);
            println!("publicKey:    {}", info.public_key);
            println!("keys:         {:?}", ring.key_ids());
            Ok(())
        }
        KeyAction::Rotate => {
            let mut ring = state.keyring.write().await;
            let new_id = ring
                .rotate()
                .map_err(|err| anyhow::anyhow!("轮换失败: {err}"))?;
            let pruned = ring
                .prune(state.cfg.crypto.key_grace_hours)
                .map_err(|err| anyhow::anyhow!("清理旧密钥失败: {err}"))?;
            println!("新 keyId: {new_id}");
            if pruned.is_empty() {
                println!("没有需要清理的旧密钥（保留 {} 小时）", state.cfg.crypto.key_grace_hours);
            } else {
                println!("已清理旧密钥: {pruned:?}");
            }
            Ok(())
        }
    }
}
