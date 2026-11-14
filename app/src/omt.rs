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
const AUDIO: i32 = 4;
const FPA1: u32 = u32::from_le_bytes(*b"FPA1");
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

pub struct DirectRecording(Arc<AtomicBool>);

impl Drop for DirectRecording {
    fn drop(&mut self) { self.0.store(true, Relaxed); }
}

pub fn record_compressed(address: String, path: PathBuf, origin: Instant, audio: bool, indexed: bool)
    -> (DirectRecording, Arc<Mutex<String>>, Option<crate::media::ReplayIndex>, std::thread::JoinHandle<()>)
{
    let stop = Arc::new(AtomicBool::new(false));
    let error = Arc::new(Mutex::new(String::new()));
    let index = indexed.then(|| Arc::new(Mutex::new(Vec::new())));
    let (worker_stop, worker_error, worker_index) = (stop.clone(), error.clone(), index.clone());
    let worker = std::thread::spawn(move || {
        if let Err(message) = write_compressed(&address, &path, origin, audio, &worker_stop, worker_index.as_ref()) {
            *worker_error.lock().unwrap() = message;
        }
    });
    (DirectRecording(stop), error, index, worker)
}

fn write_compressed(address: &str, path: &std::path::Path, origin: Instant, audio: bool,
    stop: &AtomicBool, index: Option<&crate::media::ReplayIndex>) -> Result<(), String>
{
    let api = api()?;
    let address = CString::new(address).map_err(|_| "Invalid OMT address")?;
    let kinds = if audio { VIDEO | AUDIO } else { VIDEO };
    // CompressedOnly exposes VMX1 without decoding on this recording connection.
    let receiver = unsafe { (api.recv_create)(address.as_ptr(), kinds, 2, 4) };
    if receiver.is_null() { return Err("Could not create compressed OMT receiver".into()); }
    let result = (|| -> Result<(), String> {
        let mut mux: Option<crate::omt_record::Mux<std::io::BufWriter<std::fs::File>>> = None;
        let (mut video_format, mut sound, mut first) = (None, None, None);
        let mut pending = Vec::new();
        let mut clock = crate::audio::NetworkClock::default();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut video_written = false;
        while !stop.load(Relaxed) {
            let raw = unsafe { (api.recv)(receiver, kinds, 10).as_ref() };
            let now = Instant::now();
            if mux.is_none() && now >= deadline && video_format.is_none() {
                return Err("No compressed OMT video received".into());
            }
            if let Some(raw) = raw {
                let stamp = clock.stamp(raw.timestamp, now).unwrap_or(now);
                let bytes = if raw.frame_type == VIDEO {
                    let format = (raw.width as u32, raw.height as u32, (raw.rate_n as u32, raw.rate_d as u32));
                    if !(1..=8192).contains(&raw.width) || !(1..=8192).contains(&raw.height)
                        || raw.rate_n <= 0 || raw.rate_d <= 0 || raw.compressed_data.is_null()
                        || !(1..=32*1024*1024).contains(&raw.compressed_length) { continue; }
                    if video_format.is_some_and(|previous| previous != format) {
                        if let Some(mux) = mux.take() { mux.finish().map_err(|error| error.to_string())?; }
                        return Err("OMT source changed format during recording".into());
                    }
                    video_format = Some(format);
                    Some((1, unsafe { std::slice::from_raw_parts(raw.compressed_data, raw.compressed_length as usize) }.to_vec()))
                } else if raw.frame_type == AUDIO && raw.codec == FPA1 {
                    if !(1..=32).contains(&raw.channels) || !(1..=192000).contains(&raw.sample_rate)
                        || !(1..=8192).contains(&raw.samples) || raw.data.is_null() { continue; }
                    let channels = raw.channels as usize;
                    let samples = raw.samples as usize;
                    if raw.data_length < (channels * samples * 4) as i32 { continue; }
                    let format = (raw.sample_rate as u32, raw.channels as u32);
                    if mux.is_some() && sound != Some(format) { continue; }
                    sound = Some(format);
                    let planar = unsafe { std::slice::from_raw_parts(raw.data, channels * samples * 4) };
                    let mut interleaved = Vec::with_capacity(planar.len());
                    for sample in 0..samples {
                        for channel in 0..channels {
                            let i = (channel * samples + sample) * 4;
                            interleaved.extend_from_slice(&planar[i..i + 4]);
                        }
                    }
                    Some((2, interleaved))
                } else { None };
                if let Some((track, bytes)) = bytes { pending.push((stamp, track, bytes)); }
            }
            if mux.is_none() {
                let Some((w, h, fps)) = video_format else {
                    if pending.len() >= 40 { pending.remove(0); }
                    continue;
                };
                if audio && sound.is_none() && pending.len() < 8 && now < deadline { continue; }
                pending.sort_by_key(|packet| packet.0);
                first = pending.first().map(|packet| packet.0);
                let output = std::fs::OpenOptions::new().write(true).create_new(true).open(path)
                    .map_err(|error| error.to_string())?;
                mux = Some(crate::omt_record::Mux::new(std::io::BufWriter::new(output), w, h, fps, sound)
                    .map_err(|error| error.to_string())?);
            }
            for (stamp, track, bytes) in pending.drain(..) {
                let media_ms = stamp.saturating_duration_since(first.unwrap()).as_millis() as u64;
                mux.as_mut().unwrap().packet(track, media_ms, &bytes).map_err(|error| error.to_string())?;
                if track == 1 {
                    video_written = true;
                    if let Some(index) = index { index.lock().unwrap().push((stamp.max(origin), media_ms)); }
                }
            }
        }
        if let Some(mux) = mux { mux.finish().map_err(|error| error.to_string())?; }
        if video_written { Ok(()) } else { Err("No compressed OMT frames recorded".into()) }
    })();
    unsafe { (api.recv_destroy)(receiver); }
    result
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
    if let Err(error) = receive(&shared, address, false) {
        *shared.status.lock().unwrap() = error;
    }
}

pub fn thumbnail(shared: Arc<Shared>, address: &str) -> Result<(), String> {
    receive(&shared, address, true)
}

fn receive(shared: &Shared, address: &str, thumbnail: bool) -> Result<(), String> {
    let api = api()?;
    let address = CString::new(address).map_err(|_| "Invalid OMT address")?;
    unsafe {
        let media_types = if thumbnail { VIDEO } else { VIDEO | AUDIO };
        let receiver = (api.recv_create)(address.as_ptr(), media_types, 2, 0);
        if receiver.is_null() { return Err("Could not create OMT receiver".into()); }
        *shared.status.lock().unwrap() = "Waiting for OMT video…".into();
        let mut last_frame = None;
        let mut audio_phase = 0;
        let mut clock = crate::audio::NetworkClock::default();
        while !shared.stop.load(Relaxed) {
            let media = (api.recv)(receiver, media_types, 5);
            let now = Instant::now();
            if let Some(audio) = media.as_ref().filter(|frame| frame.frame_type == AUDIO && frame.codec == FPA1) {
                let frames = audio.samples.max(0) as usize;
                let channels = audio.channels.max(0) as usize;
                let stride = frames * 4;
                if stride.saturating_mul(channels) <= audio.data_length.max(0) as usize {
                    crate::audio::capture_planar(shared, audio.data, frames, stride, channels,
                        audio.sample_rate as usize, 32768.0, &mut audio_phase, clock.stamp(audio.timestamp, now));
                }
            } else if let Some((video, frame)) = media.as_ref().filter(|frame| frame.frame_type == VIDEO)
                .and_then(|video| unpack(video).map(|frame| (video, frame)))
            {
                if last_frame.is_none() { *shared.status.lock().unwrap() = "Receiving OMT video".into(); }
                last_frame = Some(now);
                let stamp = if thumbnail { now } else {
                    clock.stamp(video.timestamp, now).unwrap_or(now)
                };
                clock.queue_video(frame, stamp);
                clock.present(shared, now);
                if thumbnail { break; }
            } else if last_frame.is_some_and(|t: Instant| t.elapsed() >= Duration::from_secs(3)) {
                last_frame = None;
                *shared.status.lock().unwrap() = "Waiting for OMT signal…".into();
            }
            clock.present(shared, now);
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
    frames: mpsc::SyncSender<(NetworkFrame, crate::audio::Packet)>,
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

    pub fn submit(&self, frame: NetworkFrame, audio: crate::audio::Packet) { let _ = self.frames.try_send((frame, audio)); }
    pub fn stop(&self) { self.stop.store(true, Relaxed); }
    pub fn subscribed(&self) -> bool { self.subscribed.load(Relaxed) }
    pub fn status(&self) -> (String, bool) { self.status.lock().unwrap().clone() }
}

fn send(name: &str, fps: f64, quality: u8, frames: mpsc::Receiver<(NetworkFrame, crate::audio::Packet)>,
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
            let (frame, packet) = match frames.recv_timeout(Duration::from_millis(100)) {
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
            if !packet.is_empty() {
                // The mixer windows are contiguous, so each packet is sent whole.
                let count = packet.len();
                let planar = crate::audio::planar(&packet[..count], 1.0 / 32768.0);
                let mut audio: MediaFrame = std::mem::zeroed();
                audio.frame_type = AUDIO;
                audio.timestamp = video.timestamp;
                audio.codec = FPA1;
                audio.sample_rate = crate::audio::RATE as i32;
                audio.channels = 2;
                audio.samples = count as i32;
                audio.data = planar.as_ptr().cast();
                audio.data_length = (planar.len() * 4) as i32;
                (api.send)(sender, &mut audio);
            }
        }
        subscribed.store(false, Relaxed);
        (api.send_destroy)(sender);
    }
    Ok(())
}

#[test]
#[ignore = "requires the native OMT runtime and FFmpeg"]
fn omt_compressed_loopback() {
    let name = format!("LiveMix OMT direct {}", crate::replay_stamp());
    let sender = Sender::start(name.clone(), 25.0, 50);
    let source = (0..3).find_map(|_| discover().unwrap().into_iter().find(|source| source.contains(&name)))
        .unwrap_or_else(|| panic!("OMT sender was not discovered: {:?}", sender.status()));
    let path = std::env::temp_dir().join(format!("livemix-omt-{}.mkv", crate::replay_stamp()));
    let (recording, error, index, worker) = record_compressed(source, path.clone(), Instant::now(), true, true);
    let frame = NetworkFrame { pixels: Arc::new([20, 55, 180, 255].repeat(320 * 180)), width: 320, height: 180 };
    let packet: crate::audio::Packet = vec![[4800, -4800]; 1920].into();
    let index = index.unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline && !worker.is_finished() && index.lock().unwrap().len() < 32 {
        sender.submit(frame.clone(), packet.clone());
        std::thread::sleep(Duration::from_millis(40));
    }
    drop(recording);
    worker.join().unwrap();
    sender.stop();
    assert!(error.lock().unwrap().is_empty(), "{:?}", error);
    assert!(index.lock().unwrap().len() >= 32);
    let result = crate::stream::command().args(["-v", "error", "-xerror", "-ss", "0.50", "-i"])
        .arg(&path).args(["-map", "0:v:0", "-map", "0:a:0", "-f", "null", "-"]).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let folder = path.parent().unwrap();
    let mut segment = crate::replay::Segment { start: 0, end: 1000, ..Default::default() };
    segment.files[0] = Some(path.file_name().unwrap().to_string_lossy().into_owned());
    let index = index.lock().unwrap();
    let first = index[0].0;
    segment.frames[0] = index.iter().map(|(stamp, media_ms)| crate::replay::FrameStamp {
        show_ms: stamp.saturating_duration_since(first).as_millis() as u64, media_ms: *media_ms,
    }).collect();
    let output = path.with_extension("mp4");
    let event = crate::replay::Event { id: 1, start: 200, end: 800, speed: 50, angles: [true; 8], ..Default::default() };
    crate::replay::event_media(folder, &[segment], &event, 0, event.speed, &output).unwrap();
    assert!(std::fs::metadata(&output).unwrap().len() > 1024);
    std::fs::remove_file(output).unwrap();
    std::fs::remove_file(path).unwrap();
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
    let packet: crate::audio::Packet = vec![[4800, -4800]; 1920].into();
    let mut level = 0.0;
    while (input.seq.load(Relaxed) == 0 || level < 0.1) && Instant::now() < deadline {
        if sender.subscribed() { sender.submit(frame.clone(), packet.clone()); }
        std::thread::sleep(Duration::from_millis(40));
        level = crate::audio::input_peak((&input, false, 1.0), Instant::now());
    }
    input.stop.store(true, Relaxed);
    sender.stop();
    worker.join().unwrap();
    let frame = input.frame.lock().unwrap();
    let frame = frame.as_ref().unwrap_or_else(|| panic!("receiver: {}, sender: {:?}", input.status.lock().unwrap(), sender.status()));
    assert_eq!((frame.w, frame.h), (320, 180));
    assert!(level >= 0.1, "OMT stereo audio was not received");
    assert!(frame.rgba[0] > 145 && frame.rgba[2] < 55, "color {:?}", &frame.rgba[..4]);
}
