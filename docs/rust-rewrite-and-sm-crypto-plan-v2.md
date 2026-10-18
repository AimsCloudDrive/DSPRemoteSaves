# DSP 云存档服务 Rust 重构 + 国密管理端 实现方案 v2

> 范围：`services/acc`（Node/Express + MongoDB）全量 Rust 重写；新增管理端 + 用户门户；密码传输国密改造；存档版本、配额、审计、备份。
> 版本：v2 / 2026-10-17（基于需求拷问后的最终决策；v1 已废弃）
> 目标读者：后端、前端、C# 插件开发者、运维。

---

## 0.0 实施校正（2026-10-17 落地时验证）

> 本节记录实现过程中**与原文不一致的事实**和实际落地情况，避免后来的维护者被旧描述误导。

1. **C1 前缀不是统一的**：`sm-crypto@0.3.14` 的 `sm2.doEncrypt` 在源码里执行了
   `if (c1.length > 128) c1 = c1.substr(c1.length - 128)`，即输出 `X‖Y‖C3‖C2`，**没有 `04`**；
   BouncyCastle 的 `SM2Engine` 与 RustCrypto `sm2::pke` 输出 `04‖X‖Y‖C3‖C2`，**带 `04`**。
   服务端 `sm-envelope` 因此兼容两种格式：先按原样解，失败后补 `04` 再试，最后尝试去掉 `04`。
   注意无前缀时 X 的首字节也可能恰好是 `0x04`，不能只凭首字节判断，必须按“尝试解密”区分。
2. **C# 端未使用 BouncyCastle**：构建环境无法获取 NuGet 包，改为在插件内实现自包含的
   SM2 加密 + SM3 + SM4-ECB/PKCS#7（`SmCrypto.cs`，零外部 DLL），并用 JS 向量反向校验 50/50 通过。
   若后续能引入 BouncyCastle，可替换该文件而不影响协议。
3. **前端包管理器**：pnpm 11 默认阻止 `esbuild` 构建脚本，导致 `vite build` 无法运行；实际使用 npm。
   命令统一为 `npm --prefix web install && npm --prefix web run build`。
4. **管理端受众严格分离**：`/auth/login` 一律签发 `aud=user`；管理员必须走 `/admin/auth/login` 才能调用
   `/admin/*`。前端在检测到“管理员但只有 user 受众”时会引导到 `/login?admin=1`。
5. **实际落地位置**：`services/acc-rs/`（旧 `services/acc` 未改动）。已通过：
   - Rust 单元测试（国密 1017 组 JS→Rust、密钥环/口令、密码策略、CIDR/Range）；
   - 协议冒烟 30 项（登录、重放、时钟、明文拒绝、上传/版本/回退/删除、审计、受众隔离）；
   - 双向互通：Rust→JS 6/6、C#→JS 50/50；
   - 前端 `vue-tsc` 0 错误、中英 265 个 key 完全一致；C# 插件 0 警告 0 错误。

---

## 0. 决策速览

| # | 决策 | 结论 |
|---|---|---|
| 1 | 数据库 | MongoDB 单机 standalone（无多文档事务） |
| 2 | 历史数据 | 全新库、零数据，不做迁移 |
| 3 | 后端 | Rust：axum + tokio + 官方 `mongodb` driver |
| 4 | 管理端 | Vue 3 + Element Plus + Pinia + vue-i18n |
| 5 | 国密 | SM2 C1C3C2 + SM4-ECB/PKCS#7，按参考实现逐字节对齐 |
| 6 | SM2 密钥 | 服务端生成，文件 0600，`keyId` 支持轮换 |
| 7 | 模组认证 | 无 token；业务请求带 account + 密文密码 |
| 8 | 分片认证 | `init` 签发一次性 `uploadToken`，`chunk` 只认它 |
| 9 | 管理端认证 | JWT access 2h + refresh 7d（HttpOnly Cookie）+ `tokenVersion` 即时吊销 |
| 10 | 角色 | `admin` + `user` |
| 11 | 注册 | 开放注册 + 邮箱验证码（无图形码） |
| 12 | 邮件通道 | 腾讯企业邮免费版（自有域名，SMTP） |
| 13 | 登录标识 | 用户名；邮箱只做验证/找回，不做登录 |
| 14 | 找回密码 | 邮箱验证码自助重置；管理员重置兜底 |
| 15 | 配额 | 新账号 1GB，管理员随时调整；所有版本计入 |
| 16 | 版本 | 不限数量，完整文件；覆盖生成新版本 |
| 17 | 删除 | 删当前 → 上一版成为当前；勾选“删除备份” → 当前 + 全部版本 |
| 18 | 语言 | 简体中文 + English |
| 19 | 审计 | 保留 180 天，支持筛选/CSV 导出 |
| 20 | 部署 | Linux + Docker Compose（app/Caddy/Mongo/Redis/MinIO） |
| 21 | 管理端暴露 | 仅内网/VPN + Caddy IP allowlist |
| 22 | 备份 | 每日 03:00 Mongo + saves → 同机 MinIO，保留 30 天 |
| 23 | 兼容 | 删除旧 `/users/*`；保留 `/upload/*`、`/download/*` 路径 |
| 24 | 过渡 | 无 `dual`、无明文窗口；C# 插件必须同版本升级 |
| 25 | 密码策略 | 10~64 位，英文字母 + 数字 + 特殊字符三类齐全，不含用户名/邮箱前缀 |
| 26 | 缓存 | Redis 7 单实例：限流、防重放、验证码缓存、令牌吊销、热点读、锁；Mongo/磁盘仍是事实源 |

---

## 1. 现状分析

### 1.1 服务拓扑

```
Unity 游戏 (BepInEx 插件, .NET Framework 4.8)
        │ HTTP :9999，明文 JSON / 二进制分片
        ▼
Node + Express 5 (services/acc, TypeScript → dist)
        ├── MongoDB DSPCloudSaves.users
        └── 磁盘 saves/temp/<fileId>/、saves/users/<userName>/*
```

`src/` 只有 5 个文件、约 1500 行；`dist/` 是编译产物。服务做三件事：用户校验、分片上传、按用户列目录/下载。

### 1.2 关键事实

1. **桌面端从不调用 `/users/login`**。它在每个上传/下载请求里重复携带 `userName + password`，没有会话概念。
2. 因此 `/users/*` 这 5 个接口没有真实调用方，可以删除；真正要兼容的只有：
   - `POST /upload/init`
   - `POST /upload/chunk`
   - `POST /upload/complete`
   - `POST /download/file-list`
   - `POST /download/download`
3. 现有 `users` 集合只有 `userName / password(bcrypt) / cache / login`，`login` 字段没有参与任何鉴权。
4. 没有生产数据，旧库可以直接弃用。

### 1.3 现有实现问题

| 级别 | 位置 | 问题 | 影响 |
|---|---|---|---|
| 严重 | `index.ts:157-164` | 中间件打印整个请求体 | 明文密码进日志 |
| 严重 | 全链路 | 密码随每次请求传输，无 TLS 强制 | 抓包即得凭据 |
| 严重 | 整体 | 无会话、无角色、无审计 | 无法踢人、无法追溯 |
| 高 | `index.ts:601` | `/users/create` 无鉴权 | 任何人可注册占磁盘 |
| 高 | `server.ts:23-26` | `cors({ origin: "*" })` | 任意网页可驱动 API |
| 高 | 整体 | 无限流、无失败锁定 | 可在线爆破 |
| 高 | 整体 | 无配额、无磁盘余量检查 | 单账号可打满磁盘 |
| 中 | `index.ts:99` | `bcrypt.compareSync` 阻塞事件循环 | 并发登录卡顿 |
| 中 | `index.ts:115-121` | 上传会话只在内存，启动清空 temp | 重启丢会话 |
| 中 | `index.ts:384` | 目录名直接来自 `userName` | Windows 保留名/大小写冲突 |
| 中 | `index.ts:286-300` | 分片只认 `fileId` | 拿到 fileId 可污染他人上传 |
| 低 | `index.ts:759` | 登录失败返回 `code: 0` | 客户端误判成功 |

---

## 2. 范围与验收

### 2.1 功能

| 编号 | 功能 | 说明 |
|---|---|---|
| F1 | 自助注册 | 用户名 + 邮箱 + 密码 + 邮箱验证码 |
| F2 | 登录 | 用户名 + 密码（SM2/SM4 信封）；管理端 JWT，模组无状态 |
| F3 | 用户门户 | 看/传/下/删自己的存档，查看与回退历史版本 |
| F4 | 管理端 | 用户 CRUD、配额调整、全部存档、版本回退、审计、设置 |
| F5 | 存档版本 | 不限数量，完整文件，计入配额；删除当前回退上一版 |
| F6 | 配额 | 新账号 1GB，管理员随时调整；所有版本计入 |
| F7 | 双语 | 简体中文 / English，界面 + 接口消息 + 邮件模板 |
| F8 | 审计 | 180 天，关键操作留痕，CSV 导出 |
| F9 | 备份 | 每日 Mongo + saves → MinIO，保留 30 天 |
| F10 | 缓存 | Redis 7：限流/防重放/验证码/令牌吊销/热点读/锁；故障可降级，Mongo 为事实源 |

### 2.2 强制项

| 编号 | 要求 | 验收 |
|---|---|---|
| M1 | 密码传输必须 SM2 + SM4 组合加密 | 抓包、日志、代码检查均无明文 |
| M2 | 服务端生成并保存 SM2 密钥对 | 重启后公钥不变，私钥 0600 |
| M3 | 与 `get-login-token.mjs` 的加密方式一致 | JS ↔ Rust 双向向量全部通过 |
| M4 | Rust 全量重写 | 单一二进制，不再依赖 Node |
| M5 | 语言切换 | 中英词条 100% 覆盖，刷新保持 |

### 2.3 非目标

- 不做邮件营销、不做短信、不做第三方 OAuth。
- 不做多实例/分片存储；单实例 + 单机磁盘；Redis 单实例，不做 Cluster/Sentinel。
- 不做存档内容编辑/预览（`.dsv` 是游戏二进制格式）。
- 不做跨用户分享/公开链接。

---

## 3. 国密方案

### 3.1 参考实现的逐字节还原

`get-login-token.mjs` / `src/utils/crypto.js` 的流程：

```
1. 取公钥：data = "04..."（未压缩点 hex）
2. sm4Key = 16 字节随机数 → 32 位小写 hex 字符串
3. smkey  = SM2_doEncrypt(sm4Key 这个"字符串", publicKey, cipherMode=1) → hex
4. password_cipher = SM4_ECB_PKCS7(明文密码, key=hexToBytes(sm4Key)) → hex
5. POST /auth/login
   header: smkey: <SM2 密文 hex>
   body:   { "userName": "...", "password": "<SM4 密文 hex>", "timestamp": 1760707200000, "nonce": "..." }
```

字节级约定：

| 项 | 约定 | 备注 |
|---|---|---|
| 曲线 | `sm2p256v1`（GM/T 0003） | — |
| SM2 密文布局 | **C1C3C2**（`cipherMode=1`） | 必须显式指定 |
| C1 编码 | 未压缩点 `04 ‖ X(32) ‖ Y(32)` | sm-crypto 输出**无** `04`；BC/Rust 输出**有** `04`；服务端两种都接受（见 §0.0） |
| SM2 明文 | **SM4 密钥的 32 位 hex 字符串（32 字节 ASCII）** | 不是 16 字节裸密钥 |
| SM4 模式 | **ECB**，PKCS#7 | `sm4.encrypt` 不传 `mode` 即 ECB |
| SM4 密钥 | 16 字节，hex 解码后使用 | `sm-crypto` 对字符串 key 走 `hexToArray` |
| 密文编码 | 小写 hex | 统一小写 |
| 传输位置 | SM2 密文放 `smkey` 头；SM4 密文放密码字段 | 与参考实现一致 |

两个最容易踩的坑：

1. 参考实现加密的是 **32 字符 hex 文本**；服务端解出来必须先当字符串，再 `hex::decode` 成 16 字节。
2. `sm-crypto` 的 `sm4.encrypt(plain, key)` 默认就是 ECB；不要被某些文档误导成 CBC。

### 3.2 服务端 SM2 密钥对

```
data/keys/
├── keyring.json                 # { "active": "a1b2c3d4", "keys": ["a1b2c3d4"] }
├── sm2-a1b2c3d4.priv.pem        # PKCS#8，权限 0600
└── sm2-a1b2c3d4.pub.hex         # 04||X||Y
```

- 首次启动生成；已存在则加载；绝不写进代码/镜像/仓库。
- `keyId = hex(sha256(pub)[..8])`，随公钥返回，客户端可缓存。
- 设置 `SM2_KEY_PASSPHRASE` 时，私钥用 `argon2id` 派生密钥 + `chacha20poly1305` 加密落盘；未设置时仅靠 0600，并在启动日志打 `WARN`。
- 轮换：`POST /admin/crypto/rotate`，新密钥置为 active，旧密钥保留 24h；未知 `keyId` 返回 `2005`，客户端刷新公钥后重试一次。
- 多实例不在范围内；如果未来扩容，keyring 必须放共享卷或 KMS。

生成（`sm2` crate，需 `pke`/`getrandom` feature）：

```rust
use sm2::{SecretKey, pke::{DecryptingKey, Mode}};
use sm2::elliptic_curve::Generate;

let sk = SecretKey::generate();
// 未压缩点 04||X||Y；方法名以锁定版本为准
let pub_hex = hex::encode(sk.public_key().to_sec1_point(false).as_bytes());
let key_id  = hex::encode(&sha2::Sha256::digest(pub_hex.as_bytes())[..8]);
```

### 3.3 公钥接口

```http
GET /dsp.saves/api/crypto/public-key
200
{
  "code": 0,
  "payload": {
    "keyId": "a1b2c3d4e5f60718",
    "publicKey": "04....",
    "sm2Mode": "C1C3C2",
    "sm4Mode": "ECB/PKCS7",
    "serverTime": 1760707200000
  }
}
```

`serverTime` 用于客户端校正时钟（容差 300s）。公钥接口本身限流（60 次/分钟/IP）。

### 3.4 防重放

1. `|serverNow - timestamp| <= 300_000ms`，否则 `2003`；时间戳兼容秒/毫秒（`< 1e12` 视为秒）。
2. 重放键 = `SM3(smkey)`（有 `nonce` 时 = `SM3(nonce ‖ smkey)`），写入 TTL 10 分钟的 `moka` 缓存；命中返回 `2004`。
3. 校验顺序：时间戳 → 重放 → SM2 解密 → SM4 解密 → 验密码；避免对攻击者做无谓的椭圆曲线运算。
4. 缓存上限 10 万条，LRU 淘汰；单实例足够。

### 3.5 Rust 解密

```rust
use cipher::{BlockDecryptMut, KeyInit, block_padding::Pkcs7};
use sm2::pke::{DecryptingKey, Mode};
use sm4::Sm4;

pub fn open_envelope(
    ring: &KeyRing,
    key_id: Option<&str>,
    smkey_hex: &str,
    password_hex: &str,
) -> Result<SecretString, CryptoError> {
    let key = ring.get(key_id)?;
    let dk = DecryptingKey::new_with_mode(key.to_nonzero_scalar(), Mode::C1C3C2);
    let key_hex_bytes = dk.decrypt(&hex::decode(smkey_hex)?)?;
    let key_hex = std::str::from_utf8(&key_hex_bytes)?;

    let key_bytes: [u8; 16] = hex::decode(key_hex)?.try_into()
        .map_err(|_| CryptoError::BadSm4Key)?;

    let mut buf = hex::decode(password_hex)?;
    let cipher = ecb::Decryptor::<Sm4>::new_from_slice(&key_bytes)
        .map_err(|_| CryptoError::BadSm4Key)?;
    let plain = cipher
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| CryptoError::BadPadding)?;

    Ok(SecretString::from(std::str::from_utf8(plain)?))
}
```

注意：

- RustCrypto `sm2` 的 `pke` 输出是 `04||X||Y||C3||C2`，而 `sm-crypto` **去掉** `04`；
  服务端必须两种都解（实现见 `sm-envelope::sm2_decrypt_with`），并用双向向量锁定。
- SM4 解密后校验 UTF-8，非法字节直接拒绝。
- 明文密码用 `secrecy::SecretString` 包裹，禁止 `Debug`/日志输出。
- 椭圆曲线运算放 `spawn_blocking`，避免占满 async worker。

### 3.6 C# 桌面端改造

1. 启动时 `GET /crypto/public-key`，缓存 `publicKey + keyId`。
2. 每次业务请求：
   - `RNGCryptoServiceProvider` 生成 16 字节 → 32 位 hex；
   - `SM2Engine(Mode.C1C3C2)` 加密这 32 字节 ASCII；
   - `PaddedBufferedBlockCipher(SM4Engine, Pkcs7Padding)` 加密密码；
   - `smkey` 放请求头，`password` 放密文。
3. 收到 `2005` 刷新公钥并重试一次。
4. 密码不进入任何日志。

```csharp
static byte[] Sm2Encrypt(string pubHex, byte[] plain)
{
    var curve = GMNamedCurves.GetByName("sm2p256v1");
    var point = curve.Curve.DecodePoint(Hex.Decode(pubHex));
    var pub = new ECPublicKeyParameters(
        point, new ECDomainParameters(curve.Curve, curve.G, curve.N, curve.H));
    var engine = new SM2Engine(SM2Engine.Mode.C1C3C2);
    engine.Init(true, new ParametersWithRandom(pub, new SecureRandom()));
    return engine.ProcessBlock(plain, 0, plain.Length);
}

static string Sm4EcbHex(byte[] key, byte[] plain)
{
    var cipher = new PaddedBufferedBlockCipher(new SM4Engine(), new Pkcs7Padding());
    cipher.Init(true, new KeyParameter(key));
    var outp = new byte[cipher.GetOutputSize(plain.Length)];
    int len = cipher.ProcessBytes(plain, 0, plain.Length, outp, 0);
    len += cipher.DoFinal(outp, len);
    return BitConverter.ToString(outp, 0, len).Replace("-", "").ToLowerInvariant();
}
```

> .NET Framework 4.8 没有内置 SM2/SM4，只能引 BouncyCastle；随机源不能用 `Random`。

### 3.7 安全边界

- SM2/SM4 只保护**密码字段**；token、Cookie、存档内容靠 HTTPS。Caddy 必须启用 TLS。
- 日志统一脱敏：`password`、`smkey`、`authorization`、`token`、`cookie` 一律 `[REDACTED]`；禁止打印请求体。
- 登录失败统一返回“用户名或密码错误”，不区分用户不存在/密码错；对不存在用户执行一次假哈希，抹平时序差异。
- SM2 私钥不进镜像、不进 git；备份时排除 `data/keys`（单独冷备）。

---

## 4. 账号与认证

### 4.1 角色

| 角色 | 用户管理 | 存档 | 版本回退 | 审计 | 设置/轮换密钥 |
|---|---|---|---|---|---|
| admin | 全部 | 全部 | 全部 | 读/导出 | 是 |
| user | 仅自己 | 仅自己 | 仅自己 | 否 | 否 |

权限判断集中在 axum extractor：`AdminUser`、`CurrentUser`；业务 handler 只声明身份，不写 if-else。

### 4.2 注册流程（邮箱必填 + 验证）

```
① POST /auth/email-code                 → 发 6 位邮箱码（同一邮箱 60 秒一次）
   { email, purpose: "register" }
② POST /auth/register                   → 校验邮箱码；创建用户；自动登录
   header smkey
   { userName, email, code, password: <SM4 密文>, timestamp, nonce }
```

规则：

- 用户名 3~32 字符，字母/数字/下划线/中文，禁止 `@`、空格、路径字符；大小写不敏感唯一。
- 邮箱小写存储，唯一；`emailVerifiedAt` 在注册成功时写入。
- 密码 **10~64 位**，必须**同时**包含三类字符：
  - 至少 1 个英文字母（大写或小写均可）；
  - 至少 1 个数字；
  - 至少 1 个特殊字符（ASCII 标点，如 `!@#$%^&*`，不含空格）。
- 拒绝包含用户名或邮箱本地部分（`@` 前）的密码；拒绝常见弱密码表。
- 校验失败返回 `3003 password_weak`，`payload.missing` 给出缺失项（`length` / `letter` / `digit` / `special`），前端直接标红对应 checklist。
- 后端是最终裁判；前端只做即时提示，不能绕过服务端校验。
- 邮箱码 6 位数字，**5 分钟**有效，5 次尝试，单次使用；同一邮箱 **60 秒**内不能重复发送。
- 限流（详见 §4.8）：
  - 发邮箱码：同一邮箱 1 次/60s、5 次/小时、10 次/天；同一 IP 10 次/小时、30 次/天；同一收件域 100 次/小时；全局 100 次/小时、450 次/天；
  - 注册：同一 IP 3 个账号/天；
  - 登录失败：同一账号 5 次/10 分钟锁定 15 分钟；同一 IP 20 次/10 分钟。
- 邮箱枚举：发码接口统一返回“如果该邮箱可用，验证码已发送”，不区分邮箱是否存在；注册在**验证码通过后**才检查邮箱唯一性并返回 `409 email_taken`。这样攻击者无法用一次请求探测邮箱是否已注册。

密码校验函数（注册、改密、重置、管理员重置共用）：

```rust
fn password_issues(pw: &str, user_name: &str, email: &str) -> Vec<&'static str> {
    let mut issues = Vec::new();
    if !(10..=64).contains(&pw.chars().count()) { issues.push("length"); }
    if !pw.chars().any(|c| c.is_ascii_alphabetic()) { issues.push("letter"); }
    if !pw.chars().any(|c| c.is_ascii_digit()) { issues.push("digit"); }
    if !pw.chars().any(|c| c.is_ascii_punctuation()) { issues.push("special"); }

    let lower = pw.to_lowercase();
    let local = email.split('@').next().unwrap_or("").to_lowercase();
    if lower.contains(&user_name.to_lowercase())
        || (!local.is_empty() && lower.contains(&local))
    {
        issues.push("personal");
    }
    if COMMON_PASSWORDS.contains(&lower.as_str()) { issues.push("common"); }
    issues
}
```

- `is_ascii_alphabetic()` 只认英文 A-Z/a-z；中文、全角字符不算“字母”。
- `is_ascii_punctuation()` 覆盖 `!` 到 `~` 之间的 ASCII 标点，不含空格。
- 管理员重置密码时用 CSPRNG 生成 16 位，先分别保证字母/数字/特殊字符各至少一个，再打乱。

### 4.3 登录

```
POST /auth/login
header smkey
{ userName, password: <SM4 密文>, timestamp, nonce }
→ { token, expiresAt, user: { id, userName, role, locale, quotaBytes, usedBytes } }
```

- 登录标识只有 `userName`，邮箱不参与登录。
- 密码校验用 `argon2id`；全新库不需要兼容旧 bcrypt。
- 成功后签发 access JWT + refresh Cookie。

### 4.4 管理端 JWT

| 项 | 值 |
|---|---|
| access | JWT HS256，2h，放内存（Pinia），`Authorization: Bearer` |
| refresh | 32 字节随机，DB 存 SHA-256，7d，HttpOnly + Secure + SameSite=Strict Cookie |
| claims | `iss=dsp-saves`、`aud=user\|admin`、`sub=userId`、`ver=tokenVersion`、`jti`、`iat`、`exp` |
| 吊销 | 改密/重置/禁用/删除 → `tokenVersion++` + 删除该用户全部 refresh + `DEL user:ver:{id}`；每请求校验 `ver`（Redis 缓存，变更即失效；Redis 故障回源 Mongo）；登出把 `jti` 写 Redis 黑名单，TTL=access 剩余有效期 |
| Cookie | 用户 `dsp_user_rt`，管理端 `dsp_admin_rt`，Path 分别限定到对应 API 前缀 |
| CSRF | access 走 Header，天然免疫；refresh 接口校验 `Origin` |

登录/刷新：

```
POST /auth/login          → access + Set-Cookie refresh
POST /auth/refresh        → 新 access（refresh 轮换）
POST /auth/logout         → 删除 refresh，清 Cookie
POST /admin/auth/login    → 管理端专用，Caddy IP allowlist
```

管理端登录与用户登录拆开：`/admin/auth/login` 只允许内网/VPN；签发的 JWT `aud=admin`，用户 token 进不了 `/admin/*`。

### 4.5 模组端（无 token）

| 接口 | 凭据 |
|---|---|
| `POST /upload/init` | `userName` + 密文密码 + `smkey` |
| `POST /upload/chunk` | `uploadToken`（init 返回）+ `fileId` + `chunkIndex` |
| `POST /upload/complete` | `userName` + 密文密码 + `smkey` + `fileId` |
| `POST /download/file-list` | `userName` + 密文密码 + `smkey` |
| `POST /download/download` | `userName` + 密文密码 + `smkey` |

- `uploadToken`：32 字节随机，DB 存 SHA-256，绑定 `fileId + userId`；**空闲 10 分钟过期，每次 chunk 续期，最长 24 小时**。慢速上传 2GB 不会被固定 30 分钟掐断。
- `chunk` 不传密码，避免密文进 URL/日志，也避免每片一次 SM2。
- 模组没有登录态，服务端无法单设备踢下线；这是明确接受的取舍。

### 4.6 找回密码 / 改密 / 改邮箱

| 场景 | 流程 |
|---|---|
| 忘记密码 | `POST /auth/password/reset/request`（email）→ 发码；`POST /auth/password/reset`（email + code + 新密码密文）→ 改密，`tokenVersion++`，删全部 refresh |
| 管理员重置 | `POST /admin/users/{id}/reset-password`（新密码密文；服务端可生成 16 位随机密码，必含字母/数字/特殊字符）→ 强制下次登录改密 |
| 用户改密 | `POST /auth/password`（旧密码密文 + 新密码密文，共用一个 `smkey`）→ `tokenVersion++`，重新登录 |
| 绑定/改邮箱 | `POST /auth/email/change/request`（新邮箱 + 密码密文）→ 发码；`POST /auth/email/change`（code）→ 更新，通知旧邮箱 |
| 管理员建号 | 邮箱可选；未验证邮箱不能自助找回，只能管理员重置 |

> 所有设置新密码的入口（注册、找回重置、本人改密、管理员重置）都走同一个 `password_issues`，不允许某条路径绕过强度校验。

### 4.7 邮件通道：腾讯企业邮免费版

选它的原因：有自有域名、零按封成本、SMTP 直接可用、国内送达好；代价是免费版有账号数/容量/外发量限制（约 500 封/天/企业，具体以开通页为准），统计和退信能力弱。

**为什么不用个人 QQ 邮箱**：发件人只能是 `@qq.com`，无法用自有域名；个人账号日限额不稳定，群发触发风控会关闭 SMTP；账号被封会连带影响号主。它只适合内部告警，不适合用户验证。

配置：

```toml
[mail]
enabled = true
smtp_host = "smtp.exmail.qq.com"
smtp_port = 465
smtp_tls = "implicit"          # 465=implicit, 587=starttls
from_address = "no-reply@example.com"
from_name = "DSP 云存档"
username = "no-reply@example.com"
password = "${SMTP_PASSWORD}"  # 客户端专用密码，不进 git
daily_limit = 500
```

域名侧必须配：

- `MX`：`mxbiz1.qq.com` / `mxbiz2.qq.com`（收信/退信）；
- `SPF`：`v=spf1 include:spf.mail.qq.com ~all`；
- `DKIM`：企业邮控制台生成的 TXT；
- `DMARC`：`v=DMARC1; p=none; rua=mailto:dmarc@example.com`（可选，先观察）。

Rust 用 `lettre`（`tokio1-rustls-tls`）发信；模板中英双语，纯文本 + 简单 HTML；只发验证码和安全通知。

### 4.8 邮箱验证码防刷（无图形码）

去掉图形码后，`POST /auth/email-code` 是唯一能被匿名触发、且直接产生成本的接口。只做“同一邮箱 60 秒一次”挡不住换邮箱、换 IP、`+alias`、单域轰炸。按六层做；所有计数器放 Redis，用 Lua 保证 `INCR + EXPIRE` 原子，Redis 故障时降级到本地 `governor`/`moka`（单实例）并告警：

**① 单邮箱 / 单域**

- 同一邮箱（归一化后）60 秒内只发一次；重复请求返回 `429` + `Retry-After`，不重发。
- 同一邮箱 5 次/小时、10 次/天；超限后指数退避（60s → 120s → 300s → 900s）。
- `+alias` 归一化：`a+1@x.com`、`a+2@x.com` 计数时视为 `a@x.com`；投递仍用原地址。
- 同一收件域 100 次/小时，防止对 `qq.com`/`163.com` 单域轰炸。
- 一次性邮箱域名黑名单（可配置）；注册时拒绝或降级为人工审核。

**② 单 IP / 网段**

- 10 次/小时、30 次/天/IP；`/24` 网段 50 次/小时；IPv6 按 `/64`。
- 只信任 Caddy 的 `X-Forwarded-For`（配置 `trusted_proxies`），否则用 socket IP，防止伪造头绕过。
- 机房/VPN ASN 只做风险评分，不硬封（会误伤）。
- 验证失败也计数：同一 IP 20 次错误验证/小时 → 临时封 1 小时。

**③ 全局与队列**

- 全局 100 次/小时、450 次/天（企业邮约 500/天，留 10% 余量）。
- 发信走有界队列（进程内 `mpsc`，容量 100；需要跨重启续发时换 Redis List/Stream）+ 固定 worker；队列满返回 429，不阻塞请求。
- SMTP 连续失败 5 次 → 熔断 60 秒，返回 `503 mail_unavailable`；恢复后半开探测。
- 配额到 80% 告警，100% 拒绝并返回 `503 mail_quota_exhausted`；管理端可切通道/提额。

**④ 验证码本身**

- 6 位数字，`OsRng` 生成；禁止 `Math.random`。
- 只存 `HMAC-SHA256(code, server_secret)`，常量时间比较；不存明文。
- 5 分钟有效；5 次尝试；成功即删；重发覆盖旧码，旧码立即失效。
- 一个 `(email, purpose)` 只有一个活跃码：`_id = HMAC(email|purpose)`，天然唯一。
- 验证失败按 IP 也限流（20 次/小时），防止分布式撞码。
- 响应统一 `code_invalid`，不区分过期/错误/不存在。

**⑤ 业务层**

- 只有验证通过才创建账号；不预创建。
- 同一 IP 3 个账号/天；同一邮箱 1 个账号；新账号 1GB 配额。
- 登录失败 5 次/10 分钟锁 15 分钟，与发码限流分开计数。
- 注册发码不暴露邮箱是否存在：统一返回“如果该邮箱可用，验证码已发送”；重复在验证码通过后才返回 409。
- 找回密码：邮箱不存在时不发信，但仍返回同一句 generic。

**⑥ 观测与自动处置**

- 指标：发码/验证成功失败、按 IP/域/全局速率、SMTP 延迟/失败、队列长度、配额使用、Redis 命中率/延迟/内存/evicted。
- 告警：单 IP > 20 次/小时、全局 > 80 次/小时、失败率 > 5%、退信率 > 2%。
- 自动封禁：1 小时内触发 3 次限流 → 封 IP 1 小时（内存 + `blocked_ips` TTL 集合，重启不丢）。
- 审计：发码/验证都记 IP、邮箱掩码（`a***@x.com`）、purpose、结果。

**无图形码的替代手段（按需启用）**

| 手段 | 说明 | 成本 |
|---|---|---|
| PoW | IP 超过 5 次/小时后，客户端解 SHA-256 前导零难题；服务端微秒验证，客户端 0.5~2s | 低 |
| Turnstile/防水墙 | 无感或勾选式，作为 step-up，不默认开启（国内加载速度要测） | 中 |
| Honeypot + 时间窗 | 隐藏字段被填、或表单打开 <1s 即提交 → 静默拒绝 | 极低 |
| 一次性邮箱黑名单 | 直接降低可用地址空间 | 极低 |

**限流一览**

| 维度 | 限额 | 超限 |
|---|---|---|
| 单邮箱间隔 | 60s | 429 + retryAfter |
| 单邮箱 | 5/小时、10/天 | 429 |
| 单 IP | 10/小时、30/天 | 429 |
| /24 网段 | 50/小时 | 429 |
| 单收件域 | 100/小时 | 429 |
| 全局 | 100/小时、450/天 | 429 / 503 |
| 验证失败 | 5 次/码；20 次/小时/IP | 400 / 429 |
| 注册 | 3 账号/天/IP | 429 |

发码主流程（伪代码）：

```rust
async fn send_email_code(email: &str, purpose: Purpose, ip: IpAddr) -> Result<(), AppError> {
    let email = normalize_email(email)?;              // 小写 + 格式校验
    let domain = email.split('@').nth(1).unwrap();

    limiter.email_interval.check(&email)?;            // 60s
    limiter.email_hour.check(&email)?;                // 5/h
    limiter.email_day.check(&email)?;                 // 10/d
    limiter.ip_hour.check(&ip)?;                      // 10/h
    limiter.ip_day.check(&ip)?;                       // 30/d
    limiter.domain_hour.check(domain)?;               // 100/h
    limiter.global_hour.check(())?;                   // 100/h
    limiter.global_day.check(())?;                    // 450/d

    let code = random_code_6();                       // OsRng
    repo.upsert_code(&email, purpose, hmac(code), now + 300).await?;
    mailer.enqueue(&email, code, purpose).await?;     // 有界队列
    audit_send(&email, purpose, ip);
    Ok(())
}
```

---

## 5. Rust 架构

### 5.1 选型

| 领域 | 选择 | 理由 |
|---|---|---|
| Web | `axum` 0.8+ | tokio/tower 生态、类型化提取器、易测试 |
| 运行时 | `tokio` | 事实标准 |
| 数据库 | `mongodb` 官方 driver + `bson` | 保持 MongoDB；异步、官方维护 |
| 密码 | `argon2` + `password-hash` | argon2id；`spawn_blocking` 执行 |
| 国密 | `sm2` / `sm4` / `sm3` / `ecb` | 纯 Rust，跨平台 |
| JWT | `jsonwebtoken` | HS256；单服务足够 |
| 邮件 | `lettre` | SMTP + rustls |
| 邮箱码 | `rand` + `hmac`/`sha2` + `subtle` | 6 位数字，存 HMAC，常量时间比较 |
| 缓存 | `redis` + `deadpool-redis` | 连接池、Lua、TTL；单实例 Redis 7 |
| 配置 | `figment` + `dotenvy` | TOML + env 覆盖 |
| 日志 | `tracing` + `tracing-subscriber` | JSON、request-id、脱敏 |
| 限流 | Redis Lua + `governor`（本地降级） | 原子计数、跨重启；故障降级 |
| 备份 | `mongodump` + `tar`/`zstd` + `object_store` | MinIO S3 兼容 |
| 前端托管 | `rust-embed` / `ServeDir` | 单二进制交付 |

### 5.2 工程结构

```
services/acc-rs/
├── Cargo.toml                 # workspace
├── crates/
│   ├── sm-envelope/           # SM2/SM4 信封 + 互通测试向量
│   ├── acc-server/
│   │   ├── src/
│   │   │   ├── main.rs
│   │   │   ├── config.rs
│   │   │   ├── state.rs
│   │   │   ├── error.rs            # AppError + code/messageKey
│   │   │   ├── i18n.rs
│   │   │   ├── crypto/             # keyring, envelope
│   │   │   ├── auth/               # password, jwt, rbac, rate_limit, email_code
│   │   │   ├── cache/              # redis pool, keys, cache-aside, locks
│   │   │   ├── queue/              # email queue (optional redis stream)
│   │   │   ├── mail/               # smtp, templates
│   │   │   ├── repo/               # users, sessions, saves, versions, codes, audit
│   │   │   ├── storage/            # paths, uploads, versions, quota
│   │   │   ├── api/
│   │   │   │   ├── legacy/         # upload, download（模组）
│   │   │   │   ├── auth.rs         # 注册/登录/刷新/找回
│   │   │   │   ├── user/           # 用户门户
│   │   │   │   ├── admin/          # 管理端
│   │   │   │   ├── crypto.rs
│   │   │   │   └── health.rs
│   │   │   ├── middleware/         # auth, lang, request_id, limit
│   │   │   ├── tasks/              # cleanup, reconcile, backup
│   │   │   └── web.rs
│   │   └── migrations/             # 集合/index 初始化
│   └── acc-cli/                    # admin create、backup restore、reconcile
├── web/                            # Vue 3 SPA
└── tests/                          # 契约/安全/互通
```

### 5.3 启动流程

```
读配置 → tracing → 连接 Mongo → 连接 Redis（PING + LOAD Lua）→ 建集合/validator/index（幂等）
      → 加载/生成 SM2 keyring → 加载/生成 JWT secret
      → 清理过期上传会话/验证码/孤儿 temp
      → 启动后台任务（cleanup 60s、reconcile 10min、backup 每日 03:00）
      → 绑定端口 → 打印 active keyId（绝不打印私钥）
```

Redis 连接失败不阻止启动：readiness 标记 `degraded`，限流/防重放切本地实现，读缓存回源 Mongo 并持续重连。

### 5.4 MongoDB 集合与索引

```js
// users
{
  _id: ObjectId,
  userName: "Aims",
  userNameLower: "aims",          // 唯一索引
  email: "a@b.com",               // 可空（管理员建号）
  emailLower: "a@b.com",          // 稀疏唯一索引
  emailVerifiedAt: ISODate,
  passwordHash: "$argon2id$...",
  passwordAlgo: "argon2id",
  role: "user",                   // admin | user
  status: "active",               // active | disabled | deleted
  quotaBytes: 1073741824,         // 1GB
  usedBytes: 0,
  reservedBytes: 0,               // 进行中的上传预留，防并发超卖
  mustChangePassword: false,
  locale: "zh-CN",
  tokenVersion: 0,
  createdAt: ISODate,
  updatedAt: ISODate,
  lastLoginAt: ISODate
}
```

```js
// sessions（refresh token）
{ _id, userId, tokenHash, createdAt, expiresAt, lastSeenAt, ip, userAgent, revokedAt }
// TTL: expiresAt；索引: userId

// upload_sessions
{ _id: "uuid", userId, fileName, fileSize, chunkSize, totalChunks,
  uploadedMask: BinData, uploadTokenHash, createdAt, expiresAt }
// TTL: expiresAt；索引: userId

// saves（当前版本指针）
{ _id, userId, fileName, currentVersionId, currentSeq, sizeBytes, mtimeMs,
  sm3, nextSeq, createdAt, updatedAt }
// 唯一索引: (userId, fileName)

// save_versions（所有版本）
{ _id, saveId, userId, fileName, seq, sizeBytes, sm3, storageKey,
  createdAt, createdBy, source }   // source: mod|admin|user
// 索引: (saveId, seq desc), (userId, createdAt)

// email_codes（每个 (email,purpose) 只有一个活跃码）
{ _id: "hmac(email|purpose)", emailLower, purpose, codeHash, attempts,
  lastSentAt, expiresAt, consumedAt, ip, createdAt }
// TTL: expiresAt；_id 天然唯一，无需额外索引

// audit_logs
{ _id, actorId, actorName, action, targetType, targetId, result,
  detail, ip, userAgent, createdAt }
// TTL: 180 天；索引: (createdAt desc), (actorId, createdAt), (action, createdAt)

// settings
{ _id: "default", defaultQuotaBytes, maxFileSize, allowedExtensions, ... }
```

初始化用 `create_collection` + `$jsonSchema` validator + `create_index`，全部幂等；启动时执行，失败即退出。

### 5.5 文件布局与版本

```
saves/
├── temp/<fileId>/<index>.chunk
└── users/<userId>/
    ├── <fileName>                                  # 当前版本（硬链接）
    └── .versions/<fileName>/<seq>_<versionId>.bin  # 所有历史版本
```

- 当前文件是**硬链接**，不复制内容；同一 inode，磁盘只有一份。
- 覆盖流程：
  1. 写 `temp`，`fsync`；
  2. `rename` 到 `.versions/.../<seq>_<vid>.bin`；
  3. 删除旧 `<fileName>` 链接，`hard_link(versionPath, <fileName>)`；
  4. 插入 `save_versions`，更新 `saves` 指针，`$inc users.usedBytes +size`。
- 删除当前（`deleteVersions=false`）：
  1. 找 `seq < currentSeq` 的最大版本；
  2. 有：删除当前版本文件/doc，`hard_link(prev, <fileName>)`，更新指针，`mtimeMs=now`；
  3. 无：删除当前链接 + doc，`saves` 文档删除，`usedBytes -size`。
- 删除全部（`deleteVersions=true`）：删当前链接、所有版本 doc/文件、`saves` 文档，`usedBytes -= 所有版本大小`。
- 回退到版本 V：删除当前链接，`hard_link(V, <fileName>)`，更新指针，`mtimeMs=now`；**不删除**比 V 新的版本。
- 版本数量不限；每个版本完整文件；`usedBytes` 是全部版本之和。
- 文件名白名单：拒绝 `/ \ : * ? " < > |`、控制字符、`..`、结尾 `.`/空格、Windows 保留名；长度 ≤200 字节；NFC 归一化；大小写冲突检测。
- 符号链接：`symlink_metadata` 检查，拒绝；可选 `cap-std` 约束根目录。
- 原子写：同目录 temp → `fsync` → `rename`；崩溃后由 reconcile 修正。
- 磁盘保护：剩余 <10% 或 <2GB 时拒绝上传（507）。

### 5.6 配额

- `quotaBytes`：新账号 1GB；管理员可随时 PATCH。
- `usedBytes`：所有版本大小之和；`reservedBytes`：进行中的上传预留。
- `init` 时原子校验 `usedBytes + reservedBytes + size <= quotaBytes`，并 `$inc reservedBytes +size`；不满足 413。**先预留再传**，否则两个并发上传都能通过 init，最后一起超卖。
- `complete`：`$inc usedBytes +size, reservedBytes -size`；失败/过期/取消：`$inc reservedBytes -size`。
- 覆盖不会释放旧版本，所以配额会随版本增长；UI 必须显示“当前 + 历史版本 + 上传中”的占用。
- 管理员降低配额到低于已用量：只阻止新上传，不自动删文件；UI 显示超额警告。
- 每 10 分钟 reconcile：按 `save_versions` 聚合修正 `usedBytes`，并把超时未完成的 `reservedBytes` 释放。

### 5.7 上传会话

1. `init`：校验用户 + 参数（`fileSize <= 2GB`、`chunkSize` 5~64MB、`totalChunks <= 20000`）；**原子预留配额**（`$inc reservedBytes +size`，条件不满足则 413）；创建 `upload_sessions`，返回 `fileId + uploadToken`。
2. `chunk?fileId=&chunkIndex=`：校验 `uploadToken`、会话未过期、`chunkIndex` 范围、`Content-Length <= chunkSize`，写 `temp`，更新位图，**滑动续期**（空闲 10 分钟、最长 24 小时）。
3. `complete`：校验位图完整 → 顺序合并 + 流式 SM3 → 大小校验 → 按 5.5 生成版本 → `$inc usedBytes +size, reservedBytes -size` → 删会话与 temp。
4. 失败/取消/过期：`$inc reservedBytes -size`；清理任务每 60s 删除过期会话与目录；启动时扫描 DB 中不存在的 `temp/<fileId>` 并删除。

### 5.8 备份到 MinIO

```toml
[backup]
enabled = true
cron = "0 3 * * *"
retention_days = 30
bucket = "dsp-saves-backup"

[backup.minio]
endpoint = "http://127.0.0.1:9000"
access_key = "${MINIO_ACCESS_KEY}"
secret_key = "${MINIO_SECRET_KEY}"
region = "us-east-1"
use_path_style = true
```

- 每天 03:00：
  1. `mongodump --archive --gzip` 到临时文件；
  2. `tar --zstd` 打包 `saves/`（包含 `.versions`）；
  3. 上传到 `s3://<bucket>/mongo/YYYY-MM-DD.archive.gz`、`saves/YYYY-MM-DD.tar.zst`；
  4. 删除超过 30 天的对象。
- 加密可选（`backup.encrypt=true`，AES-256-GCM，密钥来自 env）；默认关闭，因为密钥与密文同机意义有限。
- 恢复：`acc-cli backup restore --date YYYY-MM-DD`，停服 → 下载 → `mongorestore --drop` → 解包 `saves/` → 启动。
- **同机 MinIO 的已知风险**：主机/磁盘故障会同时丢主数据和备份。已接受；预留 `mc mirror` 配置，未来加机器即可异地。
- 备份产物**不包含** `data/keys` 和 `jwt.secret`；这两个单独冷备，否则恢复后旧 token/密文全失效。

### 5.9 可观测性

- `GET /healthz`：进程存活；`GET /readyz`：Mongo + Redis + 磁盘 + keyring。
- `GET /metrics`（仅 localhost）：注册/登录成功失败、邮件发送、上传/下载字节、活跃会话、配额使用、备份结果、Redis 命中率/延迟/内存。
- 每请求 `x-request-id`，贯穿日志与错误响应。

### 5.10 Redis 缓存设计

#### 5.10.1 定位与接入

- Redis 7 单实例，仅内网；不做 Cluster/Sentinel（未来多实例再评估）。
- `redis` crate + `deadpool-redis` 连接池；Lua 用 `Script` 预加载，避免每次传脚本。
- 所有 key 带 `dsp:{env}:v1:` 前缀；对象用 JSON，调试友好。
- **Mongo/磁盘是事实源**；Redis 只做加速、短期状态和跨请求协调。Redis 丢数据不能导致业务数据丢失。

#### 5.10.2 用在哪、为什么

下表 key 省略统一前缀 `dsp:{env}:v1:`；邮箱、文件名等用 `SHA-256` 前 16 位代替，避免 PII 和超长 key。

| 场景 | 优先级 | Redis 结构 | Key | TTL | 价值 |
|---|---|---|---|---|---|
| 限流（邮箱/IP/域/全局/登录） | P0 | String + Lua | `rl:...` | 60s / 1h / 1d | 原子 `INCR+EXPIRE`、跨重启、未来多实例共享 |
| 防重放 | P0 | String `SET NX` | `replay:{SM3(smkey)}` | 10min | TTL 自动清理，避免内存泄漏 |
| 邮箱验证码缓存 | P0 | String | `code:{HMAC(email\|purpose)}` | 5min | 验证走 Redis，Mongo 兜底 |
| JWT 吊销 | P0 | String | `jwt:bl:{jti}` / `user:ver:{id}` | access 剩余 / 30s | 即时踢下线，减少 Mongo 读 |
| 热点读缓存 | P1 | String(JSON) | `user:profile:{id}` / `saves:list:{id}` / `stats:overview` / `pubkey:{keyId}` | 30~60s / 24h | 降 Mongo 读，提升管理端列表/状态 |
| 并发锁 | P0 | String `SET NX PX` | `lock:save:{uid}:{fileHash}` / `lock:backup` | 30s | 防并发覆盖同一存档、重复备份 |
| 幂等键 | P1 | String `SET NX EX` | `idem:{route}:{key}` | 5~10min | 防重复注册/完成上传/删除 |
| 发信队列（可选） | P2 | List/Stream | `queue:email` | — | 重启续发、削峰；单实例可先用进程内队列 |

#### 5.10.3 不用在哪

| 场景 | 原因 |
|---|---|
| 用户、配额、存档、版本、审计的权威数据 | Mongo/磁盘是事实源；Redis 丢了不能影响正确性 |
| SM2 私钥 | 文件 0600/HSM，绝不进 Redis |
| 存档文件内容 | 太大；走磁盘和 OS page cache |
| 密码哈希 | 不扩散到缓存，减少泄露面 |
| 长期审计 | Mongo TTL 180 天 |
| refresh token 唯一存储 | Mongo 持久化；Redis 只做黑名单/版本缓存 |
| 上传会话唯一存储 | Mongo 持久化；Redis 最多缓存元数据，不缓存分片位图 |

#### 5.10.4 一致性与失效

- **Cache-aside**：读先 Redis，miss 查 Mongo 回填；写先 Mongo，成功后 `DEL` 相关 key。
- TTL 加 ±10% 抖动，防雪崩；热点 key 用 single-flight（本地 `moka` 锁或 Redis `SET NX`）防击穿。
- 明确失效点：
  - 用户改角色/状态/配额/改密 → `DEL user:profile:{id}`、`user:ver:{id}`、`stats:overview`；
  - 存档上传/删除/回退 → `DEL saves:list:{uid}`、`stats:overview`；
  - 公钥轮换 → 新 keyId 新 key，旧 key 保留 24h，不 `DEL`。
- 允许的陈旧度：列表/统计 30~60s；`tokenVersion` 不允许陈旧——变更即 `DEL`，miss 回源 Mongo。
- 多实例时再加 Pub/Sub 广播失效；单实例直接 `DEL`。

#### 5.10.5 原子操作

限流 Lua（固定窗口）：

```lua
-- KEYS[1]=key ARGV[1]=limit ARGV[2]=window
local n = redis.call('INCR', KEYS[1])
if n == 1 then redis.call('EXPIRE', KEYS[1], ARGV[2]) end
local ttl = redis.call('TTL', KEYS[1])
if n > tonumber(ARGV[1]) then return {0, ttl} end
return {1, ttl}
```

- 60 秒间隔用 `SET key 1 NX EX 60`，返回 nil 即拒绝。
- 锁释放用 Lua 比较 owner 再 `DEL`，防误删别人的锁。
- 幂等键 `SET NX EX`，value 存上次结果 JSON；重复请求直接回放。

#### 5.10.6 内存、持久化、安全

- `maxmemory 256mb` + `maxmemory-policy noeviction`：宁可写失败返回 503，也不静默淘汰限流/防重放 key。
- `appendonly yes` + `appendfsync everysec`；RDB 可选。Redis 只做缓存，**不纳入备份**。
- `requirepass` 从 env 注入；不映射宿主端口；Redis ACL 只给 `+@read +@write -@dangerous`，禁用 `FLUSHALL/CONFIG/KEYS`。
- 监控：`used_memory`、`evicted_keys`（必须为 0）、`keyspace_hits/misses`、`latency`、连接数。

#### 5.10.7 故障降级

| 依赖 | Redis 故障时 | 说明 |
|---|---|---|
| 限流 | 本地 `governor` 同参数 + WARN | 单实例可接受；多实例会放大 N 倍，需 fail-closed |
| 防重放 | 本地 `moka` TTL | 单实例等价 |
| 邮箱码 | 回源 Mongo `email_codes` | 始终可验证 |
| JWT 吊销 | 回源 Mongo `tokenVersion` | 即时性略降，但仍正确 |
| 读缓存 | 直接查 Mongo/磁盘 | 延迟上升，功能不变 |
| 锁 | 本地 `moka` 锁 | 单实例等价 |
| 发信队列 | 进程内队列 | 重启丢未发邮件，用户可重发 |

- `/readyz` 返回 `degraded`，不直接 503；只有 Redis 和 Mongo 同时不可用才 503。
- 降级状态每 30s 重试恢复；恢复后清本地降级计数，避免双倍限额。

---

## 6. API

统一前缀 `/dsp.saves/api`。

### 6.1 公开 / 认证

| 方法 | 路径 | 说明 | 鉴权 |
|---|---|---|---|
| GET | `/crypto/public-key` | 公钥 + keyId | 公开，限流 |
| POST | `/auth/email-code` | 发邮箱验证码（register/reset） | 公开，限流 |
| POST | `/auth/register` | 注册（邮箱码 + 密文密码） | 公开 |
| POST | `/auth/login` | 登录 | 公开 |
| POST | `/auth/refresh` | 刷新 access | refresh Cookie |
| POST | `/auth/logout` | 登出 | Bearer |
| POST | `/auth/password/reset/request` | 找回密码发码 | 公开，限流 |
| POST | `/auth/password/reset` | 用邮箱码重置 | 公开 |
| GET | `/auth/me` | 当前用户 | Bearer |
| POST | `/auth/password` | 改密 | Bearer |
| POST | `/auth/email/change/request` | 改邮箱发码 | Bearer |
| POST | `/auth/email/change` | 确认改邮箱 | Bearer |

### 6.2 模组（兼容路径）

| 方法 | 路径 | 变化 |
|---|---|---|
| POST | `/upload/init` | 密码改密文 + `smkey`；返回 `uploadToken` |
| POST | `/upload/chunk` | 改用 `uploadToken`；不再带密码 |
| POST | `/upload/complete` | 密码改密文 + `smkey` |
| POST | `/download/file-list` | 密码改密文 + `smkey`；只返回当前版本 |
| POST | `/download/download` | 密码改密文 + `smkey`；支持 Range |

**模组契约（不能改，否则 C# 反序列化失败）**

```json
// POST /upload/init → 注意是 payload.data，不是 payload
{
  "code": 0,
  "payload": {
    "data": {
      "fileId": "uuid",
      "chunkSize": 5242880,
      "totalChunks": 10,
      "uploadToken": "32字节base64url"
    }
  }
}

// POST /download/file-list
{
  "code": 0,
  "payload": {
    "files": [
      { "fileName": "save1.dsv", "fileSize": 10485760, "lastModified": 1760707200000 }
    ]
  }
}

// POST /upload/chunk、/upload/complete
{ "code": 0 }

// 错误统一
{ "code": 1, "message": "..." }
```

- `payload.data` 这层嵌套是现有 C# 的 `UploadInitPayload.Data` 写死的；不要为了“好看”拍平。
- `file-list` 的 `syncFileExtensions` 同时接受数组和逗号分隔字符串；`lastModified` 是毫秒，只返回当前版本。
- `download` 的 `isChunk=true` 时必须带 `Range`，返回 206 + `Content-Range`；否则返回 200 整文件。
- `chunk` 的 `fileId` / `chunkIndex` 在 query，二进制体在 body，`uploadToken` 在请求头。

### 6.3 用户门户

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/user/saves` | 自己的存档列表（含当前版本、大小、mtime、SM3） |
| PUT | `/user/saves/{name}` | 上传/覆盖（原始 body 或分片） |
| DELETE | `/user/saves/{name}?deleteVersions=false` | 删除当前；默认回退上一版 |
| GET | `/user/saves/{name}/versions` | 版本时间线 |
| GET | `/user/saves/{name}/versions/{id}/download` | 下载指定版本 |
| POST | `/user/saves/{name}/versions/{id}/rollback` | 回退到指定版本 |
| GET | `/user/stats` | 配额/用量/文件数 |

### 6.4 管理端

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/admin/auth/login` | 管理员登录（仅内网/VPN） |
| GET | `/admin/users` | 分页/搜索/状态筛选 |
| POST | `/admin/users` | 创建用户（邮箱可选，密码密文） |
| GET | `/admin/users/{id}` | 详情 + 配额 + 会话 |
| PATCH | `/admin/users/{id}` | 改角色/状态/配额/语言 |
| POST | `/admin/users/{id}/reset-password` | 重置密码（强制改密） |
| DELETE | `/admin/users/{id}?purgeSaves=` | 软删/彻底删 |
| GET | `/admin/users/{id}/saves` | 该用户存档 |
| GET | `/admin/users/{id}/saves/{name}/versions` | 版本 |
| POST | `/admin/users/{id}/saves/{name}/versions/{vid}/rollback` | 回退 |
| DELETE | `/admin/users/{id}/saves/{name}?deleteVersions=` | 删除 |
| GET | `/admin/saves` | 全局搜索（用户名/文件名） |
| GET | `/admin/stats/overview` | 用户/文件/容量/磁盘/备份 |
| GET | `/admin/audit` | 审计查询 |
| GET | `/admin/audit/export.csv` | 导出 |
| GET/PUT | `/admin/settings` | 默认配额、上传上限、扩展名 |
| POST | `/admin/crypto/rotate` | 轮换 SM2 密钥 |

### 6.5 统一响应

```json
{ "code": 0, "messageKey": "ok", "message": "成功", "payload": {}, "traceId": "01J..." }
```

- `message` 按 `Accept-Language` 本地化；新前端优先用 `messageKey`。
- 兼容层保持 `{ code: 0|1, message }`，不破坏模组解析。

### 6.6 错误码

| code | messageKey | HTTP | 含义 |
|---|---|---|---|
| 0 | ok | 200 | 成功 |
| 1001 | invalid_param | 400 | 参数错误 |
| 1002 | unauthorized | 401 | 未认证/token 失效 |
| 1003 | forbidden | 403 | 无权限 |
| 1004 | not_found | 404 | 资源不存在 |
| 1005 | conflict | 409 | 冲突 |
| 1006 | rate_limited | 429 | 限流 |
| 1007 | account_locked | 423 | 失败过多锁定 |
| 1008 | cache_unavailable | 503 | 缓存不可用且无法降级 |
| 2001 | sm_key_invalid | 400 | SM2 解密失败 |
| 2002 | sm_cipher_invalid | 400 | SM4 填充/UTF-8 非法 |
| 2003 | sm_timestamp_expired | 400 | 时间戳超窗 |
| 2004 | sm_replay | 409 | 重放 |
| 2005 | sm_key_unknown | 400 | keyId 未知 |
| 2006 | plaintext_forbidden | 401 | 拒绝明文密码 |
| 3001 | user_exists | 409 | 用户名已存在 |
| 3002 | email_taken | 409 | 邮箱已被使用 |
| 3003 | password_weak | 400 | 密码强度不足 |
| 3004 | quota_exceeded | 413 | 超出配额 |
| 3005 | file_too_large | 413 | 文件过大 |
| 3006 | chunk_incomplete | 400 | 分片缺失（返回 `nots`） |
| 3007 | file_not_found | 404 | 文件不存在 |
| 3008 | upload_expired | 400 | 上传会话过期 |
| 3009 | disk_full | 507 | 磁盘不足 |
| 3010 | invalid_file_name | 400 | 文件名非法 |
| 3011 | code_invalid | 400 | 验证码错误/过期 |
| 3012 | email_not_verified | 403 | 邮箱未验证 |
| 3013 | version_not_found | 404 | 版本不存在 |
| 4001 | mail_send_failed | 502 | 邮件发送失败 |
| 4002 | mail_quota_exhausted | 503 | 邮件日配额耗尽 |
| 4003 | mail_unavailable | 503 | SMTP 熔断/不可用 |
| 5000 | internal | 500 | 内部错误 |

> `1006 rate_limited` 必须带 `Retry-After` 头和 `payload.retryAfterSeconds`；前端按它重置 60 秒倒计时。
> `3003 password_weak` 的 `payload.missing` 返回 `["length","letter","digit","special","personal","common"]` 的子集；前端按顺序映射到密码 checklist，不要只弹一句“密码太弱”。

---

## 7. 管理前端

### 7.1 技术栈

Vue 3.5 + Vite + Element Plus + Pinia + Vue Router + `vue-i18n@11` + `sm-crypto`；复用 `services/acc/test` 的工程结构，重写页面。

### 7.2 页面

```
/login                 登录（用户名 + 密码，SM 加密）
/register              注册（用户名/邮箱/密码 + 强度 checklist/邮箱码）
/                      概览（用户数/文件数/容量/磁盘/最近审计/备份状态）
/users                 用户列表（搜索/创建/禁用/重置密码/配额）
/users/:id/saves       用户存档（当前 + 版本时间线 + 回退 + 删除）
/saves                 全局存档搜索
/audit                 审计（筛选/详情/导出 CSV）
/settings              默认配额、上传上限、扩展名、限流参数
/profile               个人资料（改密/改邮箱/语言）
```

- 路由守卫：无 token → `/login`；非 admin 隐藏用户管理/设置/审计。
- 注册/改密/重置页显示密码 checklist：10~64 位、含英文字母、含数字、含特殊字符、不含用户名/邮箱前缀；未满足时提交按钮禁用；后端返回 `3003` 时按 `payload.missing` 标红对应项。
- 邮箱码按钮 60 秒倒计时，但**以服务端为准**：收到 `429` 时按 `payload.retryAfterSeconds` 重置倒计时；刷新页面不能绕过服务端限流。
- 所有破坏性操作二次确认；删除用户要求输入用户名。
- 版本时间线显示 seq、大小、时间、来源（mod/admin/user）、SM3 前 8 位；回退按钮带确认。
- 删除对话框：默认“删除当前版本，回退上一版”；勾选“同时删除所有历史版本”才彻底删除。

### 7.3 国际化

- `vue-i18n` 组合式 API，词条按命名空间拆分：`common/auth/users/saves/audit/settings/errors`。
- 切换语言：`i18n.global.locale` + `localStorage` + `<html lang>` + `dayjs.locale` + `Accept-Language` + `PATCH /auth/me { locale }`。
- Element Plus 用 `<el-config-provider :locale="epLocale">`；日期/数字用 `Intl`。
- CI 校验 zh-CN 与 en-US 的 key 集合一致；开发环境开启 missingWarn。
- 后端错误：`messageKey` → `t('errors.' + messageKey)`；无匹配回退 `message`。
- 密码 checklist 使用 `auth.pwd.*` 词条（`length` / `letter` / `digit` / `special` / `personal`），与后端 `payload.missing` 一一对应。
- 邮件模板同样中英双语，按用户 `locale` 选择。

### 7.4 国密交互

```ts
import { sm2, sm4 } from 'sm-crypto'

function randomHex(bytes: number) {
  const buf = new Uint8Array(bytes)
  crypto.getRandomValues(buf)
  return Array.from(buf, b => b.toString(16).padStart(2, '0')).join('')
}

export async function seal(values: string[]) {
  const { keyId, publicKey } = await getPublicKey()
  const key = randomHex(16)
  const smkey = sm2.doEncrypt(key, publicKey, 1)       // C1C3C2
  return { keyId, smkey, values: values.map(v => sm4.encrypt(v, key)) }
}
```

- 公钥缓存 `sessionStorage`，24h 或收到 `2005` 刷新。
- 注册、登录、改密、重置密码、管理员建号/重置全部走 `seal`。
- 前端不打印密码、smkey；密码框 `autocomplete="new-password"`。

### 7.5 构建

```
pnpm --dir web build
cargo build --release -p acc-server
```

- 生产：`rust-embed` 嵌入 `web/dist`；未知非 API 路径回退 `index.html`。
- 开发：`ServeDir` + Vite 代理 `/dsp.saves/api` 到 `:9999`。
- 缓存：`index.html` no-cache；带 hash 资源 immutable。
- 安全头：CSP（`default-src 'self'; style-src 'self' 'unsafe-inline'`）、nosniff、Referrer-Policy、HSTS。

---

## 8. 部署与运维

### 8.1 Docker Compose

```yaml
services:
  app:
    build: .
    env_file: .env
    volumes:
      - ./data:/app/data          # keys, jwt.secret, logs
      - ./saves:/app/saves
    depends_on: [mongo, redis]
    restart: unless-stopped
  caddy:
    image: caddy:2
    ports: ["80:80", "443:443"]
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile
      - caddy_data:/data
  mongo:
    image: mongo:7
    command: ["--wiredTigerCacheSizeGB", "1"]
    environment:
      MONGO_INITDB_ROOT_USERNAME: ${MONGO_USER}
      MONGO_INITDB_ROOT_PASSWORD: ${MONGO_PASSWORD}
    volumes: [mongo_data:/data/db]
  redis:
    image: redis:7-alpine
    command:
      - redis-server
      - --appendonly
      - "yes"
      - --appendfsync
      - everysec
      - --requirepass
      - ${REDIS_PASSWORD}
      - --maxmemory
      - 256mb
      - --maxmemory-policy
      - noeviction
    volumes: [redis_data:/data]
    healthcheck:
      test: ["CMD", "redis-cli", "-a", "${REDIS_PASSWORD}", "ping"]
      interval: 10s
      timeout: 3s
      retries: 5
  minio:
    image: minio/minio
    command: server /data --console-address ":9001"
    environment:
      MINIO_ROOT_USER: ${MINIO_ROOT_USER}
      MINIO_ROOT_PASSWORD: ${MINIO_ROOT_PASSWORD}
    volumes: [minio_data:/data]
volumes:
  mongo_data:
  redis_data:
  minio_data:
  caddy_data:
```

- Mongo/Redis/MinIO 不映射宿主端口；只在 compose 网络内。
- Redis 必须设 `requirepass`；规模上来后再用 ACL 建 app 专用用户（`+@read +@write -@dangerous`，禁 `FLUSHALL/CONFIG/KEYS`）。
- Caddy 反代 `app:9999`，自动 HTTPS。
- 管理端 IP allowlist：

```
example.com {
  @admin_blocked {
    path /dsp.saves/api/admin/*
    not remote_ip 10.0.0.0/8 192.168.0.0/16 172.16.0.0/12
  }
  respond @admin_blocked 403

  reverse_proxy app:9999
}
```

> - SPA 静态页本身是公开的，但 `/admin/*` API 被 IP 拦截；未登录/非 admin 即使打开页面也调不通。若连管理页面也要隐藏，把管理端拆到独立域名或独立监听端口。
> - 如果前面还有一层代理，`remote_ip` 会变成代理 IP；要在 Caddy 配 `trusted_proxies` 或改用 `X-Forwarded-For` 判断。
> - 生产环境给 Mongo 开 auth：compose 里加 `MONGO_INITDB_ROOT_USERNAME/PASSWORD`，URI 改成 `mongodb://user:pass@mongo:27017/?authSource=admin`；不要用无认证的裸 Mongo。

### 8.2 备份 / 恢复

- 备份任务在 app 内（`tasks/backup.rs`），每天 03:00（配置时区）。
- 依赖镜像内的 `mongodump` / `mongorestore`（`mongodb-database-tools`）和 `tar`/`zstd`。
- 恢复必须停服，避免 Mongo 与文件不一致。
- 上线前做一次**恢复演练**：备份 → 删测试数据 → 恢复 → 校验用户/版本/文件。

### 8.3 已知风险

- 同机 MinIO 只防误删，不防主机/磁盘故障；已接受，预留 `mc mirror`。
- 腾讯企业邮免费版有日发信上限，超过后验证码发不出；监控 `mail_send_failed`，必要时切 SES（`Mailer` trait 换实现）。
- Redis 单点：故障时限流/防重放降级到本地、读缓存回源 Mongo；容量满时 `noeviction` 会让写失败，必须监控 `evicted_keys=0` 与 `used_memory`。
- 缓存不一致：只缓存可回源的数据；`tokenVersion` 变更即 `DEL`，不允许陈旧；列表/统计接受 30~60s 陈旧。
- 单实例重启会短暂中断；compose `restart: unless-stopped` 自动拉起。

---

## 9. 测试

### 9.1 国密互通

- Node `sm-crypto` 生成 1000 组随机向量（密码长度 1/15/16/17/31/32/64、中文、emoji），落 `tests/fixtures/sm-vectors.json`。
- Rust 逐条解密断言；再反向加密，Node 解密断言。
- 异常向量：缺 `04`、C1C2C3、奇数 hex、非 UTF-8、16 字节裸密钥、篡改 C3。
- 固定向量入库（不含生产私钥），CI 每次跑。

### 9.2 契约回归

- 用模组真实请求体做 `reqwest` 集成测试：init → 乱序/重传分片 → complete → file-list → 整文件/Range 下载 → 覆盖 → 版本列表。
- 断言响应字段名/类型/HTTP 状态码与旧版一致（密码字段语义除外）。

### 9.3 认证与邮件

- 注册：邮箱码错误/过期/超次数；邮箱已占用（验证码通过后）返回 409；用户名大小写冲突。
- 邮箱码防刷：同一邮箱 60s 内第二次 → 429 + `retryAfter`；5 次/小时、10 次/天；同一 IP 10 次/小时、30 次/天；同收件域 100 次/小时；全局 450 次/天；超限均不调用 SMTP。
- 验证码生命周期：5 分钟过期；5 次错误后作废；重发后旧码立即失效；成功后不可复用；`+alias` 计入同一邮箱限额。
- 邮箱枚举：注册发码对已存在/不存在邮箱返回同一响应；找回密码对不存在邮箱不发信但仍返回 200。
- 邮件失败：SMTP 超时 → 4001；连续失败 5 次 → 503 熔断；配额耗尽 → 503 `mail_quota_exhausted`；队列满 → 429。
- 密码策略：9 位/65 位/缺字母/缺数字/缺特殊字符/纯 Unicode 字母/包含用户名或邮箱前缀 → `3003` 且 `payload.missing` 对应项正确；字母+数字+特殊字符齐全 → 通过；注册、找回重置、本人改密、管理员重置四条路径结果一致。
- 登录：正确/错误/锁定/禁用；JWT 过期/伪造/`ver` 失效；refresh 轮换/重放。
- 找回：邮箱不存在返回统一消息；改密后旧 token 失效；管理员重置强制改密。
- 邮件：用 Mailpit 本地 SMTP 捕信；模板中英；SPF/DKIM 用 `mail-tester` 人工验一次。

### 9.4 版本与配额

- v1→v2→v3 覆盖；回退 v2；删除当前（回退 v2）；删除全部版本；配额刚好/超出 1 字节；管理员降低配额。
- 并发上传同名文件：seq 不重复，最终 current 为最后一个成功者。
- 崩溃恢复：上传中途杀进程，重启后 temp/孤儿版本被清理，`usedBytes` 对账正确。

### 9.5 安全用例

| 用例 | 期望 |
|---|---|
| 明文密码 | 401 / 2006 |
| 重放 smkey | 409 / 2004 |
| 时间戳 ±301s | 400 / 2003 |
| `../`、`..\\`、`%2e%2e`、NUL、`CON` | 400 / 3010 |
| 分片用他人 `uploadToken` | 403 |
| 普通用户访问 `/admin/*` | 403 |
| 用户 token 访问 `/admin/*` | 403 |
| 超配额 | 413 / 3004 |
| 连续错密码 | 423 / 1007 |
| 日志扫描 | 无 password/smkey/token 明文 |

### 9.6 Redis

- 限流 Lua 原子性：并发 100 次请求，放行数严格等于 limit，无超发。
- TTL：60s/1h/1d key 到点过期；`TTL` 返回值正确。
- 降级：停掉 Redis，登录/发码走本地限流；邮箱码回源 Mongo；读缓存回源；`/readyz` 为 `degraded`。
- 恢复：Redis 重启后 30s 内恢复；本地降级计数清零，不出现双倍限额。
- 缓存失效：改配额/禁用/上传/删除后，`user:profile`、`saves:list`、`stats` 立即 miss。
- 锁：两个并发覆盖同一文件，只有一个拿到 `lock:save`，另一个 409/重试。
- 幂等：重复 `complete`/`register` 只产生一次副作用。
- 持久化：AOF 重启后限流/防重放 key 仍在（允许丢失，但不能崩）。

### 9.7 性能

- 登录 p95 < 500ms（argon2 + Mongo）；
- 1000 文件列表 p95 < 200ms（命中 Redis 时 < 50ms）；
- 2GB 上传（5MB 分片、5 并发）内存 < 200MB；
- 50 并发下载吞吐接近磁盘上限；
- Redis 命令 p95 < 5ms，命中率 > 90%（热点 key）。

---

## 10. 里程碑

| 阶段 | 内容 | 产出 | 验收 |
|---|---|---|---|
| P0 | 决策确认、接口基线 | 本文档 | 评审通过 |
| P1（3~4d） | workspace + `sm-envelope` + keyring | 加密 crate | 1000 组双向向量通过 |
| P2（5~6d） | Mongo 集合/索引 + Redis 连接/限流 Lua + repo + CLI | 数据层 | 建库幂等、Redis 限流原子、CLI 可用 |
| P3（6~7d） | 注册/登录/JWT/邮箱码/限流/审计 | `/auth/*` | 认证与邮件用例通过 |
| P4（6~7d） | 模组接口 + 上传会话 + 版本 + 配额 | `/upload/*`、`/download/*` | 契约回归 + 版本用例 |
| P5（5~6d） | 用户门户/管理端 API | `/user/*`、`/admin/*` | 接口测试通过 |
| P6（7~9d） | SPA + i18n + 版本时间线 | 可用的前端 | 中英 100%、E2E 通过 |
| P7（3~4d） | C# 模组国密改造 | 新插件 | 实机上传/下载 |
| P8（3~4d） | Docker/Caddy/MinIO/备份恢复 | 部署包 | 恢复演练通过 |
| P9（4~5d） | 压测、加固、日志脱敏、渗透自查 | 测试报告 | 基线达标 |

合计约 **42~52 人日**；两人并行（后端/前端）约 5~6 周。

---

## 11. 风险与对策

| 风险 | 概率 | 影响 | 对策 |
|---|---|---|---|
| 邮箱验证码被刷/邮件轰炸 | 高 | 高 | 邮箱 60s/5h/10d + IP 10h/30d + 域 100h + 全局 450d + 有界队列 + PoW step-up + 一次性邮箱黑名单 |
| 邮箱验证码发不出/进垃圾箱 | 中 | 高 | SPF/DKIM/DMARC + 企业邮 + 监控失败率 + 预留 SES 切换 |
| 无限版本撑爆磁盘 | 中 | 高 | 配额强制 + UI 显示版本占用 + 磁盘余量保护 + 管理员可降配额 |
| 同机 MinIO 随主机一起丢 | 中 | 高 | 已知风险；预留 `mc mirror`，后续加机器 |
| Mongo 单机无事务 | 中 | 中 | 单文档原子 + 幂等补偿 + 定时 reconcile |
| Redis 单点/内存满 | 中 | 中 | `noeviction` + 256MB + AOF + 本地降级 + 监控 `evicted_keys`/内存；Redis 不作事实源 |
| SM2/SM4 互通踩坑 | 中 | 高 | 先做 `sm-envelope` + 双向向量，再写业务 |
| SM2 私钥泄露 | 低 | 高 | 0600、可选口令、备份排除、轮换预案 |
| Caddy IP allowlist 配错 | 中 | 高 | 部署后从外网/内网各测一次 `/admin` |
| C# BouncyCastle 兼容性 | 低 | 中 | 用固定向量在 Unity 实机验证 |
| 配额统计漂移 | 中 | 中 | 每 10 分钟 reconcile + 审计 |
| 用户忘记密码且邮箱不可用 | 中 | 低 | 管理员重置兜底 |

---

## 12. 上线前置条件

1. 域名 + DNS 控制权（Caddy 证书、企业邮 MX/SPF/DKIM）。
2. 腾讯企业邮免费版已开通，`no-reply@domain` 账号可 SMTP 登录。
3. MinIO endpoint / bucket / access key / secret。
4. 服务器磁盘容量（决定 1GB × 用户数的实际上限）。
5. 初始 admin 用户名（CLI 创建）。
6. C# 插件发布渠道（决定强制升级提示）。
7. 管理端访问的 VPN/内网 CIDR。
8. Redis 密码（compose 的 `REDIS_PASSWORD`）与内存上限（默认 256MB）确认。

---

## 附录 A：配置示例

```toml
[server]
bind = "0.0.0.0:9999"
base_path = "/dsp.saves/api"
trusted_proxy_cidrs = ["172.16.0.0/12"]

[database]
uri = "mongodb://mongo:27017"
db = "DSPCloudSaves"

[redis]
enabled = true
url = "redis://:${REDIS_PASSWORD}@redis:6379/0"
prefix = "dsp:prod:v1:"
pool_size = 16
connect_timeout_ms = 1000
command_timeout_ms = 500

[redis.ttl]
replay_seconds = 600
token_version_seconds = 30
user_cache_seconds = 30
file_list_seconds = 30
stats_seconds = 60

[auth]
access_ttl_minutes = 120
refresh_ttl_days = 7
jwt_secret_file = "./data/jwt.secret"
admin_idle_minutes = 30
argon2_memory_kib = 19456
argon2_iterations = 2
argon2_parallelism = 1

[crypto]
key_dir = "./data/keys"
key_grace_hours = 24
passphrase_env = "SM2_KEY_PASSPHRASE"

[storage]
saves_dir = "./saves"
max_file_size = 2147483648
max_chunk_size = 67108864
max_total_chunks = 20000
min_free_bytes = 2147483648

[quota]
default_bytes = 1073741824

[email_code]
length = 6
ttl_seconds = 300
resend_seconds = 60
max_attempts = 5
per_email_per_hour = 5
per_email_per_day = 10
per_ip_per_hour = 10
per_ip_per_day = 30
per_domain_per_hour = 100
global_per_hour = 100
global_per_day = 450
queue_capacity = 100

[mail]
enabled = true
smtp_host = "smtp.exmail.qq.com"
smtp_port = 465
smtp_tls = "implicit"
from_address = "no-reply@example.com"
from_name = "DSP 云存档"
username = "no-reply@example.com"
password = "${SMTP_PASSWORD}"
daily_limit = 500

[backup]
enabled = true
cron = "0 3 * * *"
retention_days = 30
encrypt = false
bucket = "dsp-saves-backup"

[backup.minio]
endpoint = "http://minio:9000"
access_key = "${MINIO_ACCESS_KEY}"
secret_key = "${MINIO_SECRET_KEY}"
region = "us-east-1"
use_path_style = true

[log]
level = "info"
format = "json"
```

## 附录 B：集合初始化（mongosh）

```js
db.createCollection("users", {
  validator: { $jsonSchema: {
    bsonType: "object",
    required: ["userName", "userNameLower", "passwordHash", "role", "status",
               "quotaBytes", "usedBytes", "reservedBytes", "tokenVersion",
               "createdAt", "updatedAt"],
    properties: {
      userName: { bsonType: "string", maxLength: 32 },
      userNameLower: { bsonType: "string" },
      email: { bsonType: ["string", "null"] },
      emailLower: { bsonType: ["string", "null"] },
      role: { enum: ["admin", "user"] },
      status: { enum: ["active", "disabled", "deleted"] },
      quotaBytes: { bsonType: "long", minimum: 0 },
      usedBytes: { bsonType: "long", minimum: 0 },
      reservedBytes: { bsonType: "long", minimum: 0 },
      tokenVersion: { bsonType: "int", minimum: 0 }
    }
  } }
});

db.users.createIndex({ userNameLower: 1 }, { unique: true });
db.users.createIndex({ emailLower: 1 }, {
  unique: true,
  partialFilterExpression: { emailLower: { $type: "string" } }
});
db.users.createIndex({ status: 1, createdAt: -1 });

db.sessions.createIndex({ userId: 1 });
db.sessions.createIndex({ expiresAt: 1 }, { expireAfterSeconds: 0 });

db.upload_sessions.createIndex({ userId: 1 });
db.upload_sessions.createIndex({ expiresAt: 1 }, { expireAfterSeconds: 0 });

db.saves.createIndex({ userId: 1, fileName: 1 }, { unique: true });
db.saves.createIndex({ userId: 1, updatedAt: -1 });

db.save_versions.createIndex({ saveId: 1, seq: -1 });
db.save_versions.createIndex({ userId: 1, createdAt: -1 });

db.email_codes.createIndex({ expiresAt: 1 }, { expireAfterSeconds: 0 });

db.audit_logs.createIndex({ createdAt: -1 }, { expireAfterSeconds: 15552000 });
db.audit_logs.createIndex({ actorId: 1, createdAt: -1 });
db.audit_logs.createIndex({ action: 1, createdAt: -1 });
```

## 附录 C：新旧接口对照

| 旧 | 新 | 备注 |
|---|---|---|
| `POST /users/create` | `POST /admin/users` / `POST /auth/register` | 管理端建号 / 自助注册 |
| `POST /users/login` | `POST /auth/login` | JWT |
| `POST /users/logout` | `POST /auth/logout` | 吊销 refresh |
| `POST /users/update` | `POST /auth/password` | 本人改密 |
| `POST /users/delete` | `DELETE /admin/users/{id}` | 管理员 |
| `POST /upload/init` | 不变 | 密文密码 + uploadToken |
| `POST /upload/chunk` | 不变 | uploadToken |
| `POST /upload/complete` | 不变 | 密文密码 |
| `POST /download/file-list` | 不变 | 密文密码，只返回当前版本 |
| `POST /download/download` | 不变 | 密文密码 |
| — | `/auth/email-code` | 新增 |
| — | `/user/saves/*`、`/admin/*` | 新增门户/管理面 |
| — | `/healthz`、`/readyz`、`/metrics` | 新增运维面 |

## 附录 D：默认参数

| 项 | 默认 |
|---|---|
| 新账号配额 | 1GB |
| 单文件上限 | 2GB |
| 分片大小 | 5~64MB |
| 分片数上限 | 20,000 |
| 软件版本 | 不限制；计入配额 |
| 密码策略 | 10~64 位；英文字母 + 数字 + 特殊字符三类齐全；不含用户名/邮箱前缀 |
| 邮箱码 | 6 位，5 分钟，5 次尝试，60 秒重发；邮箱 5/小时、10/天；IP 10/小时、30/天 |
| 注册限流 | 同 IP 3 账号/天 |
| 登录锁定 | 5 次失败/10 分钟，锁 15 分钟 |
| 会话 | access 2h，refresh 7d |
| Redis | 7 单实例，256MB，`noeviction`，AOF everysec；key 前缀 `dsp:prod:v1:` |
| 缓存 TTL | 防重放 600s；tokenVersion 30s；用户/文件列表 30s；统计 60s |
| 审计保留 | 180 天 |
| 备份保留 | 30 天 |
| 语言 | zh-CN / en-US |

---

### 落地顺序（一句话）

先做 `sm-envelope` 并用 1000 组双向向量锁死互通 → 再建 Mongo 集合/索引 + Redis 限流/Lua + CLI → 然后认证（邮箱码 + JWT + 防刷层）→ 模组接口 + 版本/配额 → 管理端与用户门户 → 最后 Caddy/MinIO/备份与恢复演练。
