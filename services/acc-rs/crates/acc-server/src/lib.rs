//! DSP 云存档服务（Rust / axum）。
//!
//! 启动顺序：配置 → 日志 → Mongo → Redis → 集合/索引 → SM2 keyring → 密钥文件
//! → 后台任务 → 监听端口。

pub mod api;
pub mod auth;
pub mod cache;
pub mod config;
pub mod crypto;
pub mod error;
pub mod i18n;
pub mod mail;
pub mod repo;
pub mod state;
pub mod storage;
pub mod tasks;
pub mod util;
pub mod web;

use crate::cache::Cache;
use crate::config::Config;
use crate::mail::{MailJob, Mailer};
use crate::state::{build_state, load_or_create_secret, AppState, Secrets};
use crate::storage::Storage;
use mongodb::Client;
use sm_envelope::KeyRing;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

pub async fn run() -> anyhow::Result<()> {
    let cfg = Config::load()?;
    init_tracing(&cfg);

    let active_key_hint = cfg.crypto.key_dir.clone();
    let (state, mail_rx) = bootstrap(cfg).await?;
    let mail_worker = state.mailer.start(mail_rx);

    // 启动清理：过期会话、孤儿 temp
    if let Err(err) = tasks::cleanup::run_once(&state).await {
        warn!(error = %err, "启动清理失败");
    }
    state.storage.clean_orphan_temp(&state.db).await;

    // 后台任务
    let task_handles = tasks::spawn_all(state.clone());

    let app = api::build_router(state.clone());
    let listener = TcpListener::bind(&state.cfg.server.bind).await?;
    let local_addr = listener.local_addr()?;
    let key_id = state.keyring.read().await.active_key_id().to_string();
    info!(
        addr = %local_addr,
        key_dir = %active_key_hint.display(),
        key_id = %key_id,
        "服务已启动"
    );

    let shutdown_state = state.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        info!("收到退出信号，开始优雅关闭");
        shutdown_state.shutdown.cancel();
    })
    .await?;

    mail_worker.abort();
    for handle in task_handles {
        handle.abort();
    }
    info!("服务已停止");
    Ok(())
}

/// 初始化全部依赖（服务进程与 CLI 共用）。
pub async fn bootstrap(cfg: Config) -> anyhow::Result<(AppState, tokio::sync::mpsc::Receiver<MailJob>)> {
    info!(
        bind = %cfg.server.bind,
        base_path = %cfg.server.base_path,
        db = %cfg.database.db,
        "初始化 DSP 云存档服务"
    );

    // 1) MongoDB
    let client = Client::with_uri_str(&cfg.database.uri).await?;
    let db = if cfg.database.db.is_empty() {
        client.default_database()
    } else {
        Some(client.database(&cfg.database.db))
    }
    .ok_or_else(|| anyhow::anyhow!("未指定数据库名"))?;
    db.run_command(mongodb::bson::doc! { "ping": 1 }).await?;
    repo::init(&db).await?;
    info!("MongoDB 集合与索引已就绪");

    // 2) Redis（失败不阻塞启动）
    let cache = Cache::connect(
        &cfg.redis.url,
        &cfg.redis.prefix,
        cfg.redis.enabled,
        cfg.redis.command_timeout_ms,
    )
    .await;
    {
        let cache = cache.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                ticker.tick().await;
                cache.ensure_connected().await;
            }
        });
    }

    // 3) SM2 keyring（不打印私钥）
    let passphrase = cfg.sm2_passphrase();
    if passphrase.is_none() {
        warn!("未设置 SM2_KEY_PASSPHRASE，私钥仅依赖文件权限 0600 保护");
    }
    let keyring = KeyRing::load_or_create(&cfg.crypto.key_dir, passphrase.as_deref())?;

    // 4) 存储目录
    let storage = Storage::new(
        cfg.storage.saves_dir.clone(),
        cfg.storage.max_file_size,
        cfg.storage.max_chunk_size,
        cfg.storage.max_total_chunks,
        cfg.storage.min_free_bytes,
        cfg.storage.upload_idle_seconds,
        cfg.storage.upload_max_hours,
    );
    storage.ensure_layout().await?;

    // 5) JWT / 服务端密钥
    let jwt_secret = load_or_create_secret(&cfg.auth.jwt_secret_file, "JWT_SECRET", 48)?;
    let server_secret = load_or_create_secret(&cfg.auth.server_secret_file, "SERVER_SECRET", 48)?;

    // 6) 邮件（worker 由调用方启动）
    let (mailer, rx) = Mailer::build(
        cfg.mail.clone(),
        cache.clone(),
        cfg.email_code.queue_capacity,
    );

    let state = build_state(
        cfg,
        db,
        cache,
        keyring,
        storage,
        mailer,
        Secrets {
            jwt: jwt_secret,
            server: server_secret,
        },
    );
    Ok((state, rx))
}

fn init_tracing(cfg: &Config) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(cfg.log.level.clone()));

    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if cfg.log.format.eq_ignore_ascii_case("json") {
        builder.json().with_current_span(true).init();
    } else {
        builder.compact().init();
    }
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("安装 SIGTERM 处理器");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = term.recv() => {},
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
