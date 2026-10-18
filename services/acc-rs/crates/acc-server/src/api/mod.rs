//! HTTP 路由装配、请求上下文（traceId + 语言）与通用提取器。

pub mod admin;
pub mod auth;
pub mod health;
pub mod legacy;
pub mod user;

use crate::error::AppError;
use crate::i18n::Lang;
use crate::state::AppState;
use crate::util::new_trace_id;
use axum::extract::{DefaultBodyLimit, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::net::IpAddr;

#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub IpAddr);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(state.client_ip(&parts.headers, &parts.extensions)))
    }
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct Pagination {
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

impl Pagination {
    pub fn skip(&self) -> u64 {
        (self.page.unwrap_or(1).max(1) - 1) * self.limit()
    }

    pub fn limit(&self) -> u64 {
        self.page_size.unwrap_or(20).clamp(1, 200)
    }
}

pub fn user_agent(parts_headers: &axum::http::HeaderMap) -> Option<String> {
    parts_headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(256).collect())
}

/// 注入 traceId 与语言，并回写 `x-request-id`。
pub async fn context_middleware(
    State(_state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let trace_id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty() && v.len() <= 64)
        .map(|v| v.to_string())
        .unwrap_or_else(new_trace_id);

    let lang = Lang::from_uri(req.uri()).unwrap_or_else(|| Lang::from_headers(req.headers()));

    let mut response = crate::util::with_trace(
        trace_id.clone(),
        crate::i18n::with_lang(lang, next.run(req)),
    )
    .await;
    if let Ok(value) = trace_id.parse() {
        response.headers_mut().insert("x-request-id", value);
    }
    response
}

/// API 前缀下的未知路径返回 JSON 404，避免落到 SPA 拿到一页 HTML。
async fn api_not_found() -> axum::response::Response {
    use axum::response::IntoResponse;
    let lang = crate::i18n::current();
    (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "code": crate::error::ErrorCode::NotFound.numeric(),
            "messageKey": "not_found",
            "message": crate::i18n::error_message("not_found", lang),
            "traceId": crate::util::current_trace_id(),
        })),
    )
        .into_response()
}

pub fn build_router(state: AppState) -> Router {
    let base_path = state.cfg.server.base_path.clone();
    let max_file_size = state.cfg.storage.max_file_size;

    let api = Router::new()
        .nest("/auth", auth::routes())
        .nest("/user", user::routes())
        .nest("/admin", admin::routes())
        .merge(legacy::routes())
        .merge(health::routes())
        .fallback(api_not_found)
        .route(
            "/crypto/public-key",
            get(health::public_key).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .layer(DefaultBodyLimit::max(
            (max_file_size.max(16 * 1024 * 1024)) as usize,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            context_middleware,
        ));

    Router::new()
        .nest(&base_path, api)
        .merge(health::root_routes())
        .fallback(crate::web::spa)
        .with_state(state)
}
