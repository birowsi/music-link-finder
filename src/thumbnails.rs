//! Bounded background image fetching. GDI+ images stay on the window thread.
use std::os::windows::ffi::OsStrExt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::Read,
    path::PathBuf,
    ptr::null_mut,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};
use windows_sys::Win32::Graphics::GdiPlus::{
    GdipDisposeImage, GdipDrawImageRectI, GdipGetImageHeight, GdipGetImageWidth,
    GdipLoadImageFromFile, GpGraphics, GpImage,
};

const MAX_BYTES: u64 = 512 * 1024;

pub struct Message {
    generation: u64,
    key: String,
    path: Option<PathBuf>,
}

impl Drop for Message {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

pub struct Image {
    image: *mut GpImage,
    path: PathBuf,
    width: u32,
    height: u32,
}

impl Image {
    fn open(path: PathBuf) -> Result<Self, PathBuf> {
        let name: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut image = null_mut();
        let mut width = 0;
        let mut height = 0;
        unsafe {
            if GdipLoadImageFromFile(name.as_ptr(), &mut image) != 0 || image.is_null() {
                return Err(path);
            }
            GdipGetImageWidth(image, &mut width);
            GdipGetImageHeight(image, &mut height);
            if width == 0 || height == 0 || width > 4096 || height > 4096 {
                GdipDisposeImage(image);
                return Err(path);
            }
        }
        Ok(Self {
            image,
            path,
            width,
            height,
        })
    }

    pub unsafe fn paint(&self, graphics: *mut GpGraphics, x: i32, y: i32, width: i32, height: i32) {
        let ratio = (width as f64 / self.width as f64).min(height as f64 / self.height as f64);
        let w = (self.width as f64 * ratio).round() as i32;
        let h = (self.height as f64 * ratio).round() as i32;
        unsafe {
            GdipDrawImageRectI(
                graphics,
                self.image,
                x + (width - w) / 2,
                y + (height - h) / 2,
                w,
                h,
            );
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        unsafe {
            GdipDisposeImage(self.image);
        }
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Default)]
pub struct Thumbnails {
    generation: Arc<AtomicU64>,
    images: HashMap<String, Image>,
    failed: HashSet<String>,
}

fn image_id(url: &str) -> Option<&str> {
    let id = url
        .strip_prefix("https://i.ytimg.com/vi/")?
        .strip_suffix("/mqdefault.jpg")?;
    (id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(id)
}

fn fetch(client: &reqwest::blocking::Client, url: &str, generation: u64) -> Option<PathBuf> {
    let id = image_id(url)?;
    let response = client.get(url).send().ok()?.error_for_status().ok()?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BYTES)
    {
        return None;
    }
    let mut bytes = Vec::new();
    response.take(MAX_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_BYTES || !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    let path = std::env::temp_dir().join(format!(
        "utalink-{}-{generation}-{id}.jpg",
        std::process::id()
    ));
    fs::write(&path, bytes).ok()?;
    Some(path)
}

impl Thumbnails {
    pub fn clear(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.images.clear();
        self.failed.clear();
    }

    pub fn load(&mut self, urls: Vec<String>, notify: impl Fn(Message) + Send + Sync + 'static) {
        self.clear();
        let generation = self.generation.load(Ordering::SeqCst);
        let active = self.generation.clone();
        let mut seen = HashSet::new();
        let queue = Arc::new(Mutex::new(
            urls.into_iter()
                .filter(|url| seen.insert(url.clone()))
                .collect::<VecDeque<_>>(),
        ));
        thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(8))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .ok();
            thread::scope(|scope| {
                for _ in 0..3 {
                    let queue = &queue;
                    let active = &active;
                    let notify = &notify;
                    let client = &client;
                    scope.spawn(move || {
                        loop {
                            if active.load(Ordering::SeqCst) != generation {
                                return;
                            }
                            let Some(key) = queue.lock().unwrap().pop_front() else {
                                return;
                            };
                            let path = client
                                .as_ref()
                                .and_then(|client| fetch(client, &key, generation));
                            let message = Message {
                                generation,
                                key,
                                path,
                            };
                            if active.load(Ordering::SeqCst) == generation {
                                notify(message);
                            }
                        }
                    });
                }
            });
        });
    }

    pub fn apply(&mut self, mut message: Message) {
        if message.generation != self.generation.load(Ordering::SeqCst) {
            return;
        }
        if let Some(path) = message.path.take() {
            match Image::open(path) {
                Ok(image) => {
                    self.images.insert(message.key.clone(), image);
                    return;
                }
                Err(path) => {
                    let _ = fs::remove_file(path);
                }
            }
        }
        self.failed.insert(message.key.clone());
    }

    pub fn get(&self, url: &str) -> Option<&Image> {
        self.images.get(url)
    }
    pub fn failed(&self, url: &str) -> bool {
        self.failed.contains(url)
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thumbnail_urls_cannot_escape_the_youtube_image_host_or_path() {
        assert_eq!(
            image_id("https://i.ytimg.com/vi/25ewiMJEfm4/mqdefault.jpg"),
            Some("25ewiMJEfm4")
        );
        for url in [
            "http://i.ytimg.com/vi/25ewiMJEfm4/mqdefault.jpg",
            "https://evil.example/vi/25ewiMJEfm4/mqdefault.jpg",
            "https://i.ytimg.com/vi/../../secret/mqdefault.jpg",
            "https://i.ytimg.com/vi/short/mqdefault.jpg",
        ] {
            assert!(image_id(url).is_none());
        }
    }
    #[test]
    fn stale_messages_release_their_temporary_files() {
        let path =
            std::env::temp_dir().join(format!("utalink-stale-test-{}.jpg", std::process::id()));
        fs::write(&path, b"test").unwrap();
        let mut cache = Thumbnails::default();
        cache.clear();
        cache.apply(Message {
            generation: 0,
            key: "obsolete".into(),
            path: Some(path.clone()),
        });
        assert!(!path.exists());
        assert!(cache.images.is_empty());
    }
}
