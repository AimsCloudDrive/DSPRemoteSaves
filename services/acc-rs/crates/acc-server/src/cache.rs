//! Redis 缓存层：限流、防重放、验证码缓存、令牌吊销、热点读、锁。
//!
//! 设计原则：**Mongo/磁盘是事实源**。Redis 故障时本模块自动降级到进程内实现，
//! 业务功能不中断，只是失去跨重启/跨实例能力。

use redis::aio::ConnectionManager;
use redis::Script;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{info, warn};

const LUA_INCR_WINDOW: &str = r#"
local n = redis.call('INCR', KEYS[1])
if n == 1 then redis.call('EXPIRE', KEYS[1], ARGV[2]) end
local ttl = redis.call('TTL', KEYS[1])
if n > tonumber(ARGV[1]) then return {0, ttl} end
return {1, ttl}
"#;

const LUA_INCR_TTL: &str = r#"
local n = redis.call('INCR', KEYS[1])
if n == 1 then redis.call('EXPIRE', KEYS[1], ARGV[1]) end
return n
"#;

const LUA_RELEASE_LOCK: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
end
return 0
"#;

#[derive(Debug, Clone, Copy)]
pub struct WindowResult {
    pub allowed: bool,
    pub retry_after: u64,
}

struct LocalEntry {
    value: String,
    expires: Instant,
}

#[derive(Clone)]
pub struct Cache {
    prefix: String,
    enabled: bool,
    url: String,
    conn: Arc<RwLock<Option<ConnectionManager>>>,
    local: Arc<tokio::sync::Mutex<HashMap<String, LocalEntry>>>,
    healthy: Arc<AtomicBool>,
    command_timeout: Duration,
    last_warn: Arc<AtomicU64>,
}

impl Cache {
    /// 尝试连接 Redis；失败不阻塞启动，返回降级状态的实例。
    pub async fn connect(url: &str, prefix: &str, enabled: bool, timeout_ms: u64) -> Self {
        let cache = Self {
            prefix: prefix.to_string(),
            enabled,
            url: url.to_string(),
            conn: Arc::new(RwLock::new(None)),
            local: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            healthy: Arc::new(AtomicBool::new(false)),
            command_timeout: Duration::from_millis(timeout_ms.max(100)),
            last_warn: Arc::new(AtomicU64::new(0)),
        };

        if !enabled {
            info!("Redis 已禁用，使用进程内降级实现");
            return cache;
        }
        if cache.try_connect().await {
            info!("Redis 已连接");
        } else {
            warn!("Redis 连接失败，已降级到进程内实现（将持续重连）");
        }
        cache
    }

    async fn try_connect(&self) -> bool {
        use redis::Client;
        match tokio::time::timeout(Duration::from_millis(2000), async {
            let client = Client::open(self.url.as_str())?;
            client.get_connection_manager().await
        })
        .await
        {
            Ok(Ok(manager)) => {
                *self.conn.write().await = Some(manager);
                self.healthy.store(true, Ordering::Relaxed);
                true
            }
            Ok(Err(err)) => {
                self.warn_throttled(&format!("Redis 连接失败: {err}"));
                self.healthy.store(false, Ordering::Relaxed);
                false
            }
            Err(_) => {
                self.warn_throttled("Redis 连接超时");
                self.healthy.store(false, Ordering::Relaxed);
                false
            }
        }
    }

    /// 后台任务每 30s 调用一次，Redis 恢复后自动回到正常路径。
    pub async fn ensure_connected(&self) {
        if !self.enabled {
            return;
        }
        if self.conn.read().await.is_some() && self.healthy.load(Ordering::Relaxed) {
            return;
        }
        if self.try_connect().await {
            info!("Redis 已恢复");
        }
    }

    fn warn_throttled(&self, message: &str) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let last = self.last_warn.load(Ordering::Relaxed);
        if now.saturating_sub(last) > 60 {
            self.last_warn.store(now, Ordering::Relaxed);
            warn!("{message}");
        }
    }

    pub fn healthy(&self) -> bool {
        !self.enabled || self.healthy.load(Ordering::Relaxed)
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    fn full_key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    async fn conn(&self) -> Option<ConnectionManager> {
        if !self.enabled {
            return None;
        }
        self.conn.read().await.clone()
    }

    async fn mark_failure(&self) {
        if self.healthy.swap(false, Ordering::Relaxed) {
            warn!("Redis 操作失败，临时降级到进程内实现");
        }
    }

    /// 带超时执行 Redis 命令；失败/超时返回 `None`，由调用方走本地降级。
    async fn call<T, F>(&self, fut: F) -> Option<T>
    where
        F: Future<Output = redis::RedisResult<T>>,
    {
        match tokio::time::timeout(self.command_timeout, fut).await {
            Ok(Ok(value)) => Some(value),
            Ok(Err(err)) => {
                tracing::debug!(error = %err, "Redis 命令失败，降级");
                self.mark_failure().await;
                None
            }
            Err(_) => {
                tracing::debug!("Redis 命令超时，降级");
                self.mark_failure().await;
                None
            }
        }
    }

    // ------------------------------------------------------------------
    // 基础 KV
    // ------------------------------------------------------------------

    pub async fn get_raw(&self, key: &str) -> Option<String> {
        if let Some(mut conn) = self.conn().await {
            let full = self.full_key(key);
            if let Some(value) = self
                .call(redis::cmd("GET").arg(&full).query_async::<Option<String>>(&mut conn))
                .await
            {
                return value;
            }
        }
        let mut local = self.local.lock().await;
        let expired = local
            .get(key)
            .map(|e| e.expires <= Instant::now())
            .unwrap_or(true);
        if expired {
            local.remove(key);
            None
        } else {
            local.get(key).map(|e| e.value.clone())
        }
    }

    pub async fn set_ex(&self, key: &str, value: &str, ttl_seconds: u64) {
        if ttl_seconds == 0 {
            return;
        }
        if let Some(mut conn) = self.conn().await {
            let full = self.full_key(key);
            if self
                .call(
                    redis::cmd("SET")
                        .arg(&full)
                        .arg(value)
                        .arg("EX")
                        .arg(ttl_seconds)
                        .query_async::<()>(&mut conn),
                )
                .await
                .is_some()
            {
                return;
            }
        }
        self.local_set(key, value, ttl_seconds).await;
    }

    async fn local_set(&self, key: &str, value: &str, ttl_seconds: u64) {
        let mut local = self.local.lock().await;
        if local.len() > 20_000 {
            let now = Instant::now();
            local.retain(|_, e| e.expires > now);
        }
        local.insert(
            key.to_string(),
            LocalEntry {
                value: value.to_string(),
                expires: Instant::now() + Duration::from_secs(ttl_seconds),
            },
        );
    }

    pub async fn del(&self, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        if let Some(mut conn) = self.conn().await {
            let full: Vec<String> = keys.iter().map(|k| self.full_key(k)).collect();
            if self
                .call(redis::cmd("DEL").arg(&full).query_async::<i64>(&mut conn))
                .await
                .is_some()
            {
                let mut local = self.local.lock().await;
                for key in keys {
                    local.remove(key);
                }
                return;
            }
        }
        let mut local = self.local.lock().await;
        for key in keys {
            local.remove(key);
        }
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let raw = self.get_raw(key).await?;
        serde_json::from_str(&raw).ok()
    }

    pub async fn set_json<T: Serialize>(&self, key: &str, value: &T, ttl_seconds: u64) {
        if let Ok(raw) = serde_json::to_string(value) {
            self.set_ex(key, &raw, ttl_seconds).await;
        }
    }

    // ------------------------------------------------------------------
    // 限流
    // ------------------------------------------------------------------

    /// 固定窗口计数；超限时返回剩余等待秒数。
    pub async fn incr_window(&self, key: &str, limit: u32, window_seconds: u64) -> WindowResult {
        if limit == 0 {
            return WindowResult {
                allowed: true,
                retry_after: 0,
            };
        }
        if let Some(mut conn) = self.conn().await {
            let full = self.full_key(key);
            let script = Script::new(LUA_INCR_WINDOW);
            if let Some((allowed, ttl)) = self
                .call(
                    script
                        .key(full)
                        .arg(limit)
                        .arg(window_seconds)
                        .invoke_async::<(i64, i64)>(&mut conn),
                )
                .await
            {
                return WindowResult {
                    allowed: allowed == 1,
                    retry_after: if allowed == 1 { 0 } else { ttl.max(1) as u64 },
                };
            }
        }

        let now = Instant::now();
        let mut local = self.local.lock().await;
        let entry = local.entry(key.to_string()).or_insert_with(|| LocalEntry {
            value: "0".into(),
            expires: now + Duration::from_secs(window_seconds),
        });
        if entry.expires <= now {
            entry.value = "0".into();
            entry.expires = now + Duration::from_secs(window_seconds);
        }
        let count: u32 = entry.value.parse().unwrap_or(0) + 1;
        entry.value = count.to_string();
        let retry = entry.expires.saturating_duration_since(now).as_secs().max(1);
        WindowResult {
            allowed: count <= limit,
            retry_after: if count <= limit { 0 } else { retry },
        }
    }

    /// 固定间隔：同一 key 在 `seconds` 内只允许一次；返回需要等待的秒数。
    pub async fn interval(&self, key: &str, seconds: u64) -> Option<u64> {
        if let Some(mut conn) = self.conn().await {
            let full = self.full_key(key);
            let set_result = self
                .call(
                    redis::cmd("SET")
                        .arg(&full)
                        .arg("1")
                        .arg("NX")
                        .arg("EX")
                        .arg(seconds)
                        .query_async::<Option<String>>(&mut conn),
                )
                .await;

            if let Some(value) = set_result {
                if value.is_none() {
                    let ttl = self
                        .call(redis::cmd("TTL").arg(&full).query_async::<i64>(&mut conn))
                        .await
                        .unwrap_or(seconds as i64);
                    return Some(ttl.max(1) as u64);
                }
                return None;
            }
        }

        let mut local = self.local.lock().await;
        let now = Instant::now();
        if let Some(entry) = local.get(key) {
            if entry.expires > now {
                return Some(entry.expires.saturating_duration_since(now).as_secs().max(1));
            }
        }
        local.insert(
            key.to_string(),
            LocalEntry {
                value: "1".into(),
                expires: now + Duration::from_secs(seconds),
            },
        );
        None
    }

    /// `SET NX EX`：用于防重放、幂等键、简单锁。
    pub async fn set_nx(&self, key: &str, value: &str, ttl_seconds: u64) -> bool {
        if let Some(mut conn) = self.conn().await {
            let full = self.full_key(key);
            let result = self
                .call(
                    redis::cmd("SET")
                        .arg(&full)
                        .arg(value)
                        .arg("NX")
                        .arg("EX")
                        .arg(ttl_seconds)
                        .query_async::<Option<String>>(&mut conn),
                )
                .await;
            if let Some(value) = result {
                return value.is_some();
            }
        }

        let mut local = self.local.lock().await;
        let now = Instant::now();
        if let Some(entry) = local.get(key) {
            if entry.expires > now {
                return false;
            }
        }
        local.insert(
            key.to_string(),
            LocalEntry {
                value: value.to_string(),
                expires: now + Duration::from_secs(ttl_seconds),
            },
        );
        true
    }

    pub async fn release_lock(&self, key: &str, owner: &str) {
        if let Some(mut conn) = self.conn().await {
            let full = self.full_key(key);
            let script = Script::new(LUA_RELEASE_LOCK);
            if self
                .call(script.key(full).arg(owner).invoke_async::<i64>(&mut conn))
                .await
                .is_some()
            {
                return;
            }
        }
        let mut local = self.local.lock().await;
        if local.get(key).map(|e| e.value == owner).unwrap_or(false) {
            local.remove(key);
        }
    }

    /// 自增并保证首次设置 TTL；返回自增后的值。
    pub async fn incr_with_ttl(&self, key: &str, ttl_seconds: u64) -> i64 {
        if let Some(mut conn) = self.conn().await {
            let full = self.full_key(key);
            let script = Script::new(LUA_INCR_TTL);
            if let Some(n) = self
                .call(script.key(full).arg(ttl_seconds).invoke_async::<i64>(&mut conn))
                .await
            {
                return n;
            }
        }
        let mut local = self.local.lock().await;
        let now = Instant::now();
        let entry = local.entry(key.to_string()).or_insert_with(|| LocalEntry {
            value: "0".into(),
            expires: now + Duration::from_secs(ttl_seconds),
        });
        if entry.expires <= now {
            entry.value = "0".into();
            entry.expires = now + Duration::from_secs(ttl_seconds);
        }
        let n: i64 = entry.value.parse().unwrap_or(0) + 1;
        entry.value = n.to_string();
        n
    }

    pub async fn ping(&self) -> bool {
        if !self.enabled {
            return true;
        }
        let Some(mut conn) = self.conn().await else {
            return false;
        };
        self.call(redis::cmd("PING").query_async::<String>(&mut conn))
            .await
            .is_some()
    }
}
