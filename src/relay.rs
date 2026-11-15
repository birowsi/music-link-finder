//! Desktop-only, local MP4 hosting. The public tunnel exposes a read-only file
//! route, never the download controls or arbitrary paths on the computer.
mod dns;
mod streaming;
use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    os::windows::{io::AsRawHandle, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use streaming::BufferedStream;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    },
};

const CANCELLED: &str = "작업이 취소되었습니다.";
const MAX_VIDEO_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_SESSION_BYTES: u64 = 4 * MAX_VIDEO_BYTES;
#[derive(Clone)]
enum MediaSource {
    File(PathBuf),
    Buffered(Arc<BufferedStream>),
}
type Routes = Arc<RwLock<HashMap<String, MediaSource>>>;

#[derive(Debug)]
pub enum HostingEvent {
    Progress(String),
    Ready {
        source: String,
        url: String,
        streaming: bool,
    },
    Failed(String),
}

enum Request {
    Prepare { generation: u64, source: String },
    Stop,
}

/// One serial worker owns the tunnel, downloads, and cache. Advancing the
/// generation cancels obsolete work before it can publish a result.
pub struct RelayHost {
    sender: Sender<Request>,
    generation: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl RelayHost {
    pub fn new(notify: impl Fn(u64, HostingEvent) + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));
        let active = generation.clone();
        let quitting = shutdown.clone();
        let worker = thread::spawn(move || {
            let mut session: Option<Session> = None;
            while let Ok(request) = receiver.recv() {
                if quitting.load(Ordering::SeqCst) {
                    break;
                }
                match request {
                    Request::Stop => {
                        session = None;
                    }
                    Request::Prepare {
                        generation: current,
                        source,
                    } => {
                        let cancelled = || {
                            quitting.load(Ordering::SeqCst)
                                || active.load(Ordering::SeqCst) != current
                        };
                        if cancelled() {
                            continue;
                        }
                        let progress =
                            |text: &str| notify(current, HostingEvent::Progress(text.into()));
                        let result = (|| {
                            video_id(&source)?;
                            if session.is_none() {
                                session = Some(Session::start(&cancelled, &progress)?);
                            }
                            session
                                .as_mut()
                                .unwrap()
                                .prepare(&source, &cancelled, &progress)
                        })();
                        if cancelled() {
                            continue;
                        }
                        match result {
                            Ok(url) => {
                                let streaming = session
                                    .as_ref()
                                    .is_some_and(|session| session.is_streaming(&url));
                                notify(
                                    current,
                                    HostingEvent::Ready {
                                        source,
                                        url,
                                        streaming,
                                    },
                                );
                            }
                            Err(error) => {
                                session = None;
                                notify(current, HostingEvent::Failed(error));
                            }
                        }
                    }
                }
            }
        });
        Self {
            sender,
            generation,
            shutdown,
            worker: Some(worker),
        }
    }

    pub fn prepare(&self, source: String) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.sender.send(Request::Prepare { generation, source });
        generation
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.sender.send(Request::Stop);
    }
}

impl Drop for RelayHost {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        self.stop();
        if let Some(worker) = self.worker.take() {
            let start = Instant::now();
            while !worker.is_finished() && start.elapsed() < Duration::from_secs(8) {
                thread::sleep(Duration::from_millis(30));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

fn token() -> Result<String, String> {
    let mut bytes = [0u8; 24];
    let result = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if result < 0 {
        return Err("중계 주소 생성에 실패했습니다.".into());
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn video_id(url: &str) -> Result<&str, String> {
    let id = url
        .strip_prefix("https://www.youtube.com/watch?v=")
        .ok_or("검색 결과의 YouTube 링크만 중계할 수 있습니다.")?;
    if id.len() != 11
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err("올바르지 않은 YouTube 링크입니다.".into());
    }
    Ok(id)
}

/// A private, non-inheritable Windows job kills the entire helper process tree
/// on cancellation, normal exit, or an unexpected parent-app exit.
struct ManagedChild {
    child: Child,
    job: HANDLE,
}
unsafe impl Send for ManagedChild {}

impl ManagedChild {
    fn spawn(command: &mut Command) -> Result<Self, String> {
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let mut child = command
            .spawn()
            .map_err(|e| format!("도구 실행 실패: {e}"))?;
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let assigned = !job.is_null()
            && unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as _,
                    std::mem::size_of_val(&limits) as u32,
                ) != 0
                    && AssignProcessToJobObject(job, child.as_raw_handle() as _) != 0
            };
        if !assigned {
            let _ = child.kill();
            let _ = child.wait();
            if !job.is_null() {
                unsafe {
                    CloseHandle(job);
                }
            }
            return Err("중계 도구의 종료 관리를 설정하지 못했습니다.".into());
        }
        Ok(Self { child, job })
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.job);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn find_tool(name: &str, tools: &Path) -> Option<PathBuf> {
    let local = tools.join(name);
    if local.is_file() {
        return Some(local);
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|file| file.is_file())
    })
}

fn ensure_tool(
    name: &str,
    url: &str,
    tools: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<PathBuf, String> {
    if let Some(path) = find_tool(name, tools) {
        return Ok(path);
    }
    fs::create_dir_all(tools).map_err(|e| format!("tools 폴더 생성 실패: {e}"))?;
    let target = tools.join(name);
    let partial = tools.join(format!("{name}.download"));
    let result = (|| {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| e.to_string())?;
        let mut response = client
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("{name} 다운로드 실패: {e}"))?;
        let mut output = File::create(&partial).map_err(|e| e.to_string())?;
        let mut buffer = [0u8; 65536];
        loop {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            let length = response.read(&mut buffer).map_err(|e| e.to_string())?;
            if length == 0 {
                break;
            }
            output
                .write_all(&buffer[..length])
                .map_err(|e| e.to_string())?;
        }
        output.flush().map_err(|e| e.to_string())?;
        drop(output);
        fs::rename(&partial, &target).map_err(|e| e.to_string())?;
        Ok(target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(partial);
    }
    result
}

fn run_tool(
    command: &mut Command,
    log: &Path,
    timeout: Duration,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    run_tool_output(command, log, None, timeout, cancelled)
}

fn run_tool_output(
    command: &mut Command,
    log: &Path,
    stdout: Option<&Path>,
    timeout: Duration,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    let output = File::create(log).map_err(|e| e.to_string())?;
    let stdout = match stdout {
        Some(path) => Stdio::from(File::create(path).map_err(|e| e.to_string())?),
        None => Stdio::null(),
    };
    command
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::from(output));
    let mut process = ManagedChild::spawn(command)?;
    let start = Instant::now();
    loop {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        if start.elapsed() > timeout {
            return Err(
                "영상 준비 시간이 초과되었습니다. 선택 영상 프록싱을 다시 눌러 주세요.".into(),
            );
        }
        if let Some(status) = process.child.try_wait().map_err(|e| e.to_string())? {
            if status.success() {
                return Ok(());
            }
            let error = fs::read_to_string(log).unwrap_or_default();
            let tail = error
                .lines()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" ");
            return Err(format!(
                "영상 준비 실패: {}",
                tail.chars().take(600).collect::<String>()
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

struct LocalServer {
    address: std::net::SocketAddr,
    routes: Routes,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    thread: Option<thread::JoinHandle<()>>,
}

impl LocalServer {
    #[allow(dead_code)]
    fn start() -> Result<Self, String> {
        Self::start_on(0)
    }

    fn start_on(port: u16) -> Result<Self, String> {
        let bind_addr = if port == 0 {
            "127.0.0.1:0".to_string()
        } else {
            format!("127.0.0.1:{port}")
        };
        let listener = TcpListener::bind(&bind_addr)
            .map_err(|e| format!("중계 서버 시작 실패 ({bind_addr}): {e}"))?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let routes: Routes = Default::default();
        let shared = routes.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let active = Arc::new(AtomicUsize::new(0));
        let connections = active.clone();
        let thread = thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if connections.load(Ordering::SeqCst) >= 64 {
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                            let mut s = stream;
                            let _ = s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nAccess-Control-Allow-Origin: *\r\nRetry-After: 1\r\nConnection: close\r\n\r\n");
                            continue;
                        }
                        connections.fetch_add(1, Ordering::SeqCst);
                        let active = connections.clone();
                        let routes = shared.clone();
                        let stopping = stopping.clone();
                        thread::spawn(move || {
                            if let Err(error) = serve(stream, routes, &stopping)
                                && !matches!(
                                    error.kind(),
                                    std::io::ErrorKind::BrokenPipe
                                        | std::io::ErrorKind::ConnectionAborted
                                        | std::io::ErrorKind::ConnectionReset
                                )
                            {
                                eprintln!("Relay transfer failed: {error}");
                            }
                            active.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(30))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            address,
            routes,
            stop,
            active,
            thread: Some(thread),
        })
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let start = Instant::now();
        while self.active.load(Ordering::SeqCst) != 0 && start.elapsed() < Duration::from_secs(6) {
            thread::sleep(Duration::from_millis(30));
        }
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn byte_range(header: Option<&str>, size: u64) -> Result<(u64, u64, bool), ()> {
    if size == 0 {
        return Err(());
    }
    let Some(header) = header else {
        return Ok((0, size - 1, false));
    };
    let (start, end) = header
        .strip_prefix("bytes=")
        .ok_or(())?
        .split_once('-')
        .ok_or(())?;
    let (start, end) = if start.is_empty() {
        let suffix: u64 = end.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = start.parse().map_err(|_| ())?;
        let end: u64 = if end.is_empty() {
            size - 1
        } else {
            end.parse().map_err(|_| ())?
        };
        (start, end.min(size - 1))
    };
    if start > end || start >= size {
        return Err(());
    }
    Ok((start, end, true))
}

fn serve(mut stream: TcpStream, routes: Routes, stop: &AtomicBool) -> std::io::Result<()> {
    // On Windows, accepted sockets inherit FIONBIO from the polling listener.
    // Blocking I/O below must wait for backpressure instead of truncating MP4s
    // at WSAEWOULDBLOCK (10035).
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let mut request = String::new();
        match reader.by_ref().take(8193).read_line(&mut request) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock =>
            {
                break;
            }
            Err(e) => return Err(e),
        }
        let trimmed_request = request.trim();
        if trimmed_request.is_empty() {
            continue;
        }
        let mut words = trimmed_request.split_whitespace();
        let method = words.next().unwrap_or("");
        let path = words.next().unwrap_or("");

        let mut range = None;
        let mut connection_close = true;
        let mut bytes = request.len();
        loop {
            let mut line = String::new();
            let read = reader.by_ref().take(8193).read_line(&mut line)?;
            bytes += read;
            if bytes > 16384 || read == 0 {
                return stream.write_all(
                    b"HTTP/1.1 400 Bad Request\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((name, value)) = trimmed.split_once(':') {
                let name = name.trim();
                let value = value.trim();
                if name.eq_ignore_ascii_case("range") {
                    if range.is_some() {
                        return stream.write_all(
                            b"HTTP/1.1 400 Bad Request\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    range = Some(value.to_owned());
                } else if name.eq_ignore_ascii_case("connection") {
                    if value.eq_ignore_ascii_case("keep-alive") {
                        connection_close = false;
                    } else if value.eq_ignore_ascii_case("close") {
                        connection_close = true;
                    }
                }
            }
        }

        if method == "OPTIONS" {
            write!(
                stream,
                "HTTP/1.1 204 No Content\r\n\
Access-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n\
Access-Control-Allow-Headers: Range, Content-Type, Accept\r\n\
Access-Control-Max-Age: 86400\r\n\
Content-Length: 0\r\n\
Connection: {}\r\n\r\n",
                if connection_close {
                    "close"
                } else {
                    "keep-alive"
                }
            )?;
            stream.flush()?;
            if connection_close {
                break;
            }
            continue;
        }

        if method != "GET" && method != "HEAD" {
            return stream.write_all(
                b"HTTP/1.1 405 Method Not Allowed\r\n\
Allow: GET, HEAD, OPTIONS\r\n\
Access-Control-Allow-Origin: *\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n",
            );
        }

        let source = routes.read().unwrap().get(path).cloned();
        let Some(source) = source else {
            stream.write_all(
                b"HTTP/1.1 404 Not Found\r\n\
Access-Control-Allow-Origin: *\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n",
            )?;
            break;
        };

        let size = match &source {
            MediaSource::File(path) => path.metadata()?.len(),
            MediaSource::Buffered(source) => source.size,
        };
        let Ok((start, end, partial)) = byte_range(
            if method == "HEAD" {
                None
            } else {
                range.as_deref()
            },
            size,
        ) else {
            write!(
                stream,
                "HTTP/1.1 416 Range Not Satisfiable\r\n\
Access-Control-Allow-Origin: *\r\n\
Content-Range: bytes */{size}\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n"
            )?;
            break;
        };
        let length = end - start + 1;
        // Validate the first upstream chunk before committing success headers.
        let mut input: Option<Box<dyn Read>> = if method == "HEAD" {
            None
        } else {
            let opened: std::io::Result<Box<dyn Read>> = match &source {
                MediaSource::File(path) => File::open(path).and_then(|mut file| {
                    file.seek(SeekFrom::Start(start))?;
                    Ok(Box::new(file) as Box<dyn Read>)
                }),
                MediaSource::Buffered(source) => source
                    .reader(start, end)
                    .map(|reader| Box::new(reader) as Box<dyn Read>),
            };
            match opened {
                Ok(input) => Some(input),
                Err(_) => {
                    stream.write_all(
                        b"HTTP/1.1 502 Bad Gateway\r\n\
Access-Control-Allow-Origin: *\r\n\
Content-Length: 0\r\n\
Connection: close\r\n\r\n",
                    )?;
                    break;
                }
            }
        };

        write!(
            stream,
            "HTTP/1.1 {}\r\n\
Content-Type: video/mp4\r\n\
Accept-Ranges: bytes\r\n\
Content-Length: {length}\r\n\
Access-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n\
Access-Control-Expose-Headers: Content-Length, Content-Range, Accept-Ranges\r\n\
Cache-Control: private, no-store\r\n\
X-Content-Type-Options: nosniff\r\n\
Connection: {}\r\n",
            if partial {
                "206 Partial Content"
            } else {
                "200 OK"
            },
            if connection_close {
                "close"
            } else {
                "keep-alive"
            }
        )?;
        if partial {
            write!(stream, "Content-Range: bytes {start}-{end}/{size}\r\n")?;
        }
        stream.write_all(b"\r\n")?;
        if method == "HEAD" {
            stream.flush()?;
            if connection_close {
                break;
            }
            continue;
        }
        let input = input.as_mut().unwrap();
        let mut remaining = length;
        let mut buffer = [0u8; 65536];
        while remaining > 0 && !stop.load(Ordering::SeqCst) {
            let limit = remaining.min(buffer.len() as u64) as usize;
            let read = input.read(&mut buffer[..limit])?;
            if read == 0 {
                break;
            }
            stream.write_all(&buffer[..read])?;
            remaining -= read as u64;
        }
        stream.flush()?;
        if connection_close || remaining > 0 {
            break;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CopyPlan {
    video: bool,
    audio: bool,
}

fn copy_plan(probe: &serde_json::Value) -> Result<CopyPlan, String> {
    let streams = probe["streams"]
        .as_array()
        .ok_or("영상 스트림 정보가 없습니다.")?;
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or("영상 트랙이 없습니다.")?;
    let audio = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .ok_or("음성 트랙이 없습니다.")?;
    Ok(CopyPlan {
        video: video["codec_name"] == "h264"
            && matches!(video["pix_fmt"].as_str(), Some("yuv420p" | "yuvj420p")),
        audio: audio["codec_name"] == "aac",
    })
}

fn probe_copy_plan(
    ffmpeg: &Path,
    input: &Path,
    work: &Path,
    cancelled: &impl Fn() -> bool,
) -> Result<CopyPlan, String> {
    let Some(ffprobe) = find_tool("ffprobe.exe", ffmpeg.parent().unwrap()) else {
        return Ok(CopyPlan {
            video: false,
            audio: false,
        });
    };
    let output = work.join("probe.json");
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,codec_name,pix_fmt",
            "-of",
            "json",
        ])
        .arg(input);
    run_tool_output(
        &mut command,
        &work.join("probe.log"),
        Some(&output),
        Duration::from_secs(30),
        cancelled,
    )?;
    let probe = serde_json::from_slice(&fs::read(output).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    copy_plan(&probe)
}

fn convert_mp4(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
    log: &Path,
    plan: CopyPlan,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-nostdin", "-y", "-v", "error", "-i"])
        .arg(input)
        .args(["-map", "0:v:0", "-map", "0:a:0"]);
    if plan.video {
        command.args(["-c:v", "copy"]);
    } else {
        command.args([
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-pix_fmt",
            "yuv420p",
            "-vf",
            "scale=trunc(iw/2)*2:trunc(ih/2)*2",
        ]);
    }
    if plan.audio {
        command.args(["-c:a", "copy"]);
    } else {
        command.args(["-c:a", "aac", "-b:a", "128k"]);
    }
    command
        .args(["-movflags", "+faststart", "-fs", "1073741824"])
        .arg(output);
    run_tool(&mut command, log, Duration::from_secs(1200), cancelled)
}

struct HelperTools {
    ffmpeg: PathBuf,
    ytdlp: PathBuf,
    cloudflared: PathBuf,
    node: Option<PathBuf>,
}

#[derive(Clone)]
pub struct RelayConfig {
    pub custom_domain: Option<String>,
    pub tunnel_token: Option<String>,
    pub port: u16,
}

impl RelayConfig {
    pub fn load() -> Result<Self, String> {
        let root = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| PathBuf::from("."));
        let config_path = root.join("config.txt");

        match fs::read_to_string(&config_path) {
            Ok(content) => Self::parse(&content),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::parse(""),
            Err(_) => {
                Err("config.txt를 읽을 수 없습니다. 파일 권한과 UTF-8 인코딩을 확인하세요.".into())
            }
        }
    }

    fn parse(content: &str) -> Result<Self, String> {
        let mut config = Self {
            custom_domain: None,
            tunnel_token: None,
            port: 38291,
        };

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((key, val)) = trimmed.split_once('=') {
                let key = key.trim().to_ascii_uppercase();
                let val = val.trim();
                if val.is_empty() {
                    continue;
                }
                match key.as_str() {
                    "CUSTOM_DOMAIN" => {
                        let address = if val.contains("://") {
                            val.to_owned()
                        } else {
                            format!("https://{val}")
                        };
                        let url = reqwest::Url::parse(&address)
                            .map_err(|_| "CUSTOM_DOMAIN에 올바른 도메인을 입력하세요.")?;
                        if url.scheme() != "https"
                            || !url.username().is_empty()
                            || url.password().is_some()
                            || url.port().is_some()
                            || url.path() != "/"
                            || url.query().is_some()
                            || url.fragment().is_some()
                            || !url.host_str().is_some_and(|host| {
                                host.parse::<std::net::IpAddr>().is_err()
                                    && host.contains('.')
                                    && host.len() <= 253
                                    && host.split('.').all(|label| {
                                        !label.is_empty()
                                            && label.len() <= 63
                                            && !label.starts_with('-')
                                            && !label.ends_with('-')
                                            && label
                                                .bytes()
                                                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                                    })
                            })
                        {
                            return Err("CUSTOM_DOMAIN에는 경로나 포트가 없는 HTTPS 도메인만 사용할 수 있습니다.".into());
                        }
                        config.custom_domain = url.host_str().map(str::to_owned);
                    }
                    "TUNNEL_TOKEN" => config.tunnel_token = Some(val.to_string()),
                    "PORT" => {
                        config.port = val
                            .parse::<u16>()
                            .ok()
                            .filter(|p| *p != 0)
                            .ok_or("PORT는 1~65535 사이의 숫자여야 합니다.")?;
                    }
                    _ => {}
                }
            }
        }
        if config.tunnel_token.is_some() && config.custom_domain.is_none() {
            return Err("TUNNEL_TOKEN을 사용할 때는 CUSTOM_DOMAIN도 설정하세요.".into());
        }
        Ok(config)
    }
}

struct Session {
    tunnel: ManagedChild,
    server: LocalServer,
    directory: PathBuf,
    tools: HelperTools,
    public_url: String,
    tunnel_events: Receiver<String>,
    secret: String,
    verified_host: bool,
    named_tunnel: bool,
    tunnel_connected: bool,
}

impl Session {
    fn start(cancelled: &impl Fn() -> bool, progress: &impl Fn(&str)) -> Result<Self, String> {
        let config = RelayConfig::load()?;
        let root = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("앱 폴더를 찾을 수 없습니다.")?
            .to_owned();
        let tools = root.join("tools");
        let ffmpeg = find_tool("ffmpeg.exe", &tools).ok_or("FFmpeg가 필요합니다. tools 폴더에 ffmpeg.exe를 넣거나 PATH에 설치한 뒤 다시 시도하세요.")?;
        progress("중계 도구 확인 중 (처음에는 yt-dlp와 cloudflared를 자동으로 받습니다)…");
        let ytdlp = ensure_tool(
            "yt-dlp.exe",
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
            &tools,
            cancelled,
        )?;
        let cloudflared = ensure_tool(
            "cloudflared.exe",
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe",
            &tools,
            cancelled,
        )?;
        if cancelled() {
            return Err(CANCELLED.into());
        }
        let secret = token()?;
        let cache_root = root.join(".relay-cache");
        // Every running instance owns only its random session directory.
        // Another instance may still be serving files in the same cache root.
        let directory = cache_root.join(&secret);
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let result = Self::connect(
            directory.clone(),
            HelperTools {
                ffmpeg,
                ytdlp,
                cloudflared,
                node: find_tool("node.exe", &tools),
            },
            secret,
            config,
            cancelled,
            progress,
        );
        if result.is_err() {
            let _ = fs::remove_dir_all(directory);
        }
        result
    }

    fn connect(
        directory: PathBuf,
        tools: HelperTools,
        secret: String,
        config: RelayConfig,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(&str),
    ) -> Result<Self, String> {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        let named_tunnel = config.tunnel_token.is_some();
        let port = if named_tunnel { config.port } else { 0 };
        let server = LocalServer::start_on(port)?;

        let mut command = Command::new(&tools.cloudflared);
        let public_url = if let Some(token) = &config.tunnel_token {
            let domain = config
                .custom_domain
                .as_deref()
                .ok_or("CUSTOM_DOMAIN을 설정하세요.")?;
            progress(&format!("전용 도메인({domain}) 터널에 연결하는 중…"));
            command
                .args(["tunnel", "--no-autoupdate", "run"])
                .env("TUNNEL_TOKEN", token)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            format!("https://{domain}")
        } else {
            progress("임시 HTTPS 중계 주소를 만드는 중…");
            let yml = directory.join("tunnel.yml");
            fs::write(&yml, "{}\n").map_err(|e| e.to_string())?;
            command
                .args(["tunnel", "--config"])
                .arg(yml)
                .args([
                    "--no-autoupdate",
                    "--protocol",
                    "http2",
                    "--url",
                    &format!("http://{}", server.address),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            String::new()
        };

        let mut tunnel = ManagedChild::spawn(&mut command)?;
        let stderr = tunnel
            .child
            .stderr
            .take()
            .ok_or("터널 로그를 읽을 수 없습니다.")?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = sender.send(line);
            }
        });
        Ok(Self {
            tunnel,
            server,
            directory,
            tools,
            public_url,
            tunnel_events: receiver,
            secret,
            verified_host: named_tunnel,
            named_tunnel,
            tunnel_connected: false,
        })
    }

    fn wait_for_tunnel(&mut self, cancelled: &impl Fn() -> bool) -> Result<(), String> {
        if self.tunnel_connected {
            return Ok(());
        }
        let wait_started = Instant::now();
        if self.named_tunnel {
            let mut connected = false;
            let mut last_error = String::new();
            while !connected {
                if cancelled() {
                    return Err(CANCELLED.into());
                }
                while let Ok(line) = self.tunnel_events.try_recv() {
                    if line.contains("Registered tunnel connection")
                        || line.contains("Connection registered")
                    {
                        connected = true;
                    }
                    if line.contains("ERR") {
                        last_error = line;
                    }
                }
                if connected {
                    self.tunnel_connected = true;
                    return Ok(());
                }
                if self
                    .tunnel
                    .child
                    .try_wait()
                    .map_err(|e| e.to_string())?
                    .is_some()
                {
                    return Err(format!(
                        "전용 도메인 터널 연결이 종료되었습니다. {last_error}"
                    ));
                }
                if wait_started.elapsed() > Duration::from_secs(35) {
                    return Err(format!(
                        "전용 도메인 터널 연결 시간이 초과되었습니다. Cloudflare 토큰을 확인하세요. {last_error}"
                    ));
                }
                thread::sleep(Duration::from_millis(100));
            }
            self.tunnel_connected = true;
            return Ok(());
        }

        if !self.public_url.is_empty() {
            self.tunnel_connected = true;
            return Ok(());
        }
        let mut hostname = None;
        let mut connected = false;
        let mut last_error = String::new();
        let public_url = loop {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            while let Ok(line) = self.tunnel_events.try_recv() {
                if let Some(url) = tunnel_url(&line) {
                    hostname = Some(url);
                }
                if line.contains("Registered tunnel connection")
                    || line.contains("Connection registered")
                {
                    connected = true;
                }
                if line.contains("ERR") {
                    last_error = line;
                }
            }
            if connected && let Some(url) = hostname.take() {
                break url;
            }
            if self
                .tunnel
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err(format!("임시 터널 연결이 종료되었습니다. {last_error}"));
            }
            if wait_started.elapsed() > Duration::from_secs(60) {
                return Err(format!(
                    "임시 터널 연결 시간이 초과되었습니다. {last_error}"
                ));
            }
            thread::sleep(Duration::from_millis(100));
        };
        self.public_url = public_url;
        self.tunnel_connected = true;
        #[cfg(test)]
        eprintln!("Tunnel hostname: {}", self.public_url);
        Ok(())
    }

    fn is_streaming(&self, url: &str) -> bool {
        url.strip_prefix(&self.public_url).is_some_and(|route| {
            matches!(
                self.server.routes.read().unwrap().get(route),
                Some(MediaSource::Buffered(_))
            )
        })
    }

    fn prepare(
        &mut self,
        source: &str,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(&str),
    ) -> Result<String, String> {
        self.prepare_with_mode(source, cancelled, progress, true)
    }

    fn prepare_with_mode(
        &mut self,
        source: &str,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(&str),
        stream_first: bool,
    ) -> Result<String, String> {
        let id = video_id(source)?;
        if self
            .tunnel
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err(
                "임시 터널이 종료되었습니다. 중계 중지 후 선택 영상 프록싱을 다시 누르세요.".into(),
            );
        }
        let route = format!("/v/{}/{id}.mp4", self.secret);
        let cached_expired = matches!(
            self.server.routes.read().unwrap().get(&route),
            Some(MediaSource::Buffered(stream)) if stream.is_expired()
        );
        if cached_expired {
            self.server.routes.write().unwrap().remove(&route);
        }
        if self.server.routes.read().unwrap().contains_key(&route) {
            self.wait_for_tunnel(cancelled)?;
            let url = format!("{}{route}", self.public_url);
            verify_public(&url, &mut self.verified_host, cancelled, progress)?;
            return Ok(url);
        }
        let total = fs::read_dir(&self.directory)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .map(|m| m.len())
            .sum::<u64>();
        if total >= MAX_SESSION_BYTES {
            return Err(
                "중계 캐시가 4GB를 넘었습니다. 중계 중지로 비운 뒤 선택 영상 프록싱을 다시 누르세요.".into(),
            );
        }
        let work = self.directory.join(format!("work-{id}"));
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let result = (|| {
            if stream_first {
                progress("실시간 재생 주소 확인 중 (전체 영상 다운로드 전)…");
                let metadata_path = work.join("source.json");
                let mut resolve = Command::new(&self.tools.ytdlp);
                resolve.args(["--ignore-config", "--no-playlist", "--no-warnings", "--no-plugin-dirs",
                    "--socket-timeout", "15", "--retries", "1", "--dump-single-json",
                    "--match-filters", "!is_live & duration <= 1800", "-f",
                    "b[ext=mp4][vcodec^=avc1][acodec^=mp4a][protocol=https][height<=720]/bv*[height<=720][vcodec^=avc1]+ba[acodec^=mp4a]/b[height<=720]/bv*[height<=720]+ba"]);
                if let Some(node) = &self.tools.node {
                    resolve
                        .arg("--js-runtimes")
                        .arg(format!("node:{}", node.display()));
                }
                resolve.arg("--").arg(source);
                run_tool_output(
                    &mut resolve,
                    &work.join("resolve.log"),
                    Some(&metadata_path),
                    Duration::from_secs(90),
                    cancelled,
                )?;
                let metadata: serde_json::Value = serde_json::from_slice(
                    &fs::read(&metadata_path).map_err(|e| e.to_string())?,
                )
                .map_err(
                    |_| "영상 정보를 얻지 못했습니다. 라이브 및 30분 초과 영상은 중계하지 않습니다.",
                )?;
                if cancelled() {
                    return Err(CANCELLED.into());
                }
                match BufferedStream::from_metadata(&metadata) {
                    Ok(Some(stream)) => {
                        progress(&format!(
                            "실시간 {}p 중계 버퍼 준비 중 - 전체 다운로드 대기 없음…",
                            stream.height
                        ));
                        if stream.prime().is_ok() {
                            if cancelled() {
                                return Err(CANCELLED.into());
                            }
                            if self
                                .server
                                .routes
                                .read()
                                .unwrap()
                                .values()
                                .filter(|source| matches!(source, MediaSource::Buffered(_)))
                                .count()
                                >= 16
                            {
                                return Err("실시간 중계 영상이 16개입니다. 중계 중지로 버퍼를 비운 뒤 다시 시도하세요.".into());
                            }
                            self.wait_for_tunnel(cancelled)?;
                            let url = format!("{}{route}", self.public_url);
                            self.server
                                .routes
                                .write()
                                .unwrap()
                                .insert(route.clone(), MediaSource::Buffered(stream));
                            progress("실시간 HTTPS 중계 연결 확인 중…");
                            verify_public(&url, &mut self.verified_host, cancelled, progress)?;
                            return Ok(url);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            }

            progress("720p HD 고화질 영상 다운로드 중…");
            let mut command = Command::new(&self.tools.ytdlp);
            command.args(["--ignore-config", "--no-playlist", "--no-progress", "--no-warnings",
                "--no-plugin-dirs", "--socket-timeout", "20", "--retries", "2",
                "--match-filters", "!is_live & duration <= 1800", "--max-filesize", "1G",
                "--concurrent-fragments", "4", "--no-simulate",
                "-f", "bv*[height<=720][vcodec^=avc1]+ba[acodec^=mp4a]/b[height<=720][vcodec^=avc1]/bv*[height<=720]+ba/b[height<=720]",
                "--merge-output-format", "mp4", "--ffmpeg-location"])
                .arg(self.tools.ffmpeg.parent().unwrap());
            if let Some(node) = &self.tools.node {
                command
                    .arg("--js-runtimes")
                    .arg(format!("node:{}", node.display()));
            }
            command
                .arg("-o")
                .arg(work.join("source.%(ext)s"))
                .arg("--")
                .arg(source);
            run_tool(
                &mut command,
                &work.join("download.log"),
                Duration::from_secs(1200),
                cancelled,
            )?;
            let input = fs::read_dir(&work).map_err(|e| e.to_string())?.filter_map(Result::ok)
                .map(|entry| entry.path()).find(|path| matches!(path.extension().and_then(|s| s.to_str()), Some("mp4" | "mkv" | "webm")))
                .ok_or("다운로드된 영상이 없습니다. 라이브, 30분 초과, 1GB 초과 영상은 중계하지 않습니다.")?;
            if input.metadata().map_err(|e| e.to_string())?.len() > MAX_VIDEO_BYTES {
                return Err("영상 크기가 중계 제한(1GB)을 넘습니다.".into());
            }
            let output = work.join("ready.mp4");
            let plan = probe_copy_plan(&self.tools.ffmpeg, &input, &work, cancelled)?;
            progress(if plan.video && plan.audio {
                "VRChat 호환 MP4 정리 중 - 무손실 초고속 준비…"
            } else {
                "호환되지 않는 코덱 변환 중…"
            });
            convert_mp4(
                &self.tools.ffmpeg,
                &input,
                &output,
                &work.join("encode.log"),
                plan,
                cancelled,
            )?;
            if cancelled() {
                return Err(CANCELLED.into());
            }
            let size = output.metadata().map_err(|e| e.to_string())?.len();
            if size == 0 || size >= MAX_VIDEO_BYTES {
                return Err("변환된 영상이 중계 용량 제한을 넘었습니다.".into());
            }
            if total + size > MAX_SESSION_BYTES {
                return Err(
                    "중계 캐시가 4GB를 넘습니다. 중계 중지로 비운 뒤 선택 영상 프록싱을 다시 누르세요."
                        .into(),
                );
            }
            let ready = self.directory.join(format!("{id}.mp4"));
            fs::rename(output, &ready).map_err(|e| e.to_string())?;
            // No partial file becomes reachable before the encode finishes.
            self.server
                .routes
                .write()
                .unwrap()
                .insert(route, MediaSource::File(ready));
            self.wait_for_tunnel(cancelled)?;
            let url = format!("{}/v/{}/{id}.mp4", self.public_url, self.secret);
            progress("외부 HTTPS 주소의 MP4 재생 연결 확인 중…");
            verify_public(&url, &mut self.verified_host, cancelled, progress)?;
            Ok(url)
        })();
        let _ = fs::remove_dir_all(work);
        result
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Stop new requests and active transfers before removing session files.
        let _ = self.tunnel.child.kill();
        let _ = self.tunnel.child.wait();
        self.server.shutdown();
        self.server.routes.write().unwrap().clear();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn verify_public(
    url: &str,
    verified_host: &mut bool,
    cancelled: &impl Fn() -> bool,
    progress: &impl Fn(&str),
) -> Result<(), String> {
    if !*verified_host {
        dns::wait_for_publication(url, cancelled, progress)?;
    }
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let mut reason = String::new();
    let started = Instant::now();
    for attempt in 0..12 {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        match client.get(url).header("Range", "bytes=0-0").send() {
            Ok(mut response) => {
                if response.status() == reqwest::StatusCode::PARTIAL_CONTENT
                    && response
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        == Some("video/mp4")
                {
                    let mut byte = [0];
                    if response.read_exact(&mut byte).is_ok() {
                        *verified_host = true;
                        return Ok(());
                    }
                }
                reason = format!("HTTP {}", response.status());
            }
            Err(error) => {
                reason = format!("{:?}", error.without_url());
            }
        }
        if started.elapsed() > Duration::from_secs(45) {
            break;
        }
        progress(&format!(
            "외부 중계 주소 연결 확인 재시도 - {}초 경과…",
            started.elapsed().as_secs()
        ));
        thread::sleep(Duration::from_secs((attempt + 1).min(4)));
    }
    Err(format!(
        "영상을 준비했지만 외부 중계 주소에 연결하지 못했습니다. {reason}"
    ))
}

fn tunnel_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let url = line[start..]
        .split_whitespace()
        .next()?
        .trim_end_matches(['|', '"']);
    let host = url
        .strip_prefix("https://")?
        .strip_suffix(".trycloudflare.com")?;
    if host.is_empty()
        || !host
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    {
        return None;
    }
    Some(url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_validates_named_tunnels_without_guessing_a_domain() {
        let quick = RelayConfig::parse("# No saved tunnel\nWINDOW_WIDTH=820").unwrap();
        assert!(quick.tunnel_token.is_none());
        assert!(quick.custom_domain.is_none());
        let named = RelayConfig::parse(
            "CUSTOM_DOMAIN=https://video.example.com/\nTUNNEL_TOKEN=test-token\nPORT=12345",
        )
        .unwrap();
        assert_eq!(named.custom_domain.as_deref(), Some("video.example.com"));
        assert_eq!(named.port, 12345);
        for invalid in [
            "TUNNEL_TOKEN=test-token",
            "PORT=0",
            "PORT=65536",
            "PORT=oops",
            "CUSTOM_DOMAIN=https://user:password@example.com",
            "CUSTOM_DOMAIN=example.com/path",
            "CUSTOM_DOMAIN=example.com?query=1",
            "CUSTOM_DOMAIN=example.com:8080",
            "CUSTOM_DOMAIN=http://example.com",
            "CUSTOM_DOMAIN=127.0.0.1",
            "CUSTOM_DOMAIN=localhost",
        ] {
            assert!(RelayConfig::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn opening_and_stopping_an_idle_host_does_not_start_a_tunnel() {
        let (send, receive) = mpsc::channel();
        let host = RelayHost::new(move |_, event| {
            let _ = send.send(event);
        });
        host.stop();
        drop(host);
        assert!(matches!(
            receive.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn remuxes_compatible_tracks_and_converts_only_incompatible_codecs() {
        let mut probe = serde_json::json!({"streams": [
            {"codec_type":"video", "codec_name":"h264", "pix_fmt":"yuv420p"},
            {"codec_type":"audio", "codec_name":"aac"}
        ]});
        assert_eq!(
            copy_plan(&probe).unwrap(),
            CopyPlan {
                video: true,
                audio: true
            }
        );
        probe["streams"][1]["codec_name"] = "opus".into();
        assert_eq!(
            copy_plan(&probe).unwrap(),
            CopyPlan {
                video: true,
                audio: false
            }
        );
        probe["streams"][0]["codec_name"] = "vp9".into();
        probe["streams"][1]["codec_name"] = "aac".into();
        assert_eq!(
            copy_plan(&probe).unwrap(),
            CopyPlan {
                video: false,
                audio: true
            }
        );
        assert!(copy_plan(&serde_json::json!({"streams":[]})).is_err());
    }

    #[test]
    fn only_canonical_search_urls_and_real_tunnel_hosts_are_accepted() {
        assert!(video_id("https://www.youtube.com/watch?v=abcdefghijk").is_ok());
        for url in [
            "http://localhost/",
            "https://www.youtube.com/watch?v=../secrets",
            "https://www.youtube.com/watch?v=abcdefghijk&list=123",
        ] {
            assert!(video_id(url).is_err());
        }
        assert_eq!(
            tunnel_url("INF | https://nice-new-host.trycloudflare.com |"),
            Some("https://nice-new-host.trycloudflare.com".into())
        );
        assert!(tunnel_url("https://evil.test/https://a.trycloudflare.com").is_none());
    }

    #[test]
    fn ranges_cover_seek_suffix_and_invalid_requests() {
        assert_eq!(byte_range(None, 10), Ok((0, 9, false)));
        assert_eq!(byte_range(Some("bytes=2-4"), 10), Ok((2, 4, true)));
        assert_eq!(byte_range(Some("bytes=8-"), 10), Ok((8, 9, true)));
        assert_eq!(byte_range(Some("bytes=-3"), 10), Ok((7, 9, true)));
        assert_eq!(byte_range(Some("bytes=2-100"), 10), Ok((2, 9, true)));
        for range in [
            "bytes=10-",
            "bytes=8-2",
            "bytes=-0",
            "bytes=0-2,4-6",
            "broken",
        ] {
            assert!(byte_range(Some(range), 10).is_err());
        }
    }

    #[test]
    fn server_streams_head_and_ranges_but_never_arbitrary_files() {
        let server = LocalServer::start().unwrap();
        let path = std::env::temp_dir().join(format!("linkfinder-test-{}.mp4", token().unwrap()));
        fs::write(&path, b"0123456789").unwrap();
        server.routes.write().unwrap().insert(
            "/v/secret/video.mp4".into(),
            MediaSource::File(path.clone()),
        );
        let request = |method: &str, path: &str, extra: &str| {
            let mut client = TcpStream::connect(server.address).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            write!(
                client,
                "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{extra}\r\n"
            )
            .unwrap();
            let mut response = String::new();
            client.read_to_string(&mut response).unwrap();
            response
        };
        let response = request("GET", "/v/secret/video.mp4", "Range: bytes=2-4\r\n");
        assert!(response.starts_with("HTTP/1.1 206"));
        assert!(response.contains("Content-Range: bytes 2-4/10"));
        assert!(response.ends_with("\r\n\r\n234"));
        let response = request("HEAD", "/v/secret/video.mp4", "");
        assert!(response.contains("Content-Length: 10"));
        assert!(response.ends_with("\r\n\r\n"));
        assert!(
            request("GET", "/v/secret/video.mp4", "Range: bytes=99-\r\n")
                .starts_with("HTTP/1.1 416")
        );
        assert!(request("GET", "/v/secret/../../Cargo.toml", "").starts_with("HTTP/1.1 404"));
        assert!(request("POST", "/v/secret/video.mp4", "").starts_with("HTTP/1.1 405"));
        drop(server);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn full_transfer_survives_split_headers_and_slow_reader_backpressure() {
        let server = LocalServer::start().unwrap();
        let path =
            std::env::temp_dir().join(format!("relay-backpressure-{}.mp4", token().unwrap()));
        let bytes: Vec<u8> = (0..4 * 1024 * 1024).map(|n| (n % 251) as u8).collect();
        fs::write(&path, &bytes).unwrap();
        server
            .routes
            .write()
            .unwrap()
            .insert("/large.mp4".into(), MediaSource::File(path.clone()));
        let mut client = TcpStream::connect(server.address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        client
            .write_all(b"GET /large.mp4 HTTP/1.1\r\nHost: loca")
            .unwrap();
        thread::sleep(Duration::from_millis(40));
        client.write_all(b"lhost\r\n\r\n").unwrap();
        thread::sleep(Duration::from_millis(150));
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        let header_end = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert!(response.starts_with(b"HTTP/1.1 200"));
        assert_eq!(&response[header_end..], bytes.as_slice());
        drop(server);
        fs::remove_file(path).unwrap();
    }

    #[test]
    #[ignore = "Downloads official helpers and opens a temporary public tunnel for a generated test video"]
    fn live_tunnel_serves_a_generated_mp4() {
        let progress = |text: &str| println!("{text}");
        let mut session = Session::start(&|| false, &progress).unwrap();
        let sample = session.directory.join("sample.mp4");
        let mut command = Command::new(&session.tools.ffmpeg);
        command
            .args([
                "-nostdin",
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=160x90:r=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=44100",
                "-t",
                "1",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-movflags",
                "+faststart",
            ])
            .arg(&sample);
        run_tool(
            &mut command,
            &session.directory.join("sample.log"),
            Duration::from_secs(30),
            &|| false,
        )
        .unwrap();
        let route = format!("/v/{}/sample.mp4", session.secret);
        session
            .server
            .routes
            .write()
            .unwrap()
            .insert(route.clone(), MediaSource::File(sample));
        session.wait_for_tunnel(&|| false).unwrap();
        let url = format!("{}{route}", session.public_url);
        verify_public(&url, &mut false, &|| false, &progress).unwrap();
        println!("Public HTTPS MP4 range request verified");
        let directory = session.directory.clone();
        drop(session);
        assert!(
            !directory.exists(),
            "Session files must be removed on shutdown"
        );
    }

    #[test]
    #[ignore = "Checks live HTTPS playback, seeking, buffering, decoding, and timings without UI automation"]
    fn live_youtube_download_and_hosting() {
        let source = std::env::var("LINKFINDER_TEST_VIDEO_URL")
            .unwrap_or_else(|_| "https://www.youtube.com/watch?v=jNQXAC9IVRw".into());
        exercise_live_video(&source, true);
    }

    #[test]
    #[ignore = "Exercises the file fallback, codec-copy path, and live HTTPS playback without UI automation"]
    fn live_file_fallback() {
        let source = std::env::var("LINKFINDER_TEST_VIDEO_URL")
            .unwrap_or_else(|_| "https://www.youtube.com/watch?v=jNQXAC9IVRw".into());
        exercise_live_video(&source, false);
    }

    #[test]
    #[ignore = "Searches a real music result and checks progressive playback without a full download"]
    fn live_music_buffered_playback() {
        let results =
            crate::search::search_youtube("잇떼", crate::search::SearchMode::Pronunciation, 5)
                .unwrap();
        let video = results
            .iter()
            .find(|video| !video.is_recommendation)
            .expect("A real song result is required");
        println!("Music sample: {} ({})", video.title, video.url);
        exercise_live_video(&video.url, true);
    }

    fn exercise_live_video(source: &str, stream_first: bool) {
        let started = Instant::now();
        let progress = |text: &str| println!("[{:.3}s] {text}", started.elapsed().as_secs_f64());
        let mut session = Session::start(&|| false, &progress).unwrap();
        let url = session
            .prepare_with_mode(source, &|| false, &progress, stream_first)
            .unwrap();
        let ready_seconds = started.elapsed().as_secs_f64();
        assert!(url.starts_with("https://"));
        let id = video_id(source).unwrap();
        assert!(url.ends_with(&format!("/{id}.mp4")));
        let route = url.strip_prefix(&session.public_url).unwrap();
        let media = session
            .server
            .routes
            .read()
            .unwrap()
            .get(route)
            .unwrap()
            .clone();
        let (size, buffered_at_ready, resolution) = match &media {
            MediaSource::File(path) => (path.metadata().unwrap().len(), 0, 0),
            MediaSource::Buffered(stream) => (
                stream.size,
                stream.fetched_bytes.load(Ordering::Relaxed),
                stream.height,
            ),
        };
        let streaming = matches!(media, MediaSource::Buffered(_));
        if !stream_first {
            assert!(!streaming);
        }
        if streaming && size > 512 * 1024 {
            assert!(
                buffered_at_ready < size,
                "Playback must be ready before a full download"
            );
        }
        println!(
            "READY_SECONDS={ready_seconds:.3} STREAMING={streaming} SIZE={size} BUFFERED_AT_READY={buffered_at_ready}"
        );
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(90))
            .build()
            .unwrap();
        let head = client.head(&url).send().unwrap();
        assert_eq!(head.status(), reqwest::StatusCode::OK);
        assert_eq!(head.headers()["content-type"], "video/mp4");
        assert_eq!(
            head.headers()["content-length"]
                .to_str()
                .unwrap()
                .parse::<u64>()
                .unwrap(),
            size
        );
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/relay-benchmark");
        fs::create_dir_all(&directory).unwrap();
        let mode = if streaming { "streaming" } else { "file" };
        let stem = format!("{id}-{mode}");
        // FFmpeg acts as a real HTTP media player and seeks before the full file is fetched.
        let seek_started = Instant::now();
        let mut decode = Command::new(&session.tools.ffmpeg);
        decode
            .args(["-nostdin", "-v", "error", "-xerror", "-ss", "5", "-i"])
            .arg(&url)
            .args([
                "-t", "1", "-map", "0:v:0", "-map", "0:a:0", "-f", "null", "-",
            ]);
        run_tool(
            &mut decode,
            &directory.join(format!("{stem}-remote-seek.log")),
            Duration::from_secs(60),
            &|| false,
        )
        .unwrap();
        let seek_seconds = seek_started.elapsed().as_secs_f64();
        let mut checks = Vec::new();
        for start in [0, size / 2, size - 32] {
            let end = (start + 31).min(size - 1);
            let response = client
                .get(&url)
                .header("range", format!("bytes={start}-{end}"))
                .send()
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::PARTIAL_CONTENT);
            assert_eq!(
                response.headers()["content-range"],
                format!("bytes {start}-{end}/{size}")
            );
            let bytes = response.bytes().unwrap();
            assert_eq!(bytes.len() as u64, end - start + 1);
            checks.push((start as usize, bytes));
        }
        let fetch_started = Instant::now();
        let response = client.get(&url).send().unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let mut bytes = Vec::new();
        response
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .unwrap();
        let download_seconds = fetch_started.elapsed().as_secs_f64();
        assert_eq!(bytes.len() as u64, size);
        assert!(
            bytes.len() <= 64 * 1024 * 1024,
            "The smoke test uses a small sample"
        );
        for (start, expected) in checks {
            assert_eq!(&bytes[start..start + expected.len()], expected.as_ref());
        }
        let sample = directory.join(format!("{stem}.mp4"));
        fs::write(&sample, bytes).unwrap();
        assert_eq!(
            probe_copy_plan(&session.tools.ffmpeg, &sample, &directory, &|| false).unwrap(),
            CopyPlan {
                video: true,
                audio: true
            }
        );
        let decode_started = Instant::now();
        let mut decode = Command::new(&session.tools.ffmpeg);
        decode
            .args(["-nostdin", "-v", "error", "-xerror", "-i"])
            .arg(sample)
            .args(["-f", "null", "-"]);
        run_tool(
            &mut decode,
            &directory.join(format!("{stem}-decode.log")),
            Duration::from_secs(90),
            &|| false,
        )
        .unwrap();
        let decode_seconds = decode_started.elapsed().as_secs_f64();
        let repeat_started = Instant::now();
        assert_eq!(session.prepare(source, &|| false, &progress).unwrap(), url);
        let repeat_seconds = repeat_started.elapsed().as_secs_f64();
        let report = serde_json::json!({
            "source": source, "streaming": streaming, "height": resolution,
            "size_bytes": size, "buffered_bytes_at_ready": buffered_at_ready,
            "ready_seconds": ready_seconds, "remote_seek_decode_seconds": seek_seconds,
            "full_http_download_seconds": download_seconds, "full_decode_seconds": decode_seconds,
            "cached_prepare_seconds": repeat_seconds, "head_and_range_checks": "passed",
            "full_media_decode": "passed", "remote_seek_decode": "passed",
        });
        let path = directory.join(format!("{stem}-report.json"));
        fs::write(&path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        println!("Report: {}", path.display());
    }
}
