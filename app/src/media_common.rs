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
    pub audio_channels: AtomicU64,
    pub direct_audio: AtomicBool,
    #[cfg(windows)]
    pub(crate) gpu_frame: Mutex<Option<crate::dx_interop::Picture>>,
    #[cfg(windows)]
    pub(crate) gpu_playback: AtomicBool,
    #[cfg(windows)]
    pub(crate) cpu_frames_required: AtomicBool,
    pub video_adapter: Mutex<Option<String>>,
    pub browser_fps: AtomicU64,
    pub deinterlace_blend: AtomicBool,
    #[cfg(windows)]
    pub uvc_match: Mutex<Option<(f32, f32)>>,
    pub browser_input: Mutex<std::collections::VecDeque<crate::web_input::Command>>,
    pub decoder: Mutex<String>,
    pub audio: Mutex<crate::audio::Buffer>,
    /// Smoothed mapping between the system clock and the file's media position.
    pub audio_position: Mutex<Option<crate::audio::AudioClock>>,
    pub audio_epoch: AtomicU64,
    pub frame: Mutex<Option<Frame>>,
    pub seq: AtomicU64,
    pub status: Mutex<String>,
    pub stop: AtomicBool,
    pub paused: AtomicBool,
    pub restart: AtomicBool,
    /// Latest seek request in milliseconds, encoded as position + 1 (0 means none).
    pub seek_ms: AtomicU64,
    pub precise_seek: AtomicBool,
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
    /// Replay recordings fed straight from this source's capture thread. Weak, so a
    /// finished segment's encoder closes when the replay engine releases it.
    pub(crate) replay_tap: Mutex<Vec<std::sync::Weak<Recorder>>>,
}

impl Shared {
    /// Sends a captured picture to the replay recordings tapping this source, before the
    /// picture reaches the interface. Costs one uncontended lock when nothing taps it.
    pub(crate) fn tap_frame(&self, frame: &Frame) {
        let recorders: Vec<Arc<Recorder>> = {
            let Ok(mut taps) = self.replay_tap.lock() else { return };
            if taps.is_empty() { return; }
            taps.retain(|tap| tap.strong_count() > 0);
            taps.iter().filter_map(std::sync::Weak::upgrade).collect()
        };
        Recorder::push_direct(&recorders, frame, Instant::now());
    }

    /// Adds `recorder` to the recordings fed from this source's capture thread.
    pub(crate) fn add_replay_tap(&self, recorder: &Arc<Recorder>) {
        let mut taps = self.replay_tap.lock().unwrap();
        taps.retain(|tap| tap.strong_count() > 0);
        if !taps.iter().any(|tap| std::ptr::eq(tap.as_ptr(), Arc::as_ptr(recorder))) {
            taps.push(Arc::downgrade(recorder));
        }
    }

    pub(crate) fn remove_replay_tap(&self, recorder: &Arc<Recorder>) {
        self.replay_tap.lock().unwrap()
            .retain(|tap| tap.strong_count() > 0 && !std::ptr::eq(tap.as_ptr(), Arc::as_ptr(recorder)));
    }

    pub(crate) fn tapped(&self) -> bool {
        self.replay_tap.lock().is_ok_and(|taps| taps.iter().any(|tap| tap.strong_count() > 0))
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Limited-range BT.709 NV12 from RGBA in integer arithmetic, matching the Windows
/// recorder's conversion (`media::fill_recording_nv12`) within one level. `w` and `h` are even.
pub(crate) fn rgba_to_nv12(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    let (y_plane, uv_plane) = dst[..w * h * 3 / 2].split_at_mut(w * h);
    for (row, luma) in src.chunks_exact(w * 4).zip(y_plane.chunks_exact_mut(w)).take(h) {
        for (px, y) in row.chunks_exact(4).zip(luma) {
            let sum = 11_966 * i32::from(px[0]) + 40_254 * i32::from(px[1]) + 4_064 * i32::from(px[2]);
            *y = (16 + ((sum + 32_768) >> 16)) as u8;
        }
    }
    for (pair, chroma) in src.chunks_exact(w * 8).zip(uv_plane.chunks_exact_mut(w)).take(h / 2) {
        let (top, bottom) = pair.split_at(w * 4);
        for x in (0..w).step_by(2) {
            let mut rgb = [0i32; 3];
            for line in [top, bottom] {
                for px in line[x * 4..x * 4 + 8].chunks_exact(4) {
                    for (total, &value) in rgb.iter_mut().zip(px) { *total += i32::from(value); }
                }
            }
            let [r, g, b] = rgb;
            let u = (128 << 16) + 7_196 * b - 1_649 * r - 5_547 * g + 32_768;
            let v = (128 << 16) + 7_196 * r - 6_536 * g - 660 * b + 32_768;
            chroma[x] = (u >> 16).clamp(0, 255) as u8;
            chroma[x + 1] = (v >> 16).clamp(0, 255) as u8;
        }
    }
}

/// Largest picture height a camera's Auto mode picks (Settings → Performance).
pub static CAPTURE_LOW_LATENCY: AtomicBool = AtomicBool::new(false);

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
    /// The camera sends fields (1080i) rather than whole pictures.
    pub interlaced: bool,
}

impl CamMode {
    pub fn fps(&self) -> f64 {
        self.fps_n as f64 / self.fps_d.max(1) as f64
    }
    pub fn label(&self) -> String {
        let f = self.fps();
        let fps = if (f - f.round()).abs() < 0.01 { format!("{}", f.round() as i64) } else { format!("{f:.2}") };
        format!("{}x{}{} {} fps {}", self.w, self.h, if self.interlaced { "i" } else { "" }, fps, self.format)
    }
    /// One-line form for saving in presets: "w,h,n,d,format".
    pub fn code(&self) -> String {
        let base = format!("{},{},{},{},{}", self.w, self.h, self.fps_n, self.fps_d, self.format);
        if self.interlaced { format!("{base},i") } else { base }
    }
    pub fn parse(s: &str) -> Option<CamMode> {
        let f: Vec<&str> = s.split(',').collect();
        if f.len() != 5 && !(f.len() == 6 && f[5] == "i") {
            return None;
        }
        Some(CamMode { w: f[0].parse().ok()?, h: f[1].parse().ok()?, fps_n: f[2].parse().ok()?, fps_d: f[3].parse().ok()?, format: f[4].to_string(), interlaced: f.len() == 6 })
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
    /// The third field is an audio input read into the same input: capture cards carry audio.
    Device(String, Option<CamMode>, Option<String>),
    DeckLink(crate::decklink::CaptureConfig),
    Ndi(String),
    Omt(String),
    Srt(crate::srt::Connection),
    File(String),
    #[cfg(windows)]
    AutoFile(String, u8, bool),
    #[cfg(windows)]
    DirectShowFile(String, crate::settings_xml::DirectShow, u8, bool),
    FfmpegFile(String, u8, bool),
    AudioFile(String),
    Presentation(String),
    Web(String, bool),
    AudioDevice(String),
    Desktop,
    /// Folder of pictures: Image Sequence (frames at `fps`).
    Sequence(String, f32),
    /// Folder of pictures: Photos slideshow (`secs` per picture).
    Photos(String, f32),
}

#[derive(Clone)]
pub(crate) struct RecFrame {
    #[cfg(windows)]
    pub(crate) surface: Option<Arc<crate::dx_interop::Surface>>,
    #[cfg(windows)]
    pub(crate) nv12: bool,
    pub(crate) audio: crate::audio::Packet,
    pub(crate) pixels: Arc<Vec<u8>>,
    pub(crate) w: usize,
    pub(crate) h: usize,
    pub(crate) captured_at: Instant,
    pub(crate) timeline_origin: Option<Instant>,
    pub(crate) bgra: bool,
}

/// Writes frames to a file on a bounded, independent encoder queue.
pub(crate) type ReplayIndex = Arc<Mutex<Vec<(Instant, u64)>>>;

pub struct Recorder {
    pub(crate) tx: SyncSender<RecFrame>,
    pub path: String,
    pub error: Arc<Mutex<String>>,
    encoder: Arc<Mutex<String>>,
    dropped: AtomicU64,
    /// Audio of frames dropped at a full queue, carried by the next accepted frame.
    lost_audio: Mutex<Vec<crate::audio::Stereo>>,
    pub(crate) replay_index: Option<ReplayIndex>,
    /// Capture-thread pictures are kept at the recording's frame rate: when the next one is
    /// due, and the frame interval.
    direct_pace: Mutex<(Option<Instant>, Duration)>,
    #[cfg(windows)]
    gpu: Arc<AtomicBool>,
    #[cfg(windows)]
    nv12: (Arc<AtomicBool>, Option<(u32, u32)>),
}

/// Whether a picture captured at `at` is kept for a recording at one picture per `interval`,
/// given when the next one is `due`. A camera faster than the recording loses the surplus
/// pictures; small capture jitter is tolerated, so a camera at the same rate loses none.
pub(crate) fn pace_picture(due: &mut Option<Instant>, interval: Duration, at: Instant) -> bool {
    if let Some(next) = *due {
        if at + interval / 2 < next { return false; }
        // After a gap the schedule restarts from this picture instead of catching up.
        *due = Some(if at > next + interval { at + interval } else { next + interval });
    } else {
        *due = Some(at + interval);
    }
    true
}

#[test]
fn pacing_keeps_every_picture_at_the_same_rate_and_halves_a_double_rate() {
    let start = Instant::now();
    let interval = Duration::from_micros(33_333);
    let mut due = None;
    // 30 fps with up to 5 ms jitter: every picture is kept.
    let kept = (0..60u64).filter(|n| {
        let jitter = Duration::from_millis([0, 5, 2, 4][*n as usize % 4]);
        pace_picture(&mut due, interval, start + Duration::from_micros(33_333 * n) + jitter)
    }).count();
    assert_eq!(kept, 60);
    // 60 fps into 30 fps keeps one picture in two.
    let mut due = None;
    let kept = (0..120u64).filter(|n| pace_picture(&mut due, interval, start + Duration::from_micros(16_667 * n))).count();
    assert!((59..=61).contains(&kept), "{kept}");
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
        Self::spawn_writer_indexed(path, opts, false)
    }

    pub(crate) fn spawn_writer_indexed(path: String, opts: RecOpts, indexed: bool) -> (Recorder, std::thread::JoinHandle<()>)
    {
        // Keep the UI responsive and bound memory if the encoder falls behind. The original
        // keeps 5 encoder input frames (vMixCodecLibraryNet MFCodec::Initialize 0x060001ad
        // @0x5cd8, MFFramePool(len, 5, …)), so a short disk or encoder stall doesn't drop frames.
        // A replay camera recorded from its capture thread also queues the audio-only frames
        // the update loop sends, so it gets room for both.
        let (tx, rx) = sync_channel(if indexed { RECORD_QUEUE_FRAMES * 2 } else { RECORD_QUEUE_FRAMES });
        let error = Arc::new(Mutex::new(String::new()));
        let encoder = Arc::new(Mutex::new("Waiting for first frame".into()));
        let (p2, e2) = (path.clone(), error.clone());
        let encoder_status = Arc::clone(&encoder);
        #[cfg(windows)]
        // Replay fans out to multiple encoders. Keep their device lifetime separate
        // from the compositor, including during overlapping segment finalizations.
        let gpu = Arc::new(AtomicBool::new(!indexed && opts.hw && !crate::replay::fragmented_mp4(&path) && matches!(opts.format, RecordFormat::H264 | RecordFormat::Hevc)));
        #[cfg(windows)]
        let gpu_worker = gpu.clone();
        #[cfg(windows)]
        let nv12 = Arc::new(AtomicBool::new(indexed && opts.format == RecordFormat::H264
            && !crate::replay::fragmented_mp4(&path)));
        #[cfg(windows)]
        let nv12_worker = nv12.clone();
        let replay_index: Option<ReplayIndex> = indexed.then(Default::default);
        let index = replay_index.clone();
        let worker = std::thread::spawn(move || crate::media::record_thread(p2, rx, e2, encoder_status, opts, index,
            #[cfg(windows)]
            gpu_worker, #[cfg(windows)] nv12_worker,
        ));
        let interval = Duration::from_secs_f64(1.0 / f64::from(if opts.fps.is_finite() { opts.fps.max(1.0) } else { 30.0 }));
        (Recorder { tx, path, error, encoder, dropped: AtomicU64::new(0), lost_audio: Mutex::default(), replay_index,
            direct_pace: Mutex::new((None, interval)),
            #[cfg(windows)]
            gpu,
            #[cfg(windows)]
            nv12: (nv12, opts.size),
        }, worker)
    }
    pub fn push_audio(&self, rgba: Vec<u8>, w: usize, h: usize, captured_at: Instant, audio: crate::audio::Packet) {
        self.enqueue(RecFrame { pixels: Arc::new(rgba), w, h, captured_at, timeline_origin: None, bgra: false, audio,
            #[cfg(windows)]
            surface: None,
            #[cfg(windows)]
            nv12: false,
        });
    }

    pub fn push_group_audio(recorders: &[Arc<Self>], pixels: impl Into<Arc<Vec<u8>>>, w: usize, h: usize, captured_at: Instant, bgra: bool, audio: crate::audio::Packet) {
        let frame = RecFrame { pixels: pixels.into(), w, h, captured_at, timeline_origin: None, bgra, audio,
            #[cfg(windows)]
            surface: None,
            #[cfg(windows)]
            nv12: false,
        };
        for recorder in recorders {
            recorder.enqueue(frame.clone());
        }
    }

    /// A picture from a source's capture thread (`Shared::tap_frame`). Its audio arrives
    /// separately through `push_audio_only`, at the recording's frame cadence.
    pub(crate) fn push_direct(recorders: &[Arc<Self>], frame: &Frame, captured_at: Instant) {
        let (w, h) = (frame.w, frame.h);
        if w == 0 || h == 0 || frame.rgba.len() < w * h * 4 { return; }
        let paced: Vec<Arc<Self>> = recorders.iter().filter(|recorder| {
            let mut pace = recorder.direct_pace.lock().unwrap();
            let interval = pace.1;
            pace_picture(&mut pace.0, interval, captured_at)
        }).cloned().collect();
        let recorders = paced.as_slice();
        if recorders.is_empty() { return; }
        #[cfg(windows)]
        if w % 2 == 0 && h % 2 == 0 && recorders.iter().all(|recorder| recorder.accepts_nv12([w as u32, h as u32])) {
            // Same size as the recording: the encoder takes NV12 without converting again.
            let mut nv12 = vec![0; w * h * 3 / 2];
            rgba_to_nv12(&frame.rgba, w, h, &mut nv12);
            Self::push_nv12_group(recorders, Arc::new(nv12), [w as u32, h as u32], captured_at, Default::default());
            return;
        }
        Self::push_group_audio(recorders, frame.rgba[..w * h * 4].to_vec(), w, h, captured_at, false, Default::default());
    }

    /// Audio for a recording whose pictures come from a capture thread. It carries no picture
    /// and is written with the following pictures.
    pub(crate) fn push_audio_only(&self, captured_at: Instant, audio: crate::audio::Packet) {
        self.enqueue(RecFrame { pixels: Arc::default(), w: 0, h: 0, captured_at, timeline_origin: None, bgra: false, audio,
            #[cfg(windows)]
            surface: None,
            #[cfg(windows)]
            nv12: false,
        });
    }

    #[cfg(windows)]
    pub(crate) fn accepts_surface(&self) -> bool {
        self.gpu.load(Ordering::Acquire)
    }

    #[cfg(windows)]
    pub(crate) fn accepts_nv12(&self, [w, h]: [u32; 2]) -> bool {
        self.nv12.0.load(Ordering::Acquire) && self.nv12.1.is_none_or(|size| size == (w, h))
    }

    #[cfg(windows)]
    pub(crate) fn push_nv12_group(recorders: &[Arc<Self>], pixels: Arc<Vec<u8>>, [w, h]: [u32; 2],
        captured_at: Instant, audio: crate::audio::Packet)
    {
        let frame = RecFrame { surface: None, nv12: true, pixels, w: w as usize, h: h as usize,
            captured_at, timeline_origin: None, bgra: false, audio };
        for recorder in recorders { recorder.enqueue(frame.clone()); }
    }

    #[cfg(windows)]
    pub(crate) fn push_surface_group(recorders: &[Arc<Self>], surface: crate::dx_interop::Surface,
        [w, h]: [u32; 2], captured_at: Instant, audio: crate::audio::Packet)
    {
        let frame = RecFrame { surface: Some(Arc::new(surface)), nv12: false, pixels: Arc::default(),
            w: w as usize, h: h as usize, captured_at, timeline_origin: None, bgra: true, audio };
        for recorder in recorders { recorder.enqueue(frame.clone()); }
    }

    fn enqueue(&self, mut frame: RecFrame) {
        let mut lost = self.lost_audio.lock().unwrap();
        if !lost.is_empty() {
            lost.extend(frame.audio.iter().copied());
            frame.audio = std::mem::take(&mut *lost).into();
        }
        if let Err(std::sync::mpsc::TrySendError::Full(frame)) = self.tx.try_send(frame) {
            self.note_dropped_frames(1);
            // Keep the samples so the recording's audio stays continuous, up to one second.
            if lost.len() + frame.audio.len() <= crate::audio::RATE as usize {
                lost.extend(frame.audio.iter().copied());
            }
        }
    }

    /// Frames that never reached the encoder (full queue or failed readback).
    pub fn dropped_frames(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn note_dropped_frames(&self, count: u64) {
        self.dropped.fetch_add(count, Ordering::Relaxed);
    }

    pub fn diagnostics(&self) -> String {
        let encoder = self.encoder.lock().unwrap();
        format!("{encoder} | Dropped frames: {}", self.dropped.load(Ordering::Relaxed))
    }
}

/// Frames each recording may queue ahead of its encoder (original MFFramePool size 5).
pub(crate) const RECORD_QUEUE_FRAMES: usize = 5;

/// Recording "AudioBitRate" default, Kbps (`kh::get_k` @0xe42bc). The Windows AAC encoder gets
/// at most 192 of it (`ka::.ctor` @0xe29e4).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const RECORD_AUDIO_KBPS: u32 = 384;

/// Writes a recorded picture into an encoder buffer as opaque BGRA `w`×`h` in one pass.
/// With `scale` the picture is resized to fit (bilinear, 8-bit fixed point); otherwise it sits
/// at the top left and the rest is black. Replaces a separate resize (new buffer, float filter)
/// plus a byte-by-byte colour swap; large frames are split across up to 4 threads.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn fill_bgra(dst: &mut [u8], w: usize, h: usize, src: &[u8], sw: usize, sh: usize, src_bgra: bool, scale: bool) {
    let row = w * 4;
    let dst = &mut dst[..row * h];
    if sw == 0 || sh == 0 || src.len() < sw * sh * 4 {
        dst.chunks_exact_mut(4).for_each(|d| d.copy_from_slice(&[0, 0, 0, 255]));
        return;
    }
    if src_bgra && (sw, sh) == (w, h) {
        dst.copy_from_slice(&src[..row * h]);
        return;
    }
    let (r, b) = if src_bgra { (2, 0) } else { (0, 2) };
    let resize = scale && (sw, sh) != (w, h);
    // Source position of each output column: (offset of left pixel, offset of right, weight 0..256).
    let map = |d: usize, n: usize, s: usize| -> (usize, usize, u32) {
        let f = ((d as f64 + 0.5) * s as f64 / n as f64 - 0.5).clamp(0.0, (s - 1) as f64);
        let i = f as usize;
        (i, (i + 1).min(s - 1), ((f - i as f64) * 256.0) as u32)
    };
    let columns: Vec<(usize, usize, u32)> = if resize {
        (0..w).map(|x| { let (a, c, f) = map(x, w, sw); (a * 4, c * 4, f) }).collect()
    } else {
        Vec::new()
    };
    let rows = |first: usize, out: &mut [u8]| {
        for (y, line) in (first..).zip(out.chunks_exact_mut(row)) {
            if resize {
                let (y0, y1, fy) = map(y, h, sh);
                let (top, bottom) = (&src[y0 * sw * 4..][..sw * 4], &src[y1 * sw * 4..][..sw * 4]);
                for (d, &(a, c, fx)) in line.chunks_exact_mut(4).zip(&columns) {
                    let mix = |k: usize| -> u8 {
                        let t = u32::from(top[a + k]) * (256 - fx) + u32::from(top[c + k]) * fx;
                        let u = u32::from(bottom[a + k]) * (256 - fx) + u32::from(bottom[c + k]) * fx;
                        ((t * (256 - fy) + u * fy + 32768) >> 16) as u8
                    };
                    d.copy_from_slice(&[mix(b), mix(1), mix(r), 255]);
                }
            } else if y < sh {
                let copied = w.min(sw);
                let (head, tail) = line.split_at_mut(copied * 4);
                for (d, s) in head.chunks_exact_mut(4).zip(src[y * sw * 4..][..copied * 4].chunks_exact(4)) {
                    d.copy_from_slice(&[s[b], s[1], s[r], 255]);
                }
                tail.chunks_exact_mut(4).for_each(|d| d.copy_from_slice(&[0, 0, 0, 255]));
            } else {
                line.chunks_exact_mut(4).for_each(|d| d.copy_from_slice(&[0, 0, 0, 255]));
            }
        }
    };
    let bands = if w * h >= 1280 * 720 {
        std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4)
    } else {
        1
    };
    if bands == 1 {
        rows(0, dst);
        return;
    }
    let per_band = h.div_ceil(bands);
    std::thread::scope(|scope| {
        for (i, band) in dst.chunks_mut(per_band * row).enumerate() {
            let rows = &rows;
            scope.spawn(move || rows(i * per_band, band));
        }
    });
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
    Wmv9,
    Mjpeg,
    MagicYuv,
    Mpeg2Ts,
    Mpeg2Ps,
    H264Mkv,
    HevcMkv,
    ProRes,
    DnxHr,
    DnxHrMxf,
}

pub(crate) struct FfmpegProfile {
    pub video: &'static str,
    pub audio: &'static str,
    pub container: &'static str,
    pub pixel: &'static str,
    pub bitrate: bool,
    pub extra: &'static [&'static str],
}

impl RecordFormat {
    pub const ALL: [Self; 13] = [Self::H264, Self::Hevc, Self::Wmv, Self::Wmv9, Self::Mjpeg, Self::MagicYuv,
        Self::Mpeg2Ts, Self::Mpeg2Ps, Self::H264Mkv, Self::HevcMkv, Self::ProRes, Self::DnxHr, Self::DnxHrMxf];

    pub fn parse(value: &str) -> Self {
        let format = Self::ALL.into_iter().find(|format| format.key() == value).unwrap_or_default();
        if cfg!(windows) || format != Self::Wmv9 { format } else { Self::Wmv }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::H264 => "h264", Self::Hevc => "hevc", Self::Wmv => "wmv8", Self::Wmv9 => "wmv",
            Self::Mjpeg => "mjpeg", Self::MagicYuv => "magicyuv",
            Self::Mpeg2Ts => "mpeg2ts", Self::Mpeg2Ps => "mpeg2ps",
            Self::H264Mkv => "h264mkv", Self::HevcMkv => "hevcmkv",
            Self::ProRes => "prores", Self::DnxHr => "dnxhr", Self::DnxHrMxf => "dnxhr-mxf",
        }
    }

    pub fn codec(self) -> &'static str {
        match self {
            Self::H264 | Self::H264Mkv => "H.264",
            Self::Hevc | Self::HevcMkv => "HEVC",
            Self::Wmv => "WMV8",
            Self::Wmv9 => "WMV9",
            Self::Mjpeg => "MJPEG",
            Self::MagicYuv => "MagicYUV",
            Self::Mpeg2Ts | Self::Mpeg2Ps => "MPEG-2",
            Self::ProRes => "ProRes 422",
            Self::DnxHr | Self::DnxHrMxf => "DNxHR HQ",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::H264 | Self::Hevc => "mp4",
            Self::Wmv | Self::Wmv9 => "wmv",
            Self::Mjpeg | Self::MagicYuv => "avi",
            Self::Mpeg2Ts => "ts",
            Self::Mpeg2Ps => "mpg",
            Self::H264Mkv | Self::HevcMkv => "mkv",
            Self::ProRes | Self::DnxHr => "mov",
            Self::DnxHrMxf => "mxf",
        }
    }

    pub fn native_windows(self) -> bool {
        matches!(self, Self::H264 | Self::Hevc | Self::Wmv9)
    }

    pub fn audio_codec(self) -> &'static str {
        match self {
            Self::Wmv | Self::Wmv9 => "WMA2",
            Self::Mpeg2Ts | Self::Mpeg2Ps => "MP2",
            Self::Mjpeg | Self::MagicYuv | Self::ProRes | Self::DnxHr | Self::DnxHrMxf => "PCM",
            _ => "AAC",
        }
    }

    pub(crate) fn ffmpeg_profile(self) -> FfmpegProfile {
        let profile = |video, audio, container, pixel, bitrate, extra| FfmpegProfile {
            video, audio, container, pixel, bitrate, extra,
        };
        let h264 = "libx264";
        let hevc = "libx265";
        match self {
            Self::H264 => profile(h264, "aac", "mp4", "yuv420p", true, &["-movflags", "+faststart"]),
            Self::Hevc => profile(hevc, "aac", "mp4", "yuv420p", true, &["-tag:v", "hvc1", "-movflags", "+faststart"]),
            Self::H264Mkv => profile(h264, "aac", "matroska", "yuv420p", true, &[]),
            Self::HevcMkv => profile(hevc, "aac", "matroska", "yuv420p", true, &[]),
            Self::Wmv | Self::Wmv9 => profile("wmv2", "wmav2", "asf", "yuv420p", true, &[]),
            Self::Mjpeg => profile("mjpeg", "pcm_s16le", "avi", "yuvj422p", false, &["-q:v", "3"]),
            Self::MagicYuv => profile("magicyuv", "pcm_s16le", "avi", "gbrp", false, &[]),
            Self::Mpeg2Ts => profile("mpeg2video", "mp2", "mpegts", "yuv420p", true, &[]),
            Self::Mpeg2Ps => profile("mpeg2video", "mp2", "mpeg", "yuv420p", true, &[]),
            Self::ProRes => profile("prores_ks", "pcm_s16le", "mov", "yuv422p10le", false, &["-profile:v", "2"]),
            Self::DnxHr => profile("dnxhd", "pcm_s16le", "mov", "yuv422p", false, &["-profile:v", "dnxhr_hq"]),
            Self::DnxHrMxf => profile("dnxhd", "pcm_s16le", "mxf", "yuv422p", false, &["-profile:v", "dnxhr_hq"]),
        }
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
    pub audio: bool,
}

impl Default for RecOpts {
    fn default() -> Self {
        RecOpts { mbps: DEFAULT_MBPS, size: None, fps: RECORDING_FPS as f32, hw: true, format: RecordFormat::H264, audio: true }
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
    Audio,
    Presentation,
    Lut(u32),
    Image,
    Title,
    PresetOpen,
    SettingsOpen,
    SettingsSave,
    PresetSave,
    /// Several video files (List input); answer is the paths joined by '\n'.
    VideoMany,
    /// A folder (Image Sequence / Photos).
    Folder,
    /// A text file for a Teleprompter script: the input it replaces, or None for the Add Input dialog.
    Script(Option<u32>),
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
        if let Some(ms) = sh.seek_ms.swap(0, Ordering::Relaxed).checked_sub(1) {
            i = ((ms / (step.as_millis() as u64).max(1)) as usize).min(files.len() - 1);
            shown = false;
            sh.ended.store(false, Ordering::Relaxed);
        }
        let paused = sh.paused.load(Ordering::Relaxed) || sh.ended.load(Ordering::Relaxed);
        if paused && shown {
            std::thread::sleep(Duration::from_millis(15));
            continue;
        }
        let t0 = Instant::now();
        if let Ok(img) = open_image(&files[i]) {
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
            while t0.elapsed() < step && !sh.stop.load(Ordering::Relaxed) && !sh.restart.load(Ordering::Relaxed)
                && sh.seek_ms.load(Ordering::Relaxed) == 0 {
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

/// Opens a picture detecting its format from the file contents, so a JPEG saved
/// with a `.png` extension (or vice versa) still loads.
pub fn open_image(path: impl AsRef<std::path::Path>) -> image::ImageResult<image::DynamicImage> {
    image::ImageReader::open(path)?.with_guessed_format()?.decode()
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

#[cfg(test)]
mod fill_tests {
    use super::fill_bgra;

    #[test]
    fn copies_rgba_as_opaque_bgra_and_pads_with_black() {
        let src = [10, 20, 30, 0, 40, 50, 60, 7];
        let mut dst = vec![9u8; 3 * 2 * 4];
        fill_bgra(&mut dst, 3, 2, &src, 2, 1, false, false);
        assert_eq!(&dst[..8], &[30, 20, 10, 255, 60, 50, 40, 255]);
        assert!(dst[8..].chunks(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn scales_to_the_output_size() {
        // A 4×2 frame, left half black and right half white, halved to 2×1.
        let mut src = Vec::new();
        for _ in 0..2 {
            for x in 0..4 { let v = if x < 2 { 0 } else { 255 }; src.extend_from_slice(&[v, v, v, 255]); }
        }
        let mut dst = vec![0u8; 2 * 4];
        fill_bgra(&mut dst, 2, 1, &src, 4, 2, true, true);
        assert_eq!(dst, [0, 0, 0, 255, 255, 255, 255, 255]);
        // Upscaling a flat colour keeps it exactly, also across worker threads.
        let flat = [1u8, 2, 3, 255].repeat(640 * 360);
        let mut big = vec![0u8; 1280 * 720 * 4];
        fill_bgra(&mut big, 1280, 720, &flat, 640, 360, false, true);
        assert!(big.chunks(4).all(|p| p == [3, 2, 1, 255]));
    }

    #[test]
    fn a_short_source_records_black_instead_of_panicking() {
        let mut dst = vec![5u8; 2 * 2 * 4];
        fill_bgra(&mut dst, 2, 2, &[1, 2, 3], 2, 2, false, true);
        assert!(dst.chunks(4).all(|p| p == [0, 0, 0, 255]));
    }
}
