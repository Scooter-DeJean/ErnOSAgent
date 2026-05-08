//! Static content handlers — HTML, CSS, JS, vendor libs, and dynamically generated images.

use crate::web::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

pub async fn index() -> impl IntoResponse {
    ([
        ("content-type", "text/html; charset=utf-8"),
        ("cache-control", "no-cache, no-store, must-revalidate"),
        ("pragma", "no-cache"),
    ], include_str!("../static/index.html"))
}

pub async fn css() -> impl IntoResponse {
    ([
        ("content-type", "text/css; charset=utf-8"),
        ("cache-control", "no-cache, no-store, must-revalidate"),
    ], include_str!("../static/app.css"))
}

pub async fn js() -> impl IntoResponse {
    ([
        ("content-type", "application/javascript; charset=utf-8"),
        ("cache-control", "no-cache, no-store, must-revalidate"),
    ], include_str!("../static/app.js"))
}

// ─── Vendor assets (embedded for Android compat) ───

pub async fn vendor_highlight_js() -> impl IntoResponse {
    ([
        ("content-type", "application/javascript; charset=utf-8"),
        ("cache-control", "public, max-age=86400"),
    ], include_str!("../static/vendor/highlight.min.js"))
}

pub async fn vendor_katex_js() -> impl IntoResponse {
    ([
        ("content-type", "application/javascript; charset=utf-8"),
        ("cache-control", "public, max-age=86400"),
    ], include_str!("../static/vendor/katex.min.js"))
}

pub async fn vendor_auto_render_js() -> impl IntoResponse {
    ([
        ("content-type", "application/javascript; charset=utf-8"),
        ("cache-control", "public, max-age=86400"),
    ], include_str!("../static/vendor/auto-render.min.js"))
}

pub async fn vendor_mermaid_js() -> impl IntoResponse {
    ([
        ("content-type", "application/javascript; charset=utf-8"),
        ("cache-control", "public, max-age=86400"),
    ], include_str!("../static/vendor/mermaid.min.js"))
}

pub async fn vendor_github_dark_css() -> impl IntoResponse {
    ([
        ("content-type", "text/css; charset=utf-8"),
        ("cache-control", "public, max-age=86400"),
    ], include_str!("../static/vendor/github-dark.min.css"))
}

pub async fn vendor_katex_css() -> impl IntoResponse {
    ([
        ("content-type", "text/css; charset=utf-8"),
        ("cache-control", "public, max-age=86400"),
    ], include_str!("../static/vendor/katex.min.css"))
}

/// Serve generated images from `{data_dir}/images/{filename}`.
pub async fn serve_image(State(state): State<AppState>, Path(filename): Path<String>) -> impl IntoResponse {
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return (StatusCode::BAD_REQUEST, "Invalid filename".to_string()).into_response();
    }

    let path = state.config.general.data_dir.join("images").join(&filename);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let content_type = if filename.ends_with(".png") {
                "image/png"
            } else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") {
                "image/jpeg"
            } else if filename.ends_with(".webp") {
                "image/webp"
            } else {
                "application/octet-stream"
            };
            ([("content-type", content_type), ("cache-control", "public, max-age=86400")], bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "Image not found".to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_index_html_included() {
        let html = include_str!("../static/index.html");
        assert!(html.contains("Ern-OS"));
    }

    #[test]
    fn test_css_included() {
        let css = include_str!("../static/app.css");
        assert!(css.contains("--bg-primary"));
    }

    #[test]
    fn test_js_included() {
        let js = include_str!("../static/app.js");
        assert!(js.contains("ErnOS"));
    }
}
