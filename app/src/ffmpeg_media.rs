//! FFmpeg playback and the shared recording worker.
use crate::media_common::{Frame, RecFrame, RecOpts, Shared, rate_fraction};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn ffprobe() -> Command {
    crate::audio::ffprobe_command()
}

fn video_metadata(path: &str) -> (f64, u64) {
    let data = ffprobe().args(["-v", "error", "-select_streams", "v:0", "-show_entries",
        "stream=avg_frame_rate:format=duration", "-of", "json", path]).output().ok()
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok());
    let Some(data) = data else { return (30.0, 0) };
    let rate = data["streams"][0]["avg_frame_rate"].as_str().and_then(|s| {
        let (n, d) = s.split_once('/')?;
        let fps = n.parse::<f64>().ok()? / d.parse::<f64>().ok()?;
        (fps.is_finite() && fps > 0.0).then_some(fps)
    }).unwrap_or(30.0);
    let duration = data["format"]["duration"].as_str().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
    (rate, (duration * 1000.0) as u64)
}

pub(crate) fn play(shared: &Shared, path: &str, deinterlace: u8) {
    let (fps, duration) = video_metadata(path);
    shared.dur_ms.store(duration, Relaxed);
    let mut position = 0;
    while !shared.stop.load(Relaxed) {
        crate::audio::reset_position(shared);
        let mut cmd = crate::stream::command();
        cmd.args(["-re", "-ss", &format!("{:.3}", position as f64 / 1000.0), "-i", path]);
        if deinterlace != 0 {
            cmd.args(["-vf", if deinterlace == 1 { "bwdif=mode=send_frame:deint=interlaced" } else { "yadif=mode=send_frame:deint=interlaced" }]);
        }
        let mut count = 0_u64;
        let read = read_video(shared, cmd, |frame| {
            while count > 0 && shared.paused.load(Relaxed)
                && !shared.stop.load(Relaxed) && !shared.restart.load(Relaxed) && shared.seek_ms.load(Relaxed) == 0
            {
                std::thread::sleep(Duration::from_millis(15));
            }
            if shared.stop.load(Relaxed) || shared.restart.load(Relaxed) || shared.seek_ms.load(Relaxed) != 0 {
                return false;
            }
            publish(shared, frame);
            let ms = position + (count as f64 * 1000.0 / fps) as u64;
            shared.pos_ms.store(ms, Relaxed);
            crate::audio::mark_position(shared, ms);
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
        if !read {
            shared.set_status("FFmpeg could not decode this video. Check the file and FFmpeg installation.");
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

pub(crate) fn read_video(shared: &Shared, mut cmd: Command, mut consume: impl FnMut(Frame) -> bool) -> bool {
    let is_file = cmd.get_args().any(|arg| arg == "-re");
    let Ok(mut child) = cmd.args(["-map", "0:v:0", "-an", "-threads", "1", "-c:v", "bmp", "-pix_fmt", "bgra",
        "-f", "image2pipe", "pipe:1"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()
        else { return false };
    let mut pipe = BufReader::new(child.stdout.take().unwrap());
    let child = Mutex::new(child);
    let done = AtomicBool::new(false);
    let mut read = false;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Relaxed) && !shared.stop.load(Relaxed)
                && !(is_file && (shared.restart.load(Relaxed) || shared.seek_ms.load(Relaxed) != 0)) {
                std::thread::sleep(Duration::from_millis(30));
            }
            let mut child = child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        });
        while !shared.stop.load(Relaxed) {
            let Some(frame) = crate::srt::read_frame(&mut pipe) else { break };
            if !read { shared.set_status(""); }
            if is_file {
                if !consume(frame) { break; }
            } else {
                publish(shared, frame);
            }
            read = true;
        }
        done.store(true, Relaxed);
    });
    read
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

pub(crate) fn record(path: &str, rx: Receiver<RecFrame>, status: &Mutex<String>, opts: RecOpts) -> Result<(), String> {
    let Ok(first) = rx.recv() else { return Ok(()) };
    let profile = opts.format.ffmpeg_profile(opts.hw);
    let encoder = profile.video;
    let (n, d) = rate_fraction(opts.fps);
    let size = opts.size.unwrap_or((first.w as u32, first.h as u32));
    let mut cmd = crate::stream::command();
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y", "-f", "matroska", "-i", "pipe:0",
        "-map", "0:v:0", "-vf", &format!("scale={}:{}", size.0, size.1), "-pix_fmt", profile.pixel, "-c:v", encoder]);
    if profile.bitrate { cmd.args(["-b:v", &format!("{}M", opts.mbps.clamp(1, 100))]); }
    if opts.audio {
        cmd.args(["-map", "0:a:0", "-c:a", profile.audio]);
        if !profile.audio.starts_with("pcm") { cmd.args(["-b:a", "192k"]); }
    } else { cmd.arg("-an"); }
    if encoder.ends_with("_videotoolbox") { cmd.args(["-allow_sw", "1"]); }
    cmd.args(profile.extra);
    let mut child = cmd.args(["-f", profile.container]).arg(path)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn()
        .map_err(|e| format!("Could not start FFmpeg: {e}"))?;
    let stderr = child.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        BufReader::new(stderr).lines().map_while(Result::ok)
            .filter(|line| !line.trim().is_empty()).last().unwrap_or_default()
    });
    *status.lock().unwrap() = format!("FFmpeg · {encoder}");
    let write_result = (|| {
        let start = first.timeline_origin.unwrap_or(first.captured_at);
        let dimensions = (first.w, first.h, first.bgra);
        let mut pipe = crate::audio::AvPipe::new(child.stdin.take().unwrap(), first.w as u32, first.h as u32, n, d)?;
        let mut next = 0_u64;
        let mut previous: Option<Arc<Vec<u8>>> = None;
        for frame in std::iter::once(first).chain(rx) {
            if (frame.w, frame.h, frame.bgra) != dimensions { continue; }
            let slot = (frame.captured_at.saturating_duration_since(start).as_secs_f64()
                * f64::from(n) / f64::from(d)).round() as u64;
            if slot < next { continue; }
            let mut pixels = frame.pixels;
            if !frame.bgra {
                for pixel in Arc::make_mut(&mut pixels).chunks_exact_mut(4) { pixel.swap(0, 2); }
            }
            if let Some(previous) = &previous {
                for _ in 0..(slot - next).min(u64::from(n / d).max(1)) { pipe.frame(previous, &[])?; }
            }
            pipe.frame(&pixels, &frame.audio)?;
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
                Arc::default(), opts, Arc::default());
            let error = error.lock().unwrap().clone();
            assert!(error.is_empty(), "{error}");
        } else {
            record(path.to_str().unwrap(), rx, &Mutex::new(String::new()), opts)
                .unwrap_or_else(|error| panic!("{format:?}: {error}"));
        }
        #[cfg(not(windows))]
        record(path.to_str().unwrap(), rx, &Mutex::new(String::new()), opts)
            .unwrap_or_else(|error| panic!("{format:?}: {error}"));
        let probe = ffprobe().args(["-v", "error", "-show_entries", "stream=codec_type",
            "-of", "csv=p=0"]).arg(&path).output().unwrap();
        assert!(probe.status.success(), "{format:?}: {:?}", probe.stderr);
        let types = String::from_utf8(probe.stdout).unwrap();
        assert!(types.lines().any(|kind| kind.trim_end_matches(',') == "video"), "{format:?}: {types}");
        assert_eq!(types.lines().any(|kind| kind.trim_end_matches(',') == "audio"), with_audio, "{format:?}: {types}");
        if with_audio { crate::audio::assert_audio_file(&path); }
        if format == RecordFormat::H264 {
            let source = crate::media::start_with_pause(
                crate::media::Source::FfmpegFile(path.to_str().unwrap().to_owned(), 1), true);
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
