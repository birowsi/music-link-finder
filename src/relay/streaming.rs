//! A bounded, shared byte buffer for a progressive MP4. Only requested chunks
//! are fetched; seeking never waits for the rest of the video to download.
use super::MAX_VIDEO_BYTES;
use reqwest::{
    blocking::Client,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde_json::Value;
use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const CHUNK_SIZE: u64 = 256 * 1024;
const CACHE_CHUNKS: usize = 32; // At most 8 MiB per active video.

#[derive(Default)]
struct ChunkCache {
    chunks: HashMap<u64, Arc<Vec<u8>>>,
    order: VecDeque<u64>,
}

pub(super) struct BufferedStream {
    client: Client,
    url: String,
    headers: HeaderMap,
    pub size: u64,
    pub height: u64,
    cache: Mutex<ChunkCache>,
    pub fetched_bytes: AtomicU64,
}

fn client() -> Result<Client, String> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

fn allowed_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url
                .host_str()
                .is_some_and(|host| host.ends_with(".googlevideo.com"))
    })
}

impl BufferedStream {
    pub fn from_metadata(metadata: &Value) -> Result<Option<Arc<Self>>, String> {
        let text = |name: &str| metadata[name].as_str().unwrap_or("");
        let height = metadata["height"].as_u64().unwrap_or(0);
        if !text("vcodec").starts_with("avc1")
            || !text("acodec").starts_with("mp4a")
            || text("ext") != "mp4"
            || text("protocol") != "https"
            || height > 720
            || !allowed_url(text("url"))
        {
            return Ok(None);
        }
        let mut headers = HeaderMap::new();
        if let Some(values) = metadata["http_headers"].as_object() {
            for (name, value) in values {
                if !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "user-agent" | "referer" | "origin" | "accept" | "accept-language"
                ) {
                    continue;
                }
                if let (Ok(name), Some(value)) =
                    (HeaderName::from_bytes(name.as_bytes()), value.as_str())
                    && let Ok(value) = HeaderValue::from_str(value)
                {
                    headers.insert(name, value);
                }
            }
        }
        headers.insert("accept-encoding", HeaderValue::from_static("identity"));
        let client = client()?;
        let mut response = client
            .get(text("url"))
            .headers(headers.clone())
            .header("range", "bytes=0-0")
            .send()
            .map_err(|e| e.without_url().to_string())?;
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(format!("실시간 원본 연결 실패: HTTP {}", response.status()));
        }
        let size = response
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes 0-0/"))
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or("원본 영상 크기를 확인할 수 없습니다.")?;
        if size == 0 || size >= MAX_VIDEO_BYTES {
            return Err("원본 영상이 중계 용량 제한을 넘습니다.".into());
        }
        let mut byte = [0];
        response.read_exact(&mut byte).map_err(|e| e.to_string())?;
        Ok(Some(Arc::new(Self {
            client,
            url: text("url").into(),
            headers,
            size,
            height,
            cache: Mutex::default(),
            fetched_bytes: AtomicU64::new(1),
        })))
    }

    pub fn prime(&self) -> Result<(), String> {
        self.chunk(0).map_err(|e| e.to_string())?;
        // A tail moov atom must be available without waiting for a full download.
        self.chunk((self.size - 1) / CHUNK_SIZE)
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    fn chunk(&self, index: u64) -> io::Result<Arc<Vec<u8>>> {
        {
            let cache = self
                .cache
                .lock()
                .map_err(|_| io::Error::other("중계 버퍼 오류"))?;
            if let Some(chunk) = cache.chunks.get(&index) {
                return Ok(chunk.clone());
            }
        }
        let start = index
            .checked_mul(CHUNK_SIZE)
            .filter(|start| *start < self.size)
            .ok_or_else(|| io::Error::other("잘못된 영상 구간"))?;
        let end = (start + CHUNK_SIZE - 1).min(self.size - 1);
        let expected = end - start + 1;
        let response = self
            .client
            .get(&self.url)
            .headers(self.headers.clone())
            .header("range", format!("bytes={start}-{end}"))
            .send()
            .map_err(|e| io::Error::other(e.without_url().to_string()))?;
        let expected_range = format!("bytes {start}-{end}/{}", self.size);
        if response.status() != reqwest::StatusCode::PARTIAL_CONTENT
            || response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                != Some(expected_range.as_str())
        {
            return Err(io::Error::other(format!(
                "원본 구간 수신 실패: HTTP {}",
                response.status()
            )));
        }
        let mut data = Vec::with_capacity(expected as usize);
        response.take(expected + 1).read_to_end(&mut data)?;
        if data.len() as u64 != expected {
            return Err(io::Error::other("원본 구간이 불완전합니다."));
        }
        self.fetched_bytes.fetch_add(expected, Ordering::Relaxed);
        let data = Arc::new(data);
        let mut cache = self
            .cache
            .lock()
            .map_err(|_| io::Error::other("중계 버퍼 오류"))?;
        if let Some(existing) = cache.chunks.get(&index) {
            return Ok(existing.clone());
        }
        while cache.order.len() >= CACHE_CHUNKS {
            if let Some(oldest) = cache.order.pop_front() {
                cache.chunks.remove(&oldest);
            }
        }
        cache.order.push_back(index);
        cache.chunks.insert(index, data.clone());
        Ok(data)
    }

    pub fn reader(self: &Arc<Self>, start: u64, end: u64) -> io::Result<BufferedReader> {
        self.chunk(start / CHUNK_SIZE)?;
        Ok(BufferedReader {
            source: self.clone(),
            position: start,
            end,
        })
    }
}

pub(super) struct BufferedReader {
    source: Arc<BufferedStream>,
    position: u64,
    end: u64,
}

impl Read for BufferedReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() || self.position > self.end {
            return Ok(0);
        }
        let chunk = self.source.chunk(self.position / CHUNK_SIZE)?;
        let offset = (self.position % CHUNK_SIZE) as usize;
        let count = output
            .len()
            .min(chunk.len() - offset)
            .min((self.end - self.position + 1) as usize);
        output[..count].copy_from_slice(&chunk[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::{LocalServer, MediaSource, token};
    use std::fs;

    #[test]
    fn signed_media_must_stay_on_youtube_media_hosts() {
        assert!(allowed_url(
            "https://rr1.example.googlevideo.com/videoplayback?expire=1"
        ));
        for url in [
            "http://rr.googlevideo.com/x",
            "https://googlevideo.com.evil.test/x",
            "https://127.0.0.1/x",
            "https://user:password@rr.googlevideo.com/x",
            "https://rr.googlevideo.com:8443/x",
        ] {
            assert!(!allowed_url(url));
        }
    }

    #[test]
    fn playback_fetches_only_requested_chunks_and_reuses_its_buffer() {
        let origin = LocalServer::start().unwrap();
        let file = std::env::temp_dir().join(format!("relay-stream-test-{}.mp4", token().unwrap()));
        let bytes: Vec<u8> = (0..CHUNK_SIZE * 4).map(|n| (n % 251) as u8).collect();
        fs::write(&file, &bytes).unwrap();
        origin
            .routes
            .write()
            .unwrap()
            .insert("/sample.mp4".into(), MediaSource::File(file.clone()));
        let stream = Arc::new(BufferedStream {
            client: client().unwrap(),
            url: format!("http://{}/sample.mp4", origin.address),
            headers: HeaderMap::new(),
            size: bytes.len() as u64,
            height: 360,
            cache: Mutex::default(),
            fetched_bytes: AtomicU64::new(0),
        });
        let mut beginning = Vec::new();
        stream
            .reader(0, 9)
            .unwrap()
            .read_to_end(&mut beginning)
            .unwrap();
        assert_eq!(&beginning, &bytes[..10]);
        assert_eq!(stream.fetched_bytes.load(Ordering::Relaxed), CHUNK_SIZE);
        // Seeking goes straight to the tail, with the middle still undownloaded.
        let start = bytes.len() as u64 - 10;
        let mut tail = Vec::new();
        stream
            .reader(start, bytes.len() as u64 - 1)
            .unwrap()
            .read_to_end(&mut tail)
            .unwrap();
        assert_eq!(&tail, &bytes[bytes.len() - 10..]);
        assert_eq!(stream.fetched_bytes.load(Ordering::Relaxed), CHUNK_SIZE * 2);
        origin.routes.write().unwrap().clear();
        let mut cached = Vec::new();
        stream
            .reader(10, 19)
            .unwrap()
            .read_to_end(&mut cached)
            .unwrap();
        assert_eq!(&cached, &bytes[10..20]);
        assert_eq!(stream.fetched_bytes.load(Ordering::Relaxed), CHUNK_SIZE * 2);
        assert!(stream.reader(CHUNK_SIZE, CHUNK_SIZE + 5).is_err());
        drop(origin);
        fs::remove_file(file).unwrap();
    }
}
