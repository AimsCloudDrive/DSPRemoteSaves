//! 国密密码信封（SM2 + SM4），与 `sm-crypto` / BouncyCastle 逐字节互通。
//!
//! 约定（与参考实现 `get-login-token.mjs` 完全一致）：
//! - SM2 曲线 `sm2p256v1`，密文布局 **C1C3C2**，C1 为未压缩点 `04||X||Y`
//! - SM2 加密的明文是 **SM4 密钥的 32 位小写 hex 字符串（32 字节 ASCII）**，不是 16 字节裸密钥
//! - SM4 使用 **ECB + PKCS#7**，密钥为上述 hex 字符串解码后的 16 字节
//! - 所有密文以小写 hex 传输
//!
//! 典型流程：
//! ```text
//! 客户端：sm4Key = random 16B -> 32 hex
//!         smkey  = SM2(sm4Key 字符串)          -> 放请求头
//!         passwd = SM4-ECB-PKCS7(明文, sm4Key) -> 放 body
//! 服务端：open_envelope(keyId, smkey, passwd) -> SecretString
//! ```

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit as AeadKeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key as ChaChaKey, Nonce as ChaChaNonce};
use rand::rngs::OsRng;
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use sm2::elliptic_curve::common::getrandom::SysRng;
use sm2::elliptic_curve::sec1::ToSec1Point;
use sm2::elliptic_curve::Generate as _;
use sm2::pke::{DecryptingKey, EncryptingKey, Mode};
use sm2::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use sm2::{PublicKey, SecretKey};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use zeroize::Zeroizing;

pub const SM2_CURVE: &str = "sm2p256v1";
pub const SM2_MODE: &str = "C1C3C2";
pub const SM4_MODE: &str = "ECB/PKCS7";

const KEYRING_FILE: &str = "keyring.json";
const ENC_PEM_BEGIN: &str = "-----BEGIN ENCRYPTED SM2 PRIVATE KEY-----";
const ENC_PEM_END: &str = "-----END ENCRYPTED SM2 PRIVATE KEY-----";
const ENC_MAGIC: &[u8; 4] = b"DSPK";
const ENC_VERSION: u8 = 1;

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("SM2 key id 未知: {0}")]
    UnknownKeyId(String),
    #[error("SM2 私钥缺失")]
    MissingSecretKey,
    #[error("SM2 解密失败")]
    Sm2Decrypt,
    #[error("SM2 公钥非法")]
    Sm2PublicKey,
    #[error("SM2 密文格式非法")]
    Sm2Cipher,
    #[error("SM4 密钥非法（期望 32 位 hex）")]
    BadSm4Key,
    #[error("SM4 密文非法")]
    BadSm4Cipher,
    #[error("PKCS#7 填充非法")]
    BadPadding,
    #[error("明文不是合法 UTF-8")]
    BadUtf8,
    #[error("私钥口令错误或文件损坏")]
    BadPassphrase,
    #[error("文件 IO 失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("序列化失败: {0}")]
    Json(#[from] serde_json::Error),
    #[error("密钥文件格式非法")]
    KeyFileFormat,
}

pub type Result<T> = std::result::Result<T, CryptoError>;

// ---------------------------------------------------------------------------
// 基础算法
// ---------------------------------------------------------------------------

/// 生成 16 字节随机 SM4 密钥，返回 32 位小写 hex 字符串。
pub fn random_sm4_key_hex() -> String {
    let mut key = [0u8; 16];
    OsRng.fill_bytes(&mut key);
    hex::encode(key)
}

/// SM4-ECB/PKCS#7 加密，返回小写 hex。
pub fn sm4_ecb_encrypt_hex(key_hex: &str, plaintext: &[u8]) -> Result<String> {
    use sm4::cipher::{BlockEncrypt, KeyInit};
    let key = decode_sm4_key(key_hex)?;
    let cipher = sm4::Sm4::new_from_slice(&key).map_err(|_| CryptoError::BadSm4Key)?;

    // PKCS#7：块大小 16
    let pad = 16 - (plaintext.len() % 16);
    let mut buf = Vec::with_capacity(plaintext.len() + pad);
    buf.extend_from_slice(plaintext);
    buf.extend(std::iter::repeat(pad as u8).take(pad));

    for chunk in buf.chunks_mut(16) {
        let block = sm4::cipher::Block::<sm4::Sm4>::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }
    Ok(hex::encode(buf))
}

/// SM4-ECB/PKCS#7 解密；先做填充校验，再交给调用方做 UTF-8 校验。
pub fn sm4_ecb_decrypt(key_hex: &str, cipher_bytes: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    use sm4::cipher::{BlockDecrypt, KeyInit};
    if cipher_bytes.is_empty() || cipher_bytes.len() % 16 != 0 {
        return Err(CryptoError::BadSm4Cipher);
    }
    let key = decode_sm4_key(key_hex)?;
    let cipher = sm4::Sm4::new_from_slice(&key).map_err(|_| CryptoError::BadSm4Key)?;

    let mut buf = Zeroizing::new(cipher_bytes.to_vec());
    for chunk in buf.chunks_mut(16) {
        let block = sm4::cipher::Block::<sm4::Sm4>::from_mut_slice(chunk);
        cipher.decrypt_block(block);
    }
    let pad = *buf.last().ok_or(CryptoError::BadPadding)? as usize;
    if pad == 0 || pad > 16 || pad > buf.len() {
        return Err(CryptoError::BadPadding);
    }
    if !buf[buf.len() - pad..].iter().all(|b| *b as usize == pad) {
        return Err(CryptoError::BadPadding);
    }
    let len = buf.len() - pad;
    let mut out = Zeroizing::new(Vec::with_capacity(len));
    out.extend_from_slice(&buf[..len]);
    Ok(out)
}

/// 解析 32 位 hex 的 SM4 密钥。
fn decode_sm4_key(key_hex: &str) -> Result<[u8; 16]> {
    let raw = hex::decode(key_hex.trim()).map_err(|_| CryptoError::BadSm4Key)?;
    if raw.len() != 16 {
        return Err(CryptoError::BadSm4Key);
    }
    let mut key = [0u8; 16];
    key.copy_from_slice(&raw);
    Ok(key)
}

/// SM2 加密（C1C3C2，未压缩点）。
pub fn sm2_encrypt_hex(public_hex: &str, plaintext: &[u8]) -> Result<String> {
    let raw = hex::decode(public_hex.trim()).map_err(|_| CryptoError::Sm2PublicKey)?;
    let pk = PublicKey::from_sec1_bytes(&raw).map_err(|_| CryptoError::Sm2PublicKey)?;
    let ek = EncryptingKey::new_with_mode(pk, Mode::C1C3C2);
    let ct = ek.encrypt(&mut SysRng, plaintext).map_err(|_| CryptoError::Sm2Cipher)?;
    Ok(hex::encode(ct))
}

/// 用私钥直接解 SM2 密文（测试与 CLI 使用）。
///
/// **兼容两种线上格式**：
/// - `sm-crypto` 的 `doEncrypt` 会去掉 C1 的 `04` 前缀（`X||Y||C3||C2`）；
/// - BouncyCastle / RustCrypto 输出未压缩点 `04||X||Y||C3||C2`。
///
/// 因此先按原样解，失败后再补/去 `04` 重试。SM2 解密失败不会泄露有效信息，
/// 重试成本可接受（每个请求最多一次椭圆曲线运算）。
pub fn sm2_decrypt_with(secret: &SecretKey, cipher_hex: &str) -> Result<Vec<u8>> {
    let raw = hex::decode(cipher_hex.trim()).map_err(|_| CryptoError::Sm2Cipher)?;
    if raw.len() < 97 {
        return Err(CryptoError::Sm2Cipher);
    }
    let dk = DecryptingKey::new_with_mode(secret.to_nonzero_scalar(), Mode::C1C3C2);

    // 1) 原样（BouncyCastle / RustCrypto：04||X||Y||C3||C2）
    if let Ok(plain) = dk.decrypt(&raw) {
        return Ok(plain);
    }
    // 2) 补 04（sm-crypto：X||Y||C3||C2）。
    //    注意：无前缀时 X 的首字节也可能恰好是 0x04，不能只凭首字节判断。
    let mut with_prefix = Vec::with_capacity(raw.len() + 1);
    with_prefix.push(0x04);
    with_prefix.extend_from_slice(&raw);
    if let Ok(plain) = dk.decrypt(&with_prefix) {
        return Ok(plain);
    }
    // 3) 去掉 04（兼容重复前缀的实现）
    if raw.first() == Some(&0x04) {
        if let Ok(plain) = dk.decrypt(&raw[1..]) {
            return Ok(plain);
        }
    }
    Err(CryptoError::Sm2Decrypt)
}

/// SM3 摘要，小写 hex。
pub fn sm3_hex(data: &[u8]) -> String {
    use sm3::Digest as _;
    let mut hasher = sm3::Sm3::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// 由公钥 hex 推导 keyId：`hex(sha256(pub_hex_ascii)[..8])`，16 位小写 hex。
pub fn derive_key_id(public_hex: &str) -> String {
    let digest = Sha256::digest(public_hex.as_bytes());
    hex::encode(&digest[..8])
}

/// 一次性封装：返回 `(smkey_hex, 每个 value 的 SM4 密文)`，供测试/前端对照使用。
pub fn seal(public_hex: &str, values: &[&str]) -> Result<(String, Vec<String>)> {
    let sm4_key = random_sm4_key_hex();
    let smkey = sm2_encrypt_hex(public_hex, sm4_key.as_bytes())?;
    let mut sealed = Vec::with_capacity(values.len());
    for v in values {
        sealed.push(sm4_ecb_encrypt_hex(&sm4_key, v.as_bytes())?);
    }
    Ok((smkey, sealed))
}


/// 用指定私钥解开密码信封（KeyRing 之外的独立入口，便于测试与 CLI）。
pub fn open_envelope_with(
    secret: &SecretKey,
    smkey_hex: &str,
    password_hex: &str,
) -> Result<SecretString> {
    let mut values = open_envelope_multi(secret, smkey_hex, &[password_hex])?;
    Ok(values.remove(0))
}

/// 同一个 `smkey` 解开多个密文字段（改密时旧密码与新密码共用一个 SM4 密钥）。
///
/// SM2 只做一次，避免重复椭圆曲线运算。
pub fn open_envelope_multi(
    secret: &SecretKey,
    smkey_hex: &str,
    ciphertexts: &[&str],
) -> Result<Vec<SecretString>> {
    // 1) SM2 解出 SM4 密钥的 32 字节 hex 文本
    let key_hex_bytes = sm2_decrypt_with(secret, smkey_hex)?;
    let key_hex = std::str::from_utf8(&key_hex_bytes)
        .map_err(|_| CryptoError::BadSm4Key)?
        .trim()
        .to_string();
    // 与参考实现对齐：必须先是 32 位 hex 文本，再解码成 16 字节
    decode_sm4_key(&key_hex)?;

    // 2) 逐个 SM4 解密
    let mut out = Vec::with_capacity(ciphertexts.len());
    for cipher_hex in ciphertexts {
        let cipher_bytes =
            hex::decode(cipher_hex.trim()).map_err(|_| CryptoError::BadSm4Cipher)?;
        let plain = sm4_ecb_decrypt(&key_hex, &cipher_bytes)?;
        let text = std::str::from_utf8(&plain).map_err(|_| CryptoError::BadUtf8)?;
        out.push(SecretString::from(text.to_string()));
    }
    Ok(out)
}

/// 从 32 字节 hex 私钥构造 `SecretKey`（测试/导入口令使用）。
pub fn secret_from_hex(secret_hex: &str) -> Result<SecretKey> {
    let raw = hex::decode(secret_hex.trim()).map_err(|_| CryptoError::KeyFileFormat)?;
    SecretKey::from_slice(&raw).map_err(|_| CryptoError::KeyFileFormat)
}

/// 私钥对应的未压缩公钥 hex（`04||X||Y`）。
pub fn public_hex_of(secret: &SecretKey) -> String {
    hex::encode(secret.public_key().to_sec1_point(false).as_bytes())
}

// ---------------------------------------------------------------------------
// 密钥对
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMeta {
    #[serde(rename = "keyId")]
    pub key_id: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    #[serde(rename = "secretFile")]
    pub secret_file: String,
    /// Unix 毫秒
    #[serde(rename = "createdAt")]
    pub created_at: i64,
    #[serde(default)]
    #[serde(rename = "encrypted")]
    pub encrypted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct KeyRingFile {
    active: String,
    version: u32,
    keys: Vec<KeyMeta>,
}

pub struct KeyEntry {
    pub meta: KeyMeta,
    secret: SecretKey,
}

pub struct KeyRing {
    dir: PathBuf,
    passphrase: Option<String>,
    entries: Vec<KeyEntry>,
    active: String,
}

impl std::fmt::Debug for KeyRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只暴露 keyId，绝不打印私钥
        f.debug_struct("KeyRing")
            .field("dir", &self.dir)
            .field("active", &self.active)
            .field("keys", &self.entries.iter().map(|e| &e.meta.key_id).collect::<Vec<_>>())
            .finish()
    }
}

impl KeyRing {
    /// 加载密钥环；目录为空时生成首个密钥对。
    ///
    /// `passphrase` 为 `Some` 时，私钥落盘前用 Argon2id + ChaCha20-Poly1305 加密。
    pub fn load_or_create(dir: impl AsRef<Path>, passphrase: Option<&str>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        restrict_dir(&dir)?;

        let passphrase = passphrase.filter(|p| !p.is_empty()).map(|s| s.to_string());
        let keyring_path = dir.join(KEYRING_FILE);

        let mut ring = if keyring_path.exists() {
            let text = fs::read_to_string(&keyring_path)?;
            let file: KeyRingFile = serde_json::from_str(&text)?;
            let mut entries = Vec::with_capacity(file.keys.len());
            for meta in file.keys {
                match load_secret(&dir, &meta, passphrase.as_deref()) {
                    Ok(secret) => entries.push(KeyEntry { meta, secret }),
                    Err(err) => {
                        // 单把钥匙损坏不应让整个服务起不来，但要留下明确告警
                        warn!(key_id = %meta.key_id, error = %err, "私钥加载失败，已跳过该密钥");
                    }
                }
            }
            if entries.is_empty() {
                return Err(CryptoError::MissingSecretKey);
            }
            KeyRing {
                dir,
                passphrase,
                entries,
                active: file.active,
            }
        } else {
            let mut ring = KeyRing {
                dir,
                passphrase,
                entries: Vec::new(),
                active: String::new(),
            };
            let meta = ring.generate_entry()?;
            ring.active = meta.key_id;
            ring.persist()?;
            ring
        };

        if !ring.entries.iter().any(|e| e.meta.key_id == ring.active) {
            // keyring.json 指向的密钥丢失时，退回到最新一把可用密钥
            let fallback = ring
                .entries
                .iter()
                .max_by_key(|e| e.meta.created_at)
                .map(|e| e.meta.key_id.clone())
                .ok_or(CryptoError::MissingSecretKey)?;
            warn!(from = %ring.active, to = %fallback, "active keyId 不可用，已自动切换");
            ring.active = fallback;
            ring.persist()?;
        }

        info!(
            key_id = %ring.active,
            keys = ring.entries.len(),
            encrypted = ring.passphrase.is_some(),
            "SM2 密钥环已加载"
        );
        Ok(ring)
    }

    pub fn active_key_id(&self) -> &str {
        &self.active
    }

    pub fn active_public_key(&self) -> &str {
        self.entries
            .iter()
            .find(|e| e.meta.key_id == self.active)
            .map(|e| e.meta.public_key.as_str())
            .unwrap_or_default()
    }

    pub fn key_ids(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.meta.key_id.as_str()).collect()
    }

    /// 生成新密钥并置为 active；旧密钥保留，由 [`KeyRing::prune`] 按宽限期清理。
    pub fn rotate(&mut self) -> Result<String> {
        let meta = self.generate_entry()?;
        let id = meta.key_id.clone();
        self.active = id.clone();
        self.persist()?;
        info!(key_id = %id, "SM2 密钥已轮换");
        Ok(id)
    }

    /// 删除非 active 且超过宽限期的密钥文件。
    pub fn prune(&mut self, grace_hours: u64) -> Result<Vec<String>> {
        let cutoff = now_ms() - (grace_hours as i64) * 3_600_000;
        let mut removed = Vec::new();
        let mut kept = Vec::with_capacity(self.entries.len());
        for entry in self.entries.drain(..) {
            if entry.meta.key_id != self.active && entry.meta.created_at < cutoff {
                let path = self.dir.join(&entry.meta.secret_file);
                if let Err(err) = fs::remove_file(&path) {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        warn!(path = %path.display(), error = %err, "删除过期私钥失败");
                    }
                }
                removed.push(entry.meta.key_id);
            } else {
                kept.push(entry);
            }
        }
        self.entries = kept;
        if !removed.is_empty() {
            self.persist()?;
        }
        Ok(removed)
    }

    /// 解开封装的密码：`smkey` 头 + `password` 字段。
    pub fn open_envelope(
        &self,
        key_id: Option<&str>,
        smkey_hex: &str,
        password_hex: &str,
    ) -> Result<SecretString> {
        let entry = self.get(key_id)?;
        open_envelope_with(&entry.secret, smkey_hex, password_hex)
    }

    /// 同一个 `smkey` 解多个字段。
    pub fn open_envelope_multi(
        &self,
        key_id: Option<&str>,
        smkey_hex: &str,
        ciphertexts: &[&str],
    ) -> Result<Vec<SecretString>> {
        let entry = self.get(key_id)?;
        open_envelope_multi(&entry.secret, smkey_hex, ciphertexts)
    }

    fn get(&self, key_id: Option<&str>) -> Result<&KeyEntry> {
        match key_id.map(|s| s.trim()).filter(|s| !s.is_empty()) {
            None => self
                .entries
                .iter()
                .find(|e| e.meta.key_id == self.active)
                .ok_or_else(|| CryptoError::UnknownKeyId(self.active.clone())),
            Some(id) => self
                .entries
                .iter()
                .find(|e| e.meta.key_id.eq_ignore_ascii_case(id))
                .ok_or_else(|| CryptoError::UnknownKeyId(id.to_string())),
        }
    }

    fn generate_entry(&mut self) -> Result<KeyMeta> {
        let secret = SecretKey::generate();
        let public_hex = hex::encode(secret.public_key().to_sec1_point(false).as_bytes());
        let key_id = derive_key_id(&public_hex);
        let encrypted = self.passphrase.is_some();
        let secret_file = format!("sm2-{key_id}.priv.pem");
        let path = self.dir.join(&secret_file);

        write_secret(&path, &secret, self.passphrase.as_deref())?;

        // 公钥单独落盘，便于运维核对
        let pub_path = self.dir.join(format!("sm2-{key_id}.pub.hex"));
        fs::write(&pub_path, public_hex.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&pub_path, fs::Permissions::from_mode(0o644));
        }

        let meta = KeyMeta {
            key_id,
            public_key: public_hex,
            secret_file,
            created_at: now_ms(),
            encrypted,
        };
        self.entries.push(KeyEntry {
            meta: meta.clone(),
            secret,
        });
        Ok(meta)
    }

    fn persist(&self) -> Result<()> {
        let file = KeyRingFile {
            active: self.active.clone(),
            version: 1,
            keys: self.entries.iter().map(|e| e.meta.clone()).collect(),
        };
        let path = self.dir.join(KEYRING_FILE);
        let tmp = self.dir.join(format!("{KEYRING_FILE}.tmp"));
        fs::write(&tmp, serde_json::to_vec_pretty(&file)?)?;
        fs::rename(&tmp, &path)?;
        restrict_file(&path)?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn load_secret(dir: &Path, meta: &KeyMeta, passphrase: Option<&str>) -> Result<SecretKey> {
    let text = fs::read_to_string(dir.join(&meta.secret_file))?;
    if text.trim_start().starts_with(ENC_PEM_BEGIN) {
        let pass = passphrase.ok_or(CryptoError::BadPassphrase)?;
        let plain = decrypt_private_key(&text, pass)?;
        // 加密的是 PEM 文本（不是 DER），因此仍按 PEM 解析
        let pem = std::str::from_utf8(&plain).map_err(|_| CryptoError::KeyFileFormat)?;
        SecretKey::from_pkcs8_pem(pem).map_err(|_| CryptoError::KeyFileFormat)
    } else {
        SecretKey::from_pkcs8_pem(&text).map_err(|_| CryptoError::KeyFileFormat)
    }
}

fn write_secret(path: &Path, secret: &SecretKey, passphrase: Option<&str>) -> Result<()> {
    use sm2::pkcs8::LineEnding;
    let pem = secret
        .to_pkcs8_pem(LineEnding::LF)
        .map_err(|_| CryptoError::KeyFileFormat)?;

    let content = match passphrase {
        Some(pass) => encrypt_private_key(pem.as_bytes(), pass)?,
        None => pem.to_string(),
    };
    fs::write(path, content)?;
    restrict_file(path)?;
    Ok(())
}

/// 私钥容器：magic|ver|m_cost|t_cost|p_cost|salt|nonce|ct，整体 base64。
fn encrypt_private_key(plain: &[u8], passphrase: &str) -> Result<String> {
    let (m_cost, t_cost, p_cost) = (19_456u32, 2u32, 1u32);
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce);

    let params = Params::new(m_cost, t_cost, p_cost, Some(32)).map_err(|_| CryptoError::BadPassphrase)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), &salt, key.as_mut())
        .map_err(|_| CryptoError::BadPassphrase)?;

    let cipher = ChaCha20Poly1305::new(ChaChaKey::from_slice(key.as_ref()));
    let ct = cipher
        .encrypt(ChaChaNonce::from_slice(&nonce), plain)
        .map_err(|_| CryptoError::BadPassphrase)?;

    let mut out = Vec::with_capacity(25 + ct.len());
    out.extend_from_slice(ENC_MAGIC);
    out.push(ENC_VERSION);
    out.extend_from_slice(&m_cost.to_le_bytes());
    out.extend_from_slice(&t_cost.to_le_bytes());
    out.extend_from_slice(&p_cost.to_le_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);

    let b64 = base64::engine::general_purpose::STANDARD.encode(out);
    let mut text = String::new();
    text.push_str(ENC_PEM_BEGIN);
    text.push('\n');
    for line in b64.as_bytes().chunks(64) {
        text.push_str(std::str::from_utf8(line).map_err(|_| CryptoError::KeyFileFormat)?);
        text.push('\n');
    }
    text.push_str(ENC_PEM_END);
    text.push('\n');
    Ok(text)
}

fn decrypt_private_key(text: &str, passphrase: &str) -> Result<Zeroizing<Vec<u8>>> {
    let body: String = text
        .lines()
        .filter(|l| !l.starts_with("-----") && !l.trim().is_empty())
        .collect();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .map_err(|_| CryptoError::KeyFileFormat)?;
    if raw.len() < 25 + 16 || &raw[..4] != ENC_MAGIC {
        return Err(CryptoError::KeyFileFormat);
    }
    let mut offset = 5;
    let read_u32 = |off: &mut usize| -> u32 {
        let v = u32::from_le_bytes([raw[*off], raw[*off + 1], raw[*off + 2], raw[*off + 3]]);
        *off += 4;
        v
    };
    let m_cost = read_u32(&mut offset);
    let t_cost = read_u32(&mut offset);
    let p_cost = read_u32(&mut offset);
    let salt = &raw[offset..offset + 16];
    offset += 16;
    let nonce = &raw[offset..offset + 12];
    offset += 12;
    let ct = &raw[offset..];

    let params = Params::new(m_cost, t_cost, p_cost, Some(32)).map_err(|_| CryptoError::BadPassphrase)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, key.as_mut())
        .map_err(|_| CryptoError::BadPassphrase)?;

    let cipher = ChaCha20Poly1305::new(ChaChaKey::from_slice(key.as_ref()));
    let plain = cipher
        .decrypt(ChaChaNonce::from_slice(nonce), ct)
        .map_err(|_| CryptoError::BadPassphrase)?;
    Ok(Zeroizing::new(plain))
}

fn restrict_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn restrict_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// 便捷方法：把开放给客户端的公钥信息打包。
#[derive(Debug, Clone, Serialize)]
pub struct PublicKeyInfo {
    #[serde(rename = "keyId")]
    pub key_id: String,
    #[serde(rename = "publicKey")]
    pub public_key: String,
    #[serde(rename = "sm2Mode")]
    pub sm2_mode: &'static str,
    #[serde(rename = "sm4Mode")]
    pub sm4_mode: &'static str,
}

impl KeyRing {
    pub fn public_key_info(&self) -> PublicKeyInfo {
        PublicKeyInfo {
            key_id: self.active.clone(),
            public_key: self.active_public_key().to_string(),
            sm2_mode: SM2_MODE,
            sm4_mode: SM4_MODE,
        }
    }
}

/// 只暴露给测试的辅助：把 SecretString 取出来比较。
pub fn reveal(secret: &SecretString) -> &str {
    secret.expose_secret()
}
