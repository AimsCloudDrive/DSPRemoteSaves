# DSP 云存档 —— 自部署完整流程

本文档描述从零开始在一套「公网服务器 + 内网服务器」环境上自部署 DSP 云存档的完整流程。
所有凭据均以 `<占位符>` 表示，请替换为你自己的值。

## 1. 架构总览

```
玩家（DSP 游戏 / 浏览器）
        │
        ▼
公网服务器 <PUBLIC_IP>:9999        ← FRP 服务端 (frps) 监听
        │  FRP 隧道
        ▼
内网服务器 9999                    ← 后端 dsp-saves-app（Docker 容器）
        │  直连公网 IP
        ├──▶ MongoDB      <PUBLIC_IP>:27017   （账号/存档/审计元数据）
        ├──▶ Redis        <PUBLIC_IP>:6379    （缓存/限流/防重放）
        ├──▶ MinIO        <PUBLIC_IP>:9000    （每日备份对象存储）
        └──▶ 本地磁盘     <SAVES_DIR>         （存档文件本体）
```

- 后端跑在内网，数据库与对象存储在公网，方向为「内网 → 公网」的出站连接。
- 公网只暴露 9999（业务端口）；27017/6379/9000 需要对内网出口 IP 保持开放。

## 2. 前置要求

| 项 | 公网服务器 | 内网服务器 |
|---|---|---|
| 系统 | 任意 Linux（示例用 Debian） | 任意 Linux（示例用 Ubuntu 24.04） |
| 必须 | Docker、1Panel（可选）、MongoDB/Redis/MinIO 容器、frps | Docker、frpc |
| 建议 | ≥2G 内存 | ≥4G 内存 |

> 数据库与 MinIO 若使用 1Panel 部署，容器应在同一个 Docker 网络内，密码在对应 `.env` 中查看。

## 3. 公网侧：基础设施

### 3.1 MongoDB / Redis / MinIO

用 1Panel 或 docker compose 各起一个实例，并确认：

- MongoDB root 账号密码（示例用户 `mongo`），认证库 `admin`
- Redis 设置 `requirepass`
- MinIO 有 root 账号；为应用创建**专用备份桶与最小权限账号**（不要把 root 给应用）：

```bash
# 在 MinIO 容器内执行（mc 已内置）
mc alias set local http://127.0.0.1:9000 <MINIO_ROOT_USER> <MINIO_ROOT_PASSWORD>
mc mb --ignore-existing local/<BACKUP_BUCKET>
# 策略：仅允许该桶 Get/Put/Delete/List
mc admin policy create local dsp-backup-rw /tmp/policy.json
mc admin user add local <MINIO_APP_USER> <MINIO_APP_PASSWORD>
mc admin policy attach local dsp-backup-rw --user <MINIO_APP_USER>
```

### 3.2 frps（FRP 服务端）

`frps.toml`（Docker 挂载到容器 `/etc/frp/frps.toml`）：

```toml
bindAddr = "0.0.0.0"
bindPort = 60000

auth.method = "token"
auth.token = "<FRP_TOKEN>"

webServer.addr = "0.0.0.0"
webServer.port = 60001
webServer.user = "<DASHBOARD_USER>"
webServer.password = "<DASHBOARD_PASSWORD>"
```

容器端口映射：`60000/tcp`（frp 通道）、`60001/tcp`（dashboard，可选）、`9999/tcp`（业务入口）。
用 1Panel 或 systemd 保证开机自启与崩溃自拉。

## 4. 内网侧：后端部署

### 4.1 目录规划

```
/opt/dsp-saves/app      # 配置与 compose（config.server.toml / .env.server / docker-compose.server.yml）
/opt/dsp-saves/data     # SM2 密钥环、JWT/服务密钥、备份工作区（挂载到容器 /app/data）
<SAVES_DIR>             # 存档文件本体（挂载到容器 /app/saves）
/opt/frp/frpc_dsp.toml  # frpc 映射配置
```

### 4.2 获取镜像（二选一）

**方式 A：从已构建的机器传输**（推荐，内网机器免编译）

```bash
# 在构建机（已 docker build）上打包
docker save dsp-saves:2.0.2 | gzip > dsp-saves.tar.gz
# 传输到内网机器后
docker load -i dsp-saves.tar.gz
```

**方式 B：内网直接构建**（需要 Node 22 与 Rust 工具链）

```bash
cd services/acc-rs
docker build -t dsp-saves:2.0.2 .          # 可加 --build-arg CARGO_BUILD_JOBS=1 防小内存 OOM
```

### 4.3 配置文件

**`config.server.toml`**（要点，完整模板见仓库 `config.example.toml`）：

```toml
[server]
bind = "0.0.0.0:9999"
base_path = "/dsp.saves/api"
public_url = "http://<PUBLIC_IP>:9999"        # 对外地址，决定 Cookie/来源校验

[database]
uri = "mongodb://<DB_USER>:<DB_PASSWORD>@<PUBLIC_IP>:27017/?authSource=admin&directConnection=true"
db = "DSPCloudSaves"

[redis]
enabled = true
url = "redis://:<REDIS_PASSWORD>@<PUBLIC_IP>:6379/0"

[auth]
cookie_secure = false                          # 未上 HTTPS 前保持 false
jwt_secret_file = "/app/data/jwt.secret"
server_secret_file = "/app/data/server.secret"

[crypto]
key_dir = "/app/data/keys"
passphrase_env = "SM2_KEY_PASSPHRASE"          # 私钥口令在 .env.server，丢失后旧私钥不可解

[storage]
saves_dir = "/app/saves"

[mail]                                         # 验证码邮件（QQ 个人邮箱示例）
enabled = true
smtp_host = "smtp.qq.com"
smtp_port = 465
smtp_tls = "implicit"
from_address = "<QQ_MAILBOX>"
username = "<QQ_MAILBOX>"
password = "<QQ_SMTP_AUTH_CODE>"               # 授权码，不是 QQ 密码

[backup]
enabled = true                                 # 每日 03:00 备份到 MinIO
bucket = "<BACKUP_BUCKET>"
[backup.minio]
endpoint = "http://<PUBLIC_IP>:9000"
access_key = "<MINIO_APP_USER>"
secret_key = "<MINIO_APP_PASSWORD>"
```

**`.env.server`**（容器环境变量，含密钥，**严禁提交 git**）：

```ini
SM2_KEY_PASSPHRASE=<随机长口令>
COOKIE_SECURE=false
```

**`docker-compose.server.yml`**：

```yaml
name: dsp-saves
services:
  app:
    image: dsp-saves:2.0.2
    container_name: dsp-saves-app
    restart: unless-stopped          # 开机自启 + 崩溃自拉
    env_file: .env.server
    environment:
      TZ: Asia/Shanghai
    ports:
      - "127.0.0.1:9999:9999"        # 只绑本机回环，由 frpc 转发
    volumes:
      - <SAVES_DIR>:/app/saves
      - /opt/dsp-saves/data:/app/data
      - ./config.server.toml:/app/config.toml:ro
    healthcheck:
      test: ["CMD", "curl", "-fsS", "http://127.0.0.1:9999/healthz"]
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 20s
    logging:
      driver: json-file
      options: { max-size: "20m", max-file: "5" }
```

启动：

```bash
mkdir -p <SAVES_DIR> /opt/dsp-saves/data
docker compose -f docker-compose.server.yml up -d
curl -s http://127.0.0.1:9999/healthz      # {"status":"ok",...}
curl -s http://127.0.0.1:9999/readyz       # mongo/redis 均为 true
```

### 4.4 frpc 映射与自启

`/opt/frp/frpc_dsp.toml`：

```toml
serverAddr = "<PUBLIC_IP>"
serverPort = 60000
auth.token = "<FRP_TOKEN>"

[[proxies]]
name = "dsp-saves-web"
type = "tcp"
localIP = "127.0.0.1"
localPort = 9999
remotePort = 9999
```

`/etc/systemd/system/frpc_dsp.service`：

```ini
[Unit]
Description=frp client for DSP cloud saves forwarding
After=network.target

[Service]
Type=simple
User=frp
Group=frp
WorkingDirectory=/opt/frp
ExecStart=/opt/frp/frpc -c /opt/frp/frpc_dsp.toml
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```bash
systemctl daemon-reload && systemctl enable --now frpc_dsp.service
journalctl -u frpc_dsp.service | tail      # 应出现 start proxy success
```

## 5. 初始化

```bash
# 创建管理员（容器内）
docker exec dsp-saves-app acc-cli --config /app/config.toml \
  admin create --username admin --password '<强密码>' --email <admin@example.com>
```

- 网页端注册/找回密码依赖验证码邮件，`[mail].enabled = true` 后自动可用。
- 首次启动会自动生成 SM2 密钥环（口令加密）与 JWT/服务密钥；**务必备份 `/app/data` 目录**，私钥口令丢失将无法解密后续密码信封。

## 6. 验证清单

| 检查 | 命令/操作 | 期望 |
|---|---|---|
| 健康探针 | `curl http://127.0.0.1:9999/healthz` | `{"status":"ok"}` |
| 就绪探针 | `curl http://127.0.0.1:9999/readyz` | mongo/redis 均 true |
| 公网穿透 | 浏览器访问 `http://<PUBLIC_IP>:9999/` | 登录页正常 |
| 邮件 | 网页注册页发送验证码 | 收到验证码邮件 |
| 上传下载 | 游戏内同步或协议冒烟脚本 | 版本落库、内容一致 |
| 备份 | `docker exec dsp-saves-app acc-cli --config /app/config.toml backup run` | MinIO 出现对象 |

## 7. 日常运维

```bash
docker logs dsp-saves-app --tail 100        # 应用日志
docker compose -f docker-compose.server.yml restart   # 重启
docker compose -f docker-compose.server.yml up -d     # 配置变更后应用
docker exec dsp-saves-app acc-cli --config /app/config.toml backup run     # 立即备份
docker exec dsp-saves-app acc-cli --config /app/config.toml backup restore --date YYYY-MM-DD   # 恢复（先停服）
docker exec dsp-saves-app acc-cli --config /app/config.toml key rotate    # SM2 密钥轮换（旧密钥 24h 宽限）
```

更新版本：构建新镜像 → `docker load` → 修改 compose 的 `image` tag → `up -d`。

## 8. 安全清单

- [ ] 所有含凭据文件（`config.server*.toml`、`.env.server*`）**不提交 git**（已在 .gitignore）
- [ ] MongoDB/Redis/MinIO 端口仅对可信出口开放，或尽快迁移到内网/VPN
- [ ] HTTPS 就绪后：`public_url` 换域名、`cookie_secure = true`、业务端口收敛到回环交给反代
- [ ] 定期确认备份对象存在且可恢复（恢复演练）
- [ ] 管理端仅内网/VPN 访问（默认受众隔离已强制：管理接口必须走管理端登录）
