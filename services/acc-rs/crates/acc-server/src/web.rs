//! 前端静态资源：生产环境把 `web/dist` 嵌入二进制，未知路径回退 `index.html`。
//!
//! `web/dist` 不存在时（例如尚未构建前端），服务仍可编译启动，只在根路径返回 503，
//! 便于纯 API 部署与 CI 编译。

use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[cfg(has_web_dist)]
mod embedded {
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "../../web/dist/"]
    pub struct Assets;
}

pub async fn spa(uri: Uri) -> Response {
    #[cfg(has_web_dist)]
    {
        let path = uri.path().trim_start_matches('/');
        let path = if path.is_empty() { "index.html" } else { path };

        if let Some(file) = embedded::Assets::get(path) {
            return serve_file(path, file.data.into_owned());
        }

        match embedded::Assets::get("index.html") {
            Some(index) => serve_file("index.html", index.data.into_owned()),
            None => (
                StatusCode::NOT_FOUND,
                "web/dist 缺少 index.html：请执行 npm --prefix web run build",
            )
                .into_response(),
        }
    }

    #[cfg(not(has_web_dist))]
    {
        let _ = uri;
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "前端未构建：请在 services/acc-rs/web 执行 npm install && npm run build 后重新编译服务",
        )
            .into_response()
    }
}

#[cfg(has_web_dist)]
fn serve_file(path: &str, data: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path == "index.html" {
        "no-cache"
    } else if path.contains('.') && path.rsplit('.').next().map(|e| e.len() <= 3).unwrap_or(false) {
        // 带 hash 的构建产物
        "public, max-age=31536000, immutable"
    } else {
        "public, max-age=3600"
    };

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (
                header::REFERRER_POLICY,
                "strict-origin-when-cross-origin".to_string(),
            ),
        ],
        data,
    )
        .into_response()
}
