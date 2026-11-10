use crate::media::{Frame, Shared};
use crate::program::NetworkFrame;
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
#[cfg(windows)]
use windows::core::{HSTRING, PCSTR};
#[cfg(windows)]
use windows::Win32::Foundation::FreeLibrary;
#[cfg(windows)]
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW,
    LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR};

type Handle = *mut c_void;
const VIDEO: i32 = 2;
const BGRA: u32 = u32::from_le_bytes(*b"BGRA");
const BGRX: u32 = u32::from_le_bytes(*b"BGRX");

// libomt.h OMTMediaFrame. SDK handles own all receive storage.
#[repr(C)]
struct MediaFrame {
    frame_type: i32,
    timestamp: i64,
    codec: u32,
    width: i32,
    height: i32,
    stride: i32,
    flags: i32,
    rate_n: i32,
    rate_d: i32,
    aspect: f32,
    color_space: i32,
    sample_rate: i32,
    channels: i32,
    samples: i32,
    data: *const u8,
    data_length: i32,
    compressed_data: *const u8,
    compressed_length: i32,
    metadata: *const u8,
    metadata_length: i32,
}

struct Api {
    addresses: unsafe extern "C" fn(*mut i32) -> *const *const c_char,
    recv_create: unsafe extern "C" fn(*const c_char, i32, i32, i32) -> Handle,
    recv: unsafe extern "C" fn(Handle, i32, i32) -> *const MediaFrame,
    recv_destroy: unsafe extern "C" fn(Handle),
    send_create: unsafe extern "C" fn(*const c_char, i32) -> Handle,
    send: unsafe extern "C" fn(Handle, *mut MediaFrame) -> i32,
    connections: unsafe extern "C" fn(Handle) -> i32,
    send_destroy: unsafe extern "C" fn(Handle),
}

impl Api {
    unsafe fn load(symbol: impl Fn(&[u8]) -> Option<*const ()>) -> Option<Self> {
        macro_rules! symbol {
            ($name:literal) => {{
                let name = concat!("omt_", $name, "\0");
                std::mem::transmute(symbol(name.as_bytes())?)
            }};
        }
        Some(Self { addresses: symbol!("discovery_getaddresses"),
            recv_create: symbol!("receive_create"), recv: symbol!("receive"),
            recv_destroy: symbol!("receive_destroy"), send_create: symbol!("send_create"),
            send: symbol!("send"), connections: symbol!("send_connections"), send_destroy: symbol!("send_destroy") })
    }
}

#[cfg(windows)]
fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let dirs = [
            std::env::var_os("LIVEMIX_OMT_DIR").map(PathBuf::from),
            std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)),
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
                .map(|p| p.join("LiveMix/dependencies/omt/Libraries/Winx64")),
        ];
        for path in dirs.into_iter().flatten().filter(|p| p.is_absolute()) {
            let codec = path.join("libvmx.dll");
            let library = path.join("libomt.dll");
            if !codec.is_file() || !library.is_file() { continue; }
            unsafe {
                let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS;
                let Ok(codec) = LoadLibraryExW(&HSTRING::from(codec.as_os_str()), None, flags) else { continue };
                if let Ok(module) = LoadLibraryExW(&HSTRING::from(library.as_os_str()), None, flags) {
                    if let Some(api) = Api::load(|name| GetProcAddress(module, PCSTR(name.as_ptr())).map(|p| p as *const ())) {
                        return Some(api);
                    }
                    let _ = FreeLibrary(module);
                }
                let _ = FreeLibrary(codec);
            }
        }
        None
    }).as_ref().ok_or_else(|| "OMT SDK unavailable. Set LIVEMIX_OMT_DIR to the directory containing libomt.dll and libvmx.dll.".into())
}

#[cfg(target_os = "macos")]
fn api() -> Result<&'static Api, String> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let dirs = [
            std::env::var_os("LIVEMIX_OMT_DIR").map(PathBuf::from),
            std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)),
            std::env::var_os("HOME").map(PathBuf::from).map(|p| p.join("Library/Application Support/LiveMix/lib")),
        ];
        for dir in dirs.into_iter().flatten().filter(|path| path.is_absolute()) {
            unsafe {
                let Ok(codec) = libloading::Library::new(dir.join("libvmx.dylib")) else { continue };
                if let Ok(library) = libloading::Library::new(dir.join("libomt.dylib")) {
                    if let Some(api) = Api::load(|name| library.get::<unsafe extern "C" fn()>(name).ok().map(|p| *p as *const ())) {
                        std::mem::forget(codec);
                        std::mem::forget(library);
                        return Some(api);
                    }
                }
            }
        }
        None
    }).as_ref().ok_or_else(|| "OMT SDK unavailable. Set LIVEMIX_OMT_DIR to the directory containing libomt.dylib and libvmx.dylib.".into())
}

pub fn discover() -> Result<Vec<String>, String> {
    let api = api()?;
    // The SDK invalidates the previous array on each call.
    static LOCK: Mutex<()> = Mutex::new(());
    let mut result = Vec::new();
    for _ in 0..5 {
        {
            let _lock = LOCK.lock().unwrap();
            let mut count = 0;
            let list = unsafe { (api.addresses)(&mut count) };
            if !list.is_null() && count > 0 {
                result = unsafe { std::slice::from_raw_parts(list, count as usize) }.iter()
                    .filter(|ptr| !ptr.is_null())
                    .map(|ptr| unsafe { CStr::from_ptr(*ptr) }.to_string_lossy().into_owned())
                    .collect();
            }
        }
        if !result.is_empty() { break; }
        std::thread::sleep(Duration::from_millis(300));
    }
    result.sort_unstable();
    result.dedup();
    Ok(result)
}

pub fn run_capture(shared: Arc<Shared>, address: &str) {
    if let Err(error) = receive(&shared, address) {
        *shared.status.lock().unwrap() = error;
    }
}

fn receive(shared: &Shared, address: &str) -> Result<(), String> {
    let api = api()?;
    let address = CString::new(address).map_err(|_| "Invalid OMT address")?;
    unsafe {
        let receiver = (api.recv_create)(address.as_ptr(), VIDEO, 2, 0);
        if receiver.is_null() { return Err("Could not create OMT receiver".into()); }
        *shared.status.lock().unwrap() = "Waiting for OMT video…".into();
        let mut last_frame = None;
        while !shared.stop.load(Relaxed) {
            let video = (api.recv)(receiver, VIDEO, 100);
            if let Some(frame) = video.as_ref().and_then(|video| unpack(video)) {
                if last_frame.is_none() { *shared.status.lock().unwrap() = "Receiving OMT video".into(); }
                last_frame = Some(Instant::now());
                *shared.size.lock().unwrap() = (frame.w, frame.h);
                *shared.frame.lock().unwrap() = Some(frame);
                shared.seq.fetch_add(1, Relaxed);
            } else if last_frame.is_some_and(|t: Instant| t.elapsed() >= Duration::from_secs(3)) {
                last_frame = None;
                *shared.status.lock().unwrap() = "Waiting for OMT signal…".into();
            }
        }
        (api.recv_destroy)(receiver);
    }
    Ok(())
}

unsafe fn unpack(frame: &MediaFrame) -> Option<Frame> {
    let w = usize::try_from(frame.width).ok()?;
    let h = usize::try_from(frame.height).ok()?;
    let stride = usize::try_from(frame.stride).ok()?;
    if w == 0 || h == 0 || w > 8192 || h > 8192 || stride < w * 4 || stride > 65_536
        || frame.data.is_null() || !matches!(frame.codec, BGRA | BGRX) { return None; }
    if usize::try_from(frame.data_length).ok()? < (h - 1) * stride + w * 4 { return None; }
    let mut rgba = Vec::with_capacity(w * h * 4);
    let alpha = frame.codec == BGRA && frame.flags & 2 != 0;
    let premultiplied = alpha && frame.flags & 4 != 0;
    for y in 0..h {
        for pixel in std::slice::from_raw_parts(frame.data.add(y * stride), w * 4).chunks_exact(4) {
            let a = if alpha { pixel[3] } else { 255 };
            let straight = |c: u8| if premultiplied && a > 0 {
                (u16::from(c) * 255 / u16::from(a)).min(255) as u8
            } else { c };
            rgba.extend_from_slice(&[straight(pixel[2]), straight(pixel[1]), straight(pixel[0]), a]);
        }
    }
    Some(Frame { w, h, rgba })
}

#[derive(Clone)]
pub struct Sender {
    frames: mpsc::SyncSender<NetworkFrame>,
    stop: Arc<AtomicBool>,
    subscribed: Arc<AtomicBool>,
    status: Arc<Mutex<(String, bool)>>,
}

impl Sender {
    pub fn start(name: String, fps: f64, quality: u8) -> Self {
        let (frames, receive) = mpsc::sync_channel(2);
        let stop = Arc::new(AtomicBool::new(false));
        let subscribed = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(("Starting OMT…".into(), false)));
        let (flag, viewers, report) = (stop.clone(), subscribed.clone(), status.clone());
        std::thread::spawn(move || {
            if let Err(error) = send(&name, fps, quality, receive, &flag, &viewers, &report) {
                *report.lock().unwrap() = (error, true);
            }
        });
        Self { frames, stop, subscribed, status }
    }

    pub fn submit(&self, frame: NetworkFrame) { let _ = self.frames.try_send(frame); }
    pub fn stop(&self) { self.stop.store(true, Relaxed); }
    pub fn subscribed(&self) -> bool { self.subscribed.load(Relaxed) }
    pub fn status(&self) -> (String, bool) { self.status.lock().unwrap().clone() }
}

fn send(name: &str, fps: f64, quality: u8, frames: mpsc::Receiver<NetworkFrame>,
    stop: &AtomicBool, subscribed: &AtomicBool, status: &Mutex<(String, bool)>) -> Result<(), String>
{
    let api = api()?;
    let name = CString::new(name).map_err(|_| "Invalid OMT output name")?;
    let (n, d) = crate::media::rate_fraction(fps as f32);
    unsafe {
        let sender = (api.send_create)(name.as_ptr(), i32::from(quality));
        if sender.is_null() { return Err("Could not create OMT sender".into()); }
        *status.lock().unwrap() = ("Waiting for OMT receivers…".into(), false);
        let start = Instant::now();
        let mut last_check = start - Duration::from_millis(250);
        while !stop.load(Relaxed) {
            if last_check.elapsed() >= Duration::from_millis(250) {
                last_check = Instant::now();
                let active = (api.connections)(sender) > 0;
                if active != subscribed.swap(active, Relaxed) {
                    *status.lock().unwrap() = (if active { "Sending OMT video" } else { "Waiting for OMT receivers…" }.into(), false);
                }
            }
            let frame = match frames.recv_timeout(Duration::from_millis(100)) {
                Ok(frame) => frame,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let mut video: MediaFrame = std::mem::zeroed();
            video.frame_type = VIDEO;
            video.timestamp = (start.elapsed().as_nanos() / 100) as i64;
            video.codec = BGRA;
            video.width = frame.width as i32;
            video.height = frame.height as i32;
            video.stride = video.width * 4;
            video.rate_n = n as i32;
            video.rate_d = d as i32;
            video.aspect = video.width as f32 / video.height as f32;
            video.color_space = if video.height < 720 { 601 } else { 709 };
            video.data = frame.pixels.as_ptr();
            video.data_length = frame.pixels.len() as i32;
            (api.send)(sender, &mut video);
        }
        subscribed.store(false, Relaxed);
        (api.send_destroy)(sender);
    }
    Ok(())
}

#[test]
#[ignore = "requires the native OMT runtime and local discovery"]
fn omt_video_loopback() {
    let name = format!("LiveMix OMT test {}", std::process::id());
    let sender = Sender::start(name.clone(), 25.0, 50);
    let mut source = None;
    for _ in 0..3 {
        source = discover().unwrap().into_iter().find(|source| source.contains(&name));
        if source.is_some() { break; }
    }
    let source = source.unwrap_or_else(|| panic!("OMT sender was not discovered: {:?}", sender.status()));
    assert!(!sender.subscribed());
    let input = Arc::new(Shared::default());
    let receiver = Arc::clone(&input);
    let worker = std::thread::spawn(move || run_capture(receiver, &source));
    let frame = NetworkFrame { pixels: Arc::new([20, 55, 180, 255].repeat(320 * 180)), width: 320, height: 180 };
    let deadline = Instant::now() + Duration::from_secs(8);
    while input.seq.load(Relaxed) == 0 && Instant::now() < deadline {
        if sender.subscribed() { sender.submit(frame.clone()); }
        std::thread::sleep(Duration::from_millis(40));
    }
    input.stop.store(true, Relaxed);
    sender.stop();
    worker.join().unwrap();
    let frame = input.frame.lock().unwrap();
    let frame = frame.as_ref().unwrap_or_else(|| panic!("receiver: {}, sender: {:?}", input.status.lock().unwrap(), sender.status()));
    assert_eq!((frame.w, frame.h), (320, 180));
    assert!(frame.rgba[0] > 145 && frame.rgba[2] < 55, "color {:?}", &frame.rgba[..4]);
}
