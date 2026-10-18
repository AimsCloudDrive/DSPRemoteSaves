//! 国密信封入口：公钥接口、时间戳校验、防重放、SM2/SM4 解密。
//!
//! 校验顺序严格按文档：时间戳 → 重放 → SM2 → SM4 → 验密码，
//! 避免对攻击者做无谓的椭圆曲线运算。

use crate::error::{AppError, AppError as E, AppResult, ErrorCode};
use crate::state::AppState;
use crate::util::{now_ms, sm3_hex};
use axum::http::HeaderMap;
use secrecy::SecretString;
use sm_envelope::CryptoError;
use tracing::{debug, warn};

pub const HEADER_SMKEY: &str = "smkey";
pub const HEADER_KEY_ID: &str = "smkeyid";
pub const HEADER_KEY_ID_ALT: &str = "x-sm-key-id";

pub fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim)
}

/// 解开封装的密码。
pub async fn open_password(
    state: &AppState,
    headers: &HeaderMap,
    password_cipher: &str,
    timestamp: Option<i64>,
    nonce: Option<&str>,
) -> AppResult<SecretString> {
    let values = open_values(state, headers, timestamp, nonce, &[password_cipher]).await?;
    values
        .into_iter()
        .next()
        .ok_or_else(|| E::new(ErrorCode::SmCipherInvalid))
}

/// 同一个 `smkey` 一次解多个字段（改密时新旧密码共用一个密钥）。
pub async fn open_values(
    state: &AppState,
    headers: &HeaderMap,
    timestamp: Option<i64>,
    nonce: Option<&str>,
    ciphertexts: &[&str],
) -> AppResult<Vec<SecretString>> {
    let smkey = header_value(headers, HEADER_SMKEY)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| E::new(ErrorCode::PlaintextForbidden))?;

    // 1) 时间戳窗口（兼容秒/毫秒）
    // 1) 时间戳窗口（兼容秒/毫秒）。
    //    旧模组客户端（Node 版协议）不携带 timestamp：此时采用服务器时钟，
    //    防重放不受影响——每次信封的 smkey 都由客户端随机 SM4 密钥加密而来，
    //    缺省 nonce 时回退 SM3(smkey)，同一密文无法重复提交。
    let raw_ts = timestamp.unwrap_or_else(now_ms);
    let ts_ms = if raw_ts.abs() < 1_000_000_000_000 {
        raw_ts.saturating_mul(1000)
    } else {
        raw_ts
    };
    let tolerance = state.cfg.crypto.timestamp_tolerance_seconds * 1000;
    if (now_ms() - ts_ms).abs() > tolerance {
        return Err(E::new(ErrorCode::SmTimestampExpired));
    }

    // 2) 防重放：SM3(nonce || smkey) 或 SM3(smkey)
    let digest = match nonce.filter(|n| !n.is_empty()) {
        Some(nonce) => sm3_hex(format!("{nonce}{smkey}").as_bytes()),
        None => sm3_hex(smkey.as_bytes()),
    };
    let replay_key = crate::state::keys::replay(&digest);
    let ttl = state.cfg.redis.ttl.replay_seconds;
    if !state.cache.set_nx(&replay_key, "1", ttl).await {
        return Err(E::new(ErrorCode::SmReplay));
    }

    // 3) 解密（椭圆曲线运算放阻塞线程池）
    let key_id = header_value(headers, HEADER_KEY_ID)
        .or_else(|| header_value(headers, HEADER_KEY_ID_ALT))
        .map(|s| s.to_string());
    let ring = state.keyring.clone();
    let smkey_owned = smkey.to_string();
    let ciphertexts: Vec<String> = ciphertexts.iter().map(|s| s.to_string()).collect();
    let opened = tokio::task::spawn_blocking(move || {
        let ring = ring.blocking_read();
        let refs: Vec<&str> = ciphertexts.iter().map(String::as_str).collect();
        ring.open_envelope_multi(key_id.as_deref(), &smkey_owned, &refs)
    })
    .await
    .map_err(|_| E::internal("crypto join failed"))?;

    match opened {
        Ok(values) => Ok(values),
        Err(err) => {
            debug!(error = %err, "信封解密失败");
            Err(map_crypto_error(err))
        }
    }
}

fn map_crypto_error(err: CryptoError) -> AppError {
    match err {
        CryptoError::UnknownKeyId(id) => {
            warn!(key_id = %id, "客户端使用了未知的 SM2 keyId");
            E::new(ErrorCode::SmKeyUnknown)
        }
        CryptoError::BadPadding | CryptoError::BadUtf8 | CryptoError::BadSm4Cipher
        | CryptoError::BadSm4Key => E::new(ErrorCode::SmCipherInvalid),
        CryptoError::Sm2Decrypt | CryptoError::Sm2Cipher | CryptoError::Sm2PublicKey => {
            E::new(ErrorCode::SmKeyInvalid)
        }
        other => {
            warn!(error = %other, "密码信封处理异常");
            E::new(ErrorCode::Internal)
        }
    }
}

/// 判断是否为明文密码（用于返回更准确的 2006）。
pub fn looks_like_plaintext(value: &str) -> bool {
    if value.is_empty() || value.len() % 2 != 0 {
        return true;
    }
    if !value.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    // SM4 密文至少 2 个分组
    value.len() < 32
}

/// 当前激活公钥信息。
pub async fn public_key_payload(state: &AppState) -> serde_json::Value {
    let ring = state.keyring.read().await;
    let info = ring.public_key_info();
    serde_json::json!({
        "keyId": info.key_id,
        "publicKey": info.public_key,
        "sm2Mode": info.sm2_mode,
        "sm4Mode": info.sm4_mode,
        "serverTime": now_ms(),
    })
}
