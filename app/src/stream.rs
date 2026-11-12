use crate::program::NetworkFrame;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Clone, PartialEq, Eq)]
pub struct Destination {
    pub provider: u8,
    pub url: String,
    pub key: String,
    pub quality: u8,
    pub source: usize,
    pub hardware: bool,
    pub hevc: bool,
    pub direct: Option<u32>,
    pub bitrate: Option<u32>,
    pub source_audio: bool,
}

impl Default for Destination {
    fn default() -> Self {
        Self { provider: 0, url: String::new(), key: String::new(), quality: 1, source: 0, hardware: false, hevc: false, direct: None, bitrate: None, source_audio: false }
    }
}

impl Destination {
    pub fn audio_rate(&self) -> u32 {
        let host = self.url.split_once("://").map(|(_, rest)| rest.split(['/', ':']).next().unwrap_or(""));
        if self.provider == 1 || host.is_some_and(|host| host == "youtube.com" || host.ends_with(".youtube.com")) {
            44_100
        } else { 48_000 }
    }
}

impl std::fmt::Debug for Destination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Destination").field("provider", &self.provider)
            .field("quality", &self.quality).field("source", &self.source)
            .field("hardware", &self.hardware).finish_non_exhaustive()
    }
}

pub const COUNT: usize = 5;
pub const QUALITIES: [(&str, [u32; 2], u32); 10] = [
    ("720p", [1280, 720], 3), ("1080p", [1920, 1080], 6), ("1080p High", [1920, 1080], 9),
    ("240p", [426, 240], 1), ("360p", [640, 360], 2), ("480p", [854, 480], 3),
    ("540p", [960, 540], 4), ("900p", [1600, 900], 5), ("1440p", [2560, 1440], 15),
    ("2160p (4K)", [3840, 2160], 30),
];
pub const HARDWARE_ENCODER_LABEL: &str = if cfg!(target_os = "macos") {
    "Use Apple VideoToolbox when available"
} else {
    "Use NVIDIA NVENC, AMD AMF or Intel Quick Sync when available"
};

#[derive(Clone)]
pub struct Output {
    frames: mpsc::SyncSender<(NetworkFrame, crate::audio::Packet, std::time::Instant)>,
    state: Arc<Mutex<State>>,
}

struct State {
    status: String,
    child: Option<Child>,
    stop: bool,
    failed: bool,
    log: VecDeque<String>,
    started: std::time::Instant,
}

impl State {
    fn log(&mut self, line: impl Into<String>) {
        if self.log.len() == 80 { self.log.pop_front(); }
        let line = line.into();
        let seconds = self.started.elapsed().as_secs();
        self.log.push_back(format!("[+{seconds}s] {}", line.chars().take(1024).collect::<String>()));
    }
}

fn redact(line: &str, secrets: &[String]) -> String {
    let mut cleaned = line.split_whitespace().map(|part| {
        if part.contains("://") { "[destination hidden]" } else { part }
    }).collect::<Vec<_>>().join(" ");
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        cleaned = cleaned.replace(secret, "[hidden]");
    }
    cleaned
}

pub(crate) fn publish_url(dest: &Destination) -> Result<String, String> {
    let url = dest.url.trim();
    let (_, host) = url.split_once("://").filter(|(scheme, _)| matches!(*scheme, "rtmp" | "rtmps"))
        .ok_or("Enter an rtmp:// or rtmps:// server URL")?;
    let key = dest.key.trim();
    if host.split('/').next().unwrap_or("").is_empty()
        || url.chars().chain(key.chars()).any(char::is_whitespace)
    {
        return Err("The RTMP URL or stream key is invalid".into());
    }
    Ok(if key.is_empty() { url.into() } else { format!("{}/{}", url.trim_end_matches('/'), key) })
}

pub(crate) fn command() -> Command {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    let ffmpeg = std::env::var_os("LIVEMIX_FFMPEG").map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok().map(|p| p.with_file_name(exe)).filter(|p| p.is_file()))
        .or_else(|| ["LOCALAPPDATA", "HOME"].into_iter().filter_map(std::env::var_os)
            .map(PathBuf::from).flat_map(|p| [p.join("LiveMix/dependencies/ffmpeg"), p.join("Library/Application Support/LiveMix/bin")])
            .map(|p| p.join(exe)).find(|p| p.is_file()))
        .or_else(|| ["/opt/homebrew/bin", "/usr/local/bin"].into_iter().filter(|_| cfg!(target_os = "macos"))
            .map(|p| PathBuf::from(p).join(exe)).find(|p| p.is_file()))
        .unwrap_or_else(|| PathBuf::from(exe));
    let mut command = Command::new(ffmpeg);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    #[cfg(target_os = "macos")]
    command.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", std::env::var("PATH").unwrap_or_default()));
    command
}

impl Output {
    pub fn start_group(destinations: &[&Destination], size: [u32; 2], fps: f64) -> Result<Self, String> {
        let dest = destinations.first().ok_or("No streaming destinations")?;
        let urls = destinations.iter().map(|dest| publish_url(dest)).collect::<Result<Vec<_>, _>>()?;
        let secrets = destinations.iter().flat_map(|dest| {
            let embedded = dest.url.split_once("://").and_then(|(_, rest)| rest.rsplit_once('/'))
                .map(|(_, suffix)| suffix.to_owned()).unwrap_or_default();
            [dest.key.trim().to_owned(), embedded]
        }).collect();
        let (muxer, url) = if urls.len() == 1 {
            ("flv", urls.into_iter().next().unwrap())
        } else {
            let slaves = urls.iter().map(|url| {
                let escaped = url.replace('\\', "\\\\").replace('\'', "\\'").replace('|', "\\|");
                format!("[f=flv:onfail=ignore:flvflags=no_duration_filesize]{escaped}")
            }).collect::<Vec<_>>().join("|");
            ("tee", slaves)
        };
        let (_, resolution, mbps) = QUALITIES[usize::from(dest.quality).min(QUALITIES.len() - 1)];
        Ok(Self::start_encoder(Encoding { url, muxer, resolution: Some(resolution), kbps: dest.bitrate.unwrap_or(mbps) * 1000,
            audio_kbps: 128, audio_rate: dest.audio_rate(), gop_seconds: 2, hardware: dest.hardware, hevc: dest.hevc, secrets }, size, fps))
    }

    pub fn start_srt(config: &crate::srt::Settings, size: [u32; 2], fps: f64) -> Result<Self, String> {
        Ok(Self::start_encoder(Encoding { url: config.connection.url()?, muxer: "mpegts", resolution: None,
            kbps: config.video_kbps, audio_kbps: config.audio_kbps, audio_rate: 48_000, gop_seconds: config.gop_seconds,
            hardware: config.hardware, hevc: config.hevc, secrets: vec![config.connection.passphrase.clone()] }, size, fps))
    }

    fn start_encoder(encoding: Encoding, size: [u32; 2], fps: f64) -> Self {
        let (frames, receive) = mpsc::sync_channel(2);
        let state = Arc::new(Mutex::new(State {
            status: "Connecting…".into(), child: None, stop: false, failed: false, log: VecDeque::new(), started: std::time::Instant::now(),
        }));
        let worker = Arc::clone(&state);
        std::thread::spawn(move || {
            let result = run(&worker, receive, &encoding, size, fps);
            let mut state = worker.lock().unwrap();
            if let Some(mut child) = state.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            if !state.stop {
                state.failed = true;
                state.status = result.err().unwrap_or_else(|| "FFmpeg stopped".into());
                let message = state.status.clone();
                state.log(message);
            }
        });
        Self { frames, state }
    }

    pub fn submit_audio(&self, frame: NetworkFrame, audio: crate::audio::Packet, captured: std::time::Instant) {
        let _ = self.frames.try_send((frame, audio, captured));
    }

    pub fn status(&self) -> (String, bool) {
        let state = self.state.lock().unwrap();
        (state.status.clone(), state.failed)
    }

    pub fn diagnostics(&self) -> String {
        self.state.lock().unwrap().log.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    pub fn stop(&self) {
        let mut state = self.state.lock().unwrap();
        state.stop = true;
        if let Some(child) = &mut state.child {
            let _ = child.kill();
        }
    }
}

struct Encoding {
    url: String,
    muxer: &'static str,
    resolution: Option<[u32; 2]>,
    kbps: u32,
    audio_kbps: u32,
    audio_rate: u32,
    gop_seconds: u32,
    hardware: bool,
    hevc: bool,
    secrets: Vec<String>,
}

fn run(state: &Arc<Mutex<State>>, receive: mpsc::Receiver<(NetworkFrame, crate::audio::Packet, std::time::Instant)>, encoding: &Encoding,
    [w, h]: [u32; 2], fps: f64) -> Result<(), String>
{
    let (n, d) = crate::media::rate_fraction(fps as f32);
    let srt = encoding.muxer == "mpegts";
    if srt { crate::srt::check_ffmpeg()?; }
    let (gpu, cpu) = match (cfg!(target_os = "macos"), encoding.hevc) {
        (true, true) => ("hevc_videotoolbox", "libx265"),
        (true, false) => ("h264_videotoolbox", "libx264"),
        (false, true) => ("hevc_nvenc", "libx265"),
        (false, false) => ("h264_nvenc", "libx264"),
    };
    let amd = if encoding.hevc { "hevc_amf" } else { "h264_amf" };
    let intel = if encoding.hevc { "hevc_qsv" } else { "h264_qsv" };
    let encoder = if !encoding.hardware {
        cpu
    } else if hardware_encoder_available(gpu) {
        gpu
    } else if cfg!(windows) && hardware_encoder_available(amd) {
        amd
    } else if cfg!(windows) && hardware_encoder_available(intel) {
        intel
    } else {
        cpu
    };
    let bitrate = format!("{}k", encoding.kbps.clamp(100, 100_000));
    let mut state_lock = state.lock().unwrap();
    if state_lock.stop { return Ok(()); }
    let mut cmd = command();
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostats", "-progress", "pipe:1", "-nostdin"])
        .args(["-probesize", "32", "-analyzeduration", "1", "-f", "matroska", "-i", "pipe:0"])
        .args(["-map", "0:v:0", "-map", "0:a:0", "-c:v", encoder]);
    if encoder == cpu {
        cmd.args(["-preset", "veryfast", "-tune", "zerolatency"]);
    }
    if let Some([width, height]) = encoding.resolution {
        cmd.args(["-vf", &format!("scale={width}:{height}:force_original_aspect_ratio=decrease:force_divisible_by=2,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2")]);
    }
    if encoding.muxer == "tee" { cmd.args(["-flags", "+global_header", "-use_fifo", "1"]); }
    cmd.args(["-pix_fmt", "yuv420p", "-b:v", &bitrate, "-maxrate", &bitrate, "-bufsize", &bitrate,
        "-g", &(fps * f64::from(encoding.gop_seconds.clamp(1, 10))).round().to_string(), "-bf", "0",
        "-c:a", "aac", "-ac", "2", "-ar", &encoding.audio_rate.to_string(), "-b:a", &format!("{}k", encoding.audio_kbps.clamp(32, 384)),
        "-rw_timeout", "5000000", "-flush_packets", "1", "-f", encoding.muxer, &encoding.url]);
    state_lock.log(format!("Starting {encoder}, {bitrate} video, AAC stereo at {} Hz and {} kbps",
        encoding.audio_rate, encoding.audio_kbps));
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("Could not start FFmpeg: {e}. Set LIVEMIX_FFMPEG or install FFmpeg."))?;
    let input = child.stdin.take().unwrap();
    let progress = child.stdout.take().unwrap();
    let errors = child.stderr.take().unwrap();
    let error_state = Arc::clone(state);
    let secrets = encoding.secrets.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(errors).lines().map_while(Result::ok) {
            error_state.lock().unwrap().log(format!("FFmpeg: {}", redact(&line, &secrets)));
        }
    });
    state_lock.child = Some(child);
    drop(state_lock);

    let progress_state = Arc::clone(state);
    std::thread::spawn(move || {
        let mut live = false;
        let mut frame = String::new();
        let mut speed = String::new();
        let mut bitrate = String::new();
        let mut last = std::time::Instant::now();
        for line in BufReader::new(progress).lines().map_while(Result::ok) {
            if let Some(value) = line.strip_prefix("frame=") { frame = value.to_owned(); }
            if let Some(value) = line.strip_prefix("speed=") { speed = value.to_owned(); }
            if let Some(value) = line.strip_prefix("bitrate=") { bitrate = value.to_owned(); }
            if line.starts_with("progress=") && last.elapsed() >= Duration::from_secs(5) {
                progress_state.lock().unwrap().log(format!("Published frames {frame} · speed {speed} · bitrate {bitrate}"));
                last = std::time::Instant::now();
            }
            if !live && line.strip_prefix("frame=").and_then(|n| n.trim().parse::<u64>().ok()).is_some_and(|n| n > 0) {
                let mut state = progress_state.lock().unwrap();
                if !state.stop && !state.failed {
                    state.status = format!("Live · {encoder} · stereo AAC");
                }
                live = true;
            }
        }
    });

    let failure = if srt { "SRT disconnected. Check mode, host, port, passphrase and encoder." }
        else { "FFmpeg exited. Check the server, key and encoder." };
    let mut input = crate::audio::AvPipe::new(input, w, h, n, d).map_err(|_| failure)?;
    let mut start = None;
    let mut next = 0_u64;
    let mut previous: Option<NetworkFrame> = None;
    let mut peak = 0.0_f32;
    let mut audio_samples = 0;
    let mut audio_interval = std::time::Instant::now();
    loop {
        if state.lock().unwrap().stop { return Ok(()); }
        match receive.recv_timeout(Duration::from_millis(100)) {
            Ok((frame, audio, at)) if frame.width == w as usize && frame.height == h as usize => {
                let slot = (at.saturating_duration_since(*start.get_or_insert(at)).as_secs_f64() * fps).round() as u64;
                if slot < next { continue; }
                if let Some(previous) = &previous {
                    for _ in 0..(slot - next).min(fps.ceil() as u64) {
                        input.frame(&previous.pixels, &[]).map_err(|_| failure)?;
                    }
                }
                input.frame(&frame.pixels, &audio).map_err(|_| failure)?;
                peak = crate::audio::stereo_peaks(&audio).into_iter().fold(peak, f32::max);
                audio_samples += audio.len();
                if audio_interval.elapsed() >= Duration::from_secs(5) {
                    state.lock().unwrap().log(format!("Audio input: {audio_samples} stereo samples, peak {:.1} dBFS",
                        20.0 * peak.max(0.000_01).log10()));
                    peak = 0.0;
                    audio_samples = 0;
                    audio_interval = std::time::Instant::now();
                }
                previous = Some(frame);
                next = slot + 1;
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let mut state = state.lock().unwrap();
                if state.child.as_mut().and_then(|child| child.try_wait().ok()).flatten().is_some() {
                    return Err(failure.into());
                }
            }
        }
    }
}

fn hardware_encoder_available(encoder: &str) -> bool {
    command().args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=size=128x128:rate=30",
        "-frames:v", "1", "-c:v", encoder, "-f", "null", "-"])
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .status().is_ok_and(|status| status.success())
}

#[test]
#[ignore = "requires an FFmpeg build with RTMP listen support"]
fn ffmpeg_rtmp_loopback() {
    rtmp_loopback(1, false, false);
    rtmp_loopback(5, false, false);
    rtmp_loopback(1, true, false);
    rtmp_loopback(1, false, true);
}

#[cfg(test)]
fn rtmp_loopback(count: usize, hevc: bool, youtube: bool) {
    use std::net::TcpListener;
    use std::time::Instant;
    let mut servers = Vec::new();
    let mut destinations = Vec::new();
    for index in 0..count {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let url = format!("rtmp://127.0.0.1:{port}/live/test");
        let path = std::env::temp_dir().join(format!("livemix-rtmp-test-{}-{index}.flv", std::process::id()));
        let server = command().args(["-hide_banner", "-loglevel", "error", "-y", "-listen", "1", "-i", &url,
            "-map", "0:v:0", "-map", "0:a:0", "-c", "copy", "-t", "1", "-f", "flv"])
            .arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        servers.push((server, path));
        destinations.push(Destination { url, quality: 4, hevc, provider: u8::from(youtube), ..Default::default() });
    }
    std::thread::sleep(Duration::from_millis(250));
    let destinations: Vec<_> = destinations.iter().collect();
    let output = Output::start_group(&destinations, [320, 180], 25.0).unwrap();
    let frame = NetworkFrame { pixels: Arc::new([20, 55, 180, 255].repeat(320 * 180)), width: 320, height: 180 };
    let deadline = Instant::now() + Duration::from_secs(12);
    let packet = crate::audio::test_tone(1920);
    let received = loop {
        if servers.iter_mut().all(|(server, _)| server.try_wait().unwrap().is_some_and(|exit| exit.success())) {
            break true;
        }
        if Instant::now() >= deadline || output.status().1 { break false; }
        output.submit_audio(frame.clone(), packet.clone(), Instant::now());
        std::thread::sleep(Duration::from_millis(40));
    };
    output.stop();
    for (mut server, path) in servers {
        let _ = server.kill();
        let server = server.wait_with_output().unwrap();
        let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
        assert!(received && bytes > 500, "publisher {:?}, receiver {}", output.status(), String::from_utf8_lossy(&server.stderr));
        crate::audio::assert_audio_file(&path);
        let probe = crate::audio::ffprobe_command().args(["-v", "error", "-select_streams", "a:0",
            "-show_entries", "stream=sample_rate", "-of", "csv=p=0"]).arg(&path).output().unwrap();
        assert!(probe.status.success());
        assert_eq!(String::from_utf8_lossy(&probe.stdout).trim(), if youtube { "44100" } else { "48000" });
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn stream_diagnostics_redact_destinations_and_keys() {
    let line = "Connection failed: rtmps://user:pw@example.test/live/secret (secret)";
    let cleaned = redact(line, &["secret".to_owned()]);
    assert_eq!(cleaned, "Connection failed: [destination hidden] ([hidden])");
    let dest = Destination { url: "rtmps://a.rtmps.youtube.com/live2".into(), ..Default::default() };
    assert_eq!(dest.audio_rate(), 44_100);
}
