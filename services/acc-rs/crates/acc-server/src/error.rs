//! 统一错误码与响应。

use crate::i18n::{self, Lang};
use crate::util::current_trace_id;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Ok,
    InvalidParam,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    RateLimited,
    AccountLocked,
    CacheUnavailable,
    SmKeyInvalid,
    SmCipherInvalid,
    SmTimestampExpired,
    SmReplay,
    SmKeyUnknown,
    PlaintextForbidden,
    UserExists,
    EmailTaken,
    PasswordWeak,
    QuotaExceeded,
    FileTooLarge,
    ChunkIncomplete,
    FileNotFound,
    UploadExpired,
    DiskFull,
    InvalidFileName,
    CodeInvalid,
    EmailNotVerified,
    VersionNotFound,
    MailSendFailed,
    MailQuotaExhausted,
    MailUnavailable,
    Internal,
}

impl ErrorCode {
    pub fn numeric(self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::InvalidParam => 1001,
            Self::Unauthorized => 1002,
            Self::Forbidden => 1003,
            Self::NotFound => 1004,
            Self::Conflict => 1005,
            Self::RateLimited => 1006,
            Self::AccountLocked => 1007,
            Self::CacheUnavailable => 1008,
            Self::SmKeyInvalid => 2001,
            Self::SmCipherInvalid => 2002,
            Self::SmTimestampExpired => 2003,
            Self::SmReplay => 2004,
            Self::SmKeyUnknown => 2005,
            Self::PlaintextForbidden => 2006,
            Self::UserExists => 3001,
            Self::EmailTaken => 3002,
            Self::PasswordWeak => 3003,
            Self::QuotaExceeded => 3004,
            Self::FileTooLarge => 3005,
            Self::ChunkIncomplete => 3006,
            Self::FileNotFound => 3007,
            Self::UploadExpired => 3008,
            Self::DiskFull => 3009,
            Self::InvalidFileName => 3010,
            Self::CodeInvalid => 3011,
            Self::EmailNotVerified => 3012,
            Self::VersionNotFound => 3013,
            Self::MailSendFailed => 4001,
            Self::MailQuotaExhausted => 4002,
            Self::MailUnavailable => 4003,
            Self::Internal => 5000,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::InvalidParam => "invalid_param",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::RateLimited => "rate_limited",
            Self::AccountLocked => "account_locked",
            Self::CacheUnavailable => "cache_unavailable",
            Self::SmKeyInvalid => "sm_key_invalid",
            Self::SmCipherInvalid => "sm_cipher_invalid",
            Self::SmTimestampExpired => "sm_timestamp_expired",
            Self::SmReplay => "sm_replay",
            Self::SmKeyUnknown => "sm_key_unknown",
            Self::PlaintextForbidden => "plaintext_forbidden",
            Self::UserExists => "user_exists",
            Self::EmailTaken => "email_taken",
            Self::PasswordWeak => "password_weak",
            Self::QuotaExceeded => "quota_exceeded",
            Self::FileTooLarge => "file_too_large",
            Self::ChunkIncomplete => "chunk_incomplete",
            Self::FileNotFound => "file_not_found",
            Self::UploadExpired => "upload_expired",
            Self::DiskFull => "disk_full",
            Self::InvalidFileName => "invalid_file_name",
            Self::CodeInvalid => "code_invalid",
            Self::EmailNotVerified => "email_not_verified",
            Self::VersionNotFound => "version_not_found",
            Self::MailSendFailed => "mail_send_failed",
            Self::MailQuotaExhausted => "mail_quota_exhausted",
            Self::MailUnavailable => "mail_unavailable",
            Self::Internal => "internal",
        }
    }

    pub fn http(self) -> StatusCode {
        match self {
            Self::Ok => StatusCode::OK,
            Self::InvalidParam
            | Self::SmKeyInvalid
            | Self::SmCipherInvalid
            | Self::SmTimestampExpired
            | Self::SmKeyUnknown
            | Self::PasswordWeak
            | Self::ChunkIncomplete
            | Self::UploadExpired
            | Self::InvalidFileName
            | Self::CodeInvalid => StatusCode::BAD_REQUEST,
            Self::Unauthorized | Self::PlaintextForbidden => StatusCode::UNAUTHORIZED,
            Self::Forbidden | Self::EmailNotVerified => StatusCode::FORBIDDEN,
            Self::NotFound | Self::FileNotFound | Self::VersionNotFound => StatusCode::NOT_FOUND,
            Self::Conflict | Self::SmReplay | Self::UserExists | Self::EmailTaken => {
                StatusCode::CONFLICT
            }
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::AccountLocked => StatusCode::LOCKED,
            Self::CacheUnavailable | Self::MailQuotaExhausted | Self::MailUnavailable => {
                StatusCode::SERVICE_UNAVAILABLE
            }
            Self::QuotaExceeded | Self::FileTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::DiskFull => StatusCode::INSUFFICIENT_STORAGE,
            Self::MailSendFailed => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Debug)]
pub struct AppError {
    pub code: ErrorCode,
    /// 面向用户的补充说明（已本地化前的原始信息，一般不返回）
    pub detail: Option<String>,
    pub payload: Option<Value>,
    pub retry_after: Option<u64>,
    /// 覆盖默认本地化文案（如登录失败统一提示“用户名或密码错误”）
    pub message_override: Option<String>,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            detail: None,
            payload: None,
            retry_after: None,
            message_override: None,
        }
    }

    pub fn invalid(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidParam).with_detail(detail)
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal).with_detail(detail)
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Forbidden).with_detail(detail)
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound).with_detail(detail)
    }

    pub fn conflict(detail: impl Into<String>) -> Self {
        Self::new(ErrorCode::Conflict).with_detail(detail)
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_payload(mut self, payload: Value) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_retry(mut self, seconds: u64) -> Self {
        self.retry_after = Some(seconds);
        self
    }

    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message_override = Some(message.into());
        self
    }

    pub fn localized(&self, lang: Lang) -> String {
        self.message_override
            .clone()
            .unwrap_or_else(|| i18n::error_message(self.code.key(), lang).to_string())
    }
}

impl From<mongodb::error::Error> for AppError {
    fn from(err: mongodb::error::Error) -> Self {
        tracing::error!(error = %err, "MongoDB 操作失败");
        AppError::internal("database error")
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        tracing::error!(error = %err, "文件操作失败");
        AppError::internal("io error")
    }
}

impl From<sm_envelope::CryptoError> for AppError {
    fn from(err: sm_envelope::CryptoError) -> Self {
        use sm_envelope::CryptoError as C;
        tracing::warn!(error = %err, "密码信封解析失败");
        match err {
            C::UnknownKeyId(_) => AppError::new(ErrorCode::SmKeyUnknown),
            C::BadPadding | C::BadUtf8 | C::BadSm4Cipher | C::BadSm4Key => {
                AppError::new(ErrorCode::SmCipherInvalid)
            }
            C::Sm2Decrypt | C::Sm2Cipher | C::Sm2PublicKey => AppError::new(ErrorCode::SmKeyInvalid),
            _ => AppError::new(ErrorCode::Internal),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "[{}] {} ({detail})", self.code.numeric(), self.code.key()),
            None => write!(f, "[{}] {}", self.code.numeric(), self.code.key()),
        }
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let lang = i18n::current();
        let trace_id = current_trace_id();
        let message = self.localized(lang);

        let mut body = json!({
            "code": self.code.numeric(),
            "messageKey": self.code.key(),
            "message": message,
            "traceId": trace_id,
        });

        if let Some(payload) = self.payload {
            body["payload"] = payload;
        }
        if let Some(detail) = self.detail.as_deref() {
            // detail 只记录内部原因，返回给客户端时保持简短，避免泄露实现
            body["detail"] = json!(detail);
        }

        let mut response = (self.code.http(), Json(body)).into_response();
        if let Some(retry) = self.retry_after {
            if let Ok(value) = retry.to_string().parse() {
                response.headers_mut().insert("retry-after", value);
            }
            response.headers_mut().insert(
                "x-retry-after-seconds",
                retry.to_string().parse().unwrap_or_else(|_| "0".parse().unwrap()),
            );
        }
        response
    }
}

/// 统一成功响应（新接口）。
pub fn ok<T: serde::Serialize>(payload: T) -> Json<Value> {
    Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": i18n::ok_message(i18n::current()),
        "payload": payload,
        "traceId": current_trace_id(),
    }))
}

pub fn ok_empty() -> Json<Value> {
    Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": i18n::ok_message(i18n::current()),
        "traceId": current_trace_id(),
    }))
}
