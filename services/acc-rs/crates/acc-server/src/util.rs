//! 通用工具：时间、随机、摘要、客户端 IP、Range 解析、用户名/邮箱校验。

use axum::http::{HeaderMap, Uri};
use base64::Engine as _;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::net::{IpAddr, SocketAddr};

// ---------------------------------------------------------------------------
// 请求追踪
// ---------------------------------------------------------------------------

tokio::task_local! {
    static TRACE_ID: String;
}

pub fn current_trace_id() -> String {
    TRACE_ID
        .try_with(|v| v.clone())
        .unwrap_or_else(|_| "untracked".to_string())
}

pub async fn with_trace<F: std::future::Future>(id: String, fut: F) -> F::Output {
    TRACE_ID.scope(id, fut).await
}

/// 生成 26 位 ULID 风格 traceId（时间有序，便于排查）。
pub fn new_trace_id() -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let now = now_ms() as u64;
    let mut out = String::with_capacity(26);
    for i in (0..10).rev() {
        out.push(ALPHABET[((now >> (i * 5)) & 31) as usize] as char);
    }
    let mut random = [0u8; 10];
    OsRng.fill_bytes(&mut random);
    for byte in random {
        out.push(ALPHABET[(byte & 31) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// 时间
// ---------------------------------------------------------------------------

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn now_dt() -> mongodb::bson::DateTime {
    mongodb::bson::DateTime::from_millis(now_ms())
}

pub fn dt_ms(dt: mongodb::bson::DateTime) -> i64 {
    dt.timestamp_millis()
}

// ---------------------------------------------------------------------------
// 随机与摘要
// ---------------------------------------------------------------------------

pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf
}

/// 32 字节随机数的 base64url（无填充），用于 refresh token / uploadToken。
pub fn random_token() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes(32))
}

pub fn random_alnum(len: usize) -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

pub fn random_digits(len: usize) -> String {
    let mut buf = vec![0u8; len];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| (b'0' + (b % 10)) as char).collect()
}

pub fn sm3_hex(data: &[u8]) -> String {
    use sm3::Digest as _;
    let mut hasher = sm3::Sm3::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC 接受任意长度密钥");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    if a.len() != b.len() {
        return false;
    }
    a.ct_eq(b).into()
}

// ---------------------------------------------------------------------------
// 客户端 IP
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct Cidr {
    base: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Self> {
        let (addr, prefix) = s.split_once('/')?;
        let base: IpAddr = addr.parse().ok()?;
        let prefix: u8 = prefix.parse().ok()?;
        let max = if base.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return None;
        }
        Some(Self { base, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.base, ip) {
            (IpAddr::V4(base), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix as u32)
                };
                (u32::from(base) & mask) == (u32::from(ip) & mask)
            }
            (IpAddr::V6(base), IpAddr::V6(ip)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix as u32)
                };
                (u128::from(base) & mask) == (u128::from(ip) & mask)
            }
            _ => false,
        }
    }
}

pub fn parse_cidrs(list: &[String]) -> Vec<Cidr> {
    list.iter().filter_map(|s| Cidr::parse(s)).collect()
}

/// 只信任来自 `trusted` 网段的 `X-Forwarded-For`，否则使用 socket IP。
pub fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>, trusted: &[Cidr]) -> IpAddr {
    let peer_ip = peer.unwrap_or(IpAddr::from([127, 0, 0, 1]));
    let is_trusted = trusted.iter().any(|c| c.contains(peer_ip));
    if is_trusted {
        if let Some(value) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            if let Some(first) = value.split(',').next() {
                if let Ok(ip) = first.trim().parse::<IpAddr>() {
                    return ip;
                }
            }
        }
        if let Some(value) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()) {
            if let Ok(ip) = value.trim().parse::<IpAddr>() {
                return ip;
            }
        }
    }
    peer_ip
}

pub fn peer_ip(parts_extensions: &axum::http::Extensions) -> Option<IpAddr> {
    parts_extensions
        .get::<axum::extract::ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip())
}

/// 网段键：IPv4 取 /24，IPv6 取 /64。
pub fn subnet_key(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0/24", o[0], o[1], o[2])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    }
}

pub fn mask_email(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let head: String = local.chars().take(1).collect();
            format!("{head}***@{domain}")
        }
        None => "***".to_string(),
    }
}

pub fn mask_ip(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.0", o[0], o[1], o[2])
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            format!("{:x}:{:x}::", s[0], s[1])
        }
    }
}

// ---------------------------------------------------------------------------
// 校验
// ---------------------------------------------------------------------------

/// 用户名：3~32 字符；字母/数字/下划线/中文；禁止 @、空格、路径字符。
pub fn validate_user_name(name: &str) -> bool {
    let count = name.chars().count();
    if !(3..=32).contains(&count) {
        return false;
    }
    name.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || c == '_'
            || ('\u{4e00}'..='\u{9fff}').contains(&c)
    })
}

/// 邮箱：单 @、无空白、域名含点；统一转小写存储。
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_ascii_lowercase();
    if email.len() > 254 || email.contains(char::is_whitespace) {
        return None;
    }
    let (local, domain) = email.split_once('@')?;
    if local.is_empty() || local.len() > 64 || domain.is_empty() {
        return None;
    }
    if !domain.contains('.') || domain.starts_with('.') || domain.ends_with('.') {
        return None;
    }
    if !email
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~.@".contains(c))
    {
        return None;
    }
    Some(email)
}

pub fn email_domain(email: &str) -> &str {
    email.split_once('@').map(|(_, d)| d).unwrap_or("")
}

// ---------------------------------------------------------------------------
// HTTP 辅助
// ---------------------------------------------------------------------------

/// RFC 5987 文件名编码：用于 `Content-Disposition: filename*=UTF-8''...`。
pub fn percent_encode(input: &str) -> String {
    percent_encoding::utf8_percent_encode(input, percent_encoding::NON_ALPHANUMERIC).to_string()
}

pub fn url_decode(input: &str) -> String {
    percent_encoding::percent_decode_str(input)
        .decode_utf8_lossy()
        .to_string()
}

/// 解析单段 `Range: bytes=start-end`，返回闭区间。
pub fn parse_range(header: &str, size: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start_s, end_s) = spec.split_once('-')?;
    if start_s.is_empty() {
        // 后缀区间：bytes=-500
        let suffix: u64 = end_s.trim().parse().ok()?;
        if suffix == 0 || size == 0 {
            return None;
        }
        let start = size.saturating_sub(suffix);
        return Some((start, size - 1));
    }
    let start: u64 = start_s.trim().parse().ok()?;
    let end: u64 = if end_s.trim().is_empty() {
        size.saturating_sub(1)
    } else {
        end_s.trim().parse().ok()?
    };
    if start >= size || start > end {
        return None;
    }
    Some((start, end.min(size.saturating_sub(1))))
}

pub fn base_path_join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

pub fn raw_query(uri: &Uri) -> &str {
    uri.query().unwrap_or("")
}

pub fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub fn format_datetime(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms) {
        chrono::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_matching() {
        let c = Cidr::parse("10.0.0.0/8").unwrap();
        assert!(c.contains("10.1.2.3".parse().unwrap()));
        assert!(!c.contains("11.1.2.3".parse().unwrap()));

        let c = Cidr::parse("2001:db8::/32").unwrap();
        assert!(c.contains("2001:db8::1".parse().unwrap()));
        assert!(!c.contains("2001:db9::1".parse().unwrap()));
    }

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=900-", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=5-4", 1000), None);
    }

    #[test]
    fn user_name_rules() {
        assert!(validate_user_name("Aims_01"));
        assert!(validate_user_name("存档用户"));
        assert!(!validate_user_name("ab"));
        assert!(!validate_user_name("a@b"));
        assert!(!validate_user_name("a b"));
        assert!(!validate_user_name("a/b"));
    }

    #[test]
    fn email_rules() {
        assert_eq!(normalize_email("A+B@Example.COM").as_deref(), Some("a+b@example.com"));
        assert!(normalize_email("no-at-sign").is_none());
        assert!(normalize_email("a@b").is_none());
        assert!(normalize_email("a b@c.com").is_none());
    }
}
