use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use music_link_finder::search::{SearchMode, search_youtube};
use serde::{Deserialize, Serialize};
use std::{env, net::SocketAddr};

const PAGE: &str = include_str!("../../web/index.html");
const CSS: &str = include_str!("../../web/style.css");
const JS: &str = include_str!("../../web/app.js");
const FAVICON: &[u8] = include_bytes!("../../assets/app.ico");

#[derive(Deserialize)]
struct SearchParams {
    q: Option<String>,
    mode: Option<String>,
}

#[derive(Serialize)]
struct ApiError {
    error: String,
}

fn error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(ApiError {
            error: message.into(),
        }),
    )
        .into_response()
}

fn validate(params: &SearchParams) -> Result<(&str, SearchMode), &'static str> {
    let query = params.q.as_deref().unwrap_or("").trim();
    if query.is_empty() {
        return Err("곡명을 입력해 주세요.");
    }
    if query.chars().count() > 120 {
        return Err("곡명은 120자 이하로 입력해 주세요.");
    }
    if query.chars().any(char::is_control) {
        return Err("곡명에 제어 문자를 사용할 수 없습니다.");
    }
    let mode = match params.mode.as_deref().unwrap_or("pronunciation") {
        "pronunciation" => SearchMode::Pronunciation,
        "karaoke" => SearchMode::Karaoke,
        _ => {
            return Err("검색 모드가 올바르지 않습니다.");
        }
    };
    Ok((query, mode))
}

async fn search(Query(params): Query<SearchParams>) -> Response {
    let (query, mode) = match validate(&params) {
        Ok(value) => value,
        Err(message) => return error(StatusCode::BAD_REQUEST, message),
    };
    let query = query.to_owned();
    match tokio::task::spawn_blocking(move || search_youtube(&query, mode, 25)).await {
        Ok(Ok(results)) => Json(serde_json::json!({
            "results": results.into_iter().map(|item| serde_json::json!({
                "title": item.title, "url": item.url, "channel": item.channel,
                "thumbnail_url": item.thumbnail_url,
                "score": item.score, "matched": item.matched,
                "is_recommendation": item.is_recommendation,
                "is_fallback": item.is_fallback
            })).collect::<Vec<_>>()
        }))
        .into_response(),
        Ok(Err(message)) => error(StatusCode::BAD_GATEWAY, message),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "검색 처리 중 오류가 발생했습니다.",
        ),
    }
}

async fn home() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], PAGE)
}
async fn stylesheet() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], CSS)
}
async fn script() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        JS,
    )
}
async fn regular_font() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        include_bytes!("../../assets/fonts/PretendardJP-Regular.woff2").as_slice(),
    )
}
async fn semibold_font() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        include_bytes!("../../assets/fonts/PretendardJP-SemiBold.woff2").as_slice(),
    )
}
async fn font_license() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        include_str!("../../assets/fonts/LICENSE.txt"),
    )
}
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

async fn favicon() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/vnd.microsoft.icon"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        FAVICON,
    )
}

fn router() -> Router {
    Router::new()
        .route("/", get(home))
        .route("/style.css", get(stylesheet))
        .route("/app.js", get(script))
        .route("/favicon.ico", get(favicon))
        .route(
            "/fonts/pretendard-jp-regular-v1.3.9.woff2",
            get(regular_font),
        )
        .route(
            "/fonts/pretendard-jp-semibold-v1.3.9.woff2",
            get(semibold_font),
        )
        .route("/fonts/license.txt", get(font_license))
        .route("/api/health", get(health))
        .route("/api/search", get(search))
        .layer(DefaultBodyLimit::max(4096))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_owned())
        .parse()?;
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("Music Link Finder listening on http://{address}");
    axum::serve(listener, router()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_modes_and_defaults() {
        for mode in [
            None,
            Some("pronunciation".to_owned()),
            Some("karaoke".to_owned()),
        ] {
            assert!(
                validate(&SearchParams {
                    q: Some("  잇떼  ".into()),
                    mode
                })
                .is_ok()
            );
        }
    }

    #[test]
    fn rejects_invalid_inputs() {
        for q in [
            None,
            Some("   ".into()),
            Some("a".repeat(121)),
            Some("foo\nbar".into()),
        ] {
            assert!(validate(&SearchParams { q, mode: None }).is_err());
        }
        assert!(
            validate(&SearchParams {
                q: Some("곡명".into()),
                mode: Some("invalid".into())
            })
            .is_err()
        );
    }
}
