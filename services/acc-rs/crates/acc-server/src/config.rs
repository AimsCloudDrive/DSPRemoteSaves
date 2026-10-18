//! 配置：TOML + `${ENV}` 展开 + `ACC_` 前缀环境变量覆盖。
//!
//! 覆盖规则：环境变量 `ACC_SERVER__BIND` 覆盖 `[server] bind`（双下划线分层）。

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub auth: AuthConfig,
    pub crypto: CryptoConfig,
    pub storage: StorageConfig,
    pub quota: QuotaConfig,
    pub email_code: EmailCodeConfig,
    pub mail: MailConfig,
    pub backup: BackupConfig,
    pub log: LogConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: Default::default(),
            database: Default::default(),
            redis: Default::default(),
            auth: Default::default(),
            crypto: Default::default(),
            storage: Default::default(),
            quota: Default::default(),
            email_code: Default::default(),
            mail: Default::default(),
            backup: Default::default(),
            log: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub base_path: String,
    pub trusted_proxy_cidrs: Vec<String>,
    /// 对外访问地址，用于 refresh Cookie 的 Path / 邮件链接
    pub public_url: String,
    pub request_timeout_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "0.0.0.0:9999".into(),
            base_path: "/dsp.saves/api".into(),
            trusted_proxy_cidrs: vec![],
            public_url: "http://localhost:9999".into(),
            request_timeout_seconds: 900,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub uri: String,
    pub db: String,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            uri: "mongodb://127.0.0.1:27017".into(),
            db: "DSPCloudSaves".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedisConfig {
    pub enabled: bool,
    pub url: String,
    pub prefix: String,
    pub pool_size: u32,
    pub connect_timeout_ms: u64,
    pub command_timeout_ms: u64,
    pub ttl: RedisTtlConfig,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            url: "redis://127.0.0.1:6379/0".into(),
            prefix: "dsp:dev:v1:".into(),
            pool_size: 16,
            connect_timeout_ms: 1000,
            command_timeout_ms: 500,
            ttl: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedisTtlConfig {
    pub replay_seconds: u64,
    pub token_version_seconds: u64,
    pub user_cache_seconds: u64,
    pub file_list_seconds: u64,
    pub stats_seconds: u64,
}

impl Default for RedisTtlConfig {
    fn default() -> Self {
        Self {
            replay_seconds: 600,
            token_version_seconds: 30,
            user_cache_seconds: 30,
            file_list_seconds: 30,
            stats_seconds: 60,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub access_ttl_minutes: i64,
    pub refresh_ttl_days: i64,
    pub jwt_secret_file: PathBuf,
    pub server_secret_file: PathBuf,
    pub admin_idle_minutes: i64,
    pub argon2_memory_kib: u32,
    pub argon2_iterations: u32,
    pub argon2_parallelism: u32,
    pub cookie_secure: bool,
    /// 允许的浏览器 Origin；为空表示只允许同源
    pub allowed_origins: Vec<String>,
    pub max_login_failures: u32,
    pub login_failure_window_seconds: u64,
    pub login_lock_seconds: u64,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            access_ttl_minutes: 120,
            refresh_ttl_days: 7,
            jwt_secret_file: PathBuf::from("./data/jwt.secret"),
            server_secret_file: PathBuf::from("./data/server.secret"),
            admin_idle_minutes: 30,
            argon2_memory_kib: 19_456,
            argon2_iterations: 2,
            argon2_parallelism: 1,
            cookie_secure: true,
            allowed_origins: vec![],
            max_login_failures: 5,
            login_failure_window_seconds: 600,
            login_lock_seconds: 900,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CryptoConfig {
    pub key_dir: PathBuf,
    pub key_grace_hours: u64,
    pub passphrase_env: String,
    pub timestamp_tolerance_seconds: i64,
}

impl Default for CryptoConfig {
    fn default() -> Self {
        Self {
            key_dir: PathBuf::from("./data/keys"),
            key_grace_hours: 24,
            passphrase_env: "SM2_KEY_PASSPHRASE".into(),
            timestamp_tolerance_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub saves_dir: PathBuf,
    pub max_file_size: i64,
    pub max_chunk_size: i64,
    pub max_total_chunks: i64,
    pub min_free_bytes: u64,
    pub upload_idle_seconds: u64,
    pub upload_max_hours: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            saves_dir: PathBuf::from("./saves"),
            max_file_size: 2 * 1024 * 1024 * 1024,
            max_chunk_size: 64 * 1024 * 1024,
            max_total_chunks: 20_000,
            min_free_bytes: 2 * 1024 * 1024 * 1024,
            upload_idle_seconds: 600,
            upload_max_hours: 24,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QuotaConfig {
    pub default_bytes: i64,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            default_bytes: 1024 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EmailCodeConfig {
    pub length: usize,
    pub ttl_seconds: u64,
    pub resend_seconds: u64,
    pub max_attempts: u32,
    pub per_email_per_hour: u32,
    pub per_email_per_day: u32,
    pub per_ip_per_hour: u32,
    pub per_ip_per_day: u32,
    pub per_domain_per_hour: u32,
    pub global_per_hour: u32,
    pub global_per_day: u32,
    pub queue_capacity: usize,
    pub register_per_ip_per_day: u32,
    pub verify_failures_per_hour: u32,
    pub disposable_domains: Vec<String>,
}

impl Default for EmailCodeConfig {
    fn default() -> Self {
        Self {
            length: 6,
            ttl_seconds: 300,
            resend_seconds: 60,
            max_attempts: 5,
            per_email_per_hour: 5,
            per_email_per_day: 10,
            per_ip_per_hour: 10,
            per_ip_per_day: 30,
            per_domain_per_hour: 100,
            global_per_hour: 100,
            global_per_day: 450,
            queue_capacity: 100,
            register_per_ip_per_day: 3,
            verify_failures_per_hour: 20,
            disposable_domains: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MailConfig {
    pub enabled: bool,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// implicit（465）或 starttls（587）
    pub smtp_tls: String,
    pub from_address: String,
    pub from_name: String,
    pub username: String,
    pub password: String,
    pub daily_limit: u32,
}

impl Default for MailConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            smtp_host: "smtp.exmail.qq.com".into(),
            smtp_port: 465,
            smtp_tls: "implicit".into(),
            from_address: "no-reply@example.com".into(),
            from_name: "DSP 云存档".into(),
            username: String::new(),
            password: String::new(),
            daily_limit: 500,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BackupConfig {
    pub enabled: bool,
    /// `HH:MM` 本地时间，默认 03:00
    pub at: String,
    pub retention_days: i64,
    pub encrypt: bool,
    pub bucket: String,
    pub work_dir: PathBuf,
    pub minio: MinioConfig,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            at: "03:00".into(),
            retention_days: 30,
            encrypt: false,
            bucket: "dsp-saves-backup".into(),
            work_dir: PathBuf::from("./data/backup"),
            minio: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MinioConfig {
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub use_path_style: bool,
}

impl Default for MinioConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:9000".into(),
            access_key: String::new(),
            secret_key: String::new(),
            region: "us-east-1".into(),
            use_path_style: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    pub level: String,
    pub format: String,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: "json".into(),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let _ = dotenvy::dotenv();
        let path = std::env::var("ACC_CONFIG").unwrap_or_else(|_| "./config.toml".to_string());
        Self::load_from(Path::new(&path))
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("读取配置 {} 失败: {e}", path.display()))?;
        let expanded = expand_env(&raw)?;

        let figment = Figment::new()
            .merge(Toml::string(&expanded))
            .merge(Env::prefixed("ACC_").split("__"));

        let cfg: Config = figment
            .extract()
            .map_err(|e| anyhow::anyhow!("配置解析失败: {e}"))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.server.base_path.is_empty() || !self.server.base_path.starts_with('/') {
            anyhow::bail!("server.base_path 必须以 / 开头");
        }
        if self.storage.max_chunk_size < 1024 * 1024 {
            anyhow::bail!("storage.max_chunk_size 不能小于 1MB");
        }
        if self.auth.access_ttl_minutes <= 0 || self.auth.refresh_ttl_days <= 0 {
            anyhow::bail!("token 有效期必须为正数");
        }
        Ok(())
    }

    /// 从环境变量读取 SM2 私钥口令（不落配置文件）。
    pub fn sm2_passphrase(&self) -> Option<String> {
        std::env::var(&self.crypto.passphrase_env)
            .ok()
            .filter(|v| !v.trim().is_empty())
    }
}

/// 将 TOML 中的 `${VAR}` 替换为环境变量值；未定义则报错，避免静默使用空密码。
fn expand_env(input: &str) -> anyhow::Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(pos) = rest.find("${") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 2..];
        let Some(end) = after.find('}') else {
            anyhow::bail!("配置中的 ${{ 缺少闭合 }}");
        };
        let expr = after[..end].trim();
        if expr.is_empty() {
            anyhow::bail!("配置中存在空的 ${{}} 占位符");
        }
        // 支持 ${VAR:-默认值}
        let (key, default) = match expr.split_once(":-") {
            Some((key, default)) => (key.trim(), Some(default)),
            None => (expr, None),
        };
        match std::env::var(key) {
            Ok(value) => out.push_str(&value),
            Err(_) => match default {
                Some(value) => out.push_str(value),
                None => anyhow::bail!(
                    "环境变量 {key} 未设置（配置中引用为 ${{{key}}}）"
                ),
            },
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_env_with_default_and_required() {
        std::env::set_var("DSP_TEST_PRESENT", "value-1");
        assert_eq!(
            expand_env("a=${DSP_TEST_PRESENT}").unwrap(),
            "a=value-1"
        );
        assert_eq!(
            expand_env("a=${DSP_TEST_MISSING:-fallback}").unwrap(),
            "a=fallback"
        );
        assert!(expand_env("a=${DSP_TEST_MISSING}").is_err());
        // 中文不能被字节级替换破坏
        assert_eq!(
            expand_env("标题 = \"中文 ${DSP_TEST_PRESENT}\"  ").unwrap(),
            "标题 = \"中文 value-1\"  "
        );
        std::env::remove_var("DSP_TEST_PRESENT");
    }
}
