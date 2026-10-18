//! 应用状态：数据库、缓存、密钥环、邮件、存储与各类缓存键。

use crate::cache::Cache;
use crate::config::Config;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::i18n::Lang;
use crate::mail::Mailer;
use crate::repo::{self, Profile};
use crate::storage::Storage;
use crate::tasks::backup::BackupStatus;
use crate::util::{client_ip, now_ms, parse_cidrs, Cidr};
use axum::http::{HeaderMap, Uri};
use mongodb::bson::oid::ObjectId;
use mongodb::Database;
use sm_envelope::KeyRing;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

pub struct Secrets {
    pub jwt: Vec<u8>,
    pub server: Vec<u8>,
}

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: Database,
    pub cache: Cache,
    pub keyring: Arc<RwLock<KeyRing>>,
    pub storage: Storage,
    pub mailer: Mailer,
    pub secrets: Arc<Secrets>,
    pub trusted: Arc<Vec<Cidr>>,
    pub started_at: i64,
    pub backup: Arc<RwLock<BackupStatus>>,
    pub shutdown: CancellationToken,
}

impl AppState {
    pub fn lang(&self, headers: &HeaderMap, uri: &Uri) -> Lang {
        Lang::from_uri(uri).unwrap_or_else(|| Lang::from_headers(headers))
    }

    pub fn peer_ip(&self, extensions: &axum::http::Extensions) -> IpAddr {
        crate::util::peer_ip(extensions).unwrap_or(IpAddr::from([127, 0, 0, 1]))
    }

    /// 经过可信代理时取 `X-Forwarded-For`，否则用 socket IP。
    pub fn client_ip(&self, headers: &HeaderMap, extensions: &axum::http::Extensions) -> IpAddr {
        let peer = crate::util::peer_ip(extensions);
        client_ip(headers, peer, &self.trusted)
    }

    pub async fn load_profile(&self, id: ObjectId) -> AppResult<Profile> {
        let key = keys::user_profile(id);
        if let Some(profile) = self.cache.get_json::<Profile>(&key).await {
            return Ok(profile);
        }
        let user = repo::find_user_by_id(&self.db, id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
        let profile = Profile::from(&user);
        self.cache
            .set_json(&key, &profile, self.cfg.redis.ttl.user_cache_seconds)
            .await;
        Ok(profile)
    }

    /// `tokenVersion` 不允许陈旧：缓存 TTL 很短，变更时立即 DEL。
    pub async fn token_version(&self, id: ObjectId) -> AppResult<i32> {
        let key = keys::user_version(id);
        if let Some(raw) = self.cache.get_raw(&key).await {
            if let Ok(ver) = raw.parse::<i32>() {
                return Ok(ver);
            }
        }
        let user = repo::find_user_by_id(&self.db, id)
            .await?
            .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
        self.cache
            .set_ex(
                &key,
                &user.token_version.to_string(),
                self.cfg.redis.ttl.token_version_seconds,
            )
            .await;
        Ok(user.token_version)
    }

    pub async fn invalidate_profile(&self, id: ObjectId) {
        self.cache
            .del(&[
                keys::user_profile(id),
                keys::user_version(id),
                keys::stats_overview(),
            ])
            .await;
    }

    pub async fn invalidate_saves(&self, id: ObjectId) {
        self.cache
            .del(&[keys::saves_list(id), keys::stats_overview()])
            .await;
    }

    /// 单文件上限：以管理端设置为准，但不超过配置文件（受 body limit 约束）。
    pub async fn effective_max_file_size(&self) -> i64 {
        match crate::repo::get_settings(&self.db).await.ok().flatten() {
            Some(settings) => settings.max_file_size.clamp(1, self.cfg.storage.max_file_size),
            None => self.cfg.storage.max_file_size,
        }
    }

    pub fn uptime_seconds(&self) -> i64 {
        (now_ms() - self.started_at) / 1000
    }
}

pub mod keys {
    use mongodb::bson::oid::ObjectId;

    pub fn user_profile(id: ObjectId) -> String {
        format!("user:profile:{}", id.to_hex())
    }

    pub fn user_version(id: ObjectId) -> String {
        format!("user:ver:{}", id.to_hex())
    }

    pub fn saves_list(id: ObjectId) -> String {
        format!("saves:list:{}", id.to_hex())
    }

    pub fn stats_overview() -> String {
        "stats:overview".to_string()
    }

    pub fn jwt_blacklist(jti: &str) -> String {
        format!("jwt:bl:{jti}")
    }

    pub fn replay(digest: &str) -> String {
        format!("replay:{digest}")
    }

    pub fn save_lock(user_id: ObjectId, file_hash: &str) -> String {
        format!("lock:save:{}:{}", user_id.to_hex(), file_hash)
    }

    pub fn idem(route: &str, key: &str) -> String {
        format!("idem:{route}:{key}")
    }
}

/// 读取或生成 jwt / server 密钥；文件权限 0600。
pub fn load_or_create_secret(path: &Path, env_key: &str, bytes: usize) -> anyhow::Result<Vec<u8>> {
    if let Ok(value) = std::env::var(env_key) {
        if !value.trim().is_empty() {
            return Ok(value.into_bytes());
        }
    }
    if path.exists() {
        let data = std::fs::read(path)?;
        if data.len() >= 16 {
            return Ok(data);
        }
        anyhow::bail!("密钥文件 {} 长度异常", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let secret: Vec<u8> = crate::util::random_bytes(bytes);
    std::fs::write(path, &secret)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    tracing::info!(path = %path.display(), "已生成新的服务密钥");
    Ok(secret)
}

/// 构造共享状态（供 main 与测试复用）。
#[allow(clippy::too_many_arguments)]
pub fn build_state(
    cfg: Config,
    db: Database,
    cache: Cache,
    keyring: KeyRing,
    storage: Storage,
    mailer: Mailer,
    secrets: Secrets,
) -> AppState {
    let trusted = Arc::new(parse_cidrs(&cfg.server.trusted_proxy_cidrs));
    AppState {
        cfg: Arc::new(cfg),
        db,
        cache,
        keyring: Arc::new(RwLock::new(keyring)),
        storage,
        mailer,
        secrets: Arc::new(secrets),
        trusted,
        started_at: now_ms(),
        backup: Arc::new(RwLock::new(BackupStatus::default())),
        shutdown: CancellationToken::new(),
    }
}
