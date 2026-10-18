# DSP 云存档服务（Rust / axum）

`services/acc-rs` 是全新的云存档后端，**与旧的 `services/acc` 完全独立**：Rust + axum + MongoDB + Redis，
内置国密密码传输、存档版本、配额、审计、备份，以及 Vue 3 管理端 / 用户门户。

## 目录

```text
services/acc-rs/
├── crates/
│   ├── sm-envelope/        # SM2/SM4 密码信封 + 密钥环（含 1000+ 互通向量测试）
│   ├── acc-server/         # HTTP 服务 + 后台任务 + 内嵌前端
│   └── acc-cli/            # 运维 CLI（建管理员、对账、备份/恢复、密钥轮换）
├── web/                    # Vue 3 + Element Plus + Pinia + vue-i18n
├── tests/
│   ├── fixtures/           # JS/Rust/C# 三方国密向量
│   └── interop/            # 向量生成、双向校验、端到端冒烟脚本
├── deploy/Caddyfile        # 自动 HTTPS + 管理端来源限制
├── Dockerfile
├── docker-compose.yml
├── config.example.toml
└── .env.example
```

## 本地开发

前置：Rust（stable）、Node 20+、MongoDB 7+；Redis 可选（不可用时自动降级为进程内实现）。

```bash
cd services/acc-rs

# 1) 前端
npm --prefix web install
npm --prefix web run build

# 2) 配置
cp config.example.toml config.dev.toml   # 本地可关掉 redis / mail / backup，并把 cookie_secure=false
export ACC_CONFIG=config.dev.toml

# 3) 建管理员（密码必须 10~64 位且含字母、数字、特殊字符）
cargo run -p acc-cli -- --config config.dev.toml admin create \
  --username admin --password '<你的管理员密码>' --email admin@example.com

# 4) 启动
cargo run -p acc-server -- --config config.dev.toml
```

打开 <http://127.0.0.1:9999>：门户用用户名密码登录；管理端在登录页切到「管理控制台」，
或访问 `/login?admin=1`（生产环境该接口由 Caddy 限制为内网）。

前端热更新：`npm --prefix web run dev`，Vite 已把 `/dsp.saves/api` 代理到 `127.0.0.1:9999`。

## 测试

```bash
# Rust：国密互通（1017 组 JS→Rust）、密钥环、口令加密、密码策略、IP/网段
cargo test --workspace

# 协议层冒烟（需服务已启动）：30 项，覆盖登录/重放/时钟/明文拒绝/上传/版本/回退/删除/审计
node tests/interop/smoke.mjs

# 反向互通：Rust → JS、C# → JS
node tests/interop/verify-rust.mjs
node tests/interop/verify-csharp.mjs

# 前端类型与双语 key 一致性
npm --prefix web run type-check
npm --prefix web run i18n:check
```

## 生产部署

```bash
cp .env.example .env        # 填域名、Mongo 密码、SMTP、MinIO、SM2 口令
docker compose up -d --build
docker compose exec app acc-cli --config /app/config.toml admin create \
  --username admin --password '<强密码>' --email admin@example.com
```

- 仅 Caddy 暴露 80/443；app/mongo/redis/minio 只在 compose 内网。
- `ADMIN_ALLOW_CIDRS` 控制 `/dsp.saves/api/admin/*` 的来源网段，默认内网 + 回环。
- 数据卷：`./data`（JWT/服务密钥、SM2 keyring、备份临时文件）、`./saves`（存档与版本）、
  `mongo-data`、`redis-data`、`minio-data`。
- 备份：每日 `03:00`（容器本地时区 Asia/Shanghai）把 `mongodump` 与 `saves` 归档上传到 MinIO，
  保留 30 天；`acc-cli backup restore --date YYYY-MM-DD` 可恢复。
- 健康检查：`/healthz`、`/readyz`、`/metrics`（Prometheus 文本）。

## 接口一览

统一前缀 `/dsp.saves/api`。

| 类别 | 路径 | 说明 |
|---|---|---|
| 公钥 | `GET /crypto/public-key` | `keyId` / `publicKey` / 模式 / 服务器时间 |
| 认证 | `POST /auth/email-code`、`/auth/register`、`/auth/login`、`/auth/refresh`、`/auth/logout` | 邮箱验证码、注册、登录、刷新、登出 |
| 认证 | `POST /auth/password`、`/auth/password/reset/request`、`/auth/password/reset` | 改密、找回 |
| 认证 | `GET/PATCH /auth/me`、`POST /auth/email/change/request`、`/auth/email/change` | 资料、语言、换邮箱 |
| 门户 | `GET/PUT/DELETE /user/saves/{name}`、`GET .../versions`、`POST .../versions/{id}/rollback`、`GET .../versions/{id}/download`、`GET /user/stats` | 自己的存档 |
| 管理 | `POST /admin/auth/login`、`/admin/auth/refresh`、`/admin/auth/logout` | 管理端会话（`aud=admin`） |
| 管理 | `GET/POST /admin/users`、`GET/PATCH/DELETE /admin/users/{id}`、`POST /admin/users/{id}/reset-password` | 用户 |
| 管理 | `GET /admin/users/{id}/saves`、`.../versions`、`POST .../rollback`、`DELETE .../{name}`、`GET /admin/saves` | 存档 |
| 管理 | `GET /admin/stats/overview`、`/admin/audit`、`/admin/audit/export.csv`、`GET/PUT /admin/settings`、`POST /admin/crypto/rotate` | 概览/审计/设置/密钥 |
| 模组 | `POST /upload/init`、`/upload/chunk`、`/upload/complete`、`/download/file-list`、`/download/download` | 旧客户端契约 |

模组契约（响应结构不可改）：

- `init` → `payload.data.{fileId, chunkSize, totalChunks, uploadToken}`
- `chunk` → 请求头 `uploadToken`，不再携带密码
- `complete` / `file-list` / `download` → `userName` + 密文 `password` + `smkey` / `smkeyid` 头
- 错误统一 `{ code: 1, message }`

## 国密约定（与实现校正）

- SM2 曲线 `sm2p256v1`，C1C3C2；SM2 加密的明文是 **SM4 密钥的 32 位 hex 字符串**。
- SM4 使用 **ECB + PKCS#7**，密钥为该 hex 解码后的 16 字节。
- **C1 前缀**：`sm-crypto` 的 `doEncrypt` 会去掉 `04` 未压缩点前缀；BouncyCastle / RustCrypto 会保留。
  服务端两种都接受（先按原样解，失败后补/去 `04` 重试），因此 JS、C#、Rust 客户端可混用。
- 每个业务请求带 `timestamp`（±300s）与 `nonce`，服务端用 `SM3(nonce||smkey)` 做 600s 防重放。
- SM2 私钥由服务端生成，`keyring.json` + `sm2-<keyId>.priv.pem`（0600，可选 Argon2id+ChaCha20 口令加密）；
  keyId 轮换后旧密钥按 `key_grace_hours` 保留，未知 keyId 返回 `2005`，客户端刷新公钥重试。

## 安全要点

- 密码只以密文入站；argon2id 存储；登录失败 5 次/10 分钟锁 15 分钟；未知用户也跑一次 hash 抹平时序。
- access JWT 2h（HS256，`aud=user|admin`），refresh 7d 随机串仅存 SHA-256，HttpOnly + SameSite=Strict；
  登出拉黑 jti，改密/禁用/重置递增 `tokenVersion` 即时吊销。
- 管理端与门户受众分离：门户 token 无法调用 `/admin/*`。
- 上传：`init` 原子预留配额并签发一次性 `uploadToken`；`chunk` 只认 token；`complete` 以磁盘分片为准校验。
- 版本：每次覆盖生成新版本（完整文件），删当前自动回退上一版，勾选删除则清空所有版本；全部计入配额。
- 审计保留 180 天（Mongo TTL），管理端可筛选并导出 CSV（带 BOM）。

## 已知限制

- 单实例部署（无多文档事务）；崩溃窗口由启动清理 + 每 10 分钟配额对账兜底。
- 备份归档含 SM2 私钥（`data/keys` 不在归档内，仅 Mongo + saves）；`backup.encrypt` 预留未实现。
- `min_free_bytes` 默认 2GB，磁盘不足时上传直接 `3009`。
- 前端为单页应用，Element Plus 全量引入（约 1.2MB JS，gzip 约 414KB）。

## 后台上传器（游戏退出后继续上传）

```bash
# 构建插件 DLL（游戏内，Mono 运行）
dotnet build DSPRemoteSaves.csproj -p:BepInExCoreDir="<含 BepInEx.dll 的目录>" -p:ManagedDir="<含 UnityEngine dll 的目录>"

# 构建后台上传器（self-contained 单文件，无需玩家安装 .NET）
dotnet publish Uploader/DSPSavesUploader.csproj -c Release -r win-x64 --self-contained true -p:PublishSingleFile=true -o Uploader/publish

# 部署：把 DSPRemoteSaves.dll 与 DSPSavesUploader.exe 一起放进 BepInEx/plugins/
```

- 退出时未传完的存档由插件写任务清单（plugins/DSPRemoteSaves/pending-upload-*.json）并启动上传器进程，游戏立即退出。
- 配置项 `[Performance] ExitBackgroundUpload`（默认 true）控制该行为；日志见 plugins/DSPRemoteSaves/uploader.log。

