//! Desktop-only, local MP4 hosting. The public tunnel exposes a read-only file
//! route, never the download controls or arbitrary paths on the computer.
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
        mpsc::{self, Sender},
    },
    thread,
    time::{Duration, Instant},
};
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
type Routes = Arc<RwLock<HashMap<String, PathBuf>>>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AfterSearch {
    #[default]
    None,
    Copy,
    Proxy,
}

impl AfterSearch {
    pub fn toggle(self, selected: Self) -> Self {
        if self == selected {
            Self::None
        } else {
            selected
        }
    }
}

#[derive(Debug)]
pub enum HostingEvent {
    Progress(String),
    Ready { source: String, url: String },
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
                            Ok(url) => notify(current, HostingEvent::Ready { source, url }),
                            Err(error) => notify(current, HostingEvent::Failed(error)),
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
    let output = File::create(log).map_err(|e| e.to_string())?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(output));
    let mut process = ManagedChild::spawn(command)?;
    let start = Instant::now();
    loop {
        if cancelled() {
            return Err(CANCELLED.into());
        }
        if start.elapsed() > timeout {
            return Err("영상 준비 시간이 초과되었습니다. 다시 검색해 주세요.".into());
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
    fn start() -> Result<Self, String> {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("중계 서버 시작 실패: {e}"))?;
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
                        if connections.load(Ordering::SeqCst) >= 16 {
                            continue;
                        }
                        connections.fetch_add(1, Ordering::SeqCst);
                        let active = connections.clone();
                        let routes = shared.clone();
                        let stopping = stopping.clone();
                        thread::spawn(move || {
                            let _ = serve(stream, routes, &stopping);
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
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    reader.by_ref().take(8193).read_line(&mut request)?;
    let mut words = request.split_whitespace();
    let method = words.next().unwrap_or("");
    let path = words.next().unwrap_or("");
    if method != "GET" && method != "HEAD" {
        return stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nAllow: GET, HEAD\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }
    let file_path = routes.read().unwrap().get(path).cloned();
    let Some(file_path) = file_path else {
        return stream.write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    };
    let mut range = None;
    let mut bytes = request.len();
    loop {
        let mut line = String::new();
        let read = reader.by_ref().take(8193).read_line(&mut line)?;
        bytes += read;
        if bytes > 8192 || read == 0 {
            return stream.write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        }
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                if range.is_some() {
                    return stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
                range = Some(value.trim().to_owned());
            }
        }
    }
    let mut file = File::open(file_path)?;
    let size = file.metadata()?.len();
    let Ok((start, end, partial)) = byte_range(
        if method == "HEAD" {
            None
        } else {
            range.as_deref()
        },
        size,
    ) else {
        return write!(
            stream,
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
    };
    let length = end - start + 1;
    write!(
        stream,
        "HTTP/1.1 {}\r\nContent-Type: video/mp4\r\nAccept-Ranges: bytes\r\nContent-Length: {length}\r\nCache-Control: private, no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n",
        if partial {
            "206 Partial Content"
        } else {
            "200 OK"
        }
    )?;
    if partial {
        write!(stream, "Content-Range: bytes {start}-{end}/{size}\r\n")?;
    }
    stream.write_all(b"\r\n")?;
    if method == "HEAD" {
        return Ok(());
    }
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = length;
    let mut buffer = [0u8; 65536];
    while remaining > 0 && !stop.load(Ordering::SeqCst) {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..limit])?;
        if read == 0 {
            break;
        }
        stream.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    Ok(())
}

struct HelperTools {
    ffmpeg: PathBuf,
    ytdlp: PathBuf,
    cloudflared: PathBuf,
    node: Option<PathBuf>,
}

struct Session {
    tunnel: ManagedChild,
    server: LocalServer,
    directory: PathBuf,
    tools: HelperTools,
    public_url: String,
    secret: String,
}

impl Session {
    fn start(cancelled: &impl Fn() -> bool, progress: &impl Fn(&str)) -> Result<Self, String> {
        let root = std::env::current_exe()
            .map_err(|e| e.to_string())?
            .parent()
            .ok_or("앱 폴더를 찾을 수 없습니다.")?
            .to_owned();
        let tools = root.join("tools");
        let ffmpeg = find_tool("ffmpeg.exe", &tools).ok_or("FFmpeg가 필요합니다. tools 폴더에 ffmpeg.exe를 넣거나 PATH에 설치한 뒤 다시 시도하세요.")?;
        progress("중계 도구 확인 중 · 처음에는 yt-dlp와 cloudflared를 자동으로 받습니다…");
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
        let directory = root.join(".relay-cache").join(&secret);
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
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(&str),
    ) -> Result<Self, String> {
        progress("임시 HTTPS 중계 주소를 만드는 중…");
        let server = LocalServer::start()?;
        let config = directory.join("tunnel.yml");
        fs::write(&config, "{}\n").map_err(|e| e.to_string())?;
        let mut command = Command::new(&tools.cloudflared);
        command
            .args(["tunnel", "--config"])
            .arg(config)
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
        let start = Instant::now();
        let mut hostname = None;
        let mut connected = false;
        let mut last_error = String::new();
        let public_url = loop {
            if cancelled() {
                return Err(CANCELLED.into());
            }
            while let Ok(line) = receiver.try_recv() {
                if let Some(url) = tunnel_url(&line) {
                    hostname = Some(url);
                }
                if line.contains("Registered tunnel connection") {
                    connected = true;
                }
                if line.contains("ERR") {
                    last_error = line;
                }
            }
            if connected {
                if let Some(url) = hostname.take() {
                    break url;
                }
            }
            if tunnel
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
            {
                return Err(format!("임시 터널 연결이 종료되었습니다. {last_error}"));
            }
            if start.elapsed() > Duration::from_secs(60) {
                return Err(format!(
                    "임시 터널 연결 시간이 초과되었습니다. {last_error}"
                ));
            }
            thread::sleep(Duration::from_millis(100));
        };
        Ok(Self {
            tunnel,
            server,
            directory,
            tools,
            public_url,
            secret,
        })
    }

    fn prepare(
        &mut self,
        source: &str,
        cancelled: &impl Fn() -> bool,
        progress: &impl Fn(&str),
    ) -> Result<String, String> {
        let id = video_id(source)?;
        if self
            .tunnel
            .child
            .try_wait()
            .map_err(|e| e.to_string())?
            .is_some()
        {
            return Err("임시 터널이 종료되었습니다. 프록싱을 껐다 켜고 다시 검색하세요.".into());
        }
        let route = format!("/v/{}/{id}.mp4", self.secret);
        let url = format!("{}{route}", self.public_url);
        if self.server.routes.read().unwrap().contains_key(&route) {
            verify_public(&url, cancelled)?;
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
                "중계 캐시가 4GB를 넘었습니다. 프록싱을 껐다 켜서 비운 뒤 다시 검색하세요.".into(),
            );
        }
        let work = self.directory.join(format!("work-{id}"));
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let result = (|| {
            progress("첫 번째 영상 다운로드 중 · 최대 720p / 30분…");
            let mut command = Command::new(&self.tools.ytdlp);
            command.args(["--ignore-config", "--no-playlist", "--no-progress", "--no-warnings",
                "--no-plugin-dirs", "--socket-timeout", "20", "--retries", "2",
                "--match-filters", "!is_live & duration <= 1800", "--max-filesize", "1G",
                "-f", "bv*[height<=720][vcodec^=avc1]+ba[acodec^=mp4a]/b[height<=720][vcodec^=avc1]/bv*[height<=720]+ba/b[height<=720]",
                "--merge-output-format", "mkv", "--ffmpeg-location"])
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
                .ok_or("다운로드된 영상이 없습니다. 라이브·30분 초과·1GB 초과 영상은 중계하지 않습니다.")?;
            if input.metadata().map_err(|e| e.to_string())?.len() > MAX_VIDEO_BYTES {
                return Err("영상 크기가 중계 제한(1GB)을 넘습니다.".into());
            }
            progress("VRChat용 MP4 준비 중 · H.264 / AAC / 빠른 재생…");
            let output = work.join("ready.mp4");
            let mut command = Command::new(&self.tools.ffmpeg);
            command
                .args(["-nostdin", "-y", "-v", "error", "-i"])
                .arg(input)
                .args([
                    "-map",
                    "0:v:0",
                    "-map",
                    "0:a:0",
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
                    "-c:a",
                    "aac",
                    "-b:a",
                    "128k",
                    "-movflags",
                    "+faststart",
                    "-fs",
                    "1073741824",
                ])
                .arg(&output);
            run_tool(
                &mut command,
                &work.join("encode.log"),
                Duration::from_secs(1200),
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
                    "중계 캐시가 4GB를 넘습니다. 프록싱을 껐다 켜서 비운 뒤 다시 검색하세요."
                        .into(),
                );
            }
            let ready = self.directory.join(format!("{id}.mp4"));
            fs::rename(output, &ready).map_err(|e| e.to_string())?;
            // No partial file becomes reachable before the encode finishes.
            self.server.routes.write().unwrap().insert(route, ready);
            progress("외부 HTTPS 주소의 MP4 재생 연결 확인 중…");
            verify_public(&url, cancelled)?;
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

fn verify_public(url: &str, cancelled: &impl Fn() -> bool) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| e.to_string())?;
    let mut reason = String::new();
    for _ in 0..8 {
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
                        return Ok(());
                    }
                }
                reason = format!("HTTP {}", response.status());
            }
            Err(error) => {
                reason = error.without_url().to_string();
            }
        }
        thread::sleep(Duration::from_secs(2));
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
    fn automatic_actions_are_mutually_exclusive_and_default_off() {
        let state = AfterSearch::default();
        assert_eq!(state, AfterSearch::None);
        assert_eq!(state.toggle(AfterSearch::Copy), AfterSearch::Copy);
        assert_eq!(
            AfterSearch::Copy.toggle(AfterSearch::Proxy),
            AfterSearch::Proxy
        );
        assert_eq!(
            AfterSearch::Proxy.toggle(AfterSearch::Proxy),
            AfterSearch::None
        );
        assert_eq!(
            AfterSearch::Proxy.toggle(AfterSearch::Copy),
            AfterSearch::Copy
        );
        assert_eq!(
            AfterSearch::Copy.toggle(AfterSearch::Copy),
            AfterSearch::None
        );
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
        server
            .routes
            .write()
            .unwrap()
            .insert("/v/secret/video.mp4".into(), path.clone());
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
    #[ignore = "Downloads official helpers and opens a temporary public tunnel for a generated test video"]
    fn live_tunnel_serves_a_generated_mp4() {
        let progress = |text: &str| println!("{text}");
        let session = Session::start(&|| false, &progress).unwrap();
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
            .insert(route.clone(), sample);
        let url = format!("{}{route}", session.public_url);
        verify_public(&url, &|| false).unwrap();
        println!("Public HTTPS MP4 range request verified");
        let directory = session.directory.clone();
        drop(session);
        assert!(
            !directory.exists(),
            "Session files must be removed on shutdown"
        );
    }

    #[test]
    #[ignore = "Downloads a short public video and checks the complete relay pipeline"]
    fn live_youtube_download_and_hosting() {
        let progress = |text: &str| println!("{text}");
        let mut session = Session::start(&|| false, &progress).unwrap();
        let source = std::env::var("LINKFINDER_TEST_VIDEO_URL")
            .unwrap_or_else(|_| "https://www.youtube.com/watch?v=jNQXAC9IVRw".into());
        let url = session.prepare(&source, &|| false, &progress).unwrap();
        assert!(url.starts_with("https://"));
        assert!(url.ends_with(&format!("/{}.mp4", video_id(&source).unwrap())));
        assert_eq!(session.prepare(&source, &|| false, &progress).unwrap(), url);
        println!(
            "YouTube download, H.264/AAC conversion, public range requests, and cache reuse verified"
        );
    }
}
