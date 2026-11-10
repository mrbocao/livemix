use crate::media::{Frame, Shared};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Read};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub listen: bool,
    pub host: String,
    pub port: u16,
    pub latency_ms: u32,
    pub passphrase: String,
    pub key_bytes: u8,
    pub stream_id: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub connection: Connection,
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub gop_seconds: u32,
    pub hardware: bool,
    pub hevc: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { connection: Connection { host: "127.0.0.1".into(), port: 9000, latency_ms: 120,
            key_bytes: 16, ..Default::default() }, video_kbps: 6000, audio_kbps: 128,
            gop_seconds: 2, hardware: false, hevc: false }
    }
}

// Exclude network identifiers and passphrases from debug output.
impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SrtSettings").field("video_kbps", &self.video_kbps)
            .field("hevc", &self.hevc).finish_non_exhaustive()
    }
}

impl Connection {
    pub fn label(&self) -> String {
        if self.listen { format!("SRT Listener :{}", self.port) } else { format!("SRT {}:{}", self.host, self.port) }
    }

    pub fn url(&self) -> Result<String, String> {
        let host = if self.listen { "0.0.0.0" } else { self.host.trim() };
        let host = if let Ok(ip) = host.trim_matches(['[', ']']).parse::<std::net::Ipv6Addr>() {
            format!("[{ip}]")
        } else if !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_')) {
            host.into()
        } else {
            return Err("Enter a valid SRT host".into());
        };
        if self.port == 0 { return Err("Choose an SRT port from 1 to 65535".into()); }
        let mode = if self.listen { "listener" } else { "caller" };
        let latency = self.latency_ms.clamp(20, 8000) * 1000;
        let mut url = format!("srt://{host}:{}?mode={mode}&latency={latency}&pkt_size=1316&transtype=live", self.port);
        if !self.passphrase.is_empty() {
            if !(10..=79).contains(&self.passphrase.len()) || self.passphrase.as_bytes().contains(&0) {
                return Err("The SRT passphrase must be 10 to 79 bytes".into());
            }
            let bytes = if matches!(self.key_bytes, 16 | 24 | 32) { self.key_bytes } else { 16 };
            url.push_str(&format!("&pbkeylen={bytes}&passphrase={}", url_value(&self.passphrase)));
        }
        if !self.listen && !self.stream_id.is_empty() {
            if self.stream_id.len() > 512 || self.stream_id.as_bytes().contains(&0) {
                return Err("The SRT stream ID is too long or invalid".into());
            }
            url.push_str(&format!("&streamid={}", url_value(&self.stream_id)));
        }
        Ok(url)
    }
}

fn url_value(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            escaped.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(escaped, "%{byte:02X}");
        }
    }
    escaped
}

pub fn check_ffmpeg() -> Result<(), String> {
    let available = crate::stream::command().arg("-protocols").stdin(Stdio::null())
        .stderr(Stdio::null()).output().is_ok_and(|out| {
            out.status.success() && String::from_utf8_lossy(&out.stdout).lines().any(|line| line.trim() == "srt")
        });
    available.then_some(()).ok_or_else(|| "SRT requires FFmpeg with libsrt. Set LIVEMIX_FFMPEG to a compatible ffmpeg.exe.".into())
}

pub fn run_capture(shared: Arc<Shared>, config: Connection) {
    let result = config.url().and_then(|url| {
        check_ffmpeg()?;
        while !shared.stop.load(Relaxed) {
            *shared.status.lock().unwrap() = if config.listen { "Waiting for SRT sender…" } else { "Connecting to SRT…" }.into();
            receive_once(&shared, &url)?;
            if !shared.stop.load(Relaxed) {
                *shared.status.lock().unwrap() = "SRT signal lost. Reconnecting…".into();
                std::thread::sleep(Duration::from_millis(750));
            }
        }
        Ok(())
    });
    if let Err(error) = result { *shared.status.lock().unwrap() = error; }
}

fn receive_once(shared: &Shared, url: &str) -> Result<(), String> {
    let audio_socket = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
    audio_socket.set_nonblocking(true).map_err(|e| e.to_string())?;
    let audio_address = audio_socket.local_addr().map_err(|e| e.to_string())?;
    // A single demuxer keeps SRT video and audio on the same connection. The tee
    // leaves the video pipe running when a sender has no audio track.
    let outputs = format!("[select=v:f=image2pipe]pipe:1|[select=a:f=s16le:onfail=ignore]tcp://{audio_address}");
    let mut child = crate::stream::command().args(["-hide_banner", "-loglevel", "error", "-nostdin",
        "-rw_timeout", "5000000", "-analyzeduration", "750000", "-probesize", "262144", "-f", "mpegts", "-i", url,
        "-map", "0:v:0", "-map", "0:a:0?", "-threads:v", "1", "-c:v", "bmp", "-pix_fmt", "bgra",
        "-c:a", "pcm_s16le", "-ac", "2", "-ar", "48000", "-f", "tee", &outputs])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
        .map_err(|e| format!("Could not start the SRT receiver: {e}"))?;
    let mut pipe = BufReader::new(child.stdout.take().unwrap());
    let child = Mutex::new(child);
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            use std::io::ErrorKind;
            let socket = loop {
                if done.load(Relaxed) || shared.stop.load(Relaxed) { return; }
                match audio_socket.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(_) => return,
                }
            };
            if socket.set_read_timeout(Some(Duration::from_millis(100))).is_err() { return; }
            let mut pipe = BufReader::new(socket);
            let mut block = [0; 480 * 4];
            let mut filled = 0;
            while !done.load(Relaxed) && !shared.stop.load(Relaxed) {
                match pipe.read(&mut block[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => continue,
                    Err(_) => break,
                }
                if filled == block.len() {
                    let samples: Vec<_> = crate::audio::pcm(&block).collect();
                    shared.audio.lock().unwrap().capture(&samples);
                    filled = 0;
                }
            }
        });
        scope.spawn(|| {
            while !done.load(Relaxed) && !shared.stop.load(Relaxed) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let mut child = child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        });
        let mut receiving = false;
        while !shared.stop.load(Relaxed) {
            let Some(frame) = read_frame(&mut pipe) else { break };
            *shared.size.lock().unwrap() = (frame.w, frame.h);
            *shared.frame.lock().unwrap() = Some(frame);
            shared.seq.fetch_add(1, Relaxed);
            if !receiving {
                *shared.status.lock().unwrap() = "Receiving SRT video".into();
                receiving = true;
            }
        }
        done.store(true, Relaxed);
    });
    Ok(())
}

// Image2pipe preserves the dimensions of each frame, including changes after reconnect.
pub(crate) fn read_frame(pipe: &mut impl Read) -> Option<Frame> {
    let mut header = [0; 26];
    pipe.read_exact(&mut header).ok()?;
    if &header[..2] != b"BM" { return None; }
    let size = u32::from_le_bytes(header[2..6].try_into().ok()?) as usize;
    let width = i32::from_le_bytes(header[18..22].try_into().ok()?);
    let height = i32::from_le_bytes(header[22..26].try_into().ok()?).unsigned_abs();
    if !(1..=8192).contains(&width) || !(1..=8192).contains(&height)
        || !(header.len()..=268_435_456).contains(&size) { return None; }
    let mut bytes = vec![0; size];
    bytes[..header.len()].copy_from_slice(&header);
    pipe.read_exact(&mut bytes[header.len()..]).ok()?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Bmp).ok()?.into_rgba8();
    let (w, h) = image.dimensions();
    Some(Frame { w: w as usize, h: h as usize, rgba: image.into_raw() })
}

#[test]
#[ignore = "requires FFmpeg with libsrt and an available local UDP port"]
fn srt_video_loopback() {
    use crate::program::NetworkFrame;
    use std::time::Instant;
    for (hevc, listen_on_output) in [(false, false), (true, true)] {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let mut config = Settings::default();
        config.hevc = hevc;
        config.connection.listen = listen_on_output;
        config.connection.port = port;
        config.connection.passphrase = "test SRT&2026".into();
        config.connection.key_bytes = 32;
        let mut receiver = config.connection.clone();
        receiver.listen = !listen_on_output;
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        let worker = std::thread::spawn(move || run_capture(state, receiver));
        std::thread::sleep(Duration::from_millis(400));
        let output = crate::stream::Output::start_srt(&config, [320, 180], 25.0).unwrap();
        let frame = NetworkFrame { pixels: Arc::new([20, 55, 180, 255].repeat(320 * 180)), width: 320, height: 180 };
        let deadline = Instant::now() + Duration::from_secs(12);
        let packet = crate::audio::test_tone(1920);
        let mut level = 0.0;
        while (shared.seq.load(Relaxed) == 0 || level < 0.05) && Instant::now() < deadline && !output.status().1 {
            output.submit_audio(frame.clone(), packet.clone(), Instant::now());
            std::thread::sleep(Duration::from_millis(40));
            level = crate::audio::input_peak((&shared, false, 1.0), Instant::now());
        }
        shared.stop.store(true, Relaxed);
        output.stop();
        worker.join().unwrap();
        let frame = shared.frame.lock().unwrap();
        let frame = frame.as_ref().unwrap_or_else(|| panic!("HEVC {hevc}, receiver {}, sender {:?}",
            shared.status.lock().unwrap(), output.status()));
        assert_eq!((frame.w, frame.h), (320, 180));
        assert!(level >= 0.05, "SRT stereo audio was not received");
        assert!(frame.rgba[0] > 145 && frame.rgba[2] < 55, "color {:?}", &frame.rgba[..4]);
    }
}
