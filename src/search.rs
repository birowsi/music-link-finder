use regex::Regex;
use reqwest::blocking::Client;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchMode {
    Pronunciation,
    Karaoke,
}

#[derive(Clone, Debug)]
pub struct VideoResult {
    pub title: String,
    pub url: String,
    pub channel: String,
    pub score: i32,
    pub matched: Vec<String>,
    pub is_recommendation: bool,
}

impl VideoResult {
    fn raw(title: String, video_id: String, channel: String) -> Self {
        Self {
            title,
            url: format!("https://www.youtube.com/watch?v={video_id}"),
            channel,
            score: 0,
            matched: Vec::new(),
            is_recommendation: false,
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

const MEDIA_CONTEXT_TERMS: &[&str] = &["op", "오프닝", "opening", "ed", "엔딩", "ending"];
const KARAOKE_TERMS: &[&str] = &["karaoke", "카라오케", "노래방", "반주", "mr"];
const KOREAN_KARAOKE_MARKERS: &[&str] = &["ky karaoke", "tj karaoke", "kara-u", "금영", "태진"];
const ORIGINAL_MARKERS: &[&str] = &[
    "하츠네 미쿠",
    "하츠네미쿠",
    "初音ミク",
    "feat.",
    "official",
    "original",
];
const PERFORMANCE_MARKERS: &[&str] = &[
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
const CONSENSUS_STOPWORDS: &[&str] = &[
    "가사",
    "발음",
    "독음",
    "해석",
    "번역",
    "자막",
    "일본어",
    "한국어",
    "한글",
    "lyrics",
    "lyric",
    "full",
    "video",
    "music",
    "original",
    "원곡",
];

#[derive(Clone, Debug)]
struct SearchProfile {
    expanded_title: String,
    was_expanded: bool,
    media_kind: Option<&'static str>,
    season: Option<String>,
    identity_tokens: Vec<String>,
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
    TOKEN_RE.get_or_init(|| Regex::new(r"[0-9a-z가-힣ぁ-んァ-ン一-龥]+").unwrap())
}

fn query_tokens(value: &str) -> Vec<String> {
    token_regex()
        .find_iter(&normalize(value))
        .map(|m| m.as_str().to_owned())
        .filter(|token| token.chars().count() >= 2)
        .collect()
}

fn is_ascii_alnum_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

fn contains_term(title: &str, term: &str) -> bool {
    let title = normalize(title);
    let term = normalize(term);
    if term.is_ascii() && term.chars().all(|c| c.is_ascii_alphanumeric()) && term.len() <= 3 {
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
    let clean = song_title.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return Err("음악 제목을 입력해 주세요.".to_owned());
    }
    let normalized = normalize(&clean);
    let compact = normalized.replace(' ', "");

    if compact.contains("키미노마치마데") {
        return Ok(alias_profile(
            "키미노마치마데 너의 거리까지 君の街まで ASIAN KUNG-FU GENERATION 아지캉",
            &[
                "키미노마치마데",
                "키미노 마치마데",
                "너의 거리까지",
                "君の街まで",
            ],
        ));
    }

    if normalized == "두근어질" || normalized.contains("두근 어질") {
        return Ok(alias_profile(
            "두근어질 きゅうくらりん Kyu-kurarin 이요와 iyowa",
            &[
                "두근어질",
                "きゅうくらりん",
                "kyu-kurarin",
                "kyu kurarin",
                "큐쿠라린",
            ],
        ));
    }

    if normalized == "잇떼" || normalized == "잇테" {
        return Ok(alias_profile(
            "잇떼 말해줘 言って 요루시카",
            &["잇떼", "잇테", "말해줘", "言って"],
        ));
    }

    if normalized == "dscf" {
        return Ok(alias_profile(
            "DSCF 망상감상대상연맹 妄想感傷代償連盟 DECO*27",
            &["dscf", "망상감상대상연맹", "妄想感傷代償連盟"],
        ));
    }

    if normalized.contains("토키메키 포포롱") || normalized.contains("토키메키포포롱")
    {
        return Ok(media_profile(
            "토키메키 포포론 두근두근 포포롱 주문은 토끼입니까 2기 ED",
            "ed",
            &["토키메키", "포포론", "두근두근", "포포롱"],
        ));
    }

    let expanded = clean.replace("주문토끼", "주문은 토끼입니까");
    let lowered = normalize(&expanded);
    if lowered.contains("주문은 토끼입니까") && lowered.contains("2기") {
        let words: HashSet<_> = lowered.split_whitespace().collect();
        if words.contains("op") || words.contains("오프닝") {
            return Ok(media_profile(
                "No Poi 주문은 토끼입니까 2기 OP",
                "op",
                &["주문은", "토끼입니까"],
            ));
        }
        if words.contains("ed") || words.contains("엔딩") {
            return Ok(media_profile(
                "토키메키 포포론 주문은 토끼입니까 2기 ED",
                "ed",
                &["토키메키", "포포론", "두근두근", "포포롱"],
            ));
        }
    }

    Ok(SearchProfile {
        expanded_title: expanded.clone(),
        was_expanded: normalize(&expanded) != normalize(&clean),
        media_kind: None,
        season: None,
        identity_tokens: Vec::new(),
    })
}

fn media_profile(title: &str, kind: &'static str, identities: &[&str]) -> SearchProfile {
    SearchProfile {
        expanded_title: title.to_owned(),
        was_expanded: true,
        media_kind: Some(kind),
        season: Some("2기".to_owned()),
        identity_tokens: identities.iter().map(|item| (*item).to_owned()).collect(),
    }
}

fn alias_profile(title: &str, identities: &[&str]) -> SearchProfile {
    SearchProfile {
        expanded_title: title.to_owned(),
        was_expanded: true,
        media_kind: None,
        season: None,
        identity_tokens: identities.iter().map(|item| normalize(item)).collect(),
    }
}

fn build_queries(profile: &SearchProfile, mode: SearchMode) -> Vec<String> {
    let title = &profile.expanded_title;
    match mode {
        SearchMode::Karaoke => vec![
            format!("{title} 한글발음 노래방 karaoke"),
            format!("{title} 노래방 카라오케 반주"),
            "일본 노래방 한글발음 JPOP 보컬로이드 karaoke 인기곡".to_owned(),
        ],
        SearchMode::Pronunciation if profile.media_kind.is_some() => vec![
            format!("{title} 한글자막 가사 발음"),
            format!("{title} 애니 주제가"),
            "JPOP 보컬로이드 한글발음 가사 독음 인기곡".to_owned(),
        ],
        SearchMode::Pronunciation => vec![
            format!("{title} 한글발음 lyrics"),
            format!("{title} 원곡 한국어해석 독음 일본어 자막"),
            "JPOP 보컬로이드 한글발음 가사 독음 인기곡".to_owned(),
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
            if let Some(renderer) = map.get("videoRenderer") {
                if let Some(video_id) = renderer.get("videoId").and_then(Value::as_str) {
                    if seen.insert(video_id.to_owned()) {
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
    for index in object_start..bytes.len() {
        let byte = bytes[index];
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

fn discover_original_consensus(results: &[VideoResult], song_title: &str) -> Vec<String> {
    let query: HashSet<_> = query_tokens(song_title).into_iter().collect();
    let stopwords: HashSet<_> = CONSENSUS_STOPWORDS
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    let mut documents: HashMap<String, i32> = HashMap::new();
    let mut weighted: HashMap<String, i32> = HashMap::new();
    let sample_size = 10usize;

    for (position, result) in results.iter().take(sample_size).enumerate() {
        let tokens: HashSet<_> = query_tokens(&result.title).into_iter().collect();
        for token in tokens {
            if query.contains(&token)
                || stopwords.contains(&token)
                || token.chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }
            *documents.entry(token.clone()).or_default() += 1;
            *weighted.entry(token).or_default() += (sample_size - position) as i32;
        }
    }

    let mut repeated: Vec<_> = documents
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .map(|(token, _)| token)
        .collect();
    repeated.sort_by_key(|token| std::cmp::Reverse(*weighted.get(token).unwrap_or(&0)));
    repeated.truncate(3);
    repeated
}

fn qualifies(title: &str, mode: SearchMode) -> bool {
    let pronunciation = PRONUNCIATION_TERMS
        .iter()
        .any(|term| contains_term(title, term));
    match mode {
        SearchMode::Pronunciation => {
            pronunciation || SUBTITLE_TERMS.iter().any(|term| contains_term(title, term))
        }
        SearchMode::Karaoke => KARAOKE_TERMS.iter().any(|term| contains_term(title, term)),
    }
}

fn rank_results(
    results: Vec<VideoResult>,
    profile: &SearchProfile,
    mode: SearchMode,
    consensus: &[String],
) -> Vec<VideoResult> {
    let query = query_tokens(&profile.expanded_title);
    let mut ranked = Vec::new();

    for (position, mut result) in results.into_iter().enumerate() {
        let normalized_title = normalize(&result.title);
        let identity_hits = profile
            .identity_tokens
            .iter()
            .filter(|token| normalized_title.contains(token.as_str()))
            .count() as i32;
        let context = matching_terms(&result.title, MEDIA_CONTEXT_TERMS);
        if !qualifies(&result.title, mode)
            && !(mode == SearchMode::Pronunciation && profile.was_expanded && !context.is_empty())
        {
            continue;
        }

        let pronunciation = matching_terms(&result.title, PRONUNCIATION_TERMS);
        let subtitle = matching_terms(&result.title, SUBTITLE_TERMS);
        let karaoke = matching_terms(&result.title, KARAOKE_TERMS);
        let title_hits = query
            .iter()
            .filter(|token| normalized_title.contains(token.as_str()))
            .count() as i32;
        let is_exact = if profile.identity_tokens.is_empty() {
            title_hits > 0
        } else {
            identity_hits > 0
        };
        let original_hits = ORIGINAL_MARKERS
            .iter()
            .filter(|term| contains_term(&result.title, term))
            .count() as i32;
        let performance_hits = PERFORMANCE_MARKERS
            .iter()
            .filter(|term| contains_term(&result.title, term))
            .count() as i32;
        let korean_karaoke_hits = KOREAN_KARAOKE_MARKERS
            .iter()
            .filter(|term| contains_term(&result.title, term))
            .count() as i32;
        let consensus_hits = consensus
            .iter()
            .filter(|term| normalized_title.contains(term.as_str()))
            .count() as i32;

        let mut score = (30 - position as i32).max(0) + title_hits * 10 + identity_hits * 15;
        if profile.was_expanded
            && profile.media_kind.is_none()
            && profile
                .identity_tokens
                .first()
                .is_some_and(|primary| normalized_title.contains(primary))
        {
            score += 50;
        }
        if let Some(kind) = profile.media_kind {
            let (requested, opposite): (&[&str], &[&str]) = if kind == "op" {
                (&["op", "오프닝", "opening"], &["ed", "엔딩", "ending"])
            } else {
                (&["ed", "엔딩", "ending"], &["op", "오프닝", "opening"])
            };
            if requested
                .iter()
                .any(|term| contains_term(&result.title, term))
            {
                score += 40;
            } else {
                score -= 40;
            }
            if opposite
                .iter()
                .any(|term| contains_term(&result.title, term))
            {
                score -= 100;
            }
        }
        if let Some(season) = &profile.season {
            score += if normalized_title.contains(season) {
                30
            } else {
                -60
            };
        }

        if !pronunciation.is_empty() {
            score += 55 + (pronunciation.len() as i32 * 5).min(20);
        } else if !subtitle.is_empty() {
            score += 25 + (subtitle.len() as i32 * 5).min(10);
        } else if !context.is_empty() {
            score += 15;
        }

        match mode {
            SearchMode::Karaoke => {
                score += 50 + (karaoke.len() as i32 * 5).min(15);
                if contains_term(&result.title, "노래방") {
                    score += 20;
                }
                if !pronunciation.is_empty() {
                    score += 35;
                }
                score += (korean_karaoke_hits * 35).min(35);
                score -= performance_hits * 70;
            }
            SearchMode::Pronunciation => {
                if !karaoke.is_empty() {
                    score -= 25;
                }
                score += (original_hits * 25).min(50);
                score += (consensus_hits * 25).min(45);
                score -= performance_hits * 35;
            }
        }

        let mut matched = pronunciation;
        matched.extend(subtitle);
        matched.extend(karaoke);
        matched.extend(context);
        let mut unique = HashSet::new();
        matched.retain(|item| unique.insert(item.clone()));
        result.score = score;
        result.matched = matched;
        result.is_recommendation = !is_exact;
        ranked.push(result);
    }

    let mut exact: Vec<_> = ranked
        .iter()
        .filter(|result| !result.is_recommendation)
        .cloned()
        .collect();
    let mut recommendations: Vec<_> = ranked
        .into_iter()
        .filter(|result| result.is_recommendation)
        .collect();
    exact.sort_by(|left, right| right.score.cmp(&left.score));
    recommendations.sort_by(|left, right| right.score.cmp(&left.score));
    exact.truncate(8);
    recommendations.truncate(12);
    exact.extend(recommendations);
    exact
}

pub fn search_youtube(
    song_title: &str,
    mode: SearchMode,
    limit: usize,
) -> Result<Vec<VideoResult>, String> {
    let profile = expand_search_title(song_title)?;
    let queries = build_queries(&profile, mode);
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

    let consensus = if mode == SearchMode::Pronunciation && !profile.was_expanded {
        batches
            .get(1)
            .map(|batch| discover_original_consensus(batch, &profile.expanded_title))
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let max_len = batches.iter().map(Vec::len).max().unwrap_or(0);
    let mut interleaved = Vec::new();
    let mut seen = HashSet::new();
    for position in 0..max_len {
        for batch in &batches {
            if let Some(result) = batch.get(position) {
                if seen.insert(result.url.clone()) {
                    interleaved.push(result.clone());
                }
            }
        }
    }
    Ok(rank_results(interleaved, &profile, mode, &consensus))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            if query == "두근어질" {
                assert_eq!(
                    results[0].url,
                    "https://www.youtube.com/watch?v=53FmAEYYD2Q"
                );
                assert!(!results[0].is_recommendation);
                assert!(
                    results.iter().any(|result| result.is_recommendation),
                    "추천곡이 검색 결과 뒤에 이어지지 않음"
                );
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
