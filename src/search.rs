use regex::Regex;
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchMode {
    Pronunciation,
    Karaoke,
}

/// Evidence in the title, not a guarantee about the video's actual contents.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoKind {
    Pronunciation,
    Subtitle,
    Karaoke,
    General,
}

impl VideoKind {
    fn satisfies(self, mode: SearchMode) -> bool {
        matches!(
            (self, mode),
            (Self::Pronunciation, SearchMode::Pronunciation) | (Self::Karaoke, SearchMode::Karaoke)
        )
    }
}

#[derive(Clone, Debug)]
pub struct VideoResult {
    pub title: String,
    pub url: String,
    pub channel: String,
    pub thumbnail_url: String,
    pub score: i32,
    pub matched: Vec<String>,
    pub kind: VideoKind,
    pub is_recommendation: bool,
    pub is_fallback: bool,
}

impl VideoResult {
    fn raw(title: String, video_id: String, channel: String) -> Self {
        Self {
            title,
            url: format!("https://www.youtube.com/watch?v={video_id}"),
            channel,
            thumbnail_url: format!("https://i.ytimg.com/vi/{video_id}/mqdefault.jpg"),
            score: 0,
            matched: Vec::new(),
            kind: VideoKind::General,
            is_recommendation: false,
            is_fallback: false,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        if self.is_fallback {
            "일반 결과"
        } else if self.is_recommendation {
            if self.kind == VideoKind::Subtitle {
                "관련 추천 · 자막"
            } else {
                "관련 추천"
            }
        } else {
            match self.kind {
                VideoKind::Pronunciation => "한글 발음",
                VideoKind::Subtitle => "자막",
                VideoKind::Karaoke => "노래방 반주",
                VideoKind::General => "일반 결과",
            }
        }
    }
}

const PRONUNCIATION_TERMS: &[&str] = &[
    "한글발음",
    "한글 발음",
    "한국어발음",
    "한국어 발음",
    "발음가사",
    "발음 가사",
    "한글독음",
    "한글 독음",
    "발음",
    "독음",
];

const SUBTITLE_TERMS: &[&str] = &[
    "한글자막",
    "한글 자막",
    "한국어자막",
    "한국어 자막",
    "자체자막",
    "자체 자막",
];

const OPENING_TERMS: &[&str] = &["op", "오프닝", "opening"];
const ENDING_TERMS: &[&str] = &["ed", "엔딩", "ending"];
const KARAOKE_TERMS: &[&str] = &["karaoke", "카라오케", "노래방", "반주", "mr"];
const KOREAN_KARAOKE_MARKERS: &[&str] = &["ky", "tj", "kara-u", "금영", "태진"];
const ORIGINAL_MARKERS: &[&str] = &["official", "original", "공식", "公式"];
const PERFORMANCE_MARKERS: &[&str] = &[
    "live",
    "라이브",
    "실황",
    "concert",
    "노래방에서",
    "불러보았다",
    "불러 보았다",
    "불러봤",
    "불러 봤",
    "불러보았습니다",
    "歌ってみた",
    "singing cover",
    "korean cover",
    "korean ver",
    "한국어버전",
    "한국어 버전",
    "커버",
    "cover",
    "우타이테",
    "오르골",
    "리코더",
    "instrumental",
    "bass cover",
    "패러디",
    "란란루",
    "mad",
    "원곡:",
    "원곡：",
];
const DIRECT_LIMIT: usize = 8;
const RECOMMENDATION_LIMIT: usize = 12;
const RESULT_LIMIT: usize = DIRECT_LIMIT + RECOMMENDATION_LIMIT;

#[derive(Clone, Debug)]
struct SearchProfile {
    expanded_title: String,
    media_kind: Option<&'static str>,
    season: Option<u32>,
    // Each entry is a complete song name/alias, never an artist or a fragment.
    identity_tokens: Vec<String>,
    recommendation_context: Option<&'static str>,
    context_aliases: Vec<String>,
    is_work_query: bool,
}

pub fn normalize(value: &str) -> String {
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn token_regex() -> &'static Regex {
    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    TOKEN_RE.get_or_init(|| Regex::new(r"[\p{L}\p{N}]+").unwrap())
}

fn query_tokens(value: &str) -> Vec<String> {
    token_regex()
        .find_iter(&normalize(value))
        .map(|m| m.as_str().to_owned())
        .collect()
}

/// Match a complete name across punctuation/spacing, without matching fragments
/// such as `love` in `clover` or `노래` in `노래방`. Single-character songs count.
fn matches_name(title: &str, name: &str) -> bool {
    let target = query_tokens(name).concat();
    if target.is_empty() {
        return false;
    }
    let words = query_tokens(title);
    for start in 0..words.len() {
        let mut candidate = String::new();
        for word in &words[start..] {
            candidate.push_str(word);
            if candidate == target {
                return true;
            }
            if candidate.len() >= target.len() {
                break;
            }
        }
    }
    false
}

fn has_any(title: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| contains_term(title, term))
}

fn media_kind(title: &str) -> Option<&'static str> {
    match (has_any(title, OPENING_TERMS), has_any(title, ENDING_TERMS)) {
        (true, false) => Some("op"),
        (false, true) => Some("ed"),
        _ => None,
    }
}

fn season_regex() -> &'static Regex {
    static SEASONS: OnceLock<Regex> = OnceLock::new();
    SEASONS.get_or_init(|| Regex::new(
        r"(?:제\s*)?([0-9]+)\s*기|(?:season|시즌)\s*([0-9]+)|\bs([0-9]+)\b|\b([0-9]+)(?:st|nd|rd|th)\s+season\b"
    ).unwrap())
}

fn seasons(title: &str) -> HashSet<u32> {
    season_regex()
        .captures_iter(&normalize(title))
        .filter_map(|capture| {
            capture
                .iter()
                .skip(1)
                .flatten()
                .next()?
                .as_str()
                .parse()
                .ok()
        })
        .collect()
}

fn is_ascii_alnum_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

fn contains_term(title: &str, term: &str) -> bool {
    let title = normalize(title);
    let term = normalize(term);
    if term.is_ascii() && term.chars().all(|c| c.is_ascii_alphanumeric()) {
        for (start, _) in title.match_indices(&term) {
            let bytes = title.as_bytes();
            let end = start + term.len();
            let left_ok = start == 0 || !is_ascii_alnum_byte(bytes[start - 1]);
            let right_ok = end == bytes.len() || !is_ascii_alnum_byte(bytes[end]);
            let mr_dot_guard = term != "mr"
                || end + 1 >= bytes.len()
                || bytes[end] != b'.'
                || !bytes[end + 1].is_ascii_alphabetic();
            if left_ok && right_ok && mr_dot_guard {
                return true;
            }
        }
        false
    } else {
        title.contains(&term)
    }
}

fn matching_terms(title: &str, terms: &[&str]) -> Vec<String> {
    terms
        .iter()
        .filter(|term| contains_term(title, term))
        .map(|term| (*term).to_owned())
        .collect()
}

fn expand_search_title(song_title: &str) -> Result<SearchProfile, String> {
    let mut profile = expand_alias_title(song_title)?;
    profile.media_kind = media_kind(song_title).or(profile.media_kind);
    let requested_seasons = seasons(song_title);
    if requested_seasons.len() == 1 {
        profile.season = requested_seasons.into_iter().next();
    }
    if profile.identity_tokens.is_empty() {
        let normalized = normalize(&profile.expanded_title);
        let without_season = season_regex().replace_all(&normalized, " ");
        let names: Vec<_> = query_tokens(&without_season)
            .into_iter()
            .filter(|token| {
                !matches!(
                    token.as_str(),
                    "op" | "ed"
                        | "오프닝"
                        | "엔딩"
                        | "opening"
                        | "ending"
                        | "official"
                        | "original"
                        | "원곡"
                        | "해석"
                        | "번역"
                        | "lyrics"
                        | "lyric"
                        | "full"
                        | "hd"
                        | "mv"
                        | "music"
                        | "video"
                        | "한글발음"
                        | "한국어발음"
                        | "한글독음"
                        | "한글"
                        | "한국어"
                        | "발음"
                        | "독음"
                        | "자막"
                        | "한글자막"
                        | "한국어자막"
                        | "가사"
                        | "노래방"
                        | "karaoke"
                        | "카라오케"
                        | "반주"
                        | "mr"
                )
            })
            .collect();
        let name = names.join(" ");
        if !name.is_empty() {
            profile.is_work_query = profile.media_kind.is_some() || profile.season.is_some();
            if profile.is_work_query {
                profile.context_aliases = vec![name];
            } else {
                profile.identity_tokens = vec![name];
            }
        }
    }
    Ok(profile)
}

fn expand_alias_title(song_title: &str) -> Result<SearchProfile, String> {
    let clean = song_title.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return Err("음악 제목을 입력해 주세요.".to_owned());
    }
    if matches_name(&clean, "키미노마치마데") {
        return Ok(alias_profile(
            "키미노마치마데 너의 거리까지 君の街まで ASIAN KUNG-FU GENERATION 아지캉",
            &[
                "키미노마치마데",
                "키미노 마치마데",
                "너의 거리까지",
                "君の街まで",
            ],
            "ASIAN KUNG-FU GENERATION 아지캉",
            &["ASIAN KUNG-FU GENERATION", "아지캉"],
        ));
    }

    if matches_name(&clean, "두근어질") {
        return Ok(alias_profile(
            "두근어질 きゅうくらりん Kyu-kurarin 이요와 iyowa",
            &[
                "두근어질",
                "きゅうくらりん",
                "kyu-kurarin",
                "kyu kurarin",
                "큐쿠라린",
            ],
            "이요와 iyowa",
            &["이요와", "iyowa"],
        ));
    }

    if matches_name(&clean, "잇떼") || matches_name(&clean, "잇테") {
        return Ok(alias_profile(
            "잇떼 말해줘 言って 요루시카",
            &["잇떼", "잇테", "말해줘", "言って"],
            "요루시카 ヨルシカ",
            &["요루시카", "ヨルシカ", "yorushika"],
        ));
    }

    if matches_name(&clean, "dscf") {
        return Ok(alias_profile(
            "DSCF 망상감상대상연맹 妄想感傷代償連盟 DECO*27",
            &["dscf", "망상감상대상연맹", "妄想感傷代償連盟"],
            "DECO*27",
            &["DECO*27", "데코니나"],
        ));
    }

    if matches_name(&clean, "토키메키 포포롱") {
        return Ok(media_profile(
            "토키메키 포포론 두근두근 포포롱 주문은 토끼입니까 2기 ED",
            "ed",
            &[
                "토키메키 포포론",
                "토키메키 포포롱",
                "두근두근 포포롱",
                "ときめきポポロン",
            ],
        ));
    }

    let expanded = clean.replace("주문토끼", "주문은 토끼입니까");
    let lowered = normalize(&expanded);
    if lowered.contains("주문은 토끼입니까") && seasons(&lowered).contains(&2) {
        if media_kind(&lowered) == Some("op") {
            return Ok(media_profile(
                "No Poi 주문은 토끼입니까 2기 OP",
                "op",
                &["No Poi", "ノーポイッ", "노포이"],
            ));
        }
        if media_kind(&lowered) == Some("ed") {
            return Ok(media_profile(
                "토키메키 포포론 주문은 토끼입니까 2기 ED",
                "ed",
                &[
                    "토키메키 포포론",
                    "토키메키 포포롱",
                    "두근두근 포포롱",
                    "ときめきポポロン",
                ],
            ));
        }
    }

    Ok(SearchProfile {
        expanded_title: expanded.clone(),
        media_kind: None,
        season: None,
        identity_tokens: Vec::new(),
        recommendation_context: None,
        context_aliases: Vec::new(),
        is_work_query: false,
    })
}

fn media_profile(title: &str, kind: &'static str, identities: &[&str]) -> SearchProfile {
    SearchProfile {
        expanded_title: title.to_owned(),
        media_kind: Some(kind),
        season: Some(2),
        identity_tokens: identities.iter().map(|item| (*item).to_owned()).collect(),
        recommendation_context: Some("주문은 토끼입니까 주제가"),
        context_aliases: vec![
            "주문은 토끼입니까".into(),
            "주문토끼".into(),
            "ご注文はうさぎですか".into(),
        ],
        is_work_query: true,
    }
}

fn alias_profile(
    title: &str,
    identities: &[&str],
    artist: &'static str,
    artists: &[&str],
) -> SearchProfile {
    SearchProfile {
        expanded_title: title.to_owned(),
        media_kind: None,
        season: None,
        identity_tokens: identities.iter().map(|item| normalize(item)).collect(),
        recommendation_context: Some(artist),
        context_aliases: artists.iter().map(|item| normalize(item)).collect(),
        is_work_query: false,
    }
}

fn build_queries(profile: &SearchProfile, mode: SearchMode) -> Vec<String> {
    let title = &profile.expanded_title;
    let context = profile.recommendation_context.unwrap_or(title);
    match mode {
        SearchMode::Karaoke => vec![
            format!("{title} 한글발음 노래방 karaoke"),
            format!("{title} 노래방 카라오케 반주"),
            format!("{context} 노래방 카라오케 한글발음"),
        ],
        SearchMode::Pronunciation if profile.media_kind.is_some() => vec![
            format!("{title} 한글자막 가사 발음"),
            format!("{title} 애니 주제가"),
            format!("{context} 한글발음 가사 독음"),
        ],
        SearchMode::Pronunciation => vec![
            format!("{title} 한글발음 lyrics"),
            format!("{title} 원곡 한국어해석 독음 일본어 자막"),
            format!("{context} 한글발음 가사 독음"),
        ],
    }
}

fn text_from_renderer(value: &Value) -> String {
    if let Some(text) = value.get("simpleText").and_then(Value::as_str) {
        return text.to_owned();
    }
    value
        .get("runs")
        .and_then(Value::as_array)
        .map(|runs| {
            runs.iter()
                .filter_map(|run| run.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn collect_video_renderers(
    value: &Value,
    output: &mut Vec<VideoResult>,
    seen: &mut HashSet<String>,
) {
    match value {
        Value::Object(map) => {
            if let Some(renderer) = map.get("videoRenderer")
                && let Some(video_id) = renderer.get("videoId").and_then(Value::as_str)
                && seen.insert(video_id.to_owned())
            {
                let title = text_from_renderer(&renderer["title"]);
                let channel = text_from_renderer(&renderer["ownerText"]);
                if !title.is_empty() {
                    output.push(VideoResult::raw(
                        title,
                        video_id.to_owned(),
                        if channel.is_empty() {
                            "채널 정보 없음".to_owned()
                        } else {
                            channel
                        },
                    ));
                }
            }
            for child in map.values() {
                collect_video_renderers(child, output, seen);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_video_renderers(child, output, seen);
            }
        }
        _ => {}
    }
}

fn extract_balanced_json(source: &str, object_start: usize) -> Option<&str> {
    let bytes = source.as_bytes();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate().skip(object_start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return source.get(object_start..=index);
                }
            }
            _ => {}
        }
    }
    None
}

fn initial_data_json(html: &str) -> Result<Value, String> {
    const MARKERS: &[&str] = &["var ytInitialData =", "ytInitialData ="];
    for marker in MARKERS {
        if let Some(marker_pos) = html.find(marker) {
            let search_from = marker_pos + marker.len();
            if let Some(relative_start) = html[search_from..].find('{') {
                let object_start = search_from + relative_start;
                if let Some(json_text) = extract_balanced_json(html, object_start) {
                    return serde_json::from_str(json_text)
                        .map_err(|error| format!("YouTube 검색 JSON 해석 실패: {error}"));
                }
            }
        }
    }
    Err("YouTube 검색 데이터가 응답에 없습니다.".to_owned())
}

fn search_query(client: &Client, query: &str, limit: usize) -> Result<Vec<VideoResult>, String> {
    let html = client
        .get("https://www.youtube.com/results")
        .query(&[("search_query", query), ("hl", "ko"), ("gl", "KR")])
        .send()
        .map_err(|error| format!("YouTube 연결 실패: {error}"))?
        .error_for_status()
        .map_err(|error| format!("YouTube HTTP 오류: {error}"))?
        .text()
        .map_err(|error| format!("YouTube 응답 읽기 실패: {error}"))?;

    let data = initial_data_json(&html)?;
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    collect_video_renderers(&data, &mut results, &mut seen);
    results.truncate(limit);
    Ok(results)
}

fn classify(title: &str, mode: SearchMode) -> Option<VideoKind> {
    match mode {
        SearchMode::Karaoke => has_any(title, KARAOKE_TERMS).then_some(VideoKind::Karaoke),
        SearchMode::Pronunciation if has_any(title, PRONUNCIATION_TERMS) => {
            Some(VideoKind::Pronunciation)
        }
        SearchMode::Pronunciation if has_any(title, SUBTITLE_TERMS) => Some(VideoKind::Subtitle),
        _ => None,
    }
}

fn media_conflicts(title: &str, profile: &SearchProfile) -> bool {
    let opposite = match profile.media_kind {
        Some("op") => ENDING_TERMS,
        Some("ed") => OPENING_TERMS,
        _ => &[],
    };
    has_any(title, opposite)
        || profile
            .season
            .is_some_and(|season| seasons(title).iter().any(|found| *found != season))
}

fn is_song_match(title: &str, profile: &SearchProfile) -> bool {
    if media_conflicts(title, profile) {
        return false;
    }
    if profile
        .identity_tokens
        .iter()
        .any(|name| matches_name(title, name))
    {
        return true;
    }
    // Work names alone do not identify a song. A work query also needs positive
    // evidence for every requested descriptor, not merely an absence of conflicts.
    let normalized = normalize(title);
    let without_season = season_regex().replace_all(&normalized, " ");
    profile.is_work_query
        && profile
            .context_aliases
            .iter()
            .any(|name| matches_name(&without_season, name))
        && profile
            .media_kind
            .is_none_or(|kind| media_kind(title) == Some(kind))
        && profile
            .season
            .is_none_or(|season| seasons(title).contains(&season))
}

fn rank_results(
    results: Vec<VideoResult>,
    profile: &SearchProfile,
    mode: SearchMode,
) -> Vec<VideoResult> {
    let mut direct = Vec::new();
    let mut recommendations = Vec::new();
    for (position, mut result) in results.into_iter().enumerate() {
        let Some(kind) = classify(&result.title, mode) else {
            continue;
        };
        let is_exact = is_song_match(&result.title, profile);
        let context = format!("{} {}", result.title, result.channel);
        let related = profile.context_aliases.iter().any(|name| {
            let normalized = normalize(&result.title);
            let without_season = season_regex().replace_all(&normalized, " ");
            matches_name(&without_season, name) || matches_name(&result.channel, name)
        });
        // Unknown songs get no recommendations based on shared generic words.
        if !is_exact && !related {
            continue;
        }

        let performance = PERFORMANCE_MARKERS.iter().any(|term| {
            // Instrumental is normal for backing tracks, not a cover penalty.
            !(mode == SearchMode::Karaoke && *term == "instrumental")
                && contains_term(&result.title, term)
        });
        let preferred_source = match mode {
            SearchMode::Pronunciation => has_any(&result.title, ORIGINAL_MARKERS),
            SearchMode::Karaoke => has_any(&context, KOREAN_KARAOKE_MARKERS),
        };
        // Bounded tiers: lower-priority evidence can never outweigh song/type
        // identity, studio-vs-performance, or source preference. Score is a sort
        // key, not a confidence percentage. Stable sorting preserves input ties.
        result.score = i32::from(is_exact) * 100_000
            + i32::from(kind.satisfies(mode)) * 10_000
            + i32::from(!performance) * 1_000
            + i32::from(preferred_source) * 100
            + (99 - position.min(99)) as i32;
        result.kind = kind;
        result.is_recommendation = !is_exact;
        result.matched = match kind {
            VideoKind::Pronunciation => matching_terms(&result.title, PRONUNCIATION_TERMS),
            VideoKind::Subtitle => matching_terms(&result.title, SUBTITLE_TERMS),
            VideoKind::Karaoke => matching_terms(&result.title, KARAOKE_TERMS),
            VideoKind::General => Vec::new(),
        };
        if is_exact {
            direct.push(result);
        } else {
            recommendations.push(result);
        }
    }
    direct.sort_by_key(|result| std::cmp::Reverse(result.score));
    recommendations.sort_by_key(|result| std::cmp::Reverse(result.score));
    direct.truncate(DIRECT_LIMIT);
    recommendations.truncate(RECOMMENDATION_LIMIT);
    direct.extend(recommendations);
    direct
}

pub fn search_youtube(
    song_title: &str,
    mode: SearchMode,
    limit: usize,
) -> Result<Vec<VideoResult>, String> {
    let profile = expand_search_title(song_title)?;
    let mut queries = build_queries(&profile, mode);
    // Keep the user's unfiltered YouTube search available for a graceful fallback.
    queries.push(song_title.trim().to_owned());
    let client = Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/138 Safari/537.36",
        )
        .build()
        .map_err(|error| format!("HTTP 클라이언트 생성 실패: {error}"))?;

    let handles: Vec<_> = queries
        .into_iter()
        .map(|query| {
            let client = client.clone();
            thread::spawn(move || search_query(&client, &query, limit))
        })
        .collect();

    let mut batches = Vec::new();
    let mut errors = Vec::new();
    for handle in handles {
        match handle.join() {
            Ok(Ok(results)) => batches.push(results),
            Ok(Err(error)) => {
                errors.push(error);
                batches.push(Vec::new());
            }
            Err(_) => {
                errors.push("검색 작업이 비정상 종료됐습니다.".to_owned());
                batches.push(Vec::new());
            }
        }
    }
    if batches.iter().all(Vec::is_empty) {
        return Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| "검색 결과가 없습니다.".to_owned()));
    }

    let general = batches.pop().unwrap_or_default();
    let max_len = batches.iter().map(Vec::len).max().unwrap_or(0);
    let mut interleaved = Vec::new();
    let mut seen = HashSet::new();
    for position in 0..max_len {
        for batch in &batches {
            if let Some(result) = batch.get(position)
                && seen.insert(result.url.clone())
            {
                interleaved.push(result.clone());
            }
        }
    }
    Ok(with_general_fallback(
        rank_results(interleaved, &profile, mode),
        general,
        mode,
    ))
}

fn with_general_fallback(
    ranked: Vec<VideoResult>,
    general: Vec<VideoResult>,
    mode: SearchMode,
) -> Vec<VideoResult> {
    if ranked
        .iter()
        .any(|item| !item.is_recommendation && item.kind.satisfies(mode))
    {
        return ranked;
    }
    // Preserve identified subtitle candidates as subtitles, even when the same
    // URL also appears in the general search. Other general results keep order.
    let subtitles: HashSet<_> = ranked
        .iter()
        .filter(|item| !item.is_recommendation)
        .map(|item| item.url.clone())
        .collect();
    let mut seen = subtitles;
    let mut output: Vec<_> = general
        .into_iter()
        .filter(|item| seen.insert(item.url.clone()))
        .take(DIRECT_LIMIT)
        .map(|mut item| {
            item.is_fallback = true;
            item.is_recommendation = false;
            item.kind = VideoKind::General;
            item.score = 0;
            item.matched = vec!["일반 YouTube 검색".into()];
            item
        })
        .collect();
    // Subtitle URLs were reserved above; deduplicate the final append separately.
    let mut seen: HashSet<_> = output.iter().map(|item| item.url.clone()).collect();
    output.extend(
        ranked
            .into_iter()
            .filter(|item| seen.insert(item.url.clone())),
    );
    output.truncate(RESULT_LIMIT);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str, title: &str) -> VideoResult {
        VideoResult::raw(title.into(), id.into(), "채널".into())
    }

    #[test]
    fn search_policy_fixtures() {
        #[derive(serde::Deserialize)]
        struct Case {
            name: String,
            query: String,
            mode: String,
            candidates: Vec<(String, String, String)>,
            general: Vec<(String, String, String)>,
            expected: Value,
        }
        fn videos(rows: Vec<(String, String, String)>) -> Vec<VideoResult> {
            rows.into_iter()
                .map(|(id, title, channel)| VideoResult::raw(title, id, channel))
                .collect()
        }
        let cases: Vec<Case> =
            serde_json::from_str(include_str!("../tests/fixtures/search-policy.json")).unwrap();
        for case in cases {
            let mode = match case.mode.as_str() {
                "pronunciation" => SearchMode::Pronunciation,
                "karaoke" => SearchMode::Karaoke,
                _ => panic!("Unknown fixture mode"),
            };
            let profile = expand_search_title(&case.query).unwrap();
            let output = with_general_fallback(
                rank_results(videos(case.candidates), &profile, mode),
                videos(case.general),
                mode,
            );
            let actual: Vec<_> = output
                .iter()
                .map(|item| {
                    serde_json::json!([
                        item.url.split("?v=").nth(1).unwrap(),
                        item.kind,
                        item.is_recommendation,
                        item.is_fallback
                    ])
                })
                .collect();
            assert_eq!(serde_json::json!(actual), case.expected, "{}", case.name);
        }
    }

    #[test]
    fn complete_names_handle_spacing_punctuation_and_unicode_without_substring_matches() {
        for (title, name) in [
            ("[한글발음] 작은사랑의노래", "작은 사랑의 노래"),
            ("【ＤＳＣＦ】 한글발음", "dscf"),
            ("ASIAN KUNG-FU GENERATION", "asian kung fu generation"),
            ("[踊] 한글발음", "踊"),
            ("I 한글발음", "i"),
        ] {
            assert!(matches_name(title, name), "{title} / {name}");
        }
        for (title, name) in [
            ("clover 한글발음", "love"),
            ("봄날 한글발음", "봄"),
            ("두근두근 다른 곡 한글발음", "두근두근 포포롱"),
            ("작은 사랑의 다른 노래 한글발음", "작은 사랑의 노래"),
        ] {
            assert!(!matches_name(title, name), "{title} / {name}");
        }
    }

    #[test]
    fn english_markers_do_not_match_inside_song_names() {
        assert!(!contains_term("Believe 한글발음", "live"));
        assert!(!contains_term("unofficial 한글발음", "official"));
        assert!(!contains_term("nonkaraoke", "karaoke"));
        assert!(contains_term("【LIVE】 한글발음", "live"));
    }

    #[test]
    fn artist_prefixes_keep_alias_expansion() {
        let profile = expand_search_title("요루시카 잇떼 한글발음").unwrap();
        assert!(is_song_match("言って 한글발음", &profile));
        let profile = expand_search_title("이요와 두근 어질").unwrap();
        assert!(is_song_match("きゅうくらりん 한글발음", &profile));
    }

    #[test]
    fn aliases_inside_longer_words_do_not_replace_the_requested_song() {
        for (query, other_song) in [
            ("키미노마치마데라는다른곡", "君の街まで 한글발음"),
            ("토키메키포포롱이라는다른곡", "토키메키 포포론 한글발음"),
        ] {
            let profile = expand_search_title(query).unwrap();
            assert!(!is_song_match(other_song, &profile), "{query}");
            assert!(is_song_match(&format!("{query} 한글발음"), &profile));
        }
    }

    #[test]
    fn generic_search_modifiers_do_not_become_part_of_the_song_identity() {
        let profile = expand_search_title("작은 사랑의 노래 official 한글 발음").unwrap();
        assert!(is_song_match("작은 사랑의 노래 한글발음", &profile));
        assert!(!is_song_match(
            "다른 사랑의 노래 official 한글발음",
            &profile
        ));
        assert!(expand_search_title("   ").is_err());
        let profile = expand_search_title("한글발음 official").unwrap();
        assert!(!is_song_match("아무 곡 한글발음 official", &profile));
    }

    #[test]
    fn explicit_metadata_conflicts_override_known_song_aliases() {
        let profile = expand_search_title("주문토끼 2기 op").unwrap();
        for title in [
            "No Poi 1기 OP 한글발음",
            "No Poi 2기 ED 한글발음",
            "No Poi OP ED 한글발음",
            "No Poi 1기 2기 OP 한글발음",
        ] {
            assert!(!is_song_match(title, &profile), "{title}");
        }
        assert!(is_song_match("No Poi 한글발음", &profile));
        assert!(is_song_match("주문토끼2기 OP 한글발음", &profile));
        assert!(!is_song_match("주문토끼 OP 한글발음", &profile));
        assert!(!is_song_match("주문토끼 2기 한글발음", &profile));
    }

    #[test]
    fn season_notations_and_generic_work_constraints_are_supported() {
        for notation in [
            "2기",
            "제 2 기",
            "season 2",
            "시즌2",
            "S2",
            "2nd season",
            "Ｓ２",
        ] {
            assert_eq!(seasons(notation), HashSet::from([2]), "{notation}");
        }
        assert_eq!(seasons("12기"), HashSet::from([12]));
        assert!(seasons("s2nd 20시간").is_empty());
        let profile = expand_search_title("새 애니 3기 OP").unwrap();
        assert!(is_song_match("새 애니 season 3 opening 한글발음", &profile));
        assert!(!is_song_match("새 애니 3기 ED 한글발음", &profile));
    }

    #[test]
    fn context_is_not_assembled_across_title_and_channel_fragments() {
        let profile = expand_search_title("키미노마치마데").unwrap();
        let candidates = vec![VideoResult::raw(
            "ASIAN KUNG 한글발음".into(),
            "a".into(),
            "FU GENERATION".into(),
        )];
        assert!(rank_results(candidates, &profile, SearchMode::Pronunciation).is_empty());
    }

    #[test]
    fn subtitle_and_media_words_never_count_as_pronunciation_evidence() {
        for title in ["No Poi OP", "No Poi ED 가사", "No Poi 애니 주제가"] {
            assert_eq!(classify(title, SearchMode::Pronunciation), None);
        }
        assert_eq!(
            classify("No Poi 한글자막", SearchMode::Pronunciation),
            Some(VideoKind::Subtitle)
        );
        assert_eq!(
            classify("No Poi 한글자막 발음", SearchMode::Pronunciation),
            Some(VideoKind::Pronunciation)
        );
        assert_eq!(classify("No Poi 한글발음", SearchMode::Karaoke), None);
    }

    #[test]
    fn empty_general_search_keeps_subtitles_then_related_results() {
        let profile = expand_search_title("잇떼").unwrap();
        let ranked = rank_results(
            vec![
                video("rec", "요루시카 다른 곡 한글발음"),
                video("sub", "言って 한글자막"),
            ],
            &profile,
            SearchMode::Pronunciation,
        );
        let output = with_general_fallback(ranked, vec![], SearchMode::Pronunciation);
        assert_eq!(output.len(), 2);
        assert_eq!(output[0].kind, VideoKind::Subtitle);
        assert!(!output[0].is_recommendation);
        assert!(output[1].is_recommendation);
    }

    #[test]
    fn fallback_deduplicates_general_urls_before_applying_the_limit() {
        let mut general = vec![video("a", "원곡"), video("a", "원곡 중복")];
        general.extend((0..10).map(|i| video(&format!("g{i}"), "뮤직비디오")));
        let output = with_general_fallback(vec![], general, SearchMode::Karaoke);
        assert_eq!(output.len(), 8);
        assert_eq!(output[0].url, video("a", "").url);
        assert_eq!(output[7].url, video("g6", "").url);
        assert!(
            output
                .iter()
                .all(|item| item.is_fallback && item.kind == VideoKind::General)
        );
    }

    #[test]
    fn result_limits_and_primary_selection_survive_many_recommendations() {
        let profile = expand_search_title("잇떼").unwrap();
        let mut candidates: Vec<_> = (0..20)
            .map(|i| video(&format!("r{i}"), "요루시카 다른 곡 official 한글발음"))
            .collect();
        candidates.extend((0..12).map(|i| video(&format!("s{i}"), "言って 한글자막")));
        let ranked = rank_results(candidates.clone(), &profile, SearchMode::Pronunciation);
        assert_eq!(ranked.len(), 20);
        let general = (0..10)
            .map(|i| video(&format!("g{i}"), "일반 영상"))
            .collect();
        let output = with_general_fallback(ranked, general, SearchMode::Pronunciation);
        assert_eq!(output.len(), 20);
        assert!(output[..8].iter().all(|item| item.is_fallback));
        assert!(
            output[8..16]
                .iter()
                .all(|item| item.kind == VideoKind::Subtitle)
        );
        assert!(output[16..].iter().all(|item| item.is_recommendation));
        candidates.push(video("primary", "말해줘 LIVE 한글발음"));
        let ranked = rank_results(candidates, &profile, SearchMode::Pronunciation);
        let output = with_general_fallback(
            ranked,
            vec![video("g", "일반 영상")],
            SearchMode::Pronunciation,
        );
        assert!(output[0].url.ends_with("primary"));
        assert!(!output.iter().any(|item| item.is_fallback));
    }

    #[test]
    fn stable_ties_keep_the_merged_youtube_order() {
        let profile = expand_search_title("잇떼").unwrap();
        let mut candidates: Vec<_> = (0..100)
            .map(|i| video(&format!("skip{i}"), "무관한 영상"))
            .collect();
        candidates.extend([video("a", "言って 한글발음"), video("b", "言って 한글발음")]);
        let ranked = rank_results(candidates, &profile, SearchMode::Pronunciation);
        assert_eq!(ranked[0].score, ranked[1].score);
        assert!(ranked[0].url.ends_with('a'));
        assert!(ranked[1].url.ends_with('b'));
    }

    #[test]
    fn result_kind_serialization_and_labels_distinguish_subtitles() {
        let mut result = video("sub", "말해줘 한글자막");
        result.kind = VideoKind::Subtitle;
        assert_eq!(serde_json::to_value(result.kind).unwrap(), "subtitle");
        assert_eq!(result.kind_label(), "자막");
        result.is_recommendation = true;
        assert_eq!(result.kind_label(), "관련 추천 · 자막");
        result.is_fallback = true;
        assert_eq!(result.kind_label(), "일반 결과");
    }

    #[test]
    fn expands_media_aliases() {
        let op = expand_search_title("주문토끼 2기 op").unwrap();
        assert!(op.expanded_title.contains("No Poi"));
        let ed = expand_search_title("토키메키 포포롱").unwrap();
        assert!(ed.expanded_title.contains("포포론"));
        let iyowa = expand_search_title("두근어질").unwrap();
        assert!(iyowa.expanded_title.contains("きゅうくらりん"));
        assert!(iyowa.identity_tokens.contains(&"きゅうくらりん".to_owned()));
        let ajikan = expand_search_title("아지캉 키미노마치마데").unwrap();
        assert!(ajikan.expanded_title.contains("君の街まで"));
    }

    #[test]
    fn korean_mr_boundary_does_not_match_mr_children() {
        assert!(contains_term("Hello 한글 발음 MR", "mr"));
        assert!(!contains_term("Mr.Children 한글 발음", "mr"));
    }

    #[test]
    fn recommendations_follow_the_song_artist_and_mode() {
        for mode in [SearchMode::Pronunciation, SearchMode::Karaoke] {
            let iyowa = build_queries(&expand_search_title("두근어질").unwrap(), mode);
            let yorushika = build_queries(&expand_search_title("잇떼").unwrap(), mode);
            assert!(iyowa[2].contains("이요와"));
            assert!(yorushika[2].contains("요루시카"));
            assert_ne!(iyowa[2], yorushika[2]);
            let unknown = build_queries(&expand_search_title("처음 듣는 노래").unwrap(), mode);
            assert!(unknown.iter().all(|query| query.contains("처음 듣는 노래")));
        }
    }

    #[test]
    fn missing_mode_matches_show_general_youtube_order_before_related_results() {
        let general = vec![
            VideoResult::raw("원곡".into(), "aaaaaaaaaaa".into(), "가수".into()),
            VideoResult::raw("뮤직비디오".into(), "bbbbbbbbbbb".into(), "가수".into()),
        ];
        let mut related = VideoResult::raw(
            "다른 곡 한글발음".into(),
            "ccccccccccc".into(),
            "가수".into(),
        );
        related.is_recommendation = true;
        let mut duplicate = general[0].clone();
        duplicate.is_recommendation = true;
        let output =
            with_general_fallback(vec![duplicate, related], general, SearchMode::Pronunciation);
        assert_eq!(output.len(), 3);
        assert!(output[0].url.ends_with("aaaaaaaaaaa"));
        assert!(output[1].url.ends_with("bbbbbbbbbbb"));
        assert!(
            output[..2]
                .iter()
                .all(|item| item.is_fallback && !item.is_recommendation)
        );
        assert!(output[2].is_recommendation && !output[2].is_fallback);
    }

    #[test]
    fn successful_mode_matches_keep_the_filtered_results() {
        let mut matched =
            VideoResult::raw("곡명 한글발음".into(), "aaaaaaaaaaa".into(), "가수".into());
        matched.kind = VideoKind::Pronunciation;
        let general = VideoResult::raw(
            "곡명 뮤직비디오".into(),
            "bbbbbbbbbbb".into(),
            "가수".into(),
        );
        let output = with_general_fallback(vec![matched], vec![general], SearchMode::Pronunciation);
        assert_eq!(output.len(), 1);
        assert!(!output[0].is_fallback);
        assert!(output[0].url.ends_with("aaaaaaaaaaa"));
    }

    #[test]
    fn common_words_do_not_turn_unrelated_karaoke_into_an_exact_match() {
        let profile = expand_search_title("NASA Artemis II launch official").unwrap();
        let ranked = rank_results(
            vec![VideoResult::raw(
                "Popular song official karaoke".into(),
                "aaaaaaaaaaa".into(),
                "공식채널".into(),
            )],
            &profile,
            SearchMode::Karaoke,
        );
        assert!(ranked.is_empty());
        let profile = expand_search_title("작은 사랑의 노래").unwrap();
        let ranked = rank_results(
            vec![
                VideoResult::raw(
                    "다른 사랑 노래 한글발음".into(),
                    "bbbbbbbbbbb".into(),
                    "채널".into(),
                ),
                VideoResult::raw(
                    "작은 사랑의 노래 한글발음".into(),
                    "ccccccccccc".into(),
                    "채널".into(),
                ),
            ],
            &profile,
            SearchMode::Pronunciation,
        );
        assert!(ranked[0].url.ends_with("ccccccccccc"));
        assert_eq!(ranked.len(), 1);
    }

    #[test]
    fn studio_pronunciation_precedes_live_performance_and_unrelated_songs() {
        let profile = expand_search_title("잇떼").unwrap();
        let ranked = rank_results(
            vec![
                VideoResult::raw(
                    "【Live video】 요루시카 - 말해줘 言って 한글발음".into(),
                    "aaaaaaaaaaa".into(),
                    "공연".into(),
                ),
                VideoResult::raw(
                    "요루시카 - 말해줘 言って 한글발음".into(),
                    "bbbbbbbbbbb".into(),
                    "가사".into(),
                ),
                VideoResult::raw(
                    "요루시카 다른 곡 한글발음".into(),
                    "ccccccccccc".into(),
                    "가사".into(),
                ),
            ],
            &profile,
            SearchMode::Pronunciation,
        );
        assert!(ranked[0].url.ends_with("bbbbbbbbbbb"));
        assert!(!ranked[0].is_recommendation);
        assert!(ranked.last().unwrap().is_recommendation);
    }

    #[test]
    #[ignore = "Checks the unfiltered YouTube fallback over the live network"]
    fn live_general_fallback() {
        let results =
            search_youtube("NASA Artemis II launch official", SearchMode::Karaoke, 25).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].is_fallback);
        assert!(!results[0].is_recommendation);
        eprintln!("일반 검색 대체: {} | {}", results[0].title, results[0].url);
    }

    #[test]
    #[ignore = "실제 YouTube 연결 회귀 검사"]
    fn live_search_regressions() {
        let cases = [
            ("잇떼", SearchMode::Karaoke),
            ("잔혹한 천사의 테제", SearchMode::Karaoke),
            ("누덕누덕 스타카토", SearchMode::Pronunciation),
            ("작은 사랑의 노래", SearchMode::Pronunciation),
            ("토키메키 포포롱", SearchMode::Pronunciation),
            ("주문토끼 2기 op", SearchMode::Pronunciation),
            ("두근어질", SearchMode::Karaoke),
            ("키미노마치마데", SearchMode::Pronunciation),
        ];
        for (query, mode) in cases {
            let results = search_youtube(query, mode, 25).unwrap();
            assert!(!results.is_empty(), "검색 결과 없음: {query}");
            eprintln!(
                "일치 {} / 추천 {} / 일반 {}",
                results
                    .iter()
                    .filter(|item| !item.is_recommendation && !item.is_fallback)
                    .count(),
                results.iter().filter(|item| item.is_recommendation).count(),
                results.iter().filter(|item| item.is_fallback).count()
            );
            if query == "두근어질" {
                assert!(!results[0].is_recommendation);
                let title = normalize(&results[0].title);
                let profile = expand_search_title(query).unwrap();
                assert!(
                    profile
                        .identity_tokens
                        .iter()
                        .any(|token| title.contains(token))
                );
                assert!(results[0].kind.satisfies(mode));
                assert!(!results[0].is_fallback);
            }
            eprintln!("\n{query}");
            for result in results.iter().take(3) {
                eprintln!(
                    "  {}{} => {} | {}",
                    result.score,
                    if result.is_recommendation {
                        " [추천]"
                    } else {
                        ""
                    },
                    result.title,
                    result.url
                );
            }
        }
    }
}
