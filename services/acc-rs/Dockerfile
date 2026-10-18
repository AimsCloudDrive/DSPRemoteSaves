# ---- 1) 构建前端 ----
FROM node:22-alpine AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json* ./
# 国内环境使用 npmmirror，避免构建卡在外网
RUN npm config set registry https://registry.npmmirror.com \
    && (npm ci --no-audit --no-fund || npm install --no-audit --no-fund)
COPY web/ ./
RUN npm run build

# ---- 2) 构建后端（把前端产物嵌入二进制）----
FROM rust:1-bookworm AS build
WORKDIR /src
# 小内存服务器用 --build-arg CARGO_BUILD_JOBS=1 限制并行编译，避免 OOM
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
# crates.io 走 USTC 镜像
RUN mkdir -p /usr/local/cargo \
    && printf '[source.crates-io]\nreplace-with = "ustc"\n\n[source.ustc]\nregistry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"\n' \
    > /usr/local/cargo/config.toml
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY --from=web /app/web/dist ./web/dist
RUN cargo build --release -p acc-server -p acc-cli

# ---- 3) 运行镜像 ----
FROM debian:bookworm-slim

# tar/zstd 用于存档归档；mongodb-database-tools 提供 mongodump/mongorestore
RUN sed -i 's|deb.debian.org|mirrors.aliyun.com|g' /etc/apt/sources.list.d/debian.sources 2>/dev/null \
    || sed -i 's|deb.debian.org|mirrors.aliyun.com|g' /etc/apt/sources.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata zstd tar curl gnupg \
    && curl -fsSL https://www.mongodb.org/static/pgp/server-7.0.asc \
       | gpg --dearmor -o /usr/share/keyrings/mongodb-server.gpg \
    && echo "deb [signed-by=/usr/share/keyrings/mongodb-server.gpg] https://repo.mongodb.org/apt/debian bookworm/mongodb-org/7.0 main" \
       > /etc/apt/sources.list.d/mongodb.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends mongodb-database-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=build /src/target/release/acc-server /usr/local/bin/acc-server
COPY --from=build /src/target/release/acc-cli /usr/local/bin/acc-cli
COPY config.example.toml /app/config.toml

ENV TZ=Asia/Shanghai
EXPOSE 9999
ENTRYPOINT ["acc-server", "--config", "/app/config.toml"]
