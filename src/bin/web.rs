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
const SHARE_IMAGE_PATH: &str = "/share/utalink-v1.png";
const SHARE_IMAGE: &[u8] = include_bytes!("../../web/og-image-v1.png");

#[derive(Clone)]
struct WebState {
    page: Arc<str>,
}

fn public_origin(
    custom: Option<&str>,
    render: Option<&str>,
    port: u16,
) -> Result<String, &'static str> {
    let configured = custom
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| render.map(str::trim).filter(|value| !value.is_empty()));
    let Some(configured) = configured else {
        return Ok(format!("http://localhost:{port}"));
    };
    let url = reqwest::Url::parse(configured)
        .map_err(|_| "PUBLIC_BASE_URL / RENDER_EXTERNAL_URL must be an absolute HTTP(S) origin")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "PUBLIC_BASE_URL / RENDER_EXTERNAL_URL must contain only scheme, host and optional port",
        );
    }
    // Never build canonical/image URLs from untrusted Host/Forwarded headers.
    Ok(url.origin().ascii_serialization())
}

fn render_page(origin: &str) -> String {
    let escaped = origin
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    PAGE.replace("{{PUBLIC_ORIGIN}}", &escaped)
        .replace("{{SHARE_IMAGE_PATH}}", SHARE_IMAGE_PATH)
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

async fn share_image() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        SHARE_IMAGE,
    )
}

async fn robots() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        "User-agent: *\nAllow: /\n",
    )
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

fn router(page: String) -> Router {
    Router::new()
        .route("/", get(home))
        .route("/style.css", get(stylesheet))
        .route("/app.js", get(script))
        .route("/favicon.ico", get(favicon))
        .route(SHARE_IMAGE_PATH, get(share_image))
        .route("/robots.txt", get(robots))
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
    let origin = public_origin(
        env::var("PUBLIC_BASE_URL").ok().as_deref(),
        env::var("RENDER_EXTERNAL_URL").ok().as_deref(),
        listener.local_addr()?.port(),
    )?;
    let app = router(render_page(&origin));
    println!("Music Link Finder listening on http://{address}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_origin_uses_custom_domain_then_render_then_local_port() {
        assert_eq!(
            public_origin(
                Some(" https://EXAMPLE.org:443/ "),
                Some("https://render.example"),
                3000
            )
            .unwrap(),
            "https://example.org"
        );
        assert_eq!(
            public_origin(
                Some(" "),
                Some("https://music-link-finder.onrender.com/"),
                3000
            )
            .unwrap(),
            "https://music-link-finder.onrender.com"
        );
        assert_eq!(
            public_origin(None, None, 4317).unwrap(),
            "http://localhost:4317"
        );
        assert_eq!(
            public_origin(None, Some(""), 4317).unwrap(),
            "http://localhost:4317"
        );
        assert_eq!(
            public_origin(Some("http://localhost:4000"), None, 3000).unwrap(),
            "http://localhost:4000"
        );
    }

    #[test]
    fn invalid_origins_fail_instead_of_emitting_broken_or_unsafe_urls() {
        for value in [
            "example.org",
            "file:///tmp/site",
            "javascript:alert(1)",
            "https://user:password@example.org",
            "https://example.org/subpath",
            "https://example.org/?q=1",
            "https://example.org/#fragment",
        ] {
            assert!(
                public_origin(Some(value), Some("https://valid.example"), 3000).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn rendered_metadata_escapes_attributes_and_resolves_every_placeholder() {
        let page = render_page("https://a&b.example");
        assert!(page.contains("https://a&amp;b.example/share/utalink-v1.png"));
        assert!(!page.contains("{{"));
        let page = render_page("https://invalid.example/\"><script>");
        assert!(page.contains("&quot;&gt;&lt;script&gt;"));
    }

    #[tokio::test]
    async fn preview_bots_receive_metadata_png_and_robots_over_http() {
        use std::collections::HashMap;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let origin = "https://music-link-finder.onrender.com";
        let app = router(render_page(origin));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let meta_pattern =
            regex::Regex::new(r#"<meta (?:name|property)="([^"]+)" content="([^"]*)">"#).unwrap();
        for bot in ["Discordbot/2.0", "Twitterbot/1.0"] {
            let response = client
                .get(&base)
                .header("user-agent", bot)
                .header("host", "untrusted.example")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()["content-type"],
                "text/html; charset=utf-8"
            );
            assert_eq!(response.headers()["cache-control"], "no-cache");
            let html = response.text().await.unwrap();
            let meta: HashMap<_, _> = meta_pattern
                .captures_iter(&html)
                .map(|capture| (capture[1].to_owned(), capture[2].to_owned()))
                .collect();
            assert_eq!(meta["og:url"], format!("{origin}/"));
            assert_eq!(meta["og:type"], "website");
            assert_eq!(meta["og:image"], format!("{origin}{SHARE_IMAGE_PATH}"));
            assert_eq!(meta["og:image:type"], "image/png");
            assert_eq!(meta["og:image:width"], "1200");
            assert_eq!(meta["og:image:height"], "630");
            assert_eq!(meta["twitter:card"], "summary_large_image");
            assert_eq!(meta["twitter:image"], meta["og:image"]);
            assert_eq!(meta["twitter:title"], meta["og:title"]);
            assert_eq!(meta["twitter:description"], meta["og:description"]);
            for key in [
                "og:title",
                "og:description",
                "og:image:alt",
                "twitter:image:alt",
            ] {
                assert!(!meta[key].is_empty());
            }
            assert!(html.contains(&format!(r#"<link rel="canonical" href="{origin}/">"#)));
            assert!(!html.contains("{{"));
            assert!(!html.contains("untrusted.example"));
        }
        let url = format!("{base}{SHARE_IMAGE_PATH}");
        let response = client
            .get(&url)
            .header("user-agent", "Twitterbot/1.0")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "image/png");
        assert!(
            response.headers()["cache-control"]
                .to_str()
                .unwrap()
                .contains("immutable")
        );
        let image = response.bytes().await.unwrap();
        assert_eq!(image.as_ref(), SHARE_IMAGE);
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(u32::from_be_bytes(image[16..20].try_into().unwrap()), 1200);
        assert_eq!(u32::from_be_bytes(image[20..24].try_into().unwrap()), 630);
        assert!(image.len() < 5 * 1024 * 1024);
        let head = client
            .head(&url)
            .header("user-agent", "Discordbot/2.0")
            .send()
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()["content-type"], "image/png");
        assert_eq!(
            head.headers()["content-length"],
            SHARE_IMAGE.len().to_string()
        );
        assert!(head.bytes().await.unwrap().is_empty());
        let robots = client
            .get(format!("{base}/robots.txt"))
            .send()
            .await
            .unwrap();
        assert_eq!(robots.status(), StatusCode::OK);
        assert_eq!(robots.text().await.unwrap(), "User-agent: *\nAllow: /\n");
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
