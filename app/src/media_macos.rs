//! Video I/O using FFmpeg's AVFoundation and VideoToolbox backends on macOS.
pub use crate::media_common::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
pub(crate) use crate::ffmpeg_media::publish;
use crate::ffmpeg_media::read_video;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
#[cfg(test)]
use std::time::Duration;

pub fn blackmagic_driver_installed() -> bool {
    !crate::decklink::devices_cached().is_empty()
        || Path::new("/Library/Frameworks/DeckLinkAPI.framework").exists()
        || Path::new("/Library/Application Support/Blackmagic Design/Blackmagic DeckLink/DeckLinkAPI.bundle").exists()
}

fn avfoundation_devices() -> Vec<(String, String)> {
    let Ok(list) = crate::stream::command().args(["-hide_banner", "-f", "avfoundation", "-list_devices", "true", "-i", ""])
        .stdin(Stdio::null()).output() else { return Vec::new() };
    let output = String::from_utf8_lossy(&list.stderr);
    output.lines().skip_while(|line| !line.contains("video devices:"))
        .take_while(|line| !line.contains("audio devices:"))
        .filter_map(|line| {
            let (index, name) = line.rsplit_once('[')?.1.split_once(']')?;
            index.parse::<usize>().ok()?;
            Some((index.to_owned(), name.trim().to_owned()))
        }).collect()
}

pub fn list_devices() -> Vec<DeviceInfo> {
    avfoundation_devices().into_iter().filter(|(_, name)| !name.starts_with("Capture screen"))
        .map(|(index, name)| DeviceInfo { blackmagic: is_blackmagic(&name), name, link: format!("avfoundation:{index}") })
        .collect()
}

pub fn start_with_pause(src: Source, paused: bool) -> Arc<Shared> {
    let shared = Arc::new(Shared::default());
    shared.paused.store(paused, Relaxed);
    shared.set_status("Starting…");
    if let Source::File(path) | Source::FfmpegFile(path, _) = &src {
        let (source, path) = (shared.clone(), path.clone());
        std::thread::spawn(move || crate::audio::play_file(source, path));
    }
    let worker = shared.clone();
    std::thread::spawn(move || match src {
        Source::DeckLink(config) => crate::decklink::run_capture(worker, config),
        Source::Ndi(name) => crate::ndi::run_capture(worker, &name),
        Source::Omt(name) => crate::omt::run_capture(worker, &name),
        Source::Srt(config) => crate::srt::run_capture(worker, config),
        Source::AudioDevice(device) => crate::audio::capture_device(worker, &device),
        Source::AudioFile(path) => crate::audio::play_audio_input(worker, path),
        Source::Presentation(path) => crate::slides::run(worker, path),
        Source::Sequence(folder, fps) => run_pictures(worker, folder, true, fps, 0.0),
        Source::Photos(folder, secs) => run_pictures(worker, folder, false, 0.0, secs),
        Source::Desktop => {
            if let Some((index, _)) = avfoundation_devices().into_iter().find(|(_, name)| name.starts_with("Capture screen")) {
                capture(&worker, &index, None);
            } else {
                worker.set_status("No AVFoundation screen found. Check FFmpeg and Screen Recording permissions.");
            }
        }
        Source::Device(link, mode) => match link.strip_prefix("avfoundation:") {
            Some(index) if index.parse::<usize>().is_ok() => capture(&worker, index, mode),
            _ => worker.set_status("Select a camera available on this Mac"),
        },
        Source::File(path) => crate::ffmpeg_media::play(&worker, &path, 0),
        Source::FfmpegFile(path, deinterlace) => crate::ffmpeg_media::play(&worker, &path, deinterlace),
    });
    shared
}

pub fn start_for_gl(src: Source, paused: bool) -> Arc<Shared> {
    start_with_pause(src, paused)
}

fn capture(shared: &Shared, index: &str, mode: Option<CamMode>) {
    let rate = mode.as_ref().map_or_else(|| "30".into(), |m| format!("{}/{}", m.fps_n, m.fps_d.max(1)));
    let mut cmd = crate::stream::command();
    cmd.args(["-f", "avfoundation", "-framerate", &rate, "-pixel_format", "uyvy422"]);
    if let Some(mode) = mode {
        cmd.args(["-video_size", &format!("{}x{}", mode.w, mode.h)]);
    }
    cmd.args(["-i", &format!("{index}:none")]);
    let max_height = AUTO_CAM_MAX_H.load(Relaxed).max(240);
    cmd.args(["-vf", &format!("scale=-2:'min(ih,{max_height})'")]);
    if !read_video(shared, cmd, |_| true) && !shared.stop.load(Relaxed) {
        shared.set_status("Could not open AVFoundation video. Check the device, FFmpeg and macOS permissions.");
    }
}

pub(crate) fn record_thread(path: String, rx: Receiver<RecFrame>, error: Arc<Mutex<String>>,
    status: Arc<Mutex<String>>, opts: RecOpts, index: Option<ReplayIndex>)
{
    if let Err(message) = crate::ffmpeg_media::record(&path, rx, &status, opts, index.as_ref()) {
        *error.lock().unwrap() = message;
    }
}

pub fn storage_folder() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let folder = home.join("Documents/LiveMixStorage");
    let _ = std::fs::create_dir_all(&folder);
    folder
}

fn select_file(script: &str, argument: Option<&str>) -> Option<String> {
    let mut command = Command::new("/usr/bin/osascript");
    command.args(["-e", script]);
    if let Some(argument) = argument { command.args(["--", argument]); }
    let result = command.output().ok()?;
    result.status.success().then(|| String::from_utf8_lossy(&result.stdout).trim_end_matches(['\r', '\n']).to_owned())
        .filter(|p| !p.is_empty())
}

pub fn browse(kind: PickKind) -> Receiver<Option<String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let path = match kind {
            PickKind::Folder => select_file("POSIX path of (choose folder)", None),
            PickKind::SettingsSave => select_file(SAVE, Some("LiveMix-settings.json")).map(|p| {
                if p.to_ascii_lowercase().ends_with(".json") || p.to_ascii_lowercase().ends_with(".config") { p } else { format!("{p}.json") }
            }),
            PickKind::PresetSave => select_file(SAVE, Some("Untitled.vmix")).map(|p| {
                if p.to_ascii_lowercase().ends_with(".vmix") { p } else { format!("{p}.vmix") }
            }),
            PickKind::VideoMany => select_file(
                "set files to choose file with multiple selections allowed\nset paths to {}\nrepeat with f in files\nset end of paths to POSIX path of f\nend repeat\nset AppleScript's text item delimiters to linefeed\nreturn paths as text", None),
            _ => select_file("POSIX path of (choose file)", None),
        };
        let _ = tx.send(path);
    });
    rx
}

const SAVE: &str = "on run argv\nreturn POSIX path of (choose file name default name (item 1 of argv))\nend run";

pub fn save_picture_dialog(default_name: String, filter: u32) -> Receiver<Option<(String, u32)>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let extension = ["png", "jpg", "bmp"][filter.min(2) as usize];
        let name = format!("{default_name}.{extension}");
        let path = select_file(SAVE, Some(&name)).map(|path| {
            match Path::new(&path).extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
                Some("png") => (path, 0),
                Some("jpg" | "jpeg") => (path, 1),
                Some("bmp") => (path, 2),
                _ => (format!("{path}.{extension}"), filter.min(2)),
            }
        });
        let _ = tx.send(path);
    });
    rx
}

fn local_time() -> (libc::tm, u32) {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let seconds = now.as_secs() as libc::time_t;
    let time = unsafe {
        let mut time = std::mem::zeroed();
        libc::localtime_r(&seconds, &mut time);
        time
    };
    (time, now.subsec_millis())
}

pub fn local_clock_ms() -> u32 {
    let (t, millis) = local_time();
    ((t.tm_hour * 60 + t.tm_min) * 60 + t.tm_sec) as u32 * 1000 + millis
}

fn formatted_time(pattern: &std::ffi::CStr) -> String {
    let (time, _) = local_time();
    let mut output = [0_i8; 80];
    unsafe {
        libc::strftime(output.as_mut_ptr(), output.len(), pattern.as_ptr(), &time);
        std::ffi::CStr::from_ptr(output.as_ptr()).to_string_lossy().into_owned()
    }
}

pub fn recording_name(base: &str) -> String { format!("{base} - {}", formatted_time(c"%d %B %Y - %I-%M-%S %p")) }
pub fn snapshot_name(prefix: &str) -> String { format!("{prefix}{}", formatted_time(c"%d %B %Y %H-%M-%S")) }

pub fn monitors() -> Vec<[i32; 4]> {
    #[repr(C)]
    struct Bounds { x: f64, y: f64, w: f64, h: f64 }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGGetActiveDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
        fn CGDisplayBounds(display: u32) -> Bounds;
    }
    let mut displays = [0; 32];
    let mut count = 0;
    if unsafe { CGGetActiveDisplayList(displays.len() as u32, displays.as_mut_ptr(), &mut count) } != 0 {
        return Vec::new();
    }
    displays.iter().take(count as usize).map(|id| {
        let b = unsafe { CGDisplayBounds(*id) };
        [b.x as i32, b.y as i32, b.w as i32, b.h as i32]
    }).collect()
}

#[test]
#[ignore = "requires FFmpeg and FFprobe"]
fn ffmpeg_video_roundtrip() {
    use std::time::Instant;
    let path = std::env::temp_dir().join(format!("livemix-mac-roundtrip-{}.mp4", std::process::id()));
    let path = path.to_string_lossy().into_owned();
    let (recorder, writer) = Recorder::spawn_writer(path.clone(), RecOpts {
        hw: false, fps: 25.0, ..Default::default()
    });
    let error = recorder.error.clone();
    let origin = Instant::now();
    for i in 0..12 {
        recorder.tx.send(RecFrame {
            timeline_origin: None,
            audio: crate::audio::test_tone(1920),
            pixels: Arc::new([180, 55, 20, 255].repeat(64 * 48)),
            w: 64, h: 48, bgra: false, captured_at: origin + Duration::from_millis(i * 40),
        }).unwrap();
    }
    drop(recorder);
    writer.join().unwrap();
    let error = error.lock().unwrap().clone();
    assert!(error.is_empty(), "{error}");
    crate::audio::assert_audio_file(Path::new(&path));
    let shared = start_with_pause(Source::File(path.clone()), true);
    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.seq.load(Relaxed) == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(shared.seq.load(Relaxed), 1, "paused clip must show and hold its first frame");
    shared.stop.store(true, Relaxed);
    let frame = shared.frame.lock().unwrap().take();
    let _ = std::fs::remove_file(path);
    let frame = frame.expect("FFmpeg did not deliver a decoded frame");
    assert_eq!((frame.w, frame.h), (64, 48));
    assert!(frame.rgba[0] > 145 && frame.rgba[2] < 55);
}
