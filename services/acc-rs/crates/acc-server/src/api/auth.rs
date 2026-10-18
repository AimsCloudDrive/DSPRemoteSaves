//! 认证接口：注册、登录、刷新、登出、改密、找回、改邮箱、个人资料。

use crate::auth::email_code::{self, PURPOSE_CHANGE_EMAIL, PURPOSE_REGISTER, PURPOSE_RESET};
use crate::auth::password::{self, Argon2Params};
use crate::auth::token::{
    self, cookie_name, cookie_path, new_refresh_token, AUD_ADMIN, AUD_USER,
};
use crate::auth::CurrentUser;
use crate::crypto::open_values;
use crate::error::{ok, AppError, AppResult, ErrorCode};
use crate::i18n::{self, Lang};
use crate::mail::{MailContent, MailJob};
use crate::repo::{self, Profile, User};
use crate::state::AppState;
use crate::util::{dt_ms, mask_email, normalize_email, now_ms, now_dt, validate_user_name};
use crate::api::{user_agent, ClientIp};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use mongodb::bson::doc;
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// DTO
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmailCodeReq {
    pub email: String,
    pub purpose: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterReq {
    pub user_name: String,
    pub email: String,
    pub code: String,
    pub password: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginReq {
    /// 兼容模组/脚本里的 `account`
    #[serde(default, alias = "account")]
    pub user_name: String,
    pub password: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetRequestReq {
    pub email: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetReq {
    pub email: String,
    pub code: String,
    pub password: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangePasswordReq {
    pub old_password: String,
    pub new_password: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEmailReq {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEmailConfirmReq {
    pub code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleReq {
    pub locale: String,
}

pub fn profile_json(profile: &Profile) -> Value {
    json!({
        "id": profile.id.to_hex(),
        "userName": profile.user_name,
        "email": profile.email,
        "emailVerified": profile.email_verified(),
        "role": profile.role,
        "status": profile.status,
        "quotaBytes": profile.quota_bytes,
        "usedBytes": profile.used_bytes,
        "reservedBytes": profile.reserved_bytes,
        "mustChangePassword": profile.must_change_password,
        "locale": profile.locale,
        "createdAt": dt_ms(profile.created_at),
        "lastLoginAt": profile.last_login_at.map(dt_ms),
    })
}

pub struct TokenBundle {
    pub access: String,
    pub expires_at: i64,
    pub refresh: String,
    pub profile: Profile,
}

fn argon_params(state: &AppState) -> Argon2Params {
    Argon2Params {
        memory_kib: state.cfg.auth.argon2_memory_kib,
        iterations: state.cfg.auth.argon2_iterations,
        parallelism: state.cfg.auth.argon2_parallelism,
    }
}

fn bad_credentials() -> AppError {
    let message = match i18n::current() {
        Lang::Zh => "用户名或密码错误",
        Lang::En => "Invalid username or password",
    };
    AppError::new(ErrorCode::Unauthorized).with_message(message)
}

fn refresh_cookie(state: &AppState, audience: &str, raw: &str) -> Cookie<'static> {
    Cookie::build((cookie_name(audience), raw.to_string()))
        .path(cookie_path(&state.cfg.server.base_path, audience))
        .http_only(true)
        .secure(state.cfg.auth.cookie_secure)
        .same_site(SameSite::Strict)
        .max_age(cookie::time::Duration::days(
            state.cfg.auth.refresh_ttl_days.max(1),
        ))
        .build()
}

fn clear_cookie(state: &AppState, audience: &str) -> Cookie<'static> {
    let mut cookie = Cookie::build((cookie_name(audience), ""))
        .path(cookie_path(&state.cfg.server.base_path, audience))
        .http_only(true)
        .secure(state.cfg.auth.cookie_secure)
        .same_site(SameSite::Strict)
        .build();
    cookie.make_removal();
    cookie
}

/// 签发 access + refresh，并落库 refresh 的 SHA-256。
pub async fn issue_tokens(
    state: &AppState,
    user: &User,
    audience: &str,
    ip: &str,
    ua: Option<&str>,
) -> AppResult<TokenBundle> {
    let (access, expires_at) = token::issue_access(
        &state.secrets.jwt,
        user.id,
        user.token_version,
        audience,
        state.cfg.auth.access_ttl_minutes,
    )?;

    let (raw_refresh, hash) = new_refresh_token();
    let expires = now_ms() + state.cfg.auth.refresh_ttl_days.max(1) * 86_400_000;
    repo::create_session(
        &state.db,
        user.id,
        &hash,
        mongodb::bson::DateTime::from_millis(expires),
        ip,
        ua,
    )
    .await?;

    Ok(TokenBundle {
        access,
        expires_at,
        refresh: raw_refresh,
        profile: Profile::from(user),
    })
}

fn token_response(bundle: TokenBundle) -> Value {
    json!({
        "token": bundle.access,
        "expiresAt": bundle.expires_at,
        "user": profile_json(&bundle.profile),
    })
}

// ---------------------------------------------------------------------------
// 路由
// ---------------------------------------------------------------------------

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/email-code", post(send_code))
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/refresh", post(refresh))
        .route("/logout", post(logout))
        .route("/me", get(me).patch(update_me))
        .route("/password", post(change_password))
        .route("/password/reset/request", post(reset_request))
        .route("/password/reset", post(reset_confirm))
        .route("/email/change/request", post(change_email_request))
        .route("/email/change", post(change_email_confirm))
}

// ---------------------------------------------------------------------------
// 发码 / 注册 / 登录
// ---------------------------------------------------------------------------

async fn send_code(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<EmailCodeReq>,
) -> AppResult<impl IntoResponse> {
    let lang = Lang::from_headers(&headers);
    let email = normalize_email(&req.email)
        .ok_or_else(|| AppError::invalid("invalid email"))?;
    let purpose = email_code::normalize_purpose(&req.purpose)
        .ok_or_else(|| AppError::invalid("invalid purpose"))?;

    email_code::send(&state, &email, purpose, ip, lang).await?;

    // 统一话术，避免邮箱枚举
    let message = match lang {
        Lang::Zh => "如果该邮箱可用，验证码已发送",
        Lang::En => "If the address is available, a code has been sent",
    };
    Ok(Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": message,
        "payload": { "retryAfterSeconds": state.cfg.email_code.resend_seconds },
        "traceId": crate::util::current_trace_id(),
    })))
}

async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(req): Json<RegisterReq>,
) -> AppResult<impl IntoResponse> {
    let email = normalize_email(&req.email).ok_or_else(|| AppError::invalid("invalid email"))?;
    let user_name = req.user_name.trim().to_string();
    if !validate_user_name(&user_name) {
        return Err(AppError::invalid("invalid user name"));
    }
    // 先做本地校验，避免白白消耗验证码次数
    password::ensure_strong(&req.password, &user_name, &email)?;

    email_code::check_register_quota(&state, ip).await?;
    email_code::verify(&state, &email, PURPOSE_REGISTER, &req.code, ip).await?;

    // 验证码通过后才检查唯一性，避免用注册接口探测邮箱
    if repo::find_user_by_email(&state.db, &email).await?.is_some() {
        return Err(AppError::new(ErrorCode::EmailTaken));
    }
    if repo::find_user_by_name(&state.db, &user_name).await?.is_some() {
        return Err(AppError::new(ErrorCode::UserExists));
    }

    let settings = repo::get_settings(&state.db).await?;
    let quota = settings
        .map(|s| s.default_quota_bytes)
        .unwrap_or(state.cfg.quota.default_bytes);

    let hash = password::hash_password(req.password.clone(), argon_params(&state)).await?;
    let user = repo::create_user(
        &state.db,
        repo::NewUser {
            user_name: &user_name,
            email: Some(email.as_str()),
            password_hash: &hash,
            role: repo::ROLE_USER,
            quota_bytes: quota,
            locale: match i18n::current() {
                Lang::Zh => "zh-CN",
                Lang::En => "en-US",
            },
            email_verified: true,
        },
    )
    .await?;

    email_code::consume(&state, &email, PURPOSE_REGISTER).await?;
    repo::touch_login(&state.db, user.id).await?;
    state.invalidate_profile(user.id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(user.id),
            actor_name: &user.user_name,
            action: "auth.register",
            target_type: "user",
            target_id: Some(user.id.to_hex()),
            result: "success",
            detail: Some(mask_email(&email)),
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    let bundle = issue_tokens(&state, &user, AUD_USER, &ip.to_string(), user_agent(&headers).as_deref()).await?;
    let cookie = refresh_cookie(&state, AUD_USER, &bundle.refresh);
    Ok((jar.add(cookie), Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": crate::i18n::ok_message(i18n::current()),
        "payload": token_response(bundle),
        "traceId": crate::util::current_trace_id(),
    }))))
}

async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(req): Json<LoginReq>,
) -> AppResult<impl IntoResponse> {
    let user_name = req.user_name.trim().to_string();
    if user_name.is_empty() || req.password.is_empty() {
        return Err(AppError::invalid("missing credentials"));
    }

    email_code::check_login_allowed(&state, &user_name).await?;
    let secret = crate::crypto::open_password(
        &state,
        &headers,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?;
    let plain = secret.expose_secret().to_string();

    let user = repo::find_user_by_name(&state.db, &user_name).await?;
    let Some(user) = user else {
        // 不存在用户也跑一次哈希，抹平时序差异
        password::dummy_verify(plain).await;
        email_code::record_login_failure(&state, &user_name, ip).await;
        return Err(bad_credentials());
    };

    if !password::verify_password(plain, user.password_hash.clone()).await {
        email_code::record_login_failure(&state, &user_name, ip).await;
        repo::write_audit(
            &state.db,
            repo::AuditEntry {
                actor_id: Some(user.id),
                actor_name: &user.user_name,
                action: "auth.login",
                target_type: "user",
                target_id: Some(user.id.to_hex()),
                result: "failure",
                detail: Some("bad password".to_string()),
                ip: &ip.to_string(),
                user_agent: user_agent(&headers),
            },
        )
        .await;
        return Err(bad_credentials());
    }

    if !user.is_active() {
        return Err(AppError::new(ErrorCode::Forbidden));
    }

    email_code::clear_login_failures(&state, &user_name).await;
    repo::touch_login(&state.db, user.id).await?;
    state.invalidate_profile(user.id).await;

    // 文档要求：门户登录一律签发 user 受众；管理端必须走 /admin/auth/login（内网/VPN）
    let bundle = issue_tokens(&state, &user, AUD_USER, &ip.to_string(), user_agent(&headers).as_deref()).await?;
    let cookie = refresh_cookie(&state, AUD_USER, &bundle.refresh);

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(user.id),
            actor_name: &user.user_name,
            action: "auth.login",
            target_type: "user",
            target_id: Some(user.id.to_hex()),
            result: "success",
            detail: None,
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok((jar.add(cookie), Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": crate::i18n::ok_message(i18n::current()),
        "payload": token_response(bundle),
        "traceId": crate::util::current_trace_id(),
    }))))
}

// ---------------------------------------------------------------------------
// 刷新 / 登出
// ---------------------------------------------------------------------------

fn origin_part(url: &str) -> String {
    let mut parts = url.splitn(4, '/');
    let scheme = parts.next().unwrap_or("");
    let host = parts.next().unwrap_or("");
    format!("{scheme}://{host}")
}

fn same_origin(a: &str, b: &str) -> bool {
    origin_part(a).eq_ignore_ascii_case(&origin_part(b))
}

fn origin_allowed(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    let Some(origin) = headers.get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        // 非浏览器客户端没有 Origin
        return true;
    };
    let origin = origin.trim_end_matches('/');
    if state
        .cfg
        .auth
        .allowed_origins
        .iter()
        .any(|o| o.trim_end_matches('/') == origin)
    {
        return true;
    }
    same_origin(&state.cfg.server.public_url, origin)
}

pub async fn rotate_refresh(
    state: &AppState,
    jar: CookieJar,
    audience: &str,
    ip: &str,
    ua: Option<&str>,
) -> AppResult<(TokenBundle, CookieJar)> {
    let name = cookie_name(audience);
    let raw = jar
        .get(name)
        .map(|c| c.value().to_string())
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
    let hash = token::hash_refresh_token(&raw);
    let session = repo::find_session_by_hash(&state.db, &hash)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;

    if session.revoked_at.is_some() || session.expires_at.timestamp_millis() <= now_ms() {
        repo::delete_session_by_hash(&state.db, &hash).await?;
        return Err(AppError::new(ErrorCode::Unauthorized));
    }

    let user = repo::find_user_by_id(&state.db, session.user_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
    if !user.is_active() {
        return Err(AppError::new(ErrorCode::Forbidden));
    }

    // 轮换：旧 refresh 立即失效
    repo::delete_session_by_hash(&state.db, &hash).await?;
    let bundle = issue_tokens(state, &user, audience, ip, ua).await?;
    let cookie = refresh_cookie(state, audience, &bundle.refresh);
    Ok((bundle, jar.add(cookie)))
}

async fn refresh(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    if !origin_allowed(&state, &headers) {
        return Err(AppError::new(ErrorCode::Forbidden));
    }
    let (bundle, jar) = rotate_refresh(
        &state,
        jar,
        AUD_USER,
        &ip.to_string(),
        user_agent(&headers).as_deref(),
    )
    .await?;
    Ok((jar, Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": crate::i18n::ok_message(i18n::current()),
        "payload": token_response(bundle),
        "traceId": crate::util::current_trace_id(),
    }))))
}

async fn logout(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    // access token 仍有效时把 jti 拉黑；过期/缺失也允许登出
    if let Some(raw) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if let Ok(claims) = token::decode_access(&state.secrets.jwt, raw, &[AUD_USER, AUD_ADMIN]) {
            crate::auth::extract::blacklist_jti(&state, &claims).await;
        }
    }

    if let Some(cookie) = jar.get(cookie_name(AUD_USER)) {
        let hash = token::hash_refresh_token(cookie.value());
        let _ = repo::delete_session_by_hash(&state.db, &hash).await;
    }

    Ok((jar.add(clear_cookie(&state, AUD_USER)), Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": crate::i18n::ok_message(i18n::current()),
        "traceId": crate::util::current_trace_id(),
    }))))
}

// ---------------------------------------------------------------------------
// 个人资料
// ---------------------------------------------------------------------------

async fn me(
    State(_state): State<AppState>,
    CurrentUser(profile): CurrentUser,
) -> AppResult<impl IntoResponse> {
    Ok(ok(profile_json(&profile)))
}

async fn update_me(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
    Json(req): Json<LocaleReq>,
) -> AppResult<impl IntoResponse> {
    let locale = match req.locale.as_str() {
        "zh-CN" | "en-US" => req.locale,
        _ => return Err(AppError::invalid("unsupported locale")),
    };
    repo::update_user(&state.db, profile.id, doc! { "locale": &locale }).await?;
    state.invalidate_profile(profile.id).await;
    Ok(ok(json!({ "locale": locale })))
}

// ---------------------------------------------------------------------------
// 改密 / 找回
// ---------------------------------------------------------------------------

async fn change_password(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    CurrentUser(profile): CurrentUser,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChangePasswordReq>,
) -> AppResult<impl IntoResponse> {
    let user = repo::find_user_by_id(&state.db, profile.id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;

    let values = open_values(
        &state,
        &headers,
        req.timestamp,
        req.nonce.as_deref(),
        &[&req.old_password, &req.new_password],
    )
    .await?;
    let old_plain = values[0].expose_secret().to_string();
    let new_plain = values[1].expose_secret().to_string();

    if !password::verify_password(old_plain, user.password_hash.clone()).await {
        return Err(bad_credentials());
    }
    password::ensure_strong(&new_plain, &user.user_name, user.email.as_deref().unwrap_or(""))?;

    let hash = password::hash_password(new_plain, argon_params(&state)).await?;
    repo::update_user(
        &state.db,
        user.id,
        doc! { "passwordHash": hash, "passwordAlgo": "argon2id", "mustChangePassword": false },
    )
    .await?;
    repo::bump_token_version(&state.db, user.id, Some(false)).await?;
    repo::delete_user_sessions(&state.db, user.id).await?;
    state.invalidate_profile(user.id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(user.id),
            actor_name: &user.user_name,
            action: "auth.password.change",
            target_type: "user",
            target_id: Some(user.id.to_hex()),
            result: "success",
            detail: None,
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok(ok(json!({ "relogin": true })))
}

async fn reset_request(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<ResetRequestReq>,
) -> AppResult<impl IntoResponse> {
    let lang = Lang::from_headers(&headers);
    let email = normalize_email(&req.email).ok_or_else(|| AppError::invalid("invalid email"))?;
    email_code::send(&state, &email, PURPOSE_RESET, ip, lang).await?;
    let message = match lang {
        Lang::Zh => "如果该邮箱可用，验证码已发送",
        Lang::En => "If the address is available, a code has been sent",
    };
    Ok(Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": message,
        "payload": { "retryAfterSeconds": state.cfg.email_code.resend_seconds },
        "traceId": crate::util::current_trace_id(),
    })))
}

async fn reset_confirm(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: axum::http::HeaderMap,
    Json(req): Json<ResetReq>,
) -> AppResult<impl IntoResponse> {
    let email = normalize_email(&req.email).ok_or_else(|| AppError::invalid("invalid email"))?;
    let new_plain = crate::crypto::open_password(
        &state,
        &headers,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?
    .expose_secret()
    .to_string();

    let user = repo::find_user_by_email(&state.db, &email).await?;
    let Some(user) = user else {
        // 不暴露邮箱是否存在
        return Ok(ok(json!({ "reset": true })));
    };

    email_code::verify(&state, &email, PURPOSE_RESET, &req.code, ip).await?;
    password::ensure_strong(&new_plain, &user.user_name, &email)?;

    let hash = password::hash_password(new_plain, argon_params(&state)).await?;
    repo::update_user(
        &state.db,
        user.id,
        doc! { "passwordHash": hash, "passwordAlgo": "argon2id", "mustChangePassword": false },
    )
    .await?;
    repo::bump_token_version(&state.db, user.id, Some(false)).await?;
    repo::delete_user_sessions(&state.db, user.id).await?;
    email_code::consume(&state, &email, PURPOSE_RESET).await?;
    state.invalidate_profile(user.id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(user.id),
            actor_name: &user.user_name,
            action: "auth.password.reset",
            target_type: "user",
            target_id: Some(user.id.to_hex()),
            result: "success",
            detail: Some(mask_email(&email)),
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok(ok(json!({ "reset": true })))
}

// ---------------------------------------------------------------------------
// 改邮箱
// ---------------------------------------------------------------------------

async fn change_email_request(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    CurrentUser(profile): CurrentUser,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChangeEmailReq>,
) -> AppResult<impl IntoResponse> {
    let lang = Lang::from_headers(&headers);
    let new_email =
        normalize_email(&req.email).ok_or_else(|| AppError::invalid("invalid email"))?;
    if profile.email.as_deref() == Some(new_email.as_str()) {
        return Err(AppError::new(ErrorCode::EmailTaken));
    }
    if repo::find_user_by_email(&state.db, &new_email).await?.is_some() {
        return Err(AppError::new(ErrorCode::EmailTaken));
    }

    let user = repo::find_user_by_id(&state.db, profile.id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
    let plain = crate::crypto::open_password(
        &state,
        &headers,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?
    .expose_secret()
    .to_string();
    if !password::verify_password(plain, user.password_hash.clone()).await {
        return Err(bad_credentials());
    }

    // 记住待确认的新邮箱，确认时无需再传
    state
        .cache
        .set_ex(
            &format!("email:change:{}", profile.id.to_hex()),
            &new_email,
            state.cfg.email_code.ttl_seconds,
        )
        .await;
    email_code::send(&state, &new_email, PURPOSE_CHANGE_EMAIL, ip, lang).await?;
    Ok(ok(json!({ "sent": true, "email": new_email })))
}

async fn change_email_confirm(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    CurrentUser(profile): CurrentUser,
    headers: axum::http::HeaderMap,
    Json(req): Json<ChangeEmailConfirmReq>,
) -> AppResult<impl IntoResponse> {
    let key = format!("email:change:{}", profile.id.to_hex());
    let new_email = state
        .cache
        .get_raw(&key)
        .await
        .ok_or_else(|| AppError::new(ErrorCode::CodeInvalid))?;

    email_code::verify(&state, &new_email, PURPOSE_CHANGE_EMAIL, &req.code, ip).await?;

    let old_email = profile.email.clone();
    repo::update_user(
        &state.db,
        profile.id,
        doc! {
            "email": &new_email,
            "emailLower": &new_email,
            "emailVerifiedAt": now_dt(),
        },
    )
    .await?;
    email_code::consume(&state, &new_email, PURPOSE_CHANGE_EMAIL).await?;
    state.cache.del(&[key]).await;
    state.invalidate_profile(profile.id).await;

    if let Some(old) = old_email {
        let (subject, text) = match i18n::current() {
            Lang::Zh => (
                "DSP 云存档：邮箱已变更".to_string(),
                format!("你的账号 {} 的邮箱已变更为 {}。如非本人操作，请立即联系管理员。", profile.user_name, new_email),
            ),
            Lang::En => (
                "DSP Cloud Saves: email changed".to_string(),
                format!("The email of your account {} has been changed to {}. If this was not you, contact the administrator immediately.", profile.user_name, new_email),
            ),
        };
        let _ = state
            .mailer
            .enqueue(MailJob {
                to: old,
                lang: i18n::current(),
                content: MailContent::Notice { subject, text },
            })
            .await;
    }

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(profile.id),
            actor_name: &profile.user_name,
            action: "auth.email.change",
            target_type: "user",
            target_id: Some(profile.id.to_hex()),
            result: "success",
            detail: Some(mask_email(&new_email)),
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok(ok(json!({ "email": new_email })))
}

/// 供管理端复用的 cookie 构造。
pub fn admin_cookie(state: &AppState, raw: &str) -> Cookie<'static> {
    refresh_cookie(state, AUD_ADMIN, raw)
}

pub fn admin_clear_cookie(state: &AppState) -> Cookie<'static> {
    clear_cookie(state, AUD_ADMIN)
}

pub fn admin_origin_allowed(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    origin_allowed(state, headers)
}

/// 供管理端调用的用户 DTO。
pub fn user_json(user: &User) -> Value {
    let profile = Profile::from(user);
    let mut value = profile_json(&profile);
    value["mustChangePassword"] = json!(user.must_change_password);
    value["emailVerified"] = json!(user.email_verified_at.is_some());
    value
}
