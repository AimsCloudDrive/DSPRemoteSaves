# DSPRemoteSaves —— 戴森球计划云存档

戴森球计划（Dyson Sphere Program）的 BepInEx 云存档插件 + 全新 Rust 后端，采用国密算法（SM2/SM3/SM4）保护账号密码传输，支持多版本存档、网页管理端与用户门户。

## 功能

- **游戏内手动同步**：按 `F8`（或 `F1` → 云存档同步面板）打开定稿面板，逐行选择上传/下载；行内显示进度、剩余时间并可随时取消。**不再在启动/退出时自动传输**
- **版本管理**：每次上传形成版本链，支持回退与彻底删除
- **用户系统**：邮箱验证码注册/找回，Argon2id 密码哈希，JWT + tokenVersion 即时吊销
- **安全**：密码国密信封传输（SM2 信封 + SM4-ECB）、请求防重放、分片上传一次性令牌、限流
- **管理端**：用户管理、配额调整、全局存档检索、审计日志、密钥轮换
- **备份**：每日自动把 MongoDB dump 与存档归档上传 MinIO，保留 30 天
- **旧版后台上传器**：`Uploader/` 保留用于历史任务文件，2.2.0 起插件不再自动调用

## 仓库结构

```
DSPRemoteSaves.cs        游戏插件主体（BepInEx 5，.NET）
SmCrypto.cs              插件内自包含国密实现（SM2/SM3/SM4，零外部依赖）
Uploader/                旧版独立后台上传器（2.2.0 起不再由插件自动调用）
services/acc-rs/         全新后端（Rust/axum + MongoDB/Redis + 内嵌 Vue3 前端）
  ├─ crates/sm-envelope  国密库与密钥环
  ├─ crates/acc-server   服务端（API/任务/网页）
  ├─ crates/acc-cli      运维 CLI（建管理员/对账/备份恢复/密钥轮换）
  └─ web                 管理端与用户门户（Vue3 + Element Plus，中英双语）
docs/DEPLOYMENT.md       自部署完整流程（公网+内网 FRP 架构）
docs/rust-rewrite-*.md   设计方案与实施校正
```

## 快速开始

1. **玩家**：把 `DSPRemoteSaves.dll` 放入 `BepInEx/plugins/`，在 `BepInEx/config/com.aims.remotesaves.cfg` 填写服务器地址（如 `http://host:9999`，**不带** API 路径）、账号密码；进入游戏后按 `F8` 打开面板
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

## 模组配置

配置文件：`BepInEx/config/com.aims.remotesaves.cfg`（首次启动自动生成，缺项自动补写）。

| 配置项 | 默认值 | 说明 |
| --- | --- | --- |
| `General` / `PanelHotkey` | `F8` | 打开/关闭云存档面板的快捷键 |
| `Server` / `Remote ServerURL` | 空 | 服务器地址，**不带** API 路径 |
| `User` / `Username`、`Password` | 空 | 账号密码（密码国密封装传输） |
| `Performance` / `SyncFileExtensions` | `.dsv,.moddsv,.server` | 面板中显示的文件扩展名 |
| `Performance` / `MaxParallelUploadCount` | `3` | 同时上传的存档数量上限，超出排队 |
| `Performance` / `MaxParallelDownloadCount` | `5` | 同时下载的存档数量上限，超出排队 |
| `Performance` / `ChunkThreshold(MB)` | `5` | **分片阈值**：不超过该大小的文件整块上传（只有 1 个分片），超过才按 `ChunkSize(MB)` 切分；`0` = 始终分片；上限 64（服务端单分片上限） |
| `Performance` / `ChunkSize(MB)` | `5` | 单个分片大小 |
| `Performance` / `MaxChunkParallelUploadCount` | `5` | 单文件并行分片数 |
| `Performance` / `AutoRefresh(s)` | `30` | 面板打开时的自动刷新间隔，`0` = 不自动刷新 |

## 游戏内面板

- **入口**：`F8`；安装 BepInEx Configuration Manager 后也可在 `F1` → `General` → `云存档同步面板` 中打开（入口使用自定义绘制器显示连接状态与传输计数）。
- **方向判定**：同时存在时按“文件大小 + 修改时间”判断；大小相同且时间在 2 秒容差内显示“已一致”，否则以较新的一方为主操作，另一方向仍可手动执行。
- **覆盖保护**：下载覆盖本地前自动备份到 `%LOCALAPPDATA%\DSPRemoteSaves\backup\<时间戳>\`。
- **取消**：分片上传与流式下载都响应取消；上传取消后服务端会话按 TTL 清理。
- **上传后对齐**：服务端以提交时间记录 mtime，插件上传成功后会把本地 mtime 对齐到云端，避免刷新后误判为“云端较新”。

## 测试

```bash
cd services/acc-rs && cargo test --workspace          # 国密互通向量（sm-crypto 生成）+ 单元测试
node tests/interop/smoke.mjs                           # 30 项协议冒烟（需 SMOKE_PASS 环境变量）
node tests/interop/verify-rust.mjs                     # Rust→JS 反向互通
dotnet run --project tests/csharp-interop -- e2e <BASE> <USER> <PASS>   # C# 端到端
```
