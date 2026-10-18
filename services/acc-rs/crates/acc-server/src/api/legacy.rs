//! 模组兼容接口：`/upload/*`、`/download/*`。
//!
//! **响应结构不可改**：
//! - `init` 返回 `payload.data.{fileId,chunkSize,totalChunks,uploadToken}`
//! - `file-list` 返回 `payload.files[]`
//! - 错误统一 `{ code: 1, message }`

use crate::api::ClientIp;
use crate::crypto::open_password;
use crate::error::{AppError, AppResult, ErrorCode};
use crate::i18n;
use crate::repo::{self, UploadSession, User};
use crate::state::{keys, AppState};
use crate::storage::Storage;
use crate::util::{
    constant_time_eq, now_ms, parse_range, percent_encode, random_token, sha256_hex,
};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt;
use mongodb::bson::DateTime;
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use tracing::{info, warn};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/upload/init", post(upload_init))
        .route(
            "/upload/chunk",
            post(upload_chunk).layer(DefaultBodyLimit::disable()),
        )
        .route("/upload/complete", post(upload_complete))
        .route("/download/file-list", post(file_list))
        .route("/download/download", post(download))
}

// ---------------------------------------------------------------------------
// 响应辅助
// ---------------------------------------------------------------------------

fn legacy_json(status: StatusCode, body: Value) -> Response {
    (status, Json(body)).into_response()
}

fn legacy_ok(payload: Option<Value>) -> Response {
    let mut body = json!({ "code": 0, "message": i18n::ok_message(i18n::current()) });
    if let Some(payload) = payload {
        body["payload"] = payload;
    }
    legacy_json(StatusCode::OK, body)
}

fn legacy_result(result: AppResult<Response>) -> Response {
    match result {
        Ok(response) => response,
        Err(err) => {
            let mut body = json!({
                "code": 1,
                "message": err.localized(i18n::current()),
            });
            if let Some(payload) = err.payload.clone() {
                body["payload"] = payload;
            }
            legacy_json(err.code.http(), body)
        }
    }
}

// ---------------------------------------------------------------------------
// 请求体
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitReq {
    #[serde(default, alias = "account")]
    pub user_name: String,
    pub password: String,
    pub file_name: String,
    pub file_size: i64,
    pub chunk_size: i64,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteReq {
    #[serde(default, alias = "account")]
    pub user_name: String,
    pub password: String,
    pub file_id: String,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListReq {
    #[serde(default, alias = "account")]
    pub user_name: String,
    pub password: String,
    #[serde(default)]
    pub sync_file_extensions: Option<Value>,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadReq {
    #[serde(default, alias = "account")]
    pub user_name: String,
    pub password: String,
    pub file_name: String,
    #[serde(default)]
    pub is_chunk: bool,
    #[serde(default)]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkQuery {
    pub file_id: String,
    pub chunk_index: i64,
}

// ---------------------------------------------------------------------------
// 模组鉴权（无 token：用户名 + 密文密码）
// ---------------------------------------------------------------------------

async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    user_name: &str,
    password: &str,
    timestamp: Option<i64>,
    nonce: Option<&str>,
) -> AppResult<User> {
    let user_name = user_name.trim();
    if user_name.is_empty() || password.is_empty() {
        return Err(AppError::invalid("missing credentials"));
    }

    let secret = open_password(state, headers, password, timestamp, nonce).await?;
    let plain = secret.expose_secret().to_string();

    let user = repo::find_user_by_name(&state.db, user_name).await?;
    let Some(user) = user else {
        crate::auth::password::dummy_verify(plain).await;
        return Err(bad_credentials());
    };
    if !crate::auth::password::verify_password(plain, user.password_hash.clone()).await {
        return Err(bad_credentials());
    }
    if !user.is_active() {
        return Err(AppError::new(ErrorCode::Forbidden));
    }
    Ok(user)
}

fn bad_credentials() -> AppError {
    let message = match i18n::current() {
        i18n::Lang::Zh => "用户名或密码错误",
        i18n::Lang::En => "Invalid username or password",
    };
    AppError::new(ErrorCode::Unauthorized).with_message(message)
}

// ---------------------------------------------------------------------------
// /upload/init
// ---------------------------------------------------------------------------

async fn upload_init(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(req): Json<InitReq>,
) -> Response {
    legacy_result(init_inner(state, ip, headers, req).await)
}

async fn init_inner(
    state: AppState,
    ip: std::net::IpAddr,
    headers: HeaderMap,
    req: InitReq,
) -> AppResult<Response> {
    let user = authenticate(
        &state,
        &headers,
        &req.user_name,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?;

    let file_name = Storage::validate_file_name(&req.file_name)?;
    let max_file_size = state.effective_max_file_size().await;
    if req.file_size <= 0 || req.file_size > max_file_size {
        return Err(AppError::new(ErrorCode::FileTooLarge));
    }
    if req.chunk_size < 64 * 1024 || req.chunk_size > state.cfg.storage.max_chunk_size {
        return Err(AppError::invalid("invalid chunk size"));
    }
    let total_chunks = (req.file_size + req.chunk_size - 1) / req.chunk_size;
    if total_chunks > state.cfg.storage.max_total_chunks || total_chunks <= 0 {
        return Err(AppError::invalid("too many chunks"));
    }

    state.storage.check_disk()?;

    // 原子预留配额：先预留再传，避免并发超卖
    if !repo::reserve_quota(&state.db, user.id, req.file_size).await? {
        return Err(AppError::new(ErrorCode::QuotaExceeded));
    }

    let file_id = uuid::Uuid::new_v4().to_string();
    let token = random_token();
    let now = now_ms();
    let session = UploadSession {
        id: file_id.clone(),
        user_id: user.id,
        user_name: user.user_name.clone(),
        file_name: file_name.clone(),
        file_size: req.file_size,
        chunk_size: req.chunk_size,
        total_chunks,
        uploaded_mask: mongodb::bson::Binary {
            subtype: mongodb::bson::spec::BinarySubtype::Generic,
            bytes: vec![0u8; ((total_chunks + 7) / 8) as usize],
        },
        upload_token_hash: sha256_hex(token.as_bytes()),
        created_at: DateTime::from_millis(now),
        expires_at: DateTime::from_millis(now + (state.cfg.storage.upload_idle_seconds as i64) * 1000),
        hard_deadline: DateTime::from_millis(
            now + (state.cfg.storage.upload_max_hours as i64) * 3_600_000,
        ),
    };

    if let Err(err) = state.storage.create_temp_dir(&file_id).await {
        repo::release_reservation(&state.db, user.id, req.file_size).await?;
        return Err(err.into());
    }
    if let Err(err) = repo::create_upload_session(&state.db, &session).await {
        let _ = tokio::fs::remove_dir_all(state.storage.temp_dir(&file_id)).await;
        repo::release_reservation(&state.db, user.id, req.file_size).await?;
        return Err(err.into());
    }

    info!(
        user = %user.user_name,
        file = %file_name,
        size = req.file_size,
        chunks = total_chunks,
        ip = %ip,
        "上传会话已创建"
    );

    Ok(legacy_ok(Some(json!({
        "data": {
            "fileId": file_id,
            "chunkSize": req.chunk_size,
            "totalChunks": total_chunks,
            "uploadToken": token,
        }
    }))))
}

// ---------------------------------------------------------------------------
// /upload/chunk
// ---------------------------------------------------------------------------

async fn upload_chunk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ChunkQuery>,
    body: Body,
) -> Response {
    legacy_result(chunk_inner(state, headers, query, body).await)
}

async fn chunk_inner(
    state: AppState,
    headers: HeaderMap,
    query: ChunkQuery,
    body: Body,
) -> AppResult<Response> {
    let token = headers
        .get("uploadToken")
        .or_else(|| headers.get("upload-token"))
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;

    let session = repo::find_upload_session(&state.db, &query.file_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::UploadExpired))?;

    if !constant_time_eq(
        session.upload_token_hash.as_bytes(),
        sha256_hex(token.as_bytes()).as_bytes(),
    ) {
        return Err(AppError::new(ErrorCode::Forbidden));
    }
    if session.expires_at.timestamp_millis() <= now_ms() {
        return Err(AppError::new(ErrorCode::UploadExpired));
    }
    if query.chunk_index < 0 || query.chunk_index >= session.total_chunks {
        return Err(AppError::invalid("chunk index out of range"));
    }

    if let Some(length) = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
    {
        if length > session.chunk_size {
            return Err(AppError::new(ErrorCode::FileTooLarge));
        }
    }

    let path = state.storage.chunk_path(&query.file_id, query.chunk_index);
    let mut file = tokio::fs::File::create(&path).await?;
    let mut stream = body.into_data_stream();
    let mut written: i64 = 0;

    while let Some(item) = stream.next().await {
        let bytes: Bytes = item.map_err(|err| {
            warn!(error = %err, "读取分片流失败");
            AppError::invalid("body stream error")
        })?;
        written += bytes.len() as i64;
        if written > session.chunk_size {
            drop(file);
            let _ = tokio::fs::remove_file(&path).await;
            return Err(AppError::new(ErrorCode::FileTooLarge));
        }
        file.write_all(&bytes).await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    // 更新位图（complete 时会以磁盘为准二次校验，位图仅用于进度展示）
    let mut mask = session.uploaded_mask.bytes.clone();
    let needed = ((session.total_chunks + 7) / 8) as usize;
    if mask.len() < needed {
        mask.resize(needed, 0);
    }
    let byte = (query.chunk_index / 8) as usize;
    let bit = (query.chunk_index % 8) as u8;
    mask[byte] |= 1 << bit;

    let idle_ms = (state.cfg.storage.upload_idle_seconds as i64) * 1000;
    let next = (now_ms() + idle_ms).min(session.hard_deadline.timestamp_millis());
    repo::touch_upload_session(
        &state.db,
        &query.file_id,
        DateTime::from_millis(next),
        &mask,
    )
    .await?;

    Ok(legacy_ok(None))
}

// ---------------------------------------------------------------------------
// /upload/complete
// ---------------------------------------------------------------------------

async fn upload_complete(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    headers: HeaderMap,
    Json(req): Json<CompleteReq>,
) -> Response {
    legacy_result(complete_inner(state, ip, headers, req).await)
}

async fn complete_inner(
    state: AppState,
    ip: std::net::IpAddr,
    headers: HeaderMap,
    req: CompleteReq,
) -> AppResult<Response> {
    let user = authenticate(
        &state,
        &headers,
        &req.user_name,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?;

    let session = repo::find_upload_session(&state.db, &req.file_id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::UploadExpired))?;
    if session.user_id != user.id {
        return Err(AppError::new(ErrorCode::Forbidden));
    }

    // 以磁盘为准判断完整性，避免位图并发写入丢位
    let missing = state
        .storage
        .missing_chunks(&req.file_id, session.total_chunks)
        .await;
    if !missing.is_empty() {
        return Err(AppError::new(ErrorCode::ChunkIncomplete)
            .with_payload(json!({ "nots": missing })));
    }

    let merged = state.storage.temp_dir(&req.file_id).join("merged.bin");
    let (size, sm3) = state
        .storage
        .merge_chunks(&req.file_id, session.total_chunks, &merged)
        .await?;

    if size != session.file_size {
        // 客户端声明的大小与实际不一致：释放预留，允许重新上传
        let _ = tokio::fs::remove_file(&merged).await;
        repo::release_reservation(&state.db, user.id, session.file_size).await?;
        repo::delete_upload_session(&state.db, &req.file_id).await?;
        state.storage.remove_temp_dir(&req.file_id).await;
        return Err(AppError::internal("merged size mismatch"));
    }

    let lock_key = keys::save_lock(user.id, &crate::util::sha256_hex(session.file_name.as_bytes())[..16]);
    let owner = uuid::Uuid::new_v4().to_string();
    if !state.cache.set_nx(&lock_key, &owner, 30).await {
        return Err(AppError::new(ErrorCode::Conflict).with_message("文件正在被写入，请重试"));
    }

    let outcome = state
        .storage
        .commit_version(
            &state.db,
            &user,
            &session.file_name,
            &merged,
            size,
            &sm3,
            Some(user.id),
            "mod",
        )
        .await;

    state.cache.release_lock(&lock_key, &owner).await;

    match outcome {
        Ok((save, version)) => {
            repo::delete_upload_session(&state.db, &req.file_id).await?;
            state.storage.remove_temp_dir(&req.file_id).await;
            state.invalidate_saves(user.id).await;

            repo::write_audit(
                &state.db,
                repo::AuditEntry {
                    actor_id: Some(user.id),
                    actor_name: &user.user_name,
                    action: "save.upload",
                    target_type: "save",
                    target_id: Some(save.id.to_hex()),
                    result: "success",
                    detail: Some(format!(
                        "{} seq={} size={}",
                        save.file_name, version.seq, size
                    )),
                    ip: &ip.to_string(),
                    user_agent: None,
                },
            )
            .await;

            info!(
                user = %user.user_name,
                file = %save.file_name,
                seq = version.seq,
                size,
                sm3 = %sm3,
                "存档上传完成"
            );
            Ok(legacy_ok(None))
        }
        Err(err) => {
            // 保留会话与分片，客户端可直接重试 complete
            warn!(error = %err, file = %session.file_name, "提交版本失败");
            Err(err)
        }
    }
}

// ---------------------------------------------------------------------------
// /download/file-list
// ---------------------------------------------------------------------------

fn parse_extensions(value: Option<Value>) -> Option<Vec<String>> {
    // 空列表 / 空字符串一律视为“不过滤”：旧模组客户端会发送空数组，
    // 若当作过滤条件会导致 file-list 恒为空。
    let parsed = match value {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => Some(
            items
                .into_iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_lowercase()))
                .filter(|s| !s.is_empty())
                .collect::<Vec<String>>(),
        ),
        Some(Value::String(text)) => Some(
            text.split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect::<Vec<String>>(),
        ),
        _ => None,
    };
    match parsed {
        Some(list) if list.is_empty() => None,
        other => other,
    }
}

async fn file_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<FileListReq>,
) -> Response {
    legacy_result(file_list_inner(state, headers, req).await)
}

async fn file_list_inner(
    state: AppState,
    headers: HeaderMap,
    req: FileListReq,
) -> AppResult<Response> {
    let user = authenticate(
        &state,
        &headers,
        &req.user_name,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?;

    let extensions = parse_extensions(req.sync_file_extensions);
    let saves = repo::list_saves(&state.db, user.id).await?;
    let files: Vec<Value> = saves
        .into_iter()
        .filter(|save| match &extensions {
            None => true,
            Some(list) => {
                let lower = save.file_name.to_lowercase();
                list.iter().any(|ext| lower.ends_with(ext.as_str()))
            }
        })
        .map(|save| {
            json!({
                "fileName": save.file_name,
                "fileSize": save.size_bytes,
                "lastModified": save.mtime_ms,
            })
        })
        .collect();

    Ok(legacy_ok(Some(json!({ "files": files }))))
}

// ---------------------------------------------------------------------------
// /download/download
// ---------------------------------------------------------------------------

async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DownloadReq>,
) -> Response {
    legacy_result(download_inner(state, headers, req).await)
}

async fn download_inner(
    state: AppState,
    headers: HeaderMap,
    req: DownloadReq,
) -> AppResult<Response> {
    let user = authenticate(
        &state,
        &headers,
        &req.user_name,
        &req.password,
        req.timestamp,
        req.nonce.as_deref(),
    )
    .await?;

    let file_name = Storage::validate_file_name(&req.file_name)?;
    let save = repo::find_save(&state.db, user.id, &file_name)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::FileNotFound))?;

    let path = state.storage.current_path(user.id, &save.file_name);
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|_| AppError::new(ErrorCode::FileNotFound))?;
    let size = meta.len();

    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        percent_encode(&save.file_name)
    );

    if req.is_chunk {
        let range_header = headers
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::invalid("Range header required"))?;
        let (start, end) = parse_range(range_header, size)
            .ok_or_else(|| AppError::new(ErrorCode::InvalidParam))?;

        let file = tokio::fs::File::open(&path).await?;
        let stream = ReaderStream::new(file);
        let mut response = Response::new(Body::from_stream(stream));
        *response.status_mut() = StatusCode::PARTIAL_CONTENT;
        let h = response.headers_mut();
        h.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
        h.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
        h.insert(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{size}").parse().unwrap(),
        );
        h.insert(
            header::CONTENT_LENGTH,
            (end - start + 1).to_string().parse().unwrap(),
        );
        h.insert(
            header::CONTENT_DISPOSITION,
            disposition.parse().unwrap(),
        );
        return Ok(response);
    }

    let file = tokio::fs::File::open(&path).await?;
    let stream = ReaderStream::new(file);
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
    h.insert(header::CONTENT_LENGTH, size.to_string().parse().unwrap());
    h.insert(header::ACCEPT_RANGES, "bytes".parse().unwrap());
    h.insert(
        header::CONTENT_DISPOSITION,
        disposition.parse().unwrap(),
    );
    Ok(response)
}

