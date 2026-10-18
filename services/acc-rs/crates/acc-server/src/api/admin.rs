//! 管理端接口：管理员登录、用户 CRUD、全局存档、审计、设置、密钥轮换。
//!
//! 所有 `/admin/*` 路由都要求 `aud=admin` 的 access token（内网/VPN 由 Caddy 另加限制）。

use crate::api::auth::{
    admin_clear_cookie, admin_cookie, admin_origin_allowed, issue_tokens, profile_json,
    rotate_refresh, user_json,
};
use crate::api::{user_agent, ClientIp, Pagination};
use crate::auth::password::{self, Argon2Params};
use crate::auth::token::AUD_ADMIN;
use crate::auth::AdminUser;
use crate::crypto::open_password;
use crate::error::{ok, AppError, AppResult, ErrorCode};
use crate::i18n::{self, Lang};
use crate::repo::{self, AuditFilter};
use crate::state::AppState;
use crate::storage::Storage;
use crate::util::{
    csv_escape, dt_ms, format_datetime, normalize_email, now_dt, now_ms, validate_user_name,
};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::extract::cookie::CookieJar;
use mongodb::bson::{doc, oid::ObjectId};
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{json, Value};
use tracing::info;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(admin_login))
        .route("/auth/refresh", post(admin_refresh))
        .route("/auth/logout", post(admin_logout))
        .route("/users", get(list_users).post(create_user))
        .route(
            "/users/{id}",
            get(get_user).patch(patch_user).delete(delete_user),
        )
        .route("/users/{id}/reset-password", post(reset_password))
        .route("/users/{id}/saves", get(user_saves))
        .route("/users/{id}/saves/{name}/versions", get(user_save_versions))
        .route(
            "/users/{id}/saves/{name}/versions/{vid}/rollback",
            post(admin_rollback),
        )
        .route("/users/{id}/saves/{name}", axum::routing::delete(admin_delete_save))
        .route("/saves", get(search_saves))
        .route("/stats/overview", get(stats_overview))
        .route("/audit", get(list_audit))
        .route("/audit/export.csv", get(export_audit))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/crypto/rotate", post(rotate_key))
}

// ---------------------------------------------------------------------------
// 管理端登录
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminLoginReq {
    #[serde(default, alias = "account")]
    pub user_name: String,
    pub password: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

async fn admin_login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    jar: CookieJar,
    Json(req): Json<AdminLoginReq>,
) -> AppResult<impl IntoResponse> {
    let user_name = req.user_name.trim().to_string();
    if user_name.is_empty() || req.password.is_empty() {
        return Err(AppError::invalid("missing credentials"));
    }

    let secret = open_password(
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
        password::dummy_verify(plain).await;
        return Err(bad_credentials());
    };
    if !password::verify_password(plain, user.password_hash.clone()).await {
        return Err(bad_credentials());
    }
    if !user.is_active() || !user.is_admin() {
        return Err(AppError::new(ErrorCode::Forbidden));
    }

    repo::touch_login(&state.db, user.id).await?;
    state.invalidate_profile(user.id).await;
    let bundle = issue_tokens(&state, &user, AUD_ADMIN, &ip.to_string(), user_agent(&headers).as_deref()).await?;
    let cookie = admin_cookie(&state, &bundle.refresh);

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(user.id),
            actor_name: &user.user_name,
            action: "admin.login",
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
        "message": i18n::ok_message(i18n::current()),
        "payload": {
            "token": bundle.access,
            "expiresAt": bundle.expires_at,
            "user": profile_json(&bundle.profile),
        },
        "traceId": crate::util::current_trace_id(),
    }))))
}

async fn admin_refresh(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    if !admin_origin_allowed(&state, &headers) {
        return Err(AppError::new(ErrorCode::Forbidden));
    }
    let (bundle, jar) = rotate_refresh(
        &state,
        jar,
        AUD_ADMIN,
        &ip.to_string(),
        user_agent(&headers).as_deref(),
    )
    .await?;
    if !bundle.profile.is_admin() {
        return Err(AppError::new(ErrorCode::Forbidden));
    }
    Ok((jar, Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": i18n::ok_message(i18n::current()),
        "payload": {
            "token": bundle.access,
            "expiresAt": bundle.expires_at,
            "user": profile_json(&bundle.profile),
        },
        "traceId": crate::util::current_trace_id(),
    }))))
}

async fn admin_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> AppResult<impl IntoResponse> {
    if let Some(raw) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if let Ok(claims) = crate::auth::token::decode_access(&state.secrets.jwt, raw, &[AUD_ADMIN]) {
            crate::auth::extract::blacklist_jti(&state, &claims).await;
        }
    }
    if let Some(cookie) = jar.get(crate::auth::token::cookie_name(AUD_ADMIN)) {
        let hash = crate::auth::token::hash_refresh_token(cookie.value());
        let _ = repo::delete_session_by_hash(&state.db, &hash).await;
    }
    Ok((jar.add(admin_clear_cookie(&state)), Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": i18n::ok_message(i18n::current()),
        "traceId": crate::util::current_trace_id(),
    }))))
}

fn bad_credentials() -> AppError {
    let message = match i18n::current() {
        Lang::Zh => "用户名或密码错误",
        Lang::En => "Invalid username or password",
    };
    AppError::new(ErrorCode::Unauthorized).with_message(message)
}

// ---------------------------------------------------------------------------
// 用户管理
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserListQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub search: Option<String>,
    pub status: Option<String>,
    pub role: Option<String>,
}

fn pagination(page: Option<u64>, page_size: Option<u64>) -> Pagination {
    Pagination { page, page_size }
}

async fn list_users(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Query(query): Query<UserListQuery>,
) -> AppResult<impl IntoResponse> {
    let page = pagination(query.page, query.page_size);
    let (users, total) = repo::list_users(
        &state.db,
        query.search.as_deref(),
        query.status.as_deref(),
        query.role.as_deref(),
        page.skip(),
        page.limit(),
    )
    .await?;
    Ok(ok(json!({
        "items": users.iter().map(user_json).collect::<Vec<_>>(),
        "total": total,
        "page": page.page.unwrap_or(1).max(1),
        "pageSize": page.limit(),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserReq {
    pub user_name: String,
    pub email: Option<String>,
    pub password: String,
    pub role: Option<String>,
    pub quota_bytes: Option<i64>,
    pub locale: Option<String>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

async fn create_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(req): Json<CreateUserReq>,
) -> AppResult<impl IntoResponse> {
    let user_name = req.user_name.trim().to_string();
    if !validate_user_name(&user_name) {
        return Err(AppError::invalid("invalid user name"));
    }
    let email = match req.email.as_deref().filter(|e| !e.trim().is_empty()) {
        Some(email) => Some(normalize_email(email).ok_or_else(|| AppError::invalid("invalid email"))?),
        None => None,
    };

    let plain = open_password(
        &state,
        &headers,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?
    .expose_secret()
    .to_string();
    password::ensure_strong(&plain, &user_name, email.as_deref().unwrap_or(""))?;

    if repo::find_user_by_name(&state.db, &user_name).await?.is_some() {
        return Err(AppError::new(ErrorCode::UserExists));
    }
    if let Some(email) = &email {
        if repo::find_user_by_email(&state.db, email).await?.is_some() {
            return Err(AppError::new(ErrorCode::EmailTaken));
        }
    }

    let role = match req.role.as_deref() {
        Some("admin") => repo::ROLE_ADMIN,
        _ => repo::ROLE_USER,
    };
    let quota = req
        .quota_bytes
        .unwrap_or(state.cfg.quota.default_bytes)
        .max(0);
    let locale = match req.locale.as_deref() {
        Some("en-US") => "en-US",
        _ => "zh-CN",
    };

    let hash = password::hash_password(plain, argon_params(&state)).await?;
    let user = repo::create_user(
        &state.db,
        repo::NewUser {
            user_name: &user_name,
            email: email.as_deref(),
            password_hash: &hash,
            role,
            quota_bytes: quota,
            locale,
            email_verified: email.is_some(),
        },
    )
    .await?;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.user.create",
            target_type: "user",
            target_id: Some(user.id.to_hex()),
            result: "success",
            detail: Some(format!("role={role} quota={quota}")),
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok(ok(user_json(&user)))
}

async fn get_user(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    let user = repo::find_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
    let sessions = repo::list_user_sessions(&state.db, user_id).await?;
    Ok(ok(json!({
        "user": user_json(&user),
        "sessions": sessions.iter().map(|s| json!({
            "id": s.id.to_hex(),
            "createdAt": dt_ms(s.created_at),
            "expiresAt": dt_ms(s.expires_at),
            "ip": s.ip,
            "userAgent": s.user_agent,
        })).collect::<Vec<_>>(),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserReq {
    pub role: Option<String>,
    pub status: Option<String>,
    pub quota_bytes: Option<i64>,
    pub locale: Option<String>,
    pub must_change_password: Option<bool>,
}

async fn patch_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<PatchUserReq>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    if user_id == admin.id {
        return Err(AppError::invalid("cannot modify yourself"));
    }

    let mut set = doc! {};
    if let Some(role) = req.role.as_deref() {
        if !["admin", "user"].contains(&role) {
            return Err(AppError::invalid("invalid role"));
        }
        set.insert("role", role);
    }
    if let Some(status) = req.status.as_deref() {
        if !["active", "disabled", "deleted"].contains(&status) {
            return Err(AppError::invalid("invalid status"));
        }
        set.insert("status", status);
    }
    if let Some(quota) = req.quota_bytes {
        if quota < 0 {
            return Err(AppError::invalid("invalid quota"));
        }
        set.insert("quotaBytes", quota);
    }
    if let Some(locale) = req.locale.as_deref() {
        if !["zh-CN", "en-US"].contains(&locale) {
            return Err(AppError::invalid("invalid locale"));
        }
        set.insert("locale", locale);
    }
    if let Some(flag) = req.must_change_password {
        set.insert("mustChangePassword", flag);
    }
    if set.is_empty() {
        return Err(AppError::invalid("nothing to update"));
    }

    // 禁用/角色变更必须立即吊销所有 token
    let security_change = req.status.is_some() || req.role.is_some();
    let user = repo::update_user(&state.db, user_id, set).await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
    if security_change {
        repo::bump_token_version(&state.db, user_id, None).await?;
        repo::delete_user_sessions(&state.db, user_id).await?;
    }
    state.invalidate_profile(user_id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.user.update",
            target_type: "user",
            target_id: Some(user_id.to_hex()),
            result: "success",
            detail: Some(format!(
                "status={:?} role={:?} quota={:?}",
                req.status, req.role, req.quota_bytes
            )),
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok(ok(user_json(&user)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetPasswordReq {
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

async fn reset_password(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<ResetPasswordReq>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    let user = repo::find_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;

    let (plain, generated) = match req.password.as_deref().filter(|p| !p.is_empty()) {
        Some(cipher) => (
            open_password(&state, &headers, cipher, req.timestamp, req.nonce.as_deref())
                .await?
                .expose_secret()
                .to_string(),
            false,
        ),
        None => (password::random_password(16), true),
    };
    password::ensure_strong(&plain, &user.user_name, user.email.as_deref().unwrap_or(""))?;

    let hash = password::hash_password(plain.clone(), argon_params(&state)).await?;
    repo::update_user(
        &state.db,
        user_id,
        doc! {
            "passwordHash": hash,
            "passwordAlgo": "argon2id",
            "mustChangePassword": true,
        },
    )
    .await?;
    repo::bump_token_version(&state.db, user_id, Some(true)).await?;
    repo::delete_user_sessions(&state.db, user_id).await?;
    state.invalidate_profile(user_id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.user.reset_password",
            target_type: "user",
            target_id: Some(user_id.to_hex()),
            result: "success",
            detail: Some(if generated { "generated".to_string() } else { "provided".to_string() }),
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok(ok(json!({
        "mustChangePassword": true,
        "generatedPassword": if generated { Some(plain) } else { None },
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteUserQuery {
    #[serde(default)]
    pub purge_saves: bool,
}

async fn delete_user(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DeleteUserQuery>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    if user_id == admin.id {
        return Err(AppError::invalid("cannot delete yourself"));
    }
    let user = repo::find_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;

    if query.purge_saves {
        let saves = repo::list_saves(&state.db, user_id).await?;
        for save in &saves {
            state
                .storage
                .delete_save(&state.db, &user, save, true)
                .await?;
        }
        repo::users(&state.db).delete_one(doc! { "_id": user_id }).await?;
        repo::delete_user_sessions(&state.db, user_id).await?;
    } else {
        repo::update_user(&state.db, user_id, doc! { "status": "deleted" }).await?;
        repo::bump_token_version(&state.db, user_id, None).await?;
        repo::delete_user_sessions(&state.db, user_id).await?;
    }
    state.invalidate_profile(user_id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: if query.purge_saves {
                "admin.user.purge"
            } else {
                "admin.user.delete"
            },
            target_type: "user",
            target_id: Some(user_id.to_hex()),
            result: "success",
            detail: Some(user.user_name.clone()),
            ip: &ip.to_string(),
            user_agent: user_agent(&headers),
        },
    )
    .await;

    Ok(ok(json!({ "deleted": true, "purged": query.purge_saves })))
}

// ---------------------------------------------------------------------------
// 存档管理
// ---------------------------------------------------------------------------

async fn user_saves(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path(id): Path<String>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    let user = repo::find_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
    let saves = repo::list_saves(&state.db, user_id).await?;
    Ok(ok(json!({
        "user": user_json(&user),
        "files": saves.iter().map(|s| json!({
            "id": s.id.to_hex(),
            "fileName": s.file_name,
            "sizeBytes": s.size_bytes,
            "mtimeMs": s.mtime_ms,
            "sm3": s.sm3,
            "currentSeq": s.current_seq,
            "updatedAt": dt_ms(s.updated_at),
        })).collect::<Vec<_>>(),
    })))
}

async fn user_save_versions(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Path((id, name)): Path<(String, String)>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    let file_name = Storage::validate_file_name(&name)?;
    let save = repo::find_save(&state.db, user_id, &file_name)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::FileNotFound))?;
    let versions = repo::list_versions(&state.db, save.id, 1000).await?;
    Ok(ok(json!({
        "fileName": save.file_name,
        "currentSeq": save.current_seq,
        "versions": versions.iter().map(|v| json!({
            "id": v.id.to_hex(),
            "seq": v.seq,
            "sizeBytes": v.size_bytes,
            "sm3": v.sm3,
            "createdAt": dt_ms(v.created_at),
            "source": v.source,
            "current": Some(v.id) == save.current_version_id,
        })).collect::<Vec<_>>(),
    })))
}

async fn admin_rollback(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    ClientIp(ip): ClientIp,
    Path((id, name, vid)): Path<(String, String, String)>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    let file_name = Storage::validate_file_name(&name)?;
    let user = repo::find_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
    let save = repo::find_save(&state.db, user_id, &file_name)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::FileNotFound))?;
    let version_id = parse_id(&vid)?;
    let version = repo::find_version(&state.db, version_id)
        .await?
        .filter(|v| v.save_id == save.id)
        .ok_or_else(|| AppError::new(ErrorCode::VersionNotFound))?;

    let updated = state.storage.rollback(&state.db, &user, &save, &version).await?;
    state.invalidate_saves(user_id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.save.rollback",
            target_type: "save",
            target_id: Some(save.id.to_hex()),
            result: "success",
            detail: Some(format!("{} seq={}", save.file_name, version.seq)),
            ip: &ip.to_string(),
            user_agent: None,
        },
    )
    .await;

    Ok(ok(json!({
        "fileName": updated.file_name,
        "currentSeq": updated.current_seq,
        "sizeBytes": updated.size_bytes,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminDeleteSaveQuery {
    #[serde(default)]
    pub delete_versions: bool,
}

async fn admin_delete_save(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    ClientIp(ip): ClientIp,
    Path((id, name)): Path<(String, String)>,
    Query(query): Query<AdminDeleteSaveQuery>,
) -> AppResult<impl IntoResponse> {
    let user_id = parse_id(&id)?;
    let file_name = Storage::validate_file_name(&name)?;
    let user = repo::find_user_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
    let save = repo::find_save(&state.db, user_id, &file_name)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::FileNotFound))?;

    let outcome = state
        .storage
        .delete_save(&state.db, &user, &save, query.delete_versions)
        .await?;
    state.invalidate_saves(user_id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.save.delete",
            target_type: "save",
            target_id: Some(save.id.to_hex()),
            result: "success",
            detail: Some(format!(
                "{} all={} freed={}",
                save.file_name, query.delete_versions, outcome.freed_bytes
            )),
            ip: &ip.to_string(),
            user_agent: None,
        },
    )
    .await;

    Ok(ok(json!({
        "deletedAll": outcome.deleted_all,
        "freedBytes": outcome.freed_bytes,
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSearchQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub user_name: Option<String>,
    pub file_name: Option<String>,
}

async fn search_saves(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Query(query): Query<SaveSearchQuery>,
) -> AppResult<impl IntoResponse> {
    let page = pagination(query.page, query.page_size);
    let (rows, total) = repo::search_saves(
        &state.db,
        query.user_name.as_deref(),
        query.file_name.as_deref(),
        page.skip(),
        page.limit(),
    )
    .await?;
    Ok(ok(json!({
        "items": rows.iter().map(|(save, user)| json!({
            "id": save.id.to_hex(),
            "userId": user.id.to_hex(),
            "userName": user.user_name,
            "fileName": save.file_name,
            "sizeBytes": save.size_bytes,
            "mtimeMs": save.mtime_ms,
            "currentSeq": save.current_seq,
            "updatedAt": dt_ms(save.updated_at),
        })).collect::<Vec<_>>(),
        "total": total,
    })))
}

// ---------------------------------------------------------------------------
// 概览 / 审计 / 设置 / 密钥
// ---------------------------------------------------------------------------

async fn stats_overview(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> AppResult<impl IntoResponse> {
    let total_users = repo::count_users(&state.db, doc! {}).await?;
    let active_users = repo::count_users(&state.db, doc! { "status": "active" }).await?;
    let disabled_users = repo::count_users(&state.db, doc! { "status": "disabled" }).await?;
    let total_saves = state.db.collection::<mongodb::bson::Document>("saves").count_documents(doc! {}).await?;
    let total_versions = state
        .db
        .collection::<mongodb::bson::Document>("save_versions")
        .count_documents(doc! {})
        .await?;
    let usage = repo::aggregate_usage(&state.db).await?;
    let total_bytes: i64 = usage.iter().map(|(_, bytes)| *bytes).sum();

    let (recent, _) = repo::list_audit(
        &state.db,
        AuditFilter {
            actor: None,
            action: None,
            result: None,
            from_ms: None,
            to_ms: None,
        },
        0,
        10,
    )
    .await?;

    Ok(ok(json!({
        "users": { "total": total_users, "active": active_users, "disabled": disabled_users },
        "saves": { "files": total_saves, "versions": total_versions, "bytes": total_bytes },
        "disk": {
            "freeBytes": state.storage.available_space(),
            "minFreeBytes": state.cfg.storage.min_free_bytes,
        },
        "backup": state.backup.read().await.clone(),
        "recentAudit": recent.iter().map(audit_json).collect::<Vec<_>>(),
        "serverTime": now_ms(),
        "uptimeSeconds": state.uptime_seconds(),
        "activeKeyId": state.keyring.read().await.active_key_id(),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditQuery {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
    pub actor: Option<String>,
    pub action: Option<String>,
    pub result: Option<String>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

fn audit_json(log: &repo::AuditLog) -> Value {
    json!({
        "id": log.id.to_hex(),
        "actorId": log.actor_id.map(|id| id.to_hex()),
        "actorName": log.actor_name,
        "action": log.action,
        "targetType": log.target_type,
        "targetId": log.target_id,
        "result": log.result,
        "detail": log.detail,
        "ip": log.ip,
        "userAgent": log.user_agent,
        "createdAt": dt_ms(log.created_at),
    })
}

async fn list_audit(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
    Query(query): Query<AuditQuery>,
) -> AppResult<impl IntoResponse> {
    let (logs, total) = repo::list_audit(
        &state.db,
        AuditFilter {
            actor: query.actor.as_deref(),
            action: query.action.as_deref(),
            result: query.result.as_deref(),
            from_ms: query.from_ms,
            to_ms: query.to_ms,
        },
        pagination(query.page, query.page_size).skip(),
        pagination(query.page, query.page_size).limit(),
    )
    .await?;
    Ok(ok(json!({
        "items": logs.iter().map(audit_json).collect::<Vec<_>>(),
        "total": total,
    })))
}

async fn export_audit(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Query(query): Query<AuditQuery>,
) -> AppResult<Response> {
    let (logs, _) = repo::list_audit(
        &state.db,
        AuditFilter {
            actor: query.actor.as_deref(),
            action: query.action.as_deref(),
            result: query.result.as_deref(),
            from_ms: query.from_ms,
            to_ms: query.to_ms,
        },
        0,
        20_000,
    )
    .await?;

    // BOM 让 Excel 正确识别 UTF-8
    let mut csv = String::from("\u{feff}");
    csv.push_str("time,actor,action,targetType,targetId,result,ip,detail\n");
    for log in &logs {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            csv_escape(&format_datetime(dt_ms(log.created_at))),
            csv_escape(&log.actor_name),
            csv_escape(&log.action),
            csv_escape(&log.target_type),
            csv_escape(log.target_id.as_deref().unwrap_or("")),
            csv_escape(&log.result),
            csv_escape(&log.ip),
            csv_escape(log.detail.as_deref().unwrap_or("")),
        ));
    }

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.audit.export",
            target_type: "audit",
            target_id: None,
            result: "success",
            detail: Some(format!("rows={}", logs.len())),
            ip: "-",
            user_agent: None,
        },
    )
    .await;

    let filename = format!("audit-{}.csv", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    Ok((
        StatusCode::OK,
        [
            ("content-type", "text/csv; charset=utf-8".to_string()),
            (
                "content-disposition",
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        csv,
    )
        .into_response())
}

async fn get_settings(
    State(state): State<AppState>,
    AdminUser(_admin): AdminUser,
) -> AppResult<impl IntoResponse> {
    let settings = repo::get_settings(&state.db)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;
    Ok(ok(json!({
        "defaultQuotaBytes": settings.default_quota_bytes,
        "maxFileSize": settings.max_file_size,
        "allowedExtensions": settings.allowed_extensions,
        "updatedAt": dt_ms(settings.updated_at),
    })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsReq {
    pub default_quota_bytes: Option<i64>,
    pub max_file_size: Option<i64>,
    pub allowed_extensions: Option<Vec<String>>,
}

async fn put_settings(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(req): Json<SettingsReq>,
) -> AppResult<impl IntoResponse> {
    let mut settings = repo::get_settings(&state.db)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::NotFound))?;

    if let Some(quota) = req.default_quota_bytes {
        if quota < 0 {
            return Err(AppError::invalid("invalid quota"));
        }
        settings.default_quota_bytes = quota;
    }
    if let Some(max) = req.max_file_size {
        if max <= 0 || max > 10 * 1024 * 1024 * 1024 {
            return Err(AppError::invalid("invalid max file size"));
        }
        settings.max_file_size = max;
    }
    if let Some(exts) = req.allowed_extensions {
        settings.allowed_extensions = exts
            .into_iter()
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
    }
    settings.updated_at = now_dt();
    settings.updated_by = Some(admin.id);
    repo::put_settings(&state.db, &settings).await?;
    state.cache.del(&[crate::state::keys::stats_overview()]).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.settings.update",
            target_type: "settings",
            target_id: Some("default".to_string()),
            result: "success",
            detail: Some(format!(
                "quota={} max={}",
                settings.default_quota_bytes, settings.max_file_size
            )),
            ip: "-",
            user_agent: None,
        },
    )
    .await;

    Ok(ok(json!({
        "defaultQuotaBytes": settings.default_quota_bytes,
        "maxFileSize": settings.max_file_size,
        "allowedExtensions": settings.allowed_extensions,
    })))
}

async fn rotate_key(
    State(state): State<AppState>,
    AdminUser(admin): AdminUser,
    ClientIp(ip): ClientIp,
) -> AppResult<impl IntoResponse> {
    let mut ring = state.keyring.write().await;
    let new_id = ring.rotate().map_err(|err| {
        tracing::error!(error = %err, "SM2 密钥轮换失败");
        AppError::internal("rotate failed")
    })?;
    let pruned = ring
        .prune(state.cfg.crypto.key_grace_hours)
        .unwrap_or_default();
    let info = ring.public_key_info();
    drop(ring);

    info!(key_id = %new_id, pruned = ?pruned, "SM2 密钥已轮换");
    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(admin.id),
            actor_name: &admin.user_name,
            action: "admin.crypto.rotate",
            target_type: "key",
            target_id: Some(new_id.clone()),
            result: "success",
            detail: Some(format!("pruned={pruned:?}")),
            ip: &ip.to_string(),
            user_agent: None,
        },
    )
    .await;

    Ok(ok(json!({
        "keyId": info.key_id,
        "publicKey": info.public_key,
        "pruned": pruned,
    })))
}

fn parse_id(raw: &str) -> AppResult<ObjectId> {
    ObjectId::parse_str(raw).map_err(|_| AppError::new(ErrorCode::NotFound))
}

fn argon_params(state: &AppState) -> Argon2Params {
    Argon2Params {
        memory_kib: state.cfg.auth.argon2_memory_kib,
        iterations: state.cfg.auth.argon2_iterations,
        parallelism: state.cfg.auth.argon2_parallelism,
    }
}
