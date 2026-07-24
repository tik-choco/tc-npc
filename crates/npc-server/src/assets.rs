//! Serves `web/dist`, embedded into the binary at compile time via
//! `rust-embed`, at `/` with SPA fallback (any unmatched path serves
//! `index.html` so client-side routing works on hard refresh / deep links).

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebDist;

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    serve(path)
}

fn serve(path: &str) -> Response {
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = WebDist::get(path) {
        return file_response(path, file.data.into_owned());
    }

    // SPA fallback: any unknown path (client-side route) gets index.html.
    match WebDist::get("index.html") {
        Some(file) => file_response("index.html", file.data.into_owned()),
        None => (
            StatusCode::NOT_FOUND,
            "web/dist is empty — build the web frontend first",
        )
            .into_response(),
    }
}

fn file_response(path: &str, data: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
