//! 健康检查、就绪探针、指标与公钥接口。

use crate::error::ErrorCode;
use crate::state::AppState;
use crate::util::now_ms;
use axum::extract::{State, ConnectInfo};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use mongodb::bson::doc;
use serde_json::json;
use std::net::SocketAddr;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
}

/// 根路径也提供探针，方便容器/负载均衡直接访问。
pub fn root_routes() -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
}

async fn healthz() -> impl IntoResponse {
    Json(json!({ "status": "ok", "time": now_ms() }))
}

async fn readyz(State(state): State<AppState>) -> Response {
    let mongo_ok = state.db.run_command(doc! { "ping": 1 }).await.is_ok();
    let redis_enabled = state.cache.enabled();
    let redis_ok = if redis_enabled {
        state.cache.ping().await
    } else {
        true
    };
    let disk_free = state.storage.available_space();
    let ring = state.keyring.read().await;
    let key_id = ring.active_key_id().to_string();
    drop(ring);

    let disk_ok = disk_free
        .map(|free| free >= state.cfg.storage.min_free_bytes)
        .unwrap_or(true);

    let status = if !mongo_ok {
        "unavailable"
    } else if (redis_enabled && !redis_ok) || !disk_ok {
        "degraded"
    } else {
        "ok"
    };

    let body = json!({
        "status": status,
        "mongo": mongo_ok,
        "redis": redis_ok,
        "redisEnabled": redis_enabled,
        "disk": { "freeBytes": disk_free, "ok": disk_ok },
        "keyId": key_id,
        "uptimeSeconds": state.uptime_seconds(),
        "backup": state.backup.read().await.clone(),
    });

    let code = if status == "unavailable" {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (code, Json(body)).into_response()
}

/// Prometheus 兼容的纯文本指标。
async fn metrics(State(state): State<AppState>) -> Response {
    let users = crate::repo::count_users(&state.db, doc! {}).await.unwrap_or(0);
    let active_users = crate::repo::count_users(&state.db, doc! { "status": "active" })
        .await
        .unwrap_or(0);
    let sessions = crate::repo::count_active_sessions(&state.db)
        .await
        .unwrap_or(0);
    let disk_free = state.storage.available_space().unwrap_or(0);
    let backup = state.backup.read().await.clone();

    let mut out = String::new();
    out.push_str("# HELP dsp_users_total Total users\n# TYPE dsp_users_total gauge\n");
    out.push_str(&format!("dsp_users_total {users}\n"));
    out.push_str("# HELP dsp_users_active Active users\n# TYPE dsp_users_active gauge\n");
    out.push_str(&format!("dsp_users_active {active_users}\n"));
    out.push_str("# HELP dsp_sessions_active Active refresh sessions\n# TYPE dsp_sessions_active gauge\n");
    out.push_str(&format!("dsp_sessions_active {sessions}\n"));
    out.push_str("# HELP dsp_disk_free_bytes Free disk bytes\n# TYPE dsp_disk_free_bytes gauge\n");
    out.push_str(&format!("dsp_disk_free_bytes {disk_free}\n"));
    out.push_str("# HELP dsp_redis_up Redis availability\n# TYPE dsp_redis_up gauge\n");
    out.push_str(&format!(
        "dsp_redis_up {}\n",
        if state.cache.healthy() { 1 } else { 0 }
    ));
    out.push_str("# HELP dsp_backup_last_success_ms Last successful backup time\n# TYPE dsp_backup_last_success_ms gauge\n");
    out.push_str(&format!(
        "dsp_backup_last_success_ms {}\n",
        backup.last_ok_at.unwrap_or(0)
    ));

    (
        StatusCode::OK,
        [("content-type", "text/plain; version=0.0.4")],
        out,
    )
        .into_response()
}

/// 公钥接口：60 次/分钟/IP。
pub async fn public_key(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Response {
    let ip = crate::util::client_ip(&headers, Some(peer.ip()), &state.trusted);
    let result = state
        .cache
        .incr_window(&format!("rl:pubkey:{}", ip), 60, 60)
        .await;
    if !result.allowed {
        return (
            ErrorCode::RateLimited.http(),
            Json(json!({
                "code": ErrorCode::RateLimited.numeric(),
                "messageKey": ErrorCode::RateLimited.key(),
                "message": "too many requests",
                "payload": { "retryAfterSeconds": result.retry_after },
            })),
        )
            .into_response();
    }

    let payload = crate::crypto::public_key_payload(&state).await;
    Json(json!({
        "code": 0,
        "messageKey": "ok",
        "message": crate::i18n::ok_message(crate::i18n::current()),
        "payload": payload,
        "traceId": crate::util::current_trace_id(),
    }))
    .into_response()
}
