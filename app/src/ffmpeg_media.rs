//! FFmpeg playback and the shared recording worker.
use crate::media_common::{Frame, RecFrame, RecOpts, Shared, rate_fraction};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(windows)]
const GPU_DECODER: (&str, &str, &str) = ("d3d11va", "d3d11", "D3D11VA");
#[cfg(target_os = "macos")]
const GPU_DECODER: (&str, &str, &str) = ("videotoolbox", "videotoolbox_vld", "VideoToolbox");

#[derive(Clone, Copy)]
struct VideoMetadata {
    fps: f64,
    duration_ms: u64,
    download: Option<&'static str>,
}

fn video_metadata(path: &str) -> Option<VideoMetadata> {
    let mut probe = crate::audio::ffprobe_command().args(["-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=codec_name,width,height,pix_fmt,avg_frame_rate:format=duration",
        "-of", "json", "-i", path])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match probe.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            _ => { let _ = probe.kill(); let _ = probe.wait(); return None; }
        }
    }
    let output = probe.wait_with_output().ok()?;
    parse_video_metadata(&output.stdout)
}

fn parse_video_metadata(bytes: &[u8]) -> Option<VideoMetadata> {
    let data: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let stream = &data["streams"][0];
    let fps = stream["avg_frame_rate"].as_str().and_then(|s| {
        let (n, d) = s.split_once('/')?;
        let fps = n.parse::<f64>().ok()? / d.parse::<f64>().ok()?;
        (fps.is_finite() && (1.0..=240.0).contains(&fps)).then_some(fps)
    }).unwrap_or(30.0);
    let duration_ms = (data["format"]["duration"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0) * 1000.0) as u64;
    let pixels = stream["width"].as_u64().unwrap_or(0).saturating_mul(stream["height"].as_u64().unwrap_or(0));
    let heavy = pixels > 1920 * 1080 || pixels as f64 * fps > f64::from(1920 * 1080 * 60);
    let supported = matches!(stream["codec_name"].as_str(), Some("h264" | "hevc" | "av1" | "vp9" | "mpeg2video" | "vc1"));
    let download = if heavy && supported {
        match stream["pix_fmt"].as_str() {
            Some("yuv420p" | "yuvj420p" | "nv12") => Some("nv12"),
            Some("yuv420p10le" | "p010le") => Some("p010le"),
            _ => None,
        }
    } else { None };
    Some(VideoMetadata { fps, duration_ms, download })
}

#[cfg(target_os = "macos")]
pub(crate) fn play(shared: &Shared, path: &str, deinterlace: u8) {
    play_configured(shared, path, deinterlace, false, None);
}

pub(crate) fn play_configured(shared: &Shared, path: &str, deinterlace: u8, hardware: bool, fallback: Option<&str>) {
    let (meta_tx, meta_rx) = std::sync::mpsc::sync_channel(1);
    let probe_path = path.to_owned();
    std::thread::spawn(move || { let _ = meta_tx.send(video_metadata(&probe_path)); });
    let (hwaccel, hw_format, hw_name) = GPU_DECODER;
    let mut metadata = None;
    let mut gpu_failed = false;
    let mut position = 0;
    while !shared.stop.load(Relaxed) {
        if let Ok(Some(info)) = meta_rx.try_recv() {
            shared.dur_ms.store(info.duration_ms, Relaxed);
            metadata = Some(info);
        }
        let gpu = metadata.and_then(|info| info.download).filter(|_| hardware && !gpu_failed);
        crate::audio::reset_position(shared);
        let mut cmd = crate::stream::command();
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if gpu.is_some() {
            cmd.args(["-xerror", "-hwaccel", hwaccel, "-hwaccel_output_format", hw_format]);
        }
        cmd.args(["-re", "-ss", &format!("{:.3}", position as f64 / 1000.0), "-i", path]);
        let mut filters = Vec::new();
        if let Some(format) = gpu {
            filters.push("hwdownload");
            filters.push(if format == "p010le" { "format=p010le" } else { "format=nv12" });
        }
        match deinterlace {
            1 => filters.push("bwdif=mode=send_frame:deint=interlaced"),
            2 => filters.push("yadif=mode=send_frame:deint=interlaced"),
            _ => {}
        }
        if !filters.is_empty() { cmd.args(["-vf", &filters.join(",")]); }
        let mut count = 0;
        let mut next_position = position as f64;
        let mut upgrade = false;
        let report = read_video_attempt(shared, cmd, gpu.is_some(), |frame| {
            loop {
                if let Ok(Some(info)) = meta_rx.try_recv() {
                    shared.dur_ms.store(info.duration_ms, Relaxed);
                    metadata = Some(info);
                }
                if shared.stop.load(Relaxed) || shared.restart.load(Relaxed) || shared.seek_ms.load(Relaxed) != 0 {
                    return false;
                }
                if count > 0 && gpu.is_none() && hardware && !gpu_failed
                    && metadata.is_some_and(|info| info.download.is_some())
                {
                    upgrade = true;
                    return false;
                }
                if count == 0 || !shared.paused.load(Relaxed) { break; }
                std::thread::sleep(Duration::from_millis(15));
            }
            if count == 0 {
                let method = match deinterlace { 1 => " · BWDIF", 2 => " · YADIF", _ => "" };
                let mut decoder = if gpu.is_some() {
                    format!("FFmpeg · {hw_name} · system-memory output{method}")
                } else {
                    format!("FFmpeg · software{method}")
                };
                if gpu_failed { decoder.push_str(&format!(" · {hw_name} fallback")); }
                if let Some(reason) = fallback { decoder.push_str(&format!(" · fallback: {reason}")); }
                *shared.decoder.lock().unwrap() = decoder;
            }
            let ms = next_position as u64;
            shared.pos_ms.store(ms, Relaxed);
            crate::audio::mark_position(shared, next_position);
            publish(shared, frame);
            next_position += 1000.0 / metadata.map_or(30.0, |info| info.fps);
            count += 1;
            true
        });
        if shared.stop.load(Relaxed) { break; }
        let seek = shared.seek_ms.swap(0, Relaxed);
        let restart = shared.restart.swap(false, Relaxed);
        if seek > 0 || restart {
            shared.ended.store(false, Relaxed);
            position = seek.saturating_sub(1);
            continue;
        }
        let resume = || if shared.paused.load(Relaxed) { shared.pos_ms.load(Relaxed) } else { next_position as u64 };
        if upgrade {
            position = resume();
            continue;
        }
        let duration = shared.dur_ms.load(Relaxed);
        let near_end = count > 0 && duration > 0 && next_position as u64 + 120 >= duration;
        if gpu.is_some() && (count == 0 || (!report.completed && !near_end)) {
            gpu_failed = true;
            position = if count > 0 { resume() } else { position };
            continue;
        }
        if count == 0 || (!report.completed && !near_end) {
            let reason = fallback.map_or_else(String::new, |reason| format!("{reason}. "));
            shared.set_status(format!("{reason}FFmpeg could not decode this video. Check the file and FFmpeg installation."));
            break;
        }
        if shared.looping.load(Relaxed) && !shared.paused.load(Relaxed) {
            position = 0;
            continue;
        }
        shared.ended.store(true, Relaxed);
        while !shared.stop.load(Relaxed) && !shared.restart.load(Relaxed) && shared.seek_ms.load(Relaxed) == 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
        shared.ended.store(false, Relaxed);
        shared.restart.store(false, Relaxed);
        position = shared.seek_ms.swap(0, Relaxed).saturating_sub(1);
    }
}

struct ReadResult {
    completed: bool,
}

#[cfg(target_os = "macos")]
pub(crate) fn read_video(shared: &Shared, cmd: Command) -> bool {
    let mut shown = false;
    read_video_attempt(shared, cmd, false, |frame| {
        publish(shared, frame);
        shown = true;
        true
    });
    shown
}

fn read_video_attempt(shared: &Shared, mut cmd: Command, watchdog: bool, mut consume: impl FnMut(Frame) -> bool) -> ReadResult {
    use std::sync::atomic::AtomicU64;
    let is_file = cmd.get_args().any(|arg| arg == "-re");
    let Ok(mut child) = cmd.args(["-map", "0:v:0", "-an", "-threads", "1", "-c:v", "bmp", "-pix_fmt", "bgra",
        "-f", "image2pipe", "pipe:1"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
        else { return ReadResult { completed: false }; };
    let mut pipe = BufReader::new(child.stdout.take().unwrap());
    let epoch = std::time::Instant::now();
    let activity = AtomicU64::new(0);
    let interrupted = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    let completed = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            loop {
                if let Ok(Some(status)) = child.try_wait() {
                    completed.store(status.success(), Relaxed);
                    return;
                }
                let now = epoch.elapsed().as_millis() as u64;
                let last = activity.load(Relaxed);
                let transport_changed = is_file && (shared.restart.load(Relaxed) || shared.seek_ms.load(Relaxed) != 0);
                let stalled = watchdog && (last == 0 || !shared.paused.load(Relaxed)) && now.saturating_sub(last) >= 5_000;
                let eof_stall = done.load(Relaxed) && now.saturating_sub(last) >= 500;
                if shared.stop.load(Relaxed) || interrupted.load(Relaxed) || transport_changed || stalled || eof_stall {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
        });
        while !shared.stop.load(Relaxed) {
            let Some(frame) = crate::srt::read_frame(&mut pipe) else { break };
            if activity.load(Relaxed) == 0 { shared.set_status(""); }
            activity.store(1 + epoch.elapsed().as_millis() as u64, Relaxed);
            if !consume(frame) {
                interrupted.store(true, Relaxed);
                break;
            }
        }
        activity.store(1 + epoch.elapsed().as_millis() as u64, Relaxed);
        done.store(true, Relaxed);
    });
    ReadResult { completed: completed.load(Relaxed) }
}
pub(crate) fn publish(shared: &Shared, mut frame: Frame) {
    let key = shared.key.load(Relaxed);
    if key != 0 {
        let rgb = [(key >> 16) as u8, (key >> 8) as u8, key as u8];
        let threshold = ((key >> 24) as u8) as f32 * 1.7;
        for pixel in frame.rgba.chunks_exact_mut(4) {
            let squared_distance: i32 = pixel[..3].iter().zip(rgb)
                .map(|(&a, b)| (i32::from(a) - i32::from(b)).pow(2)).sum();
            let alpha = ((squared_distance as f32).sqrt() - threshold) / 40.0;
            pixel[3] = (alpha.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    *shared.size.lock().unwrap() = (frame.w, frame.h);
    *shared.frame.lock().unwrap() = Some(frame);
    shared.seq.fetch_add(1, Relaxed);
}

pub(crate) fn record(path: &str, rx: Receiver<RecFrame>, status: &Mutex<String>, opts: RecOpts, index: Option<&crate::media_common::ReplayIndex>) -> Result<(), String> {
    let Ok(first) = rx.recv() else { return Ok(()) };
    let profile = opts.format.ffmpeg_profile();
    let (n, d) = rate_fraction(opts.fps);
    let size = opts.size.unwrap_or((first.w as u32, first.h as u32));
    let bitrate = format!("{}M", opts.mbps.clamp(1, 100));
    let hardware_requested = opts.hw && matches!(profile.video, "libx264" | "libx265");
    let encoder = if hardware_requested {
        crate::stream::hardware_encoder(profile.video == "libx265", [size.0, size.1], (n, d),
            &["-b:v", &bitrate], || false).unwrap_or(profile.video)
    } else { profile.video };
    let mut cmd = crate::stream::command();
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-f", "matroska", "-i", "pipe:0",
        "-map", "0:v:0", "-vf", &format!("scale={}:{}", size.0, size.1), "-pix_fmt", profile.pixel, "-c:v", encoder]);
    if profile.bitrate { cmd.args(["-b:v", &bitrate]); }
    if opts.audio {
        cmd.args(["-map", "0:a:0", "-c:a", profile.audio]);
        if !profile.audio.starts_with("pcm") { cmd.args(["-b:a", "192k"]); }
    } else { cmd.arg("-an"); }
    crate::stream::hardware_only(&mut cmd, encoder);
    cmd.args(profile.extra);
    let mut child = cmd.args(["-f", profile.container]).arg(path)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;
    let stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        BufReader::new(stderr).lines().map_while(Result::ok)
            .filter(|line| !line.trim().is_empty()).last().unwrap_or_default()
    });
    *status.lock().unwrap() = if hardware_requested && encoder == profile.video {
        format!("FFmpeg · {encoder} · hardware unavailable for this format")
    } else { format!("FFmpeg · {encoder}") };
    let write_result = (|| {
        let start = first.timeline_origin.unwrap_or(first.captured_at);
        let dimensions = (first.w, first.h, first.bgra);
        let mut pipe = crate::audio::AvPipe::new(child.stdin.take().unwrap(), first.w as u32, first.h as u32, n, d)?;
        let mut next = 0_u64;
        let mut previous: Option<Arc<Vec<u8>>> = None;
        let mut written = 0;
        for frame in std::iter::once(first).chain(rx) {
            if (frame.w, frame.h, frame.bgra) != dimensions { continue; }
            let slot = (frame.captured_at.saturating_duration_since(start).as_secs_f64()
                * f64::from(n) / f64::from(d)).round() as u64;
            // A late frame's audio also covers the repeated pictures written before it.
            pipe.queue_audio(&frame.audio);
            if slot < next { continue; }
            let mut pixels = frame.pixels;
            if !frame.bgra {
                for pixel in Arc::make_mut(&mut pixels).chunks_exact_mut(4) { pixel.swap(0, 2); }
            }
            if let Some(previous) = &previous {
                let limit = u64::from(n / d).max(1) * if index.is_some() { 12 } else { 1 };
                for _ in 0..(slot - next).min(limit) {
                    pipe.frame(previous, &[])?;
                    written += 1;
                }
            }
            pipe.frame(&pixels, &[])?;
            if let Some(index) = index {
                index.lock().unwrap().push((frame.captured_at, written * 1000 * u64::from(d) / u64::from(n)));
            }
            written += 1;
            previous = Some(pixels);
            next = slot + 1;
        }
        Ok::<(), std::io::Error>(())
    })();
    let exit = child.wait().map_err(|e| e.to_string())?;
    let detail = errors.join().unwrap_or_default();
    if !exit.success() {
        return Err(format!("FFmpeg {encoder} exited with {exit}. {detail}"));
    }
    write_result.map_err(|err| format!("Could not send recording frames to FFmpeg {encoder}: {err}"))
}

#[cfg(test)]
#[test]
#[ignore = "requires FFmpeg and FFprobe"]
fn recording_profiles_mux_video_and_stereo_audio() {
    use crate::media_common::RecordFormat;
    use std::{path::PathBuf, time::Instant};

    let start = Instant::now();
    let pixels = Arc::new(vec![128; 320 * 180 * 4]);
    let audio = crate::audio::test_tone(1600);
    let write = |format: RecordFormat, with_audio| {
        let path: PathBuf = std::env::temp_dir().join(format!(
            "livemix-codec-{}-{}-{with_audio}.{}", std::process::id(), format.key(), format.extension(),
        ));
        let (tx, rx) = std::sync::mpsc::channel();
        for index in 0..15 {
            tx.send(RecFrame {
                #[cfg(windows)]
                surface: None,
                audio: audio.clone(),
                pixels: pixels.clone(),
                w: 320,
                h: 180,
                captured_at: start + Duration::from_secs_f64(f64::from(index) / 30.0),
                timeline_origin: Some(start),
                bgra: true,
            }).unwrap();
        }
        drop(tx);
        let opts = RecOpts { hw: false, format, audio: with_audio, ..RecOpts::default() };
        #[cfg(windows)]
        if format == RecordFormat::Wmv9 {
            let error = Arc::new(Mutex::new(String::new()));
            crate::media::record_thread(path.to_str().unwrap().to_owned(), rx, error.clone(),
                Arc::default(), opts, None, Arc::default());
            let error = error.lock().unwrap().clone();
            assert!(error.is_empty(), "{error}");
        } else {
            record(path.to_str().unwrap(), rx, &Mutex::new(String::new()), opts, None)
                .unwrap_or_else(|error| panic!("{format:?}: {error}"));
        }
        #[cfg(not(windows))]
        record(path.to_str().unwrap(), rx, &Mutex::new(String::new()), opts)
            .unwrap_or_else(|error| panic!("{format:?}: {error}"));
        let probe = crate::audio::ffprobe_command().args(["-v", "error", "-show_entries", "stream=codec_type",
            "-of", "csv=p=0"]).arg(&path).output().unwrap();
        assert!(probe.status.success(), "{format:?}: {:?}", probe.stderr);
        let types = String::from_utf8(probe.stdout).unwrap();
        assert!(types.lines().any(|kind| kind.trim_end_matches(',') == "video"), "{format:?}: {types}");
        assert_eq!(types.lines().any(|kind| kind.trim_end_matches(',') == "audio"), with_audio, "{format:?}: {types}");
        if with_audio { crate::audio::assert_audio_file(&path); }
        if format == RecordFormat::H264 {
            let source = crate::media::start_with_pause(
                crate::media::Source::FfmpegFile(path.to_str().unwrap().to_owned(), 1, false), true);
            let deadline = Instant::now() + Duration::from_secs(5);
            while source.seq.load(Relaxed) == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            let dimensions = *source.size.lock().unwrap();
            source.stop.store(true, Relaxed);
            assert_eq!(dimensions, (320, 180), "{}", source.status.lock().unwrap());
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match std::fs::remove_file(&path) {
                Ok(()) => break,
                Err(error) if (error.kind() == std::io::ErrorKind::PermissionDenied
                    || (cfg!(windows) && error.raw_os_error() == Some(32))) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("Could not remove {}: {error}", path.display()),
            }
        }
    };
    for format in RecordFormat::ALL.into_iter().filter(|format| cfg!(windows) || *format != RecordFormat::Wmv9) {
        write(format, true);
    }
    write(RecordFormat::H264Mkv, false);
}


#[test]
fn high_resolution_decode_requires_a_supported_codec_and_pixel_format() {
    for (codec, width, height, pixel, expected) in [
        ("h264", 1920, 1080, "yuv420p", None),
        ("hevc", 3840, 2160, "yuv420p10le", Some("p010le")),
        ("h264", 2560, 1440, "yuv420p", Some("nv12")),
        ("h264", 2560, 1440, "yuv444p", None),
        ("prores", 3840, 2160, "yuv420p", None),
    ] {
        let probe = serde_json::json!({ "streams": [{
            "codec_name": codec, "width": width, "height": height, "pix_fmt": pixel,
            "avg_frame_rate": "60000/1001"
        }], "format": { "duration": "2.000" } });
        let info = parse_video_metadata(probe.to_string().as_bytes()).unwrap();
        assert_eq!(info.download, expected);
        assert_eq!(info.duration_ms, 2000);
    }
}

#[cfg(windows)]
#[test]
#[ignore = "requires FFmpeg, FFprobe and a D3D11VA decoder"]
fn high_resolution_ffmpeg_plays_and_seeks_with_d3d11va() {
    let path = std::env::temp_dir().join(format!("livemix-d3d11va-{}.mp4", std::process::id()));
    let created = crate::stream::command().args(["-y", "-v", "error", "-f", "lavfi", "-i",
        "color=blue:size=2560x1440:rate=25", "-t", "3", "-c:v", "libx264", "-preset", "ultrafast",
        "-pix_fmt", "yuv420p"]).arg(&path).output().unwrap();
    assert!(created.status.success(), "{}", String::from_utf8_lossy(&created.stderr));
    let shared = crate::media::start_for_gl(crate::media::Source::FfmpegFile(path.to_string_lossy().into_owned(), 0, true), true);
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while std::time::Instant::now() < deadline && !shared.decoder.lock().unwrap().starts_with("FFmpeg · D3D11VA") {
        std::thread::sleep(Duration::from_millis(30));
    }
    let decoder = shared.decoder.lock().unwrap().clone();
    if decoder.starts_with("FFmpeg · D3D11VA") {
        shared.seek_ms.store(1001, Relaxed);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while shared.pos_ms.load(Relaxed) != 1000 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    shared.stop.store(true, Relaxed);
    std::thread::sleep(Duration::from_millis(150));
    let _ = std::fs::remove_file(path);
    assert!(decoder.starts_with("FFmpeg · D3D11VA"), "{decoder} / {}", shared.status.lock().unwrap());
    assert_eq!(shared.pos_ms.load(Relaxed), 1000);
}
