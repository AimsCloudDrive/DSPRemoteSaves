//! JWT access token 与 refresh token 工具。

use crate::error::{AppError, ErrorCode};
use crate::util::random_token;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

pub const ISSUER: &str = "dsp-saves";
pub const AUD_USER: &str = "user";
pub const AUD_ADMIN: &str = "admin";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub ver: i32,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
}

/// 签发 access token；返回 `(token, expires_at_ms)`。
pub fn issue_access(
    secret: &[u8],
    user_id: ObjectId,
    token_version: i32,
    audience: &str,
    ttl_minutes: i64,
) -> Result<(String, i64), AppError> {
    let now = chrono::Utc::now();
    let exp = now + chrono::Duration::minutes(ttl_minutes);
    let claims = Claims {
        iss: ISSUER.to_string(),
        aud: audience.to_string(),
        sub: user_id.to_hex(),
        ver: token_version,
        jti: uuid::Uuid::new_v4().simple().to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret),
    )
    .map_err(|err| {
        tracing::error!(error = %err, "JWT 签发失败");
        AppError::internal("jwt encode failed")
    })?;
    Ok((token, exp.timestamp_millis()))
}

/// 校验 access token；`audiences` 为允许的受众列表。
pub fn decode_access(
    secret: &[u8],
    token: &str,
    audiences: &[&str],
) -> Result<Claims, AppError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(audiences);
    validation.leeway = 5;
    validation.validate_exp = true;

    decode::<Claims>(token, &DecodingKey::from_secret(secret), &validation)
        .map(|data| data.claims)
        .map_err(|err| {
            tracing::debug!(error = %err, "JWT 校验失败");
            AppError::new(ErrorCode::Unauthorized)
        })
}

/// refresh token：32 字节随机串，DB 只存 SHA-256。
pub fn new_refresh_token() -> (String, String) {
    let raw = random_token();
    let hash = crate::util::sha256_hex(raw.as_bytes());
    (raw, hash)
}

pub fn hash_refresh_token(raw: &str) -> String {
    crate::util::sha256_hex(raw.as_bytes())
}

pub fn cookie_name(audience: &str) -> &'static str {
    if audience == AUD_ADMIN {
        "dsp_admin_rt"
    } else {
        "dsp_user_rt"
    }
}

/// Cookie Path 限定到对应 API 前缀，避免用户端 cookie 被管理端接口带上。
pub fn cookie_path(base_path: &str, audience: &str) -> String {
    let base = base_path.trim_end_matches('/');
    if audience == AUD_ADMIN {
        format!("{base}/admin/auth")
    } else {
        format!("{base}/auth")
    }
}
