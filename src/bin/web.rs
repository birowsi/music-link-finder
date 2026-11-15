use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use music_link_finder::search::{SearchMode, search_youtube};
use serde::{Deserialize, Serialize};
use std::{env, net::SocketAddr, sync::Arc};

const PAGE: &str = include_str!("../../web/index.html");
const CSS: &str = include_str!("../../web/style.css");
const JS: &str = include_str!("../../web/app.js");
const FAVICON: &[u8] = include_bytes!("../../assets/app.ico");

#[derive(Clone)]
struct WebState {
    page: Arc<str>,
}

fn render_page() -> String {
    PAGE.replace("{{STYLESHEET_URL}}", &asset_path("style", "css", CSS))
        .replace("{{SCRIPT_URL}}", &asset_path("app", "js", JS))
}

fn asset_path(name: &str, extension: &str, content: &str) -> String {
    // Stable FNV-1a content fingerprint for cache identity, not security.
    // Different contents get a different path even if a CDN ignores query strings.
    let fingerprint = content
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("/assets/{name}.{fingerprint:016x}.{extension}")
}

fn text_asset(
    content: &'static str,
    content_type: &'static str,
    versioned: bool,
) -> impl IntoResponse {
    let cache = if versioned {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
        ],
        content,
    )
}

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
                "duration_seconds": item.duration_seconds,
                "duration_label": item.duration_label(),
                "is_live": item.is_live,
                "score": item.score, "matched": item.matched,
                "kind": item.kind,
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

async fn home(State(state): State<WebState>) -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-cache")],
        Html(state.page.to_string()),
    )
}

async fn stylesheet() -> impl IntoResponse {
    text_asset(CSS, "text/css; charset=utf-8", false)
}
async fn script() -> impl IntoResponse {
    text_asset(JS, "application/javascript; charset=utf-8", false)
}
async fn versioned_stylesheet() -> impl IntoResponse {
    text_asset(CSS, "text/css; charset=utf-8", true)
}
async fn versioned_script() -> impl IntoResponse {
    text_asset(JS, "application/javascript; charset=utf-8", true)
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

fn router(page: String) -> Router {
    Router::new()
        .route("/", get(home))
        .route(&asset_path("style", "css", CSS), get(versioned_stylesheet))
        .route(&asset_path("app", "js", JS), get(versioned_script))
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
        .with_state(WebState { page: page.into() })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_owned())
        .parse()?;
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    let app = router(render_page());
    println!("Music Link Finder listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_assets_get_new_paths_and_unchanged_assets_keep_the_same_path() {
        let blue = asset_path("style", "css", ":root { --accent: blue; }");
        let pink = asset_path("style", "css", ":root { --accent: pink; }");
        assert_ne!(blue, pink);
        assert_eq!(
            pink,
            asset_path("style", "css", ":root { --accent: pink; }")
        );
        assert!(pink.starts_with("/assets/style."));
        assert!(pink.ends_with(".css"));
        assert!(!pink.contains('?'));
    }

    #[tokio::test]
    async fn page_and_assets_keep_cache_policy_without_social_previews() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = router(render_page());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let response = client.get(&base).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()["content-type"],
            "text/html; charset=utf-8"
        );
        assert_eq!(response.headers()["cache-control"], "no-cache");
        let html = response.text().await.unwrap();
        assert!(html.contains(&format!(
            r#"<link rel="stylesheet" href="{}">"#,
            asset_path("style", "css", CSS)
        )));
        assert!(html.contains(&format!(
            r#"<script defer src="{}"></script>"#,
            asset_path("app", "js", JS)
        )));
        for removed in ["{{", "og:", "twitter:", r#"rel="canonical""#] {
            assert!(!html.contains(removed), "unexpected markup: {removed}");
        }
        for (path, content, content_type, cache) in [
            (
                "/style.css".to_owned(),
                CSS,
                "text/css; charset=utf-8",
                "no-cache",
            ),
            (
                "/app.js".to_owned(),
                JS,
                "application/javascript; charset=utf-8",
                "no-cache",
            ),
            (
                asset_path("style", "css", CSS),
                CSS,
                "text/css; charset=utf-8",
                "public, max-age=31536000, immutable",
            ),
            (
                asset_path("app", "js", JS),
                JS,
                "application/javascript; charset=utf-8",
                "public, max-age=31536000, immutable",
            ),
        ] {
            let response = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.headers()["content-type"], content_type);
            assert_eq!(response.headers()["cache-control"], cache);
            assert_eq!(response.text().await.unwrap(), content);
        }
        for path in [
            "/assets/style.0000000000000000.css",
            "/share/utalink-v1.png",
            "/robots.txt",
        ] {
            let response = client.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
        server.abort();
    }

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
