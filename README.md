# DSPRemoteSaves —— 戴森球计划云存档

戴森球计划（Dyson Sphere Program）的 BepInEx 云存档插件 + 全新 Rust 后端，采用国密算法（SM2/SM3/SM4）保护账号密码传输，支持多版本存档、网页管理端与用户门户。

## 功能

- **游戏内同步**：运行时监控存档变化自动上传，退出时未传完的存档转交独立进程后台上传，下次启动双向补齐
- **版本管理**：每次上传形成版本链，支持回退与彻底删除
- **用户系统**：邮箱验证码注册/找回，Argon2id 密码哈希，JWT + tokenVersion 即时吊销
- **安全**：密码国密信封传输（SM2 信封 + SM4-ECB）、请求防重放、分片上传一次性令牌、限流
- **管理端**：用户管理、配额调整、全局存档检索、审计日志、密钥轮换
- **备份**：每日自动把 MongoDB dump 与存档归档上传 MinIO，保留 30 天
- **后台上传器**：游戏退出后由独立进程继续完成上传（`Uploader/`）

## 仓库结构

```
DSPRemoteSaves.cs        游戏插件主体（BepInEx 5，.NET）
SmCrypto.cs              插件内自包含国密实现（SM2/SM3/SM4，零外部依赖）
Uploader/                独立后台上传器（net9 self-contained 单文件）
services/acc-rs/         全新后端（Rust/axum + MongoDB/Redis + 内嵌 Vue3 前端）
  ├─ crates/sm-envelope  国密库与密钥环
  ├─ crates/acc-server   服务端（API/任务/网页）
  ├─ crates/acc-cli      运维 CLI（建管理员/对账/备份恢复/密钥轮换）
  └─ web                 管理端与用户门户（Vue3 + Element Plus，中英双语）
docs/DEPLOYMENT.md       自部署完整流程（公网+内网 FRP 架构）
docs/rust-rewrite-*.md   设计方案与实施校正
```

## 快速开始

1. **玩家**：把 `DSPRemoteSaves.dll`（与可选的 `DSPSavesUploader.exe`）放入 `BepInEx/plugins/`，在 `BepInEx/config/com.aims.remotesaves.cfg` 填写服务器地址（如 `http://host:9999`，**不带** API 路径）、账号密码
2. **服务端自部署**：见 [`docs/DEPLOYMENT.md`](docs/DEPLOYMENT.md)
3. **开发构建**：

```bash
# 插件
dotnet build DSPRemoteSaves.csproj -p:BepInExCoreDir="<含 BepInEx.dll 的目录>" -p:ManagedDir="<含 UnityEngine dll 的目录>"
# 后端（Rust）
cd services/acc-rs && cargo build -p acc-server -p acc-cli
# 后端前端
cd services/acc-rs/web && npm install && npm run build
```

## 测试

```bash
cd services/acc-rs && cargo test --workspace          # 国密互通向量（sm-crypto 生成）+ 单元测试
node tests/interop/smoke.mjs                           # 30 项协议冒烟（需 SMOKE_PASS 环境变量）
node tests/interop/verify-rust.mjs                     # Rust→JS 反向互通
dotnet run --project tests/csharp-interop -- e2e <BASE> <USER> <PASS>   # C# 端到端
```
