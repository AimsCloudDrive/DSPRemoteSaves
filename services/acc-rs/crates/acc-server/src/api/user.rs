//! 用户门户：自己的存档列表、上传/覆盖、版本时间线、回退、删除与用量。

use crate::api::ClientIp;
use crate::auth::CurrentUser;
use crate::error::{ok, AppError, AppResult, ErrorCode};
use crate::repo::{self, Profile};
use crate::state::{keys, AppState};
use crate::storage::Storage;
use crate::util::{dt_ms, now_ms, sha256_hex};
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/saves", get(list_saves))
        .route(
            "/saves/{name}",
            put(upload_save)
                .layer(DefaultBodyLimit::disable())
                .delete(delete_save),
        )
        .route("/saves/{name}/versions", get(list_versions))
        .route(
            "/saves/{name}/versions/{id}/download",
            get(download_version),
        )
        .route(
            "/saves/{name}/versions/{id}/rollback",
            post(rollback_version),
        )
        .route("/stats", get(stats))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteQuery {
    #[serde(default)]
    pub delete_versions: bool,
}

fn save_json(state: &AppState, user: &Profile, save: &repo::SaveDoc) -> Value {
    json!({
        "id": save.id.to_hex(),
        "fileName": save.file_name,
        "sizeBytes": save.size_bytes,
        "mtimeMs": save.mtime_ms,
        "sm3": save.sm3,
        "currentSeq": save.current_seq,
        "currentVersionId": save.current_version_id.map(|v| v.to_hex()),
        "createdAt": dt_ms(save.created_at),
        "updatedAt": dt_ms(save.updated_at),
        "downloadUrl": format!(
            "{}{}/user/saves/{}/versions/{}/download",
            state.cfg.server.public_url.trim_end_matches('/'),
            state.cfg.server.base_path,
            crate::util::percent_encode(&save.file_name),
            save.current_version_id.map(|v| v.to_hex()).unwrap_or_default()
        ),
        "owner": user.user_name,
    })
}

async fn list_saves(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
) -> AppResult<impl IntoResponse> {
    let key = keys::saves_list(profile.id);
    if let Some(cached) = state.cache.get_json::<Value>(&key).await {
        return Ok(ok(cached));
    }
    let saves = repo::list_saves(&state.db, profile.id).await?;
    let total: i64 = saves.iter().map(|s| s.size_bytes).sum();
    let payload = json!({
        "files": saves.iter().map(|s| save_json(&state, &profile, s)).collect::<Vec<_>>(),
        "totalBytes": total,
        "quotaBytes": profile.quota_bytes,
        "usedBytes": profile.used_bytes,
        "reservedBytes": profile.reserved_bytes,
    });
    state
        .cache
        .set_json(&key, &payload, state.cfg.redis.ttl.file_list_seconds)
        .await;
    Ok(ok(payload))
}

/// 流式写入临时文件，同时计算 SM3；超过 `limit` 立即中断。
async fn stream_to_temp(
    state: &AppState,
    body: Body,
    file_id: &str,
    limit: i64,
) -> AppResult<(std::path::PathBuf, i64, String)> {
    use sm3::Digest as _;
    let dir = state.storage.temp_dir(file_id);
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join("upload.bin");
    let mut file = tokio::fs::File::create(&path).await?;
    let mut hasher = sm3::Sm3::new();
    let mut written: i64 = 0;
    let mut stream = body.into_data_stream();

    while let Some(item) = stream.next().await {
        let chunk = item.map_err(|_| AppError::invalid("body stream error"))?;
        written += chunk.len() as i64;
        if written > limit {
            drop(file);
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Err(AppError::new(ErrorCode::FileTooLarge));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    Ok((path, written, hex::encode(hasher.finalize())))
}

async fn upload_save(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
    ClientIp(ip): ClientIp,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> AppResult<Response> {
    let file_name = Storage::validate_file_name(&name)?;
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .ok_or_else(|| AppError::invalid("Content-Length required"))?;

    if declared <= 0 || declared > state.effective_max_file_size().await {
        return Err(AppError::new(ErrorCode::FileTooLarge));
    }
    state.storage.check_disk()?;

    // 预留配额；失败时 413
    if !repo::reserve_quota(&state.db, profile.id, declared).await? {
        return Err(AppError::new(ErrorCode::QuotaExceeded));
    }

    let file_id = format!("portal-{}", uuid::Uuid::new_v4().simple());
    let user = repo::find_user_by_id(&state.db, profile.id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;

    let staged = match stream_to_temp(&state, body, &file_id, declared).await {
        Ok(v) => v,
        Err(err) => {
            repo::release_reservation(&state.db, profile.id, declared).await?;
            state.storage.remove_temp_dir(&file_id).await;
            return Err(err);
        }
    };
    let (path, size, sm3) = staged;

    if size != declared {
        let _ = tokio::fs::remove_file(&path).await;
        repo::release_reservation(&state.db, profile.id, declared).await?;
        state.storage.remove_temp_dir(&file_id).await;
        return Err(AppError::invalid("size mismatch"));
    }

    let lock_key = keys::save_lock(
        profile.id,
        &sha256_hex(file_name.as_bytes())[..16],
    );
    let owner = uuid::Uuid::new_v4().to_string();
    if !state.cache.set_nx(&lock_key, &owner, 30).await {
        let _ = tokio::fs::remove_file(&path).await;
        repo::release_reservation(&state.db, profile.id, declared).await?;
        state.storage.remove_temp_dir(&file_id).await;
        return Err(AppError::new(ErrorCode::Conflict));
    }

    let result = state
        .storage
        .commit_version(
            &state.db,
            &user,
            &file_name,
            &path,
            size,
            &sm3,
            Some(user.id),
            "user",
        )
        .await;
    state.cache.release_lock(&lock_key, &owner).await;
    state.storage.remove_temp_dir(&file_id).await;

    match result {
        Ok((save, version)) => {
            state.invalidate_saves(profile.id).await;
            repo::write_audit(
                &state.db,
                repo::AuditEntry {
                    actor_id: Some(user.id),
                    actor_name: &user.user_name,
                    action: "save.upload.user",
                    target_type: "save",
                    target_id: Some(save.id.to_hex()),
                    result: "success",
                    detail: Some(format!("{} seq={} size={}", save.file_name, version.seq, size)),
                    ip: &ip.to_string(),
                    user_agent: None,
                },
            )
            .await;
            Ok(ok(json!({
                "fileName": save.file_name,
                "seq": version.seq,
                "sizeBytes": size,
                "sm3": sm3,
            }))
            .into_response())
        }
        Err(err) => {
            let _ = tokio::fs::remove_file(&path).await;
            repo::release_reservation(&state.db, profile.id, declared).await?;
            Err(err)
        }
    }
}

async fn delete_save(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
    ClientIp(ip): ClientIp,
    Path(name): Path<String>,
    Query(query): Query<DeleteQuery>,
) -> AppResult<impl IntoResponse> {
    let file_name = Storage::validate_file_name(&name)?;
    let user = repo::find_user_by_id(&state.db, profile.id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
    let save = repo::find_save(&state.db, profile.id, &file_name)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::FileNotFound))?;

    let outcome = state
        .storage
        .delete_save(&state.db, &user, &save, query.delete_versions)
        .await?;
    state.invalidate_saves(profile.id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(user.id),
            actor_name: &user.user_name,
            action: if query.delete_versions {
                "save.delete.all"
            } else {
                "save.delete.current"
            },
            target_type: "save",
            target_id: Some(save.id.to_hex()),
            result: "success",
            detail: Some(format!("{} freed={}", save.file_name, outcome.freed_bytes)),
            ip: &ip.to_string(),
            user_agent: None,
        },
    )
    .await;

    Ok(ok(json!({
        "deletedAll": outcome.deleted_all,
        "freedBytes": outcome.freed_bytes,
        "currentSeq": outcome.current_seq,
    })))
}

async fn list_versions(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
    Path(name): Path<String>,
) -> AppResult<impl IntoResponse> {
    let file_name = Storage::validate_file_name(&name)?;
    let save = repo::find_save(&state.db, profile.id, &file_name)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::FileNotFound))?;
    let versions = repo::list_versions(&state.db, save.id, 500).await?;
    let items: Vec<Value> = versions
        .iter()
        .map(|v| {
            json!({
                "id": v.id.to_hex(),
                "seq": v.seq,
                "sizeBytes": v.size_bytes,
                "sm3": v.sm3,
                "createdAt": dt_ms(v.created_at),
                "source": v.source,
                "current": Some(v.id) == save.current_version_id,
            })
        })
        .collect();
    Ok(ok(json!({
        "fileName": save.file_name,
        "currentSeq": save.current_seq,
        "versions": items,
    })))
}

async fn find_own_version(
    state: &AppState,
    user_id: mongodb::bson::oid::ObjectId,
    name: &str,
    version_id: &str,
) -> AppResult<(repo::SaveDoc, repo::SaveVersion)> {
    let file_name = Storage::validate_file_name(name)?;
    let save = repo::find_save(&state.db, user_id, &file_name)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::FileNotFound))?;
    let vid = mongodb::bson::oid::ObjectId::parse_str(version_id)
        .map_err(|_| AppError::new(ErrorCode::VersionNotFound))?;
    let version = repo::find_version(&state.db, vid)
        .await?
        .filter(|v| v.save_id == save.id)
        .ok_or_else(|| AppError::new(ErrorCode::VersionNotFound))?;
    Ok((save, version))
}

async fn download_version(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
    Path((name, version_id)): Path<(String, String)>,
) -> AppResult<Response> {
    let (_save, version) = find_own_version(&state, profile.id, &name, &version_id).await?;
    let path = state
        .storage
        .resolve_key(&version.storage_key)
        .ok_or_else(|| AppError::new(ErrorCode::VersionNotFound))?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::new(ErrorCode::VersionNotFound))?;
    let meta = file.metadata().await?;

    let mut response = Response::new(Body::from_stream(ReaderStream::new(file)));
    *response.status_mut() = StatusCode::OK;
    let h = response.headers_mut();
    h.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
    h.insert(
        header::CONTENT_LENGTH,
        meta.len().to_string().parse().unwrap(),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        format!(
            "attachment; filename*=UTF-8''{}",
            crate::util::percent_encode(&version.file_name)
        )
        .parse()
        .unwrap(),
    );
    Ok(response)
}

async fn rollback_version(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
    ClientIp(ip): ClientIp,
    Path((name, version_id)): Path<(String, String)>,
) -> AppResult<impl IntoResponse> {
    let user = repo::find_user_by_id(&state.db, profile.id)
        .await?
        .ok_or_else(|| AppError::new(ErrorCode::Unauthorized))?;
    let (save, version) = find_own_version(&state, profile.id, &name, &version_id).await?;

    let updated = state.storage.rollback(&state.db, &user, &save, &version).await?;
    state.invalidate_saves(profile.id).await;

    repo::write_audit(
        &state.db,
        repo::AuditEntry {
            actor_id: Some(user.id),
            actor_name: &user.user_name,
            action: "save.rollback",
            target_type: "save",
            target_id: Some(save.id.to_hex()),
            result: "success",
            detail: Some(format!("{} -> seq {}", save.file_name, version.seq)),
            ip: &ip.to_string(),
            user_agent: None,
        },
    )
    .await;

    Ok(ok(save_json(&state, &profile, &updated)))
}

async fn stats(
    State(state): State<AppState>,
    CurrentUser(profile): CurrentUser,
) -> AppResult<impl IntoResponse> {
    let saves = repo::list_saves(&state.db, profile.id).await?;
    let mut versions = 0usize;
    for save in &saves {
        versions += repo::list_versions(&state.db, save.id, 1000).await?.len();
    }
    Ok(ok(json!({
        "quotaBytes": profile.quota_bytes,
        "usedBytes": profile.used_bytes,
        "reservedBytes": profile.reserved_bytes,
        "fileCount": saves.len(),
        "versionCount": versions,
        // 当前生效的单文件上限（管理端设置，受配置文件硬上限约束）
        "maxFileSize": state.effective_max_file_size().await,
        "diskFreeBytes": state.storage.available_space(),
        "serverTime": now_ms(),
    })))
}
