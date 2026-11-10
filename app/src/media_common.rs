use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Frame {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

#[derive(Default)]
pub struct Shared {
    pub frame: Mutex<Option<Frame>>,
    pub seq: AtomicU64,
    pub status: Mutex<String>,
    pub stop: AtomicBool,
    pub paused: AtomicBool,
    pub restart: AtomicBool,
    /// Latest seek request in milliseconds, encoded as position + 1 (0 means none).
    pub seek_ms: AtomicU64,
    pub looping: AtomicBool,
    pub ended: AtomicBool,
    pub pos_ms: AtomicU64,
    pub dur_ms: AtomicU64,
    /// Chroma key: 0 = off, else packed 0x00RRGGBB key colour | (tolerance << 24).
    pub key: AtomicU64,
    pub size: Mutex<(usize, usize)>,
    /// Camera: every mode the device offers (resolution, frame rate, video format) and the one
    /// in use, read when the camera opens.
    pub modes: Mutex<Vec<CamMode>>,
    pub mode: Mutex<Option<CamMode>>,
}

/// Largest picture height a camera's Auto mode picks (Settings → Performance).
pub static AUTO_CAM_MAX_H: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1080);

/// One capture mode of a camera: picture size, frame rate and the video format it sends
/// (MJPG, YUY2, NV12, H264 …).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CamMode {
    pub w: u32,
    pub h: u32,
    pub fps_n: u32,
    pub fps_d: u32,
    pub format: String,
}

impl CamMode {
    pub fn fps(&self) -> f64 {
        self.fps_n as f64 / self.fps_d.max(1) as f64
    }
    pub fn label(&self) -> String {
        let f = self.fps();
        let fps = if (f - f.round()).abs() < 0.01 { format!("{}", f.round() as i64) } else { format!("{f:.2}") };
        format!("{}x{} {} fps {}", self.w, self.h, fps, self.format)
    }
    /// One-line form for saving in presets: "w,h,n,d,format".
    pub fn code(&self) -> String {
        format!("{},{},{},{},{}", self.w, self.h, self.fps_n, self.fps_d, self.format)
    }
    pub fn parse(s: &str) -> Option<CamMode> {
        let f: Vec<&str> = s.split(',').collect();
        if f.len() != 5 {
            return None;
        }
        Some(CamMode { w: f[0].parse().ok()?, h: f[1].parse().ok()?, fps_n: f[2].parse().ok()?, fps_d: f[3].parse().ok()?, format: f[4].to_string() })
    }
}

impl Shared {
    pub(crate) fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap() = s.into();
    }
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub name: String,
    pub link: String,
    pub blackmagic: bool,
}

pub fn is_blackmagic(name: &str) -> bool {
    let n = name.to_lowercase();
    ["blackmagic", "decklink", "intensity", "ultrastudio", "atem", "web presenter"].iter().any(|k| n.contains(k))
}

#[derive(Clone)]
pub enum Source {
    /// Camera: symbolic link and the chosen mode (None = Auto: best up to 1920x1080).
    Device(String, Option<CamMode>),
    DeckLink(crate::decklink::CaptureConfig),
    Ndi(String),
    Omt(String),
    Srt(crate::srt::Connection),
    File(String),
    Desktop,
    /// Folder of pictures: Image Sequence (frames at `fps`).
    Sequence(String, f32),
    /// Folder of pictures: Photos slideshow (`secs` per picture).
    Photos(String, f32),
}

#[derive(Clone)]
pub(crate) struct RecFrame {
    pub(crate) pixels: Arc<Vec<u8>>,
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) captured_at: Instant,
    pub(crate) bgra: bool,
}

/// Writes frames to a file on a bounded, independent encoder queue.
pub struct Recorder {
    pub(crate) tx: SyncSender<RecFrame>,
    pub path: String,
    pub error: Arc<Mutex<String>>,
    encoder: Arc<Mutex<String>>,
    dropped: AtomicU64,
}

impl Recorder {
    #[allow(dead_code)]
    pub fn start(path: String) -> Recorder {
        Self::start_with(path, DEFAULT_MBPS)
    }
    /// Starts a recording at `mbps` megabits per second (Recording Setup "Bit Rate").
    pub fn start_with(path: String, mbps: u32) -> Recorder {
        Self::start_opts(path, RecOpts { mbps, ..RecOpts::default() })
    }
    /// Starts a recording with the project's size and frame rate and the encoder choice.
    pub fn start_opts(path: String, opts: RecOpts) -> Recorder {
        Self::spawn_writer(path, opts).0
    }

    pub(crate) fn spawn_writer(path: String, opts: RecOpts) -> (Recorder, std::thread::JoinHandle<()>) {
        // Keep the UI responsive and bound memory if the encoder falls behind.
        let (tx, rx) = sync_channel(3);
        let error = Arc::new(Mutex::new(String::new()));
        let encoder = Arc::new(Mutex::new("Waiting for first frame".into()));
        let (p2, e2) = (path.clone(), error.clone());
        let encoder_status = Arc::clone(&encoder);
        let worker = std::thread::spawn(move || crate::media::record_thread(p2, rx, e2, encoder_status, opts));
        (Recorder { tx, path, error, encoder, dropped: AtomicU64::new(0) }, worker)
    }
    pub fn push(&self, rgba: Vec<u8>, w: usize, h: usize) {
        self.enqueue(RecFrame { pixels: Arc::new(rgba), w, h, captured_at: Instant::now(), bgra: false });
    }

    pub fn push_group(recorders: &[Arc<Self>], pixels: impl Into<Arc<Vec<u8>>>, w: usize, h: usize, captured_at: Instant, bgra: bool) {
        let frame = RecFrame { pixels: pixels.into(), w, h, captured_at, bgra };
        for recorder in recorders {
            recorder.enqueue(frame.clone());
        }
    }

    fn enqueue(&self, frame: RecFrame) {
        if matches!(self.tx.try_send(frame), Err(std::sync::mpsc::TrySendError::Full(_))) {
            self.note_dropped_frames(1);
        }
    }

    pub fn note_dropped_frames(&self, count: u64) {
        self.dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub fn diagnostics(&self) -> String {
        let encoder = self.encoder.lock().unwrap();
        format!("{encoder} | Dropped frames: {}", self.dropped.load(Ordering::Relaxed))
    }
}

/// MP4 video bit rate unless Recording Setup says otherwise: the original's recording settings
/// key "MPEGBitRate" defaults to 8 Mbps (kh::get_j @0xe4298).
pub const DEFAULT_MBPS: u32 = 8;
pub const RECORDING_FPS: u32 = 30;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RecordFormat {
    #[default]
    H264,
    Hevc,
    Wmv,
}

impl RecordFormat {
    pub fn parse(value: &str) -> Self {
        match value {
            "hevc" => Self::Hevc,
            "wmv" => Self::Wmv,
            _ => Self::H264,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Wmv => "wmv",
        }
    }

    pub fn codec(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
            Self::Wmv => "WMV9",
        }
    }

    pub fn extension(self) -> &'static str {
        if self == Self::Wmv { "wmv" } else { "mp4" }
    }

}

/// How a recording is written.
#[derive(Clone, Copy, Debug)]
pub struct RecOpts {
    /// Video bit rate in Mbps.
    pub mbps: u32,
    /// Picture size of the file (the project's Output Size); None = as captured.
    pub size: Option<(u32, u32)>,
    /// Frames per second written (the project's frame rate).
    pub fps: f32,
    /// Use the graphics card's encoder when there is one (recording key "HardwareAcceleration").
    pub hw: bool,
    pub format: RecordFormat,
}

impl Default for RecOpts {
    fn default() -> Self {
        RecOpts { mbps: DEFAULT_MBPS, size: None, fps: RECORDING_FPS as f32, hw: true, format: RecordFormat::H264 }
    }
}

/// Frame rate as a fraction: 29.97 → 30000/1001, 59.94 → 60000/1001, 23.976 → 24000/1001.
pub fn rate_fraction(fps: f32) -> (u32, u32) {
    let r = fps.round();
    if (fps - r).abs() > 0.001 {
        ((r * 1000.0) as u32, 1001)
    } else {
        (r.max(1.0) as u32, 1)
    }
}
/// "New File Every" choices in minutes, 0 = None (frmRecordingSetup cboRestart items; the
/// "Interval" key defaults to 0, kh::get_i @0xe4274).
pub const NEW_FILE_EVERY: [u32; 8] = [0, 1, 5, 10, 15, 20, 30, 60];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickKind {
    Video,
    Image,
    Title,
    PresetOpen,
    PresetSave,
    /// Several video files (List input); answer is the paths joined by '\n'.
    VideoMany,
    /// A folder (Image Sequence / Photos).
    Folder,
}

fn picture_files(folder: &str) -> Vec<std::path::PathBuf> {
    let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(folder)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    matches!(e.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "gif")
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Image Sequence (one picture per frame at `fps`) or Photos (each picture for `secs`).
pub(crate) fn run_pictures(sh: Arc<Shared>, folder: String, sequence: bool, fps: f32, secs: f32) {
    let files = picture_files(&folder);
    if files.is_empty() {
        sh.set_status("No pictures (PNG/JPG/BMP) in this folder");
        return;
    }
    let step = if sequence { Duration::from_secs_f32(1.0 / fps.max(1.0)) } else { Duration::from_secs_f32(secs.max(0.5)) };
    let total_ms = (step.as_millis() as u64) * files.len() as u64;
    sh.dur_ms.store(total_ms, Ordering::Relaxed);
    sh.set_status("");
    let mut i = 0usize;
    let mut shown = false;
    while !sh.stop.load(Ordering::Relaxed) {
        if sh.restart.swap(false, Ordering::Relaxed) {
            i = 0;
            shown = false;
            sh.ended.store(false, Ordering::Relaxed);
        }
        let paused = sh.paused.load(Ordering::Relaxed) || sh.ended.load(Ordering::Relaxed);
        if paused && shown {
            std::thread::sleep(Duration::from_millis(15));
            continue;
        }
        let t0 = Instant::now();
        if let Ok(img) = image::open(&files[i]) {
            let img = img.to_rgba8();
            let (w, h) = (img.width() as usize, img.height() as usize);
            *sh.size.lock().unwrap() = (w, h);
            *sh.frame.lock().unwrap() = Some(Frame { w, h, rgba: img.into_raw() });
            sh.seq.fetch_add(1, Ordering::Relaxed);
        }
        shown = true;
        sh.pos_ms.store(step.as_millis() as u64 * i as u64, Ordering::Relaxed);
        if !sh.paused.load(Ordering::Relaxed) {
            // wait for the next picture (keep reacting to stop / pause / restart)
            while t0.elapsed() < step && !sh.stop.load(Ordering::Relaxed) && !sh.restart.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5).min(step));
            }
            if i + 1 < files.len() {
                i += 1;
            } else if sh.looping.load(Ordering::Relaxed) {
                i = 0;
            } else {
                sh.ended.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Saves a picture by its extension like kb::d @0xe3930 (.png → PNG, .jpg → JPEG, else BMP).
pub fn save_picture(path: &str, rgba: &[u8], w: usize, h: usize) -> Result<(), String> {
    let lower = path.to_lowercase();
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec()).ok_or("bad picture size")?;
    let res = if lower.ends_with(".png") {
        img.save_with_format(path, image::ImageFormat::Png)
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        image::DynamicImage::ImageRgba8(img).to_rgb8().save_with_format(path, image::ImageFormat::Jpeg)
    } else {
        image::DynamicImage::ImageRgba8(img).to_rgb8().save_with_format(path, image::ImageFormat::Bmp)
    };
    res.map_err(|e| e.to_string())
}
