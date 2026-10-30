//! The web app in `web/`. Release builds carry it inside the binary; debug builds read it
//! from disk, so it can be edited without restarting the server.

use axum::{
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "web/"]
struct Web;

pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    let Some(file) = Web::get(path) else {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    };
    let hash: String = file
        .metadata
        .sha256_hash()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let etag = format!("\"{hash}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|tag| tag.as_bytes() == etag.as_bytes())
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }
    let content_type = HeaderValue::from_str(file.metadata.mimetype())
        .unwrap_or(HeaderValue::from_static("application/octet-stream"));
    (
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::ETAG,
                HeaderValue::from_str(&etag).expect("hex is a valid header"),
            ),
            // Always ask whether the file changed; the ETag makes that cheap.
            (header::CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        file.data,
    )
        .into_response()
}
