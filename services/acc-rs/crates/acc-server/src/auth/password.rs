//! 密码策略与 argon2id 哈希。

use crate::error::{AppError, ErrorCode};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, SaltString};
use argon2::{Algorithm, Argon2, Params, PasswordVerifier, Version};
use rand::RngCore;
use std::sync::OnceLock;

/// 常见弱密码（小写比较）。表不必很大，重点是挡住最常用的几个。
pub const COMMON_PASSWORDS: &[&str] = &[
    "1234567890", "123456789", "12345678", "12345678910", "password", "password1",
    "password123", "password123!", "password1234", "passw0rd", "p@ssw0rd", "p@ssword123", "qwerty123",
    "qwertyuiop", "abc123456", "abcd1234", "abcd123456", "iloveyou", "admin123",
    "administrator", "welcome1", "welcome123", "letmein123", "monkey123", "dragon123",
    "sunshine1", "princess1", "football1", "baseball1", "superman1", "starwars1",
    "1qaz2wsx3edc", "1q2w3e4r5t", "qazwsxedc", "zaq12wsx", "asdfghjkl", "1qaz@wsx",
    "aa123456", "a123456789", "1111111111", "0000000000", "8888888888", "6666666666",
    "woaini1314", "5201314520", "13145201314", "a12345678", "abc123!@#", "test1234!",
];

#[derive(Debug, Clone, Copy)]
pub struct Argon2Params {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            memory_kib: 19_456,
            iterations: 2,
            parallelism: 1,
        }
    }
}

impl Argon2Params {
    fn build(&self) -> Argon2<'static> {
        let params = Params::new(
            self.memory_kib.max(8 * 1024),
            self.iterations.max(1),
            self.parallelism.max(1),
            Some(32),
        )
        .expect("argon2 参数合法");
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
    }
}

/// 密码强度校验（注册、找回、改密、管理员重置共用）。
pub fn password_issues(pw: &str, user_name: &str, email: &str) -> Vec<&'static str> {
    let mut issues = Vec::new();
    let count = pw.chars().count();
    if !(10..=64).contains(&count) {
        issues.push("length");
    }
    if !pw.chars().any(|c| c.is_ascii_alphabetic()) {
        issues.push("letter");
    }
    if !pw.chars().any(|c| c.is_ascii_digit()) {
        issues.push("digit");
    }
    if !pw.chars().any(|c| c.is_ascii_punctuation()) {
        issues.push("special");
    }

    let lower = pw.to_lowercase();
    let user_lower = user_name.to_lowercase();
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    if (!user_lower.is_empty() && lower.contains(&user_lower))
        || (!local.is_empty() && lower.contains(&local))
    {
        issues.push("personal");
    }
    if COMMON_PASSWORDS.contains(&lower.as_str()) {
        issues.push("common");
    }
    issues
}

pub fn ensure_strong(pw: &str, user_name: &str, email: &str) -> Result<(), AppError> {
    let issues = password_issues(pw, user_name, email);
    if issues.is_empty() {
        Ok(())
    } else {
        Err(AppError::new(ErrorCode::PasswordWeak)
            .with_payload(serde_json::json!({ "missing": issues })))
    }
}

/// 在阻塞线程池里算哈希，避免阻塞 async worker。
pub async fn hash_password(password: String, params: Argon2Params) -> Result<String, AppError> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        let argon = params.build();
        argon
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|err| {
                tracing::error!(error = %err, "argon2 哈希失败");
                AppError::internal("password hash failed")
            })
    })
    .await
    .map_err(|_| AppError::internal("password hash join failed"))?
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || match PasswordHash::new(&hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    })
    .await
    .unwrap_or(false)
}

/// 用户不存在时也跑一次校验，抹平“用户不存在/密码错误”的时序差异。
pub async fn dummy_verify(password: String) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY.get_or_init(|| {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(b"dummy-password-for-timing", &salt)
            .map(|h| h.to_string())
            .unwrap_or_default()
    });
    let _ = verify_password(password, hash.clone()).await;
}

/// 管理员重置用的 16 位随机密码：先保证三类字符齐全，再打乱。
pub fn random_password(len: usize) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    const DIGITS: &[u8] = b"23456789";
    const SPECIAL: &[u8] = b"!@#$%^&*_-+=?";
    let len = len.max(12);

    let mut out: Vec<u8> = Vec::with_capacity(len);
    out.push(LETTERS[rand_index(LETTERS.len())]);
    out.push(DIGITS[rand_index(DIGITS.len())]);
    out.push(SPECIAL[rand_index(SPECIAL.len())]);

    let all: Vec<u8> = LETTERS.iter().chain(DIGITS).chain(SPECIAL).copied().collect();
    while out.len() < len {
        out.push(all[rand_index(all.len())]);
    }

    // Fisher-Yates
    let mut rng_bytes = vec![0u8; out.len() * 4];
    rand::thread_rng().fill_bytes(&mut rng_bytes);
    for i in (1..out.len()).rev() {
        let j = (u32::from_be_bytes([
            rng_bytes[i * 4],
            rng_bytes[i * 4 + 1],
            rng_bytes[i * 4 + 2],
            rng_bytes[i * 4 + 3],
        ]) as usize)
            % (i + 1);
        out.swap(i, j);
    }
    String::from_utf8(out).unwrap_or_else(|_| "Aa1!Aa1!Aa1!Aa1!".into())
}

fn rand_index(len: usize) -> usize {
    let mut byte = [0u8; 4];
    OsRng.fill_bytes(&mut byte);
    (u32::from_be_bytes(byte) as usize) % len
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_policy_matrix() {
        assert!(password_issues("Abcdef123!", "aims", "player@example.com").is_empty());
        assert!(password_issues("Ab1!", "aims", "player@example.com").contains(&"length"));
        assert!(password_issues(&"a".repeat(65), "aims", "player@example.com").contains(&"length"));
        assert!(password_issues("1234567890!", "aims", "player@example.com").contains(&"letter"));
        assert!(password_issues("Abcdefghij!", "aims", "player@example.com").contains(&"digit"));
        assert!(password_issues("Abcdef1234", "aims", "player@example.com").contains(&"special"));
        assert!(password_issues("密码密码密码密码密码1!", "aims", "a@b.com").contains(&"letter"));
        assert!(password_issues("Aims1234!xyz", "Aims", "player@example.com").contains(&"personal"));
        assert!(password_issues("Localtest1!", "someone", "localtest@b.com").contains(&"personal"));
        assert!(password_issues("Password123!", "aims", "player@example.com").contains(&"common"));
    }

    #[test]
    fn random_password_meets_policy() {
        for _ in 0..50 {
            let pw = random_password(16);
            assert_eq!(pw.chars().count(), 16);
            assert!(password_issues(&pw, "smokeuser", "player@example.com").is_empty(), "弱密码: {pw}");
        }
    }
}
