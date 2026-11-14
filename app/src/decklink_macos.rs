//! DeckLink on macOS through an FFmpeg build linked against the Desktop Video SDK.
use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::os::fd::AsRawFd;
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

use crate::{audio, media, stream};

pub const CONNECTIONS: [(&str, u32); 7] = [
    ("Auto", 0), ("SDI", 1), ("HDMI", 2), ("Optical SDI", 4),
    ("Component", 8), ("Composite", 16), ("S-Video", 32),
];

pub fn connection_name(value: u32) -> &'static str {
    CONNECTIONS.iter().find(|(_, port)| *port == value).map_or("Auto", |(name, _)| name)
}

pub fn device_index(link: &str) -> Option<usize> { link.strip_prefix("decklink:")?.parse().ok() }

fn command() -> Command {
    let mut cmd = if let Some(path) = std::env::var_os("LIVEMIX_DECKLINK_FFMPEG") {
        Command::new(path)
    } else {
        stream::command()
    };
    cmd.args(["-hide_banner", "-nostdin"]).stdin(Stdio::null());
    cmd
}

#[derive(Clone)]
pub struct Device {
    pub index: usize,
    pub name: String,
    pub capture: bool,
    pub playback: bool,
    address: String,
}

static DEVICES: Mutex<Vec<Device>> = Mutex::new(Vec::new());
type ModeCache = Mutex<HashMap<(usize, bool), Vec<Mode>>>;
static MODES: OnceLock<ModeCache> = OnceLock::new();

pub fn devices_cached() -> Vec<Device> {
    DEVICES.lock().unwrap().clone()
}

pub fn refresh_devices() -> Vec<Device> {
    let mut list = Vec::<Device>::new();
    for playback in [false, true] {
        let direction = if playback { "-sinks" } else { "-sources" };
        let Ok(result) = command().args([direction, "decklink"]).output() else { continue };
        let text = String::from_utf8_lossy(&result.stdout);
        for line in text.lines() {
            let Some((address, description)) = line.trim().trim_start_matches('*').trim().split_once(" [") else { continue };
            let Some((name, _)) = description.rsplit_once("] (") else { continue };
            if address.is_empty() || name.is_empty() { continue; }
            if let Some(device) = list.iter_mut().find(|device| device.address == address) {
                device.capture |= !playback;
                device.playback |= playback;
            } else {
                list.push(Device { index: 0, name: name.into(), capture: !playback, playback, address: address.into() });
            }
        }
    }
    list.sort_by(|a, b| a.address.cmp(&b.address));
    for (index, device) in list.iter_mut().enumerate() { device.index = index; }
    MODES.get_or_init(Default::default).lock().unwrap().clear();
    *DEVICES.lock().unwrap() = list.clone();
    list
}

fn device(index: usize) -> Result<Device, String> {
    let mut list = devices_cached();
    if list.is_empty() { list = refresh_devices(); }
    list.get(index).cloned().ok_or_else(|| {
        "DeckLink device unavailable. Install Blackmagic Desktop Video and set LIVEMIX_DECKLINK_FFMPEG to a DeckLink-enabled FFmpeg.".into()
    })
}

#[derive(Clone)]
pub struct Mode {
    pub mode: u32,
    w: u32,
    h: u32,
    n: u32,
    d: u32,
    fields: &'static str,
}

impl Mode {
    pub fn label(&self) -> String {
        format!("{}x{}{} {:.2} fps", self.w, self.h, if self.fields == "progressive" { "p" } else { "i" }, self.fps())
    }

    fn fps(&self) -> f64 { f64::from(self.n) / f64::from(self.d) }
}

fn parse_mode(line: &str) -> Option<Mode> {
    let line = if line.starts_with('[') { line.split_once("] ")?.1 } else { line };
    let (format, data) = line.trim().split_once(char::is_whitespace)?;
    if !(3..=4).contains(&format.len()) || !format.is_ascii() { return None; }
    let mut code = *b"    ";
    code[..format.len()].copy_from_slice(format.as_bytes());
    let (size, rate) = data.trim_start().split_once(" at ")?;
    let (w, h) = size.split_once('x')?;
    let (rate, suffix) = rate.split_once(" fps")?;
    let (n, d) = rate.split_once('/')?;
    let (n, d) = (n.parse().ok()?, d.parse().ok()?);
    if n == 0 || d == 0 { return None; }
    let fields = if suffix.contains("lower field") { "bb" } else if suffix.contains("upper field") { "tt" } else { "progressive" };
    Some(Mode { mode: u32::from_be_bytes(code), w: w.parse().ok()?, h: h.parse().ok()?, n, d, fields })
}

pub fn display_modes(index: usize, playback: bool) -> Vec<Mode> {
    let cache = MODES.get_or_init(Default::default);
    if let Some(modes) = cache.lock().unwrap().get(&(index, playback)).cloned() { return modes; }
    let Ok(device) = device(index) else { return Vec::new() };
    let mut cmd = command();
    if playback {
        cmd.args(["-f", "lavfi", "-i", "color=size=64x48:rate=30", "-f", "decklink", "-list_formats", "1", &device.address]);
    } else {
        cmd.args(["-f", "decklink", "-list_formats", "1", "-i", &device.address]);
    }
    let modes: Vec<_> = cmd.output().ok().into_iter().flat_map(|result| {
        String::from_utf8_lossy(&result.stderr).lines().filter_map(parse_mode).collect::<Vec<_>>()
    }).collect();
    cache.lock().unwrap().insert((index, playback), modes.clone());
    modes
}

fn format_code(mode: u32) -> String { String::from_utf8_lossy(&mode.to_be_bytes()).trim_end().to_owned() }

#[derive(Clone, Copy)]
pub struct CaptureConfig { pub device: usize, pub port: u32, pub mode: u32 }

pub fn run_capture(shared: Arc<media::Shared>, config: CaptureConfig) {
    if let Err(error) = capture(&shared, config) { shared.set_status(error); }
}

fn capture(shared: &media::Shared, config: CaptureConfig) -> Result<(), String> {
    let device = device(config.device)?;
    let (audio_read, audio_write) = UnixStream::pair().map_err(|e| e.to_string())?;
    let mut cmd = command();
    let fd = audio_write.as_raw_fd();
    // Give FFmpeg an independent audio pipe without reopening the capture device.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.args(["-loglevel", "error", "-f", "decklink", "-channels", "2", "-audio_input", "embedded"]);
    if config.port != 0 {
        let port = match config.port {
            1 => "sdi", 2 => "hdmi", 4 => "optical_sdi", 8 => "component", 16 => "composite", _ => "s_video",
        };
        cmd.args(["-video_input", port]);
    }
    if config.mode != 0 { cmd.args(["-format_code", &format_code(config.mode)]); }
    let mut child = cmd.args(["-i", &device.address,
        "-map", "0:v:0", "-threads:v", "1", "-c:v", "bmp", "-pix_fmt", "bgra", "-f", "image2pipe", "pipe:1",
        "-map", "0:a:0", "-c:a", "pcm_s16le", "-ac", "2", "-ar", "48000", "-f", "s16le", "pipe:3"])
        .stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
        .map_err(|e| format!("Could not start DeckLink capture: {e}"))?;
    drop(audio_write);
    let mut video = BufReader::new(child.stdout.take().unwrap());
    let child = Mutex::new(child);
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Relaxed) && !shared.stop.load(Relaxed) { std::thread::sleep(Duration::from_millis(25)); }
            stop_child(&mut child.lock().unwrap());
        });
        scope.spawn(move || {
            let mut audio = BufReader::new(audio_read);
            let mut bytes = [0u8; 480 * 4];
            while audio.read_exact(&mut bytes).is_ok() {
                let samples: Vec<_> = audio::pcm(&bytes).collect();
                shared.audio.lock().unwrap().capture(&samples);
            }
        });
        while !shared.stop.load(Relaxed) {
            let Some(frame) = crate::srt::read_frame(&mut video) else { break };
            media::publish(shared, frame);
            shared.set_status("");
        }
        done.store(true, Relaxed);
    });
    if shared.stop.load(Relaxed) { Ok(()) } else { Err("DeckLink capture stopped. Check the Desktop Video driver, input connector and signal format.".into()) }
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[derive(Clone, Copy, PartialEq)]
pub struct OutputConfig {
    pub device: usize,
    pub key_fill: bool,
    pub port: u32,
    pub mode: u32,
    pub size: (u32, u32),
    pub fps: f32,
    pub interlaced: bool,
}

struct OutputFrame { pixels: Arc<Vec<u8>>, w: usize, h: usize, audio: audio::Packet }

#[derive(Default)]
struct Playback {
    stop: AtomicBool,
    finished: AtomicBool,
    frames: AtomicU64,
    error: Mutex<Option<String>>,
    child: Mutex<Option<Child>>,
}

#[derive(Clone)]
pub struct OutputSink {
    tx: mpsc::SyncSender<OutputFrame>,
    state: Arc<Playback>,
}

impl OutputSink {
    pub fn start(config: OutputConfig) -> Self {
        let (tx, rx) = mpsc::sync_channel(3);
        let state = Arc::new(Playback::default());
        let worker = state.clone();
        std::thread::spawn(move || {
            let result = output(&worker, config, rx);
            if let Some(mut child) = worker.child.lock().unwrap().take() { stop_child(&mut child); }
            if !worker.stop.load(Relaxed) { *worker.error.lock().unwrap() = result.err(); }
            worker.finished.store(true, Relaxed);
        });
        Self { tx, state }
    }

    pub fn submit(&self, pixels: Arc<Vec<u8>>, w: usize, h: usize, audio: audio::Packet) {
        if !self.state.stop.load(Relaxed) { let _ = self.tx.try_send(OutputFrame { pixels, w, h, audio }); }
    }

    pub fn stop(&self) {
        self.state.stop.store(true, Relaxed);
        if let Some(child) = self.state.child.lock().unwrap().as_mut() { let _ = child.kill(); }
    }
    pub fn error(&self) -> Option<String> { self.state.error.lock().unwrap().clone() }
    pub fn finished(&self) -> bool { self.state.finished.load(Relaxed) }
    pub fn frames(&self) -> u64 { self.state.frames.load(Relaxed) }
}

fn output(state: &Playback, config: OutputConfig, rx: mpsc::Receiver<OutputFrame>) -> Result<(), String> {
    let device = device(config.device)?;
    let modes = display_modes(config.device, true);
    let mode = modes.iter().find(|mode| mode.mode == config.mode).or_else(|| {
        modes.iter().find(|mode| {
            (mode.w, mode.h) == config.size && (mode.fps() - f64::from(config.fps)).abs() < 0.03
                && (mode.fields != "progressive") == config.interlaced
        })
    }).ok_or("Select a supported DeckLink output format or match the project format to the device.")?;
    if config.key_fill {
        let keyer = crate::decklink_keyer_macos::Keyer::open(&device.name, &device.address, mode.mode, mode.w, mode.h)?;
        let mut latest: Option<OutputFrame> = None;
        while !state.stop.load(Relaxed) {
            if latest.is_none() {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(frame) => latest = Some(frame),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            while let Ok(frame) = rx.try_recv() { latest = Some(frame); }
            let frame = latest.as_ref().unwrap();
            keyer.display(&frame.pixels, frame.w, frame.h)?;
            state.frames.fetch_add(1, Relaxed);
        }
        return Ok(());
    }
    let (n, d) = media::rate_fraction(config.fps);
    let mut cmd = command();
    cmd.args(["-loglevel", "error", "-probesize", "32", "-analyzeduration", "1", "-f", "matroska", "-i", "pipe:0",
        "-vf", &format!("fps={}/{},scale={}:{}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={}:{}:(ow-iw)/2:(oh-ih)/2",
            mode.n, mode.d, mode.w, mode.h, mode.w, mode.h),
        "-field_order", mode.fields, "-pix_fmt", "uyvy422", "-c:v", "rawvideo", "-c:a", "pcm_s16le",
        "-ar", "48000", "-ac", "2", "-f", "decklink", &device.address]);
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|e| format!("Could not start DeckLink output: {e}"))?;
    let input = child.stdin.take().unwrap();
    let mut slot = state.child.lock().unwrap();
    if state.stop.load(Relaxed) { stop_child(&mut child); return Ok(()); }
    *slot = Some(child);
    drop(slot);
    let mut pipe = audio::AvPipe::new(input, config.size.0, config.size.1, n, d).map_err(|e| e.to_string())?;
    while !state.stop.load(Relaxed) {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(frame) if (frame.w as u32, frame.h as u32) == config.size => {
                pipe.frame(&frame.pixels, &frame.audio).map_err(|_| "DeckLink output stopped. Check the driver, profile, port and display mode.")?;
                state.frames.fetch_add(1, Relaxed);
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let mut child = state.child.lock().unwrap();
                if child.as_mut().and_then(|child| child.try_wait().ok()).flatten().is_some() {
                    return Err("DeckLink output stopped. Check the driver, profile, port and display mode.".into());
                }
            }
        }
    }
    Ok(())
}

#[test]
fn decklink_mode_parses_fractional_rates_and_field_order() {
    let mode = parse_mode("[decklink @ 0x1] Hi59\t\t1920x1080 at 30000/1001 fps (interlaced, upper field first)").unwrap();
    assert_eq!(mode.mode, u32::from_be_bytes(*b"Hi59"));
    assert_eq!((mode.w, mode.h, mode.n, mode.d, mode.fields), (1920, 1080, 30_000, 1001, "tt"));
    assert!(parse_mode("Supported formats for example").is_none());
}
