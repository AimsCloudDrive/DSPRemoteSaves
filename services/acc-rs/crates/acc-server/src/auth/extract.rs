//! 鉴权提取器：`CurrentUser` / `AdminUser`。
//!
//! 业务 handler 只声明身份，权限判断集中在这里。

use crate::auth::token::{self, AUD_ADMIN, AUD_USER};
use crate::error::{AppError, ErrorCode};
use crate::repo::Profile;
use crate::state::{keys, AppState};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use mongodb::bson::oid::ObjectId;

pub struct CurrentUser(pub Profile);

pub struct AdminUser(pub Profile);

fn bearer(headers: &HeaderMap) -> Result<&str, AppError> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
    Ok(token)
}

async fn authenticate(parts: &mut Parts, state: &AppState) -> Result<Profile, AppError> {
    let raw = bearer(&parts.headers)?;
    let claims = token::decode_access(&state.secrets.jwt, raw, &[AUD_USER, AUD_ADMIN])?;
    let profile = authenticate_with_claims(state, claims).await?;

    // 管理员重置后强制改密：只放行必要的三个接口
    if profile.must_change_password {
        let path = parts.uri.path();
        let allowed = path.ends_with("/auth/me")
            || path.ends_with("/auth/password")
            || path.ends_with("/auth/logout");
        if !allowed {
            return Err(AppError::new(ErrorCode::Forbidden)
                .with_payload(serde_json::json!({ "reason": "must_change_password" })));
        }
    }
    Ok(profile)
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(CurrentUser(authenticate(parts, state).await?))
    }
}

impl FromRequestParts<AppState> for AdminUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let raw = bearer(&parts.headers)?;
        // 用户 token（aud=user）不能进入管理端
        let claims = token::decode_access(&state.secrets.jwt, raw, &[AUD_ADMIN])?;
        let profile = authenticate_with_claims(state, claims).await?;
        if !profile.is_admin() {
            return Err(AppError::new(ErrorCode::Forbidden));
        }
        Ok(AdminUser(profile))
    }
}

async fn authenticate_with_claims(
    state: &AppState,
    claims: token::Claims,
) -> Result<Profile, AppError> {
    // 登出后的 access token 在剩余有效期内必须失效
    if is_blacklisted(state, &claims.jti).await {
        return Err(AppError::new(ErrorCode::Unauthorized));
    }
    let user_id = ObjectId::parse_str(&claims.sub)
        .map_err(|_| AppError::new(ErrorCode::Unauthorized))?;
    let current = state.token_version(user_id).await?;
    if current != claims.ver {
        return Err(AppError::new(ErrorCode::Unauthorized));
    }
    let profile = state.load_profile(user_id).await?;
    if !profile.is_active() {
        return Err(AppError::new(ErrorCode::Forbidden));
    }
    Ok(profile)
}

/// 登出时把 access token 的 jti 加入黑名单，TTL = 剩余有效期。
pub async fn blacklist_jti(state: &AppState, claims: &token::Claims) {
    let ttl = claims.exp - chrono::Utc::now().timestamp();
    if ttl > 0 {
        state
            .cache
            .set_ex(&keys::jwt_blacklist(&claims.jti), "1", ttl as u64)
            .await;
    }
}

pub async fn is_blacklisted(state: &AppState, jti: &str) -> bool {
    state
        .cache
        .get_raw(&keys::jwt_blacklist(jti))
        .await
        .is_some()
}
