use crate::media::{Frame, Shared};
use crate::program::NetworkFrame;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows::core::{HSTRING, PCSTR};
#[cfg(windows)]
use windows::Win32::Foundation::FreeLibrary;
#[cfg(windows)]
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

type Handle = *mut c_void;

#[repr(C)]
struct Source {
    name: *const c_char,
    address: *const c_char,
}

#[repr(C)]
struct FindSettings {
    show_local: bool,
    groups: *const c_char,
    extra_ips: *const c_char,
}

#[repr(C)]
struct ReceiveSettings {
    source: Source,
    color_format: i32,
    bandwidth: i32,
    allow_fields: bool,
    receiver_name: *const c_char,
}

#[repr(C)]
struct SendSettings {
    name: *const c_char,
    groups: *const c_char,
    clock_video: bool,
    clock_audio: bool,
}

#[repr(C)]
struct Video {
    width: i32,
    height: i32,
    fourcc: u32,
    rate_n: i32,
    rate_d: i32,
    aspect: f32,
    format: i32,
    timecode: i64,
    pixels: *const u8,
    stride: i32,
    metadata: *const c_char,
    timestamp: i64,
}

#[repr(C)]
struct Audio {
    rate: i32,
    channels: i32,
    samples: i32,
    timecode: i64,
    fourcc: u32,
    data: *const u8,
    stride: i32,
    metadata: *const c_char,
    timestamp: i64,
}

const FLTP: u32 = u32::from_le_bytes(*b"FLTp");
const NDI_PCM_SCALE: f32 = 3276.8; // SMPTE reference, 20 dB of headroom above NDI unity.

struct Api {
    find_create: unsafe extern "C" fn(*const FindSettings) -> Handle,
    find_wait: unsafe extern "C" fn(Handle, u32) -> bool,
    find_sources: unsafe extern "C" fn(Handle, *mut u32) -> *const Source,
    find_destroy: unsafe extern "C" fn(Handle),
    recv_create: unsafe extern "C" fn(*const ReceiveSettings) -> Handle,
    recv_capture: unsafe extern "C" fn(Handle, *mut Video, *mut Audio, Handle, u32) -> i32,
    recv_free: unsafe extern "C" fn(Handle, *const Video),
    recv_free_audio: unsafe extern "C" fn(Handle, *const Audio),
    recv_destroy: unsafe extern "C" fn(Handle),
    send_create: unsafe extern "C" fn(*const SendSettings) -> Handle,
    send_video: unsafe extern "C" fn(Handle, *const Video),
    send_audio: unsafe extern "C" fn(Handle, *const Audio),
    connections: unsafe extern "C" fn(Handle, u32) -> i32,
    send_destroy: unsafe extern "C" fn(Handle),
}

impl Api {
    unsafe fn load(symbol: impl Fn(&[u8]) -> Option<*const ()>) -> Option<Self> {
        macro_rules! symbol {
            ($name:literal) => {{
                let name = concat!("NDIlib_", $name, "\0");
                std::mem::transmute(symbol(name.as_bytes())?)
            }};
        }
        let api = Self {
            find_create: symbol!("find_create_v2"),
            find_wait: symbol!("find_wait_for_sources"),
            find_sources: symbol!("find_get_current_sources"),
            find_destroy: symbol!("find_destroy"),
            recv_create: symbol!("recv_create_v3"),
            recv_capture: symbol!("recv_capture_v3"),
            recv_free: symbol!("recv_free_video_v2"),
            recv_free_audio: symbol!("recv_free_audio_v3"),
            recv_destroy: symbol!("recv_destroy"),
            send_create: symbol!("send_create"),
            send_video: symbol!("send_send_video_async_v2"),
            send_audio: symbol!("send_send_audio_v3"),
            connections: symbol!("send_get_no_connections"),
            send_destroy: symbol!("send_destroy"),
        };
        let initialize: unsafe extern "C" fn() -> bool = symbol!("initialize");
        initialize().then_some(api)
    }
}

#[cfg(windows)]
fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let dll = "Processing.NDI.Lib.x64.dll";
        let mut candidates: Vec<_> = ["NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"].into_iter()
            .filter_map(std::env::var_os).map(|p| PathBuf::from(p).join(dll)).collect();
        if let Some(programs) = std::env::var_os("ProgramFiles").map(PathBuf::from) {
            for dir in ["NDI/NDI 6 Runtime", "NDI/NDI 6 Tools/Runtime", "NDI/NDI 5 Runtime", "NDI/NDI 5 Tools/Runtime"] {
                candidates.push(programs.join(dir).join(dll));
            }
        }
        for path in candidates.into_iter().filter(|p| p.is_absolute() && p.is_file()) {
            unsafe {
                if let Ok(module) = LoadLibraryW(&HSTRING::from(path.as_os_str())) {
                    if let Some(api) = Api::load(|name| GetProcAddress(module, PCSTR(name.as_ptr())).map(|p| p as *const ())) {
                        // Keep the runtime loaded for the lifetime of the process.
                        return Some(api);
                    }
                    let _ = FreeLibrary(module);
                }
            }
        }
        None
    }).as_ref().ok_or_else(|| "NDI runtime unavailable. Install the 64-bit NDI Runtime or NDI Tools and restart LiveMix.".into())
}

#[cfg(target_os = "macos")]
fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let mut paths: Vec<_> = ["LIVEMIX_NDI_DIR", "NDI_RUNTIME_DIR_V6", "NDI_RUNTIME_DIR_V5"].into_iter()
            .filter_map(std::env::var_os).map(PathBuf::from).collect();
        paths.extend(["/usr/local/lib", "/opt/homebrew/lib", "/Library/NDI SDK for Apple/lib/macOS"].into_iter().map(PathBuf::from));
        for dir in paths.into_iter().filter(|path| path.is_absolute()) {
            unsafe {
                if let Ok(library) = libloading::Library::new(dir.join("libndi.dylib")) {
                    if let Some(api) = Api::load(|name| library.get::<unsafe extern "C" fn()>(name).ok().map(|p| *p as *const ())) {
                        std::mem::forget(library);
                        return Some(api);
                    }
                }
            }
        }
        None
    }).as_ref().ok_or_else(|| "NDI runtime unavailable. Install the Mac runtime or set LIVEMIX_NDI_DIR.".into())
}

pub fn discover() -> Result<Vec<String>, String> {
    let api = api()?;
    unsafe {
        let find = (api.find_create)(&FindSettings { show_local: true, groups: null(), extra_ips: null() });
        if find.is_null() { return Err("Could not start NDI discovery".into()); }
        let deadline = Instant::now() + Duration::from_millis(1400);
        while Instant::now() < deadline {
            (api.find_wait)(find, 150);
        }
        let mut len = 0;
        let sources = (api.find_sources)(find, &mut len);
        let mut names = Vec::new();
        if !sources.is_null() {
            for source in std::slice::from_raw_parts(sources, len as usize) {
                if !source.name.is_null() {
                    names.push(CStr::from_ptr(source.name).to_string_lossy().into_owned());
                }
            }
        }
        (api.find_destroy)(find);
        names.sort_unstable();
        names.dedup();
        Ok(names)
    }
}

const BGRA: u32 = u32::from_le_bytes(*b"BGRA");
const BGRX: u32 = u32::from_le_bytes(*b"BGRX");

pub fn run_capture(shared: Arc<Shared>, name: &str) {
    let result = receive(&shared, name, false);
    if let Err(error) = result {
        *shared.status.lock().unwrap() = error;
    }
}

pub fn thumbnail(shared: Arc<Shared>, name: &str) -> Result<(), String> {
    receive(&shared, name, true)
}

fn receive(shared: &Shared, name: &str, thumbnail: bool) -> Result<(), String> {
    let api = api()?;
    let name = CString::new(name).map_err(|_| "Invalid NDI source name")?;
    unsafe {
        let recv = (api.recv_create)(&ReceiveSettings {
            source: Source { name: name.as_ptr(), address: null() },
            color_format: 0, bandwidth: if thumbnail { 0 } else { 100 }, allow_fields: false, receiver_name: null(),
        });
        if recv.is_null() { return Err("Could not open NDI source".into()); }
        let mut last_frame = Instant::now();
        let mut receiving = false;
        let mut audio_phase = 0;
        let mut clock = crate::audio::NetworkClock::default();
        *shared.status.lock().unwrap() = "Waiting for NDI video…".into();
        while !shared.stop.load(Relaxed) {
            let mut video: Video = std::mem::zeroed();
            let mut audio: Audio = std::mem::zeroed();
            let kind = (api.recv_capture)(recv, &mut video, &mut audio, null_mut(), 5);
            let now = Instant::now();
            if kind == 1 {
                if let Some(frame) = unpack(&video) {
                    let stamp = if thumbnail { now } else { clock.stamp(video.timestamp, now).unwrap_or(now) };
                    clock.queue_video(frame, stamp);
                    clock.present(shared, now);
                    last_frame = now;
                    if !receiving {
                        *shared.status.lock().unwrap() = "Receiving NDI video".into();
                        receiving = true;
                    }
                }
                (api.recv_free)(recv, &video);
                if thumbnail && receiving { break; }
            } else if kind == 2 {
                if !thumbnail && audio.fourcc == FLTP {
                    crate::audio::capture_planar(shared, audio.data, audio.samples as usize, audio.stride as usize,
                        audio.channels as usize, audio.rate as usize, NDI_PCM_SCALE, &mut audio_phase,
                        clock.stamp(audio.timestamp, now));
                }
                (api.recv_free_audio)(recv, &audio);
            }
            clock.present(shared, now);
            if receiving && last_frame.elapsed() >= Duration::from_secs(3) {
                *shared.status.lock().unwrap() = "Waiting for NDI signal…".into();
                receiving = false;
            }
        }
        (api.recv_destroy)(recv);
    }
    Ok(())
}

unsafe fn unpack(video: &Video) -> Option<Frame> {
    let w = usize::try_from(video.width).ok()?;
    let h = usize::try_from(video.height).ok()?;
    let stride = usize::try_from(video.stride).ok()?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || stride < w * 4
        || stride > 65_536 || video.pixels.is_null() || !matches!(video.fourcc, BGRA | BGRX)
    {
        return None;
    }
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let row = std::slice::from_raw_parts(video.pixels.add(y * stride), w * 4);
        rgba.extend_from_slice(row);
    }
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        if video.fourcc == BGRX { pixel[3] = 255; }
    }
    Some(Frame { w, h, rgba })
}

#[derive(Clone)]
pub struct Sender {
    frames: mpsc::SyncSender<(NetworkFrame, bool, crate::audio::Packet)>,
    stop: Arc<AtomicBool>,
    subscribed: Arc<AtomicBool>,
    status: Arc<Mutex<(String, bool)>>,
}

impl Sender {
    pub fn start(name: String, fps: f64) -> Self {
        let (frames, receive) = mpsc::sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let subscribed = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(("Starting NDI…".into(), false)));
        let (flag, viewers, report) = (Arc::clone(&stop), Arc::clone(&subscribed), Arc::clone(&status));
        std::thread::spawn(move || {
            if let Err(error) = send(&name, fps, receive, &flag, &viewers, &report) {
                *report.lock().unwrap() = (error, true);
            }
        });
        Self { frames, stop, subscribed, status }
    }

    pub fn submit_pixels(&self, frame: NetworkFrame, uyvy: bool, audio: crate::audio::Packet) {
        let _ = self.frames.try_send((frame, uyvy, audio));
    }

    pub fn stop(&self) {
        self.stop.store(true, Relaxed);
    }

    pub fn subscribed(&self) -> bool {
        self.subscribed.load(Relaxed)
    }

    pub fn status(&self) -> (String, bool) {
        self.status.lock().unwrap().clone()
    }
}

fn send(name: &str, fps: f64, frames: mpsc::Receiver<(NetworkFrame, bool, crate::audio::Packet)>, stop: &AtomicBool, subscribed: &AtomicBool,
    status: &Mutex<(String, bool)>) -> Result<(), String>
{
    let api = api()?;
    let name = CString::new(name).map_err(|_| "Invalid NDI output name")?;
    let (rate_n, rate_d) = crate::media::rate_fraction(fps as f32);
    unsafe {
        let sender = (api.send_create)(&SendSettings {
            name: name.as_ptr(), groups: null(), clock_video: false, clock_audio: false,
        });
        if sender.is_null() { return Err("Could not create NDI sender".into()); }
        *status.lock().unwrap() = ("Waiting for NDI receivers…".into(), false);
        let mut last_check = Instant::now() - Duration::from_millis(250);
        let mut outstanding = None;
        while !stop.load(Relaxed) {
            if last_check.elapsed() >= Duration::from_millis(250) {
                last_check = Instant::now();
                let active = (api.connections)(sender, 0) > 0;
                if active != subscribed.swap(active, Relaxed) {
                    *status.lock().unwrap() = (if active { "Sending NDI video" } else { "Waiting for NDI receivers…" }.into(), false);
                }
                if !active && outstanding.is_some() {
                    (api.send_video)(sender, null());
                    drop(outstanding.take());
                }
            }
            let (frame, uyvy, packet) = match frames.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if !subscribed.load(Relaxed) { continue; }
            let video = Video {
                width: frame.width as i32, height: frame.height as i32,
                fourcc: if uyvy { u32::from_le_bytes(*b"UYVY") } else { BGRX },
                rate_n: rate_n as i32, rate_d: rate_d as i32,
                aspect: frame.width as f32 / frame.height as f32, format: 1,
                timecode: i64::MAX, pixels: frame.pixels.as_ptr(), stride: frame.width as i32 * if uyvy { 2 } else { 4 },
                metadata: null(), timestamp: 0,
            };
            (api.send_video)(sender, &video);
            if !packet.is_empty() {
                // The mixer windows are contiguous, so each packet is sent whole.
                let count = packet.len();
                let planar = crate::audio::planar(&packet[..count], 1.0 / NDI_PCM_SCALE);
                (api.send_audio)(sender, &Audio {
                    rate: crate::audio::RATE as i32, channels: 2, samples: count as i32,
                    timecode: i64::MAX, fourcc: FLTP, data: planar.as_ptr().cast(),
                    stride: (count * 4) as i32, metadata: null(), timestamp: 0,
                });
            }
            // NDI may read the previous frame until this submission returns.
            drop(outstanding.replace(frame));
        }
        subscribed.store(false, Relaxed);
        (api.send_video)(sender, null());
        drop(outstanding);
        (api.send_destroy)(sender);
    }
    Ok(())
}

#[test]
#[ignore = "requires the native NDI runtime and local discovery"]
fn ndi_video_loopback() {
    let name = format!("LiveMix test {}", std::process::id());
    let sender = Sender::start(name.clone(), 25.0);
    let source = discover().unwrap().into_iter().find(|source| source.contains(&name))
        .unwrap_or_else(|| panic!("sender was not discovered: {:?}", sender.status()));
    let input = Arc::new(Shared::default());
    let receiver = Arc::clone(&input);
    let worker = std::thread::spawn(move || run_capture(receiver, &source));
    let frame = NetworkFrame { pixels: Arc::new([20, 55, 180, 255].repeat(320 * 180)), width: 320, height: 180 };
    let deadline = Instant::now() + Duration::from_secs(8);
    let packet: crate::audio::Packet = vec![[4800, -4800]; 1920].into();
    let mut level = 0.0;
    while (input.seq.load(Relaxed) == 0 || level < 0.1) && Instant::now() < deadline {
        sender.submit_pixels(frame.clone(), false, packet.clone());
        std::thread::sleep(Duration::from_millis(40));
        level = crate::audio::input_peak((&input, false, 1.0), Instant::now());
    }
    input.stop.store(true, Relaxed);
    sender.stop();
    worker.join().unwrap();
    let frame = input.frame.lock().unwrap();
    let frame = frame.as_ref().unwrap_or_else(|| panic!("{}", input.status.lock().unwrap()));
    assert_eq!((frame.w, frame.h), (320, 180));
    assert!(level >= 0.1, "NDI stereo audio was not received");
    assert!(frame.rgba[0] > 160 && frame.rgba[2] < 40, "color {:?}", &frame.rgba[..4]);
}
