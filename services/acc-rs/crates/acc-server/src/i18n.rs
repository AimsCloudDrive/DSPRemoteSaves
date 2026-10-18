//! 双语（zh-CN / en-US）：`Accept-Language` 或 `?lang=` 决定当前请求语言。
//!
//! 语言通过 task-local 传递给 `AppError::into_response`，handler 里无需手动处理。

use axum::http::{HeaderMap, Uri};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    #[serde(rename = "zh-CN")]
    Zh,
    #[serde(rename = "en-US")]
    En,
}

impl Lang {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Zh => "zh-CN",
            Self::En => "en-US",
        }
    }

    pub fn from_tag(tag: &str) -> Self {
        if tag.trim().to_ascii_lowercase().starts_with("en") {
            Self::En
        } else {
            Self::Zh
        }
    }

    pub fn from_headers(headers: &HeaderMap) -> Self {
        if let Some(value) = headers.get("accept-language").and_then(|v| v.to_str().ok()) {
            // 取权重最高的第一个语言标签即可，服务只支持中英
            for part in value.split(',') {
                let tag = part.split(';').next().unwrap_or("").trim();
                if tag.to_ascii_lowercase().starts_with("en") {
                    return Self::En;
                }
                if tag.to_ascii_lowercase().starts_with("zh") {
                    return Self::Zh;
                }
            }
        }
        Self::Zh
    }

    pub fn from_uri(uri: &Uri) -> Option<Self> {
        let query = uri.query()?;
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=')?;
            if k == "lang" {
                return Some(Self::from_tag(v));
            }
        }
        None
    }
}

tokio::task_local! {
    static CURRENT_LANG: Lang;
}

pub fn current() -> Lang {
    CURRENT_LANG.try_with(|l| *l).unwrap_or_default()
}

pub async fn with_lang<F: std::future::Future>(lang: Lang, fut: F) -> F::Output {
    CURRENT_LANG.scope(lang, fut).await
}

pub fn ok_message(lang: Lang) -> &'static str {
    match lang {
        Lang::Zh => "成功",
        Lang::En => "OK",
    }
}

pub fn error_message(key: &str, lang: Lang) -> &'static str {
    let (zh, en) = match key {
        "ok" => ("成功", "OK"),
        "invalid_param" => ("参数错误", "Invalid parameter"),
        "unauthorized" => ("未登录或登录已过期", "Not signed in or session expired"),
        "forbidden" => ("没有权限执行该操作", "You are not allowed to perform this action"),
        "not_found" => ("资源不存在", "Resource not found"),
        "conflict" => ("资源冲突", "Resource conflict"),
        "rate_limited" => ("请求过于频繁，请稍后再试", "Too many requests, please try again later"),
        "account_locked" => ("失败次数过多，账号已临时锁定", "Too many failures, account temporarily locked"),
        "cache_unavailable" => ("缓存服务不可用", "Cache service unavailable"),
        "sm_key_invalid" => ("加密密钥无效，请重新获取公钥", "Invalid encryption key, please refresh the public key"),
        "sm_cipher_invalid" => ("密码密文格式错误", "Malformed password ciphertext"),
        "sm_timestamp_expired" => ("请求时间戳超出允许范围，请校准系统时间", "Request timestamp is out of range, please check your clock"),
        "sm_replay" => ("重复的请求已被拒绝", "Duplicate request rejected"),
        "sm_key_unknown" => ("服务端密钥已更新，请重新获取公钥", "Server key rotated, please refresh the public key"),
        "plaintext_forbidden" => ("必须使用加密方式提交密码", "Password must be submitted encrypted"),
        "user_exists" => ("用户名已被占用", "Username already taken"),
        "email_taken" => ("邮箱已被使用", "Email already in use"),
        "password_weak" => ("密码强度不足", "Password is too weak"),
        "quota_exceeded" => ("存储配额不足", "Storage quota exceeded"),
        "file_too_large" => ("文件超过大小限制", "File exceeds the size limit"),
        "chunk_incomplete" => ("分片不完整", "Upload chunks are incomplete"),
        "file_not_found" => ("文件不存在", "File not found"),
        "upload_expired" => ("上传会话已过期", "Upload session expired"),
        "disk_full" => ("服务器磁盘空间不足", "Server disk is full"),
        "invalid_file_name" => ("文件名非法", "Invalid file name"),
        "code_invalid" => ("验证码错误或已过期", "Verification code is invalid or expired"),
        "email_not_verified" => ("邮箱未验证", "Email not verified"),
        "version_not_found" => ("版本不存在", "Version not found"),
        "mail_send_failed" => ("邮件发送失败，请稍后重试", "Failed to send email, please retry later"),
        "mail_quota_exhausted" => ("今日邮件配额已用完", "Daily email quota exhausted"),
        "mail_unavailable" => ("邮件服务暂时不可用", "Mail service temporarily unavailable"),
        "internal" => ("服务器内部错误", "Internal server error"),
        _ => ("请求失败", "Request failed"),
    };
    match lang {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

/// 校验类提示（密码强度、注册等）按缺失项返回。
pub fn password_issue_label(issue: &str, lang: Lang) -> &'static str {
    match (issue, lang) {
        ("length", Lang::Zh) => "长度需为 10~64 位",
        ("length", Lang::En) => "Must be 10-64 characters",
        ("letter", Lang::Zh) => "需包含英文字母",
        ("letter", Lang::En) => "Must contain a letter",
        ("digit", Lang::Zh) => "需包含数字",
        ("digit", Lang::En) => "Must contain a digit",
        ("special", Lang::Zh) => "需包含特殊字符",
        ("special", Lang::En) => "Must contain a special character",
        ("personal", Lang::Zh) => "不能包含用户名或邮箱前缀",
        ("personal", Lang::En) => "Must not contain your username or email prefix",
        ("common", Lang::Zh) => "该密码过于常见",
        ("common", Lang::En) => "This password is too common",
        _ => "",
    }
}
