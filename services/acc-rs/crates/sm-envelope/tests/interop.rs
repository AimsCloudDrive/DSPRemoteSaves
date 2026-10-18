//! 国密互通测试：
//! 1. 用 `sm-crypto` 生成的 1000+ 组向量逐条解密；
//! 2. Rust 反向封装，产出 `rust-vectors.json` 供 Node 侧校验；
//! 3. 异常向量（截断、C1C2C3、篡改 C3、裸密钥）必须报错；
//! 4. KeyRing 落盘、口令加密、轮换与宽限期清理。

use sm_envelope::*;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Vectors {
    private_key: String,
    #[allow(dead_code)]
    public_key: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    plain: String,
    #[serde(rename = "sm4Key")]
    sm4_key: String,
    smkey: String,
    password: String,
}

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

#[test]
fn js_to_rust_vectors_all_pass() {
    let path = fixtures_dir().join("sm-vectors.json");
    let vectors: Vectors =
        serde_json::from_str(&fs::read_to_string(&path).expect("缺少 sm-vectors.json")).unwrap();
    let secret = secret_from_hex(&vectors.private_key).unwrap();

    assert!(vectors.cases.len() >= 1000, "向量数量不足");
    for (idx, case) in vectors.cases.iter().enumerate() {
        // sm-crypto 输出 X||Y||C3||C2（无 04），长度为 128+64+32 字节的 hex
        assert_eq!(case.smkey.len(), (64 + 32 + 32) * 2, "case {idx}: 密文长度异常");
        let plain = open_envelope_with(&secret, &case.smkey, &case.password)
            .unwrap_or_else(|e| panic!("case {idx} 解密失败: {e}"));
        assert_eq!(plain.expose_secret(), case.plain, "case {idx} 明文不一致");
    }
}

#[test]
fn rust_to_js_vectors_written() {
    // 生成一个固定私钥的向量文件，交给 verify-rust.mjs 反向校验
    let secret = secret_from_hex(
        "3945208f7b2144b13f36e38ac6d39f95889393692860b51a42fb81ef4df7c5b8",
    )
    .unwrap();
    let public = public_hex_of(&secret);

    let plains = [
        "Abcdef123!",
        "密码Abc123!@#",
        "emoji🔐🚀Pass123!",
        "a",
        &"x".repeat(64),
        &"Long-Password-2026!".repeat(3),
    ];

    let mut cases = Vec::new();
    for plain in plains {
        let sm4_key = random_sm4_key_hex();
        let smkey = sm2_encrypt_hex(&public, sm4_key.as_bytes()).unwrap();
        let password = sm4_ecb_encrypt_hex(&sm4_key, plain.as_bytes()).unwrap();
        // 自校验
        let opened = open_envelope_with(&secret, &smkey, &password).unwrap();
        assert_eq!(opened.expose_secret(), plain);

        cases.push(RustCase {
            plain: plain.to_string(),
            sm4_key: sm4_key.clone(),
            smkey,
            password,
        });
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Out<'a> {
        private_key: &'a str,
        public_key: String,
        cases: Vec<RustCase>,
    }
    let out = Out {
        private_key: "3945208f7b2144b13f36e38ac6d39f95889393692860b51a42fb81ef4df7c5b8",
        public_key: public,
        cases,
    };
    let path = fixtures_dir().join("rust-vectors.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(&out).unwrap()).unwrap();
}

#[derive(Serialize)]
struct RustCase {
    plain: String,
    #[serde(rename = "sm4Key")]
    sm4_key: String,
    smkey: String,
    password: String,
}

#[test]
fn rejects_malformed_envelopes() {
    let secret = secret_from_hex(
        "3945208f7b2144b13f36e38ac6d39f95889393692860b51a42fb81ef4df7c5b8",
    )
    .unwrap();
    let public = public_hex_of(&secret);
    let (smkey, sealed) = seal(&public, &["Abcdef123!"]).unwrap();
    let password = &sealed[0];

    assert!(open_envelope_with(&secret, &smkey, password).is_ok());

    // Rust/BC 生成的是 04 前缀；去掉前缀（sm-crypto 格式）也必须能解
    assert!(smkey.starts_with("04"));
    let no_prefix = &smkey[2..];
    assert!(open_envelope_with(&secret, no_prefix, password).is_ok());

    // 奇数长度 hex
    assert!(open_envelope_with(&secret, &smkey[..smkey.len() - 1], password).is_err());

    // C1C2C3 与 C1C3C2 混淆：C1 之后 64 hex 是 C3，再之后是 C2
    let (c1, rest) = smkey.split_at(2 + 128);
    let (c3, c2) = rest.split_at(64);
    let wrong_mode = format!("{c1}{c2}{c3}");
    assert!(open_envelope_with(&secret, &wrong_mode, password).is_err());

    // 篡改 C3 一个字节
    let mut tampered = smkey.clone().into_bytes();
    let pos = 2 + 128 + 2;
    tampered[pos] = if tampered[pos] == b'0' { b'1' } else { b'0' };
    let tampered = String::from_utf8(tampered).unwrap();
    assert!(open_envelope_with(&secret, &tampered, password).is_err());

    // SM2 加密的是 16 字节裸密钥（错误实现）→ 必须拒绝
    let raw_key = hex::decode(random_sm4_key_hex()).unwrap();
    let bad_smkey = sm2_encrypt_hex(&public, &raw_key).unwrap();
    let bad_password = sm4_ecb_encrypt_hex(&hex::encode(&raw_key), b"Abcdef123!").unwrap();
    assert!(open_envelope_with(&secret, &bad_smkey, &bad_password).is_err());

    // SM4 密文被截断
    let truncated = &password[..password.len() - 32];
    assert!(open_envelope_with(&secret, &smkey, truncated).is_err());
}

#[test]
fn keyring_roundtrip_and_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let mut ring = KeyRing::load_or_create(dir.path(), None).unwrap();

    let info = ring.public_key_info();
    assert_eq!(info.public_key.len(), 130);
    assert!(info.public_key.starts_with("04"));
    assert_eq!(info.key_id.len(), 16);

    let (smkey, sealed) = seal(&info.public_key, &["Abcdef123!"]).unwrap();
    let opened = ring.open_envelope(Some(&info.key_id), &smkey, &sealed[0]).unwrap();
    assert_eq!(opened.expose_secret(), "Abcdef123!");

    // 未知 keyId
    assert!(ring.open_envelope(Some("deadbeefdeadbeef"), &smkey, &sealed[0]).is_err());

    // 轮换后旧密文仍可解，新密文用新 keyId
    let old_id = info.key_id.clone();
    let new_id = ring.rotate().unwrap();
    assert_ne!(old_id, new_id);
    assert_eq!(ring.open_envelope(Some(&old_id), &smkey, &sealed[0]).unwrap().expose_secret(), "Abcdef123!");

    let new_info = ring.public_key_info();
    assert_eq!(new_info.key_id, new_id);
    let (smkey2, sealed2) = seal(&new_info.public_key, &["NewPass123!"]).unwrap();
    assert!(ring.open_envelope(Some(&new_id), &smkey2, &sealed2[0]).is_ok());

    // 宽限期 0 秒后旧密钥被清理
    let removed = ring.prune(0).unwrap();
    assert_eq!(removed, vec![old_id.clone()]);
    assert!(ring.open_envelope(Some(&old_id), &smkey, &sealed[0]).is_err());

    // 重新加载：active 不变，公钥不变
    let ring2 = KeyRing::load_or_create(dir.path(), None).unwrap();
    assert_eq!(ring2.active_key_id(), new_id);
    assert_eq!(ring2.active_public_key(), new_info.public_key);
    drop(ring);
}

#[test]
fn keyring_with_passphrase() {
    let dir = tempfile::tempdir().unwrap();
    let ring = KeyRing::load_or_create(dir.path(), Some("s3cret-pass")).unwrap();
    let info = ring.public_key_info();
    let (smkey, sealed) = seal(&info.public_key, &["Abcdef123!"]).unwrap();
    drop(ring);

    // 正确口令可重新加载
    let ring = KeyRing::load_or_create(dir.path(), Some("s3cret-pass")).unwrap();
    assert_eq!(ring.open_envelope(None, &smkey, &sealed[0]).unwrap().expose_secret(), "Abcdef123!");

    // 私钥文件不是明文 PEM
    let secret_file = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().ends_with(".priv.pem"))
        .unwrap();
    let text = fs::read_to_string(secret_file.path()).unwrap();
    assert!(text.starts_with("-----BEGIN ENCRYPTED SM2 PRIVATE KEY-----"));

    // 错误口令无法加载
    assert!(KeyRing::load_or_create(dir.path(), Some("wrong-pass")).is_err());
}

#[test]
fn sm4_matches_known_vector() {
    // GB/T 32907-2016 标准向量：key/plain 均为 0123456789abcdeffedcba9876543210
    let key = "0123456789abcdeffedcba9876543210";
    let plain = hex::decode("0123456789abcdeffedcba9876543210").unwrap();
    let cipher = sm4_ecb_encrypt_hex(key, &plain).unwrap();
    // 单块无填充时标准密文是 681edf34d206965e86b3e94f536e4246；
    // 这里带 PKCS#7 填充，因此校验前两块可解回原文
    assert_eq!(&cipher[..32], "681edf34d206965e86b3e94f536e4246");
    let back = sm4_ecb_decrypt(key, &hex::decode(&cipher).unwrap()).unwrap();
    assert_eq!(back.as_slice(), plain.as_slice());
}
