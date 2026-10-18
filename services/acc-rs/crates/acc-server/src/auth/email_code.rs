//! 邮箱验证码：生成、存储（HMAC）、校验、六层限流与发信。

use crate::error::{AppError, ErrorCode};
use crate::i18n::Lang;
use crate::mail::{MailContent, MailJob};
use crate::repo::{self, EmailCode};
use crate::state::AppState;
use crate::util::{
    constant_time_eq, email_domain, hmac_sha256, mask_email, now_dt, now_ms, random_digits,
    subnet_key,
};
use mongodb::bson::DateTime;
use std::net::IpAddr;
use tracing::{info, warn};

pub const PURPOSE_REGISTER: &str = "register";
pub const PURPOSE_RESET: &str = "reset";
pub const PURPOSE_CHANGE_EMAIL: &str = "change_email";

pub fn normalize_purpose(purpose: &str) -> Option<&'static str> {
    match purpose {
        PURPOSE_REGISTER => Some(PURPOSE_REGISTER),
        PURPOSE_RESET => Some(PURPOSE_RESET),
        PURPOSE_CHANGE_EMAIL => Some(PURPOSE_CHANGE_EMAIL),
        _ => None,
    }
}

/// `_id = HMAC(email|purpose)`：一个 (email, purpose) 天然只有一个活跃码。
pub fn code_id(secret: &[u8], email: &str, purpose: &str) -> String {
    hex::encode(hmac_sha256(secret, format!("{email}|{purpose}").as_bytes()))
}

/// 只存 HMAC，不存明文验证码。
pub fn code_hash(secret: &[u8], email: &str, purpose: &str, code: &str) -> String {
    hex::encode(hmac_sha256(
        secret,
        format!("{email}|{purpose}|{code}").as_bytes(),
    ))
}

fn short_hash(value: &str) -> String {
    crate::util::sha256_hex(value.as_bytes())[..16].to_string()
}

async fn limit(
    state: &AppState,
    key: String,
    amount: u32,
    window: u64,
) -> Result<(), AppError> {
    let result = state.cache.incr_window(&key, amount, window).await;
    if result.allowed {
        Ok(())
    } else {
        Err(AppError::new(ErrorCode::RateLimited).with_retry(result.retry_after))
    }
}

/// 发送验证码：先限流，再判断是否需要真正发信（避免邮箱枚举）。
pub async fn send(
    state: &AppState,
    email: &str,
    purpose: &str,
    ip: IpAddr,
    lang: Lang,
) -> Result<(), AppError> {
    let cfg = &state.cfg.email_code;
    let domain = email_domain(email).to_string();

    if cfg
        .disposable_domains
        .iter()
        .any(|d| d.eq_ignore_ascii_case(&domain))
    {
        // 一次性邮箱：直接拒绝，避免被当作收码池
        return Err(AppError::invalid("disposable email domain"));
    }

    // ① 单邮箱 / 单域
    if let Some(retry) = state
        .cache
        .interval(&format!("code:mail:{}", short_hash(email)), cfg.resend_seconds)
        .await
    {
        return Err(AppError::new(ErrorCode::RateLimited).with_retry(retry));
    }
    let email_key = short_hash(email);
    limit(
        state,
        format!("rl:code:email:h:{email_key}"),
        cfg.per_email_per_hour,
        3600,
    )
    .await?;
    limit(
        state,
        format!("rl:code:email:d:{email_key}"),
        cfg.per_email_per_day,
        86_400,
    )
    .await?;

    // ② 单 IP / 网段
    limit(
        state,
        format!("rl:code:ip:h:{}", short_hash(&ip.to_string())),
        cfg.per_ip_per_hour,
        3600,
    )
    .await?;
    limit(
        state,
        format!("rl:code:ip:d:{}", short_hash(&ip.to_string())),
        cfg.per_ip_per_day,
        86_400,
    )
    .await?;
    limit(
        state,
        format!("rl:code:subnet:h:{}", subnet_key(ip)),
        50,
        3600,
    )
    .await?;

    // ③ 单收件域 / 全局
    limit(
        state,
        format!("rl:code:domain:h:{}", short_hash(&domain)),
        cfg.per_domain_per_hour,
        3600,
    )
    .await?;
    limit(
        state,
        "rl:code:global:h".to_string(),
        cfg.global_per_hour,
        3600,
    )
    .await?;
    limit(
        state,
        "rl:code:global:d".to_string(),
        cfg.global_per_day,
        86_400,
    )
    .await?;

    // 邮箱枚举防护：注册时邮箱已存在 / 找回时邮箱不存在，都不发信但返回成功
    let existing = repo::find_user_by_email(&state.db, email).await?;
    let should_send = match purpose {
        PURPOSE_REGISTER => existing.is_none(),
        PURPOSE_RESET => existing.is_some(),
        PURPOSE_CHANGE_EMAIL => true,
        _ => false,
    };

    let code = random_digits(cfg.length);
    let id = code_id(&state.secrets.server, email, purpose);
    let record = EmailCode {
        id: id.clone(),
        email_lower: email.to_string(),
        purpose: purpose.to_string(),
        code_hash: code_hash(&state.secrets.server, email, purpose, &code),
        attempts: 0,
        last_sent_at: now_dt(),
        expires_at: DateTime::from_millis(now_ms() + (cfg.ttl_seconds as i64) * 1000),
        consumed_at: None,
        ip: ip.to_string(),
        created_at: now_dt(),
    };
    // 重发覆盖旧码，旧码立即失效
    repo::upsert_email_code(&state.db, &record).await?;

    if should_send {
        if state.mailer.remaining_quota().await == 0 {
            return Err(AppError::new(ErrorCode::MailQuotaExhausted));
        }
        state
            .mailer
            .enqueue(MailJob {
                to: email.to_string(),
                lang,
                content: MailContent::Code {
                    code,
                    purpose: purpose.to_string(),
                },
            })
            .await?;
    } else {
        info!(
            email = %mask_email(email),
            purpose,
            "验证码请求命中隐私保护分支，未实际发信"
        );
    }

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: existing.as_ref().map(|u| u.id),
            actor_name: existing
                .as_ref()
                .map(|u| u.user_name.as_str())
                .unwrap_or("-"),
            action: "email_code.send",
            target_type: "email",
            target_id: None,
            result: if should_send { "success" } else { "skipped" },
            detail: Some(format!("{purpose} {}", mask_email(email))),
            ip: &ip.to_string(),
            user_agent: None,
        },
    )
    .await;

    Ok(())
}

/// 校验验证码（不消费）。失败会累加尝试次数与 IP 失败计数。
pub async fn verify(
    state: &AppState,
    email: &str,
    purpose: &str,
    code: &str,
    ip: IpAddr,
) -> Result<EmailCode, AppError> {
    let cfg = &state.cfg.email_code;
    let id = code_id(&state.secrets.server, email, purpose);

    let record = repo::find_email_code(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::CodeInvalid))?;

    if record.consumed_at.is_some()
        || record.expires_at.timestamp_millis() <= now_ms()
        || record.attempts >= cfg.max_attempts as i32
    {
        return Err(AppError::new(ErrorCode::CodeInvalid));
    }

    let updated = repo::incr_code_attempts(&state.db, &id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::CodeInvalid))?;

    let expected = code_hash(&state.secrets.server, email, purpose, code);
    let matched = constant_time_eq(updated.code_hash.as_bytes(), expected.as_bytes());

    if !matched {
        // 分布式撞码：按 IP 统计失败次数
        let key = format!("rl:code:verify:ip:h:{}", short_hash(&ip.to_string()));
        let result = state
            .cache
            .incr_window(&key, cfg.verify_failures_per_hour, 3600)
            .await;
        if !result.allowed {
            return Err(AppError::new(ErrorCode::RateLimited).with_retry(result.retry_after));
        }
        return Err(AppError::new(ErrorCode::CodeInvalid));
    }

    if updated.attempts > cfg.max_attempts as i32 {
        return Err(AppError::new(ErrorCode::CodeInvalid));
    }
    Ok(updated)
}

/// 消费验证码：成功后不可复用。
pub async fn consume(state: &AppState, email: &str, purpose: &str) -> Result<(), AppError> {
    let id = code_id(&state.secrets.server, email, purpose);
    repo::consume_email_code(&state.db, &id).await?;
    Ok(())
}

/// 注册按 IP 限制：3 个账号/天。
pub async fn check_register_quota(state: &AppState, ip: IpAddr) -> Result<(), AppError> {
    let key = format!("rl:register:ip:d:{}", short_hash(&ip.to_string()));
    let result = state
        .cache
        .incr_window(&key, state.cfg.email_code.register_per_ip_per_day, 86_400)
        .await;
    if result.allowed {
        Ok(())
    } else {
        Err(AppError::new(ErrorCode::RateLimited).with_retry(result.retry_after.max(60)))
    }
}

/// 登录失败锁定：5 次/10 分钟锁 15 分钟。
pub async fn check_login_allowed(state: &AppState, user_name: &str) -> Result<(), AppError> {
    let key = format!("login:lock:{}", short_hash(&user_name.to_lowercase()));
    if state.cache.get_raw(&key).await.is_some() {
        let retry = state
            .cfg
            .auth
            .login_lock_seconds
            .max(1);
        return Err(AppError::new(ErrorCode::AccountLocked).with_retry(retry));
    }
    Ok(())
}

pub async fn record_login_failure(state: &AppState, user_name: &str, ip: IpAddr) {
    let cfg = &state.cfg.auth;
    let key = format!("login:fail:{}", short_hash(&user_name.to_lowercase()));
    let result = state
        .cache
        .incr_window(&key, cfg.max_login_failures, cfg.login_failure_window_seconds)
        .await;
    if !result.allowed {
        let lock_key = format!("login:lock:{}", short_hash(&user_name.to_lowercase()));
        state
            .cache
            .set_ex(&lock_key, "1", cfg.login_lock_seconds)
            .await;
        warn!(user = %user_name, "登录失败次数超限，账号临时锁定");
    }

    // 同一 IP 20 次失败/10 分钟，防止撞库
    let ip_key = format!("login:ip:{}", short_hash(&ip.to_string()));
    state.cache.incr_window(&ip_key, 20, 600).await;
}

pub async fn clear_login_failures(state: &AppState, user_name: &str) {
    let key = format!("login:fail:{}", short_hash(&user_name.to_lowercase()));
    state.cache.del(&[key]).await;
}
