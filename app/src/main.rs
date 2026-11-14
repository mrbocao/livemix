// LiveMix — a live video production switcher.

mod model;
mod settings_xml;
#[cfg(windows)]
mod directshow;
mod locale;
mod render;
mod ffmpeg_media;
mod replay;
mod slides;
mod web_input;
mod colour_fx;
mod media_common;
#[cfg(target_os = "macos")]
#[path = "media_macos.rs"]
mod media;
#[cfg(windows)]
mod media;
mod program;
mod performance;
mod gt;
mod preset;
#[cfg(target_os = "macos")]
#[path = "decklink_macos.rs"]
mod decklink;
#[cfg(target_os = "macos")]
mod decklink_keyer_macos;
mod decklink_pixels;
#[cfg(windows)]
mod decklink;
mod scopes;
mod stream;
#[cfg(test)]
mod livecheck;
mod ndi;
mod omt;
mod srt;
mod audio;
mod dsp;
mod ptz;
mod automation;
mod midi;
mod controls;
mod shortcut_ops;
#[cfg(windows)]
mod dx_interop;

use eframe::glow::HasContext;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, Vec2};
use model::*;
use locale::tr;
/// Photos: one tick per second (frmmain::g @0x24d8b0 on the 1000 ms timer → ozzzzzg::a → na::a
/// @0x14a98c), next picture every `d` = 2 ticks (na::.ctor @0x14a6b8) → 2 s per picture.
const PHOTO_SECS: f32 = 2.0;
#[allow(unused_imports)]
use model::FULL;
use render::*;
mod app;
mod startup;
mod outputs;
mod desk;
mod sources;
mod mixer;
mod settings;
mod ui;
use desk::*;
use mixer::*;
use settings::*;
use sources::*;
use ui::*;
use replay::ui::*;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut screenshot: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--screenshot" && i + 1 < args.len() {
            screenshot = Some(args[i + 1].clone());
            i += 1;
        }
        i += 1;
    }
    let icon = startup::icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_icon(Arc::clone(&icon))
            .with_title("LiveMix — Live Production"),
        ..Default::default()
    };
    eframe::run_native(
        "LiveMix",
        options,
        Box::new(move |cc| {
            if screenshot.is_some() {
                return Ok(Box::new(startup::initialize(&cc.egui_ctx, cc.gl.as_deref(), screenshot)));
            }
            Ok(Box::new(startup::Startup::new(&cc.egui_ctx, &icon)))
        }),
    )
}

// ozzzzzs::a (0x060039c0) selects the palette independently of the layout.
fn apply_style(ctx: &egui::Context, theme: InterfaceTheme) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = WINDOW;
    v.extreme_bg_color = Color32::from_rgb(0x10, 0x24, 0x4a);
    v.widgets.inactive.bg_fill = BTN;
    v.widgets.inactive.weak_bg_fill = BTN;
    v.widgets.hovered.bg_fill = BTN_HOVER;
    v.widgets.hovered.weak_bg_fill = BTN_HOVER;
    v.widgets.active.bg_fill = Color32::from_rgb(0x6a, 0x6a, 0x6e);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x6a, 0x6a, 0x6e);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    // The original's buttons are flat (same navy as the bar); a light edge keeps them visible.
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(0x8c, 0xb4, 0xf0));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.selection.bg_fill = HOVER;
    let mut style = egui::Style::default();
    style.spacing.item_spacing = Vec2::new(4.0, 4.0);
    style.spacing.button_padding = Vec2::new(6.0, 3.0);
    if theme != InterfaceTheme::Classic {
        let palette = match theme {
            InterfaceTheme::Graphite => [[25, 25, 27], [35, 35, 38], [18, 18, 20], [48, 48, 53], [65, 65, 72], [140, 189, 174]],
            InterfaceTheme::Slate => [[21, 31, 33], [29, 44, 46], [14, 23, 25], [39, 56, 58], [53, 76, 79], [109, 199, 191]],
            InterfaceTheme::Midnight => [[20, 17, 32], [31, 25, 49], [14, 11, 24], [45, 36, 65], [65, 51, 93], [182, 147, 255]],
            _ => [[17, 23, 33], [24, 32, 45], [13, 19, 28], [35, 45, 60], [49, 65, 86], [111, 167, 246]],
        }.map(|[r, g, b]| Color32::from_rgb(r, g, b));
        let [panel, window, extreme, button, hovered, accent] = palette;
        let edge = hovered.gamma_multiply(0.8);
        v.panel_fill = panel;
        v.window_fill = window;
        v.extreme_bg_color = extreme;
        v.faint_bg_color = Color32::from_white_alpha(9);
        v.window_corner_radius = egui::CornerRadius::same(10);
        v.menu_corner_radius = egui::CornerRadius::same(8);
        v.window_stroke = Stroke::new(1.0_f32, edge);
        v.selection.bg_fill = accent.gamma_multiply(0.43);
        v.selection.stroke = Stroke::new(1.0_f32, accent);
        for widget in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            widget.corner_radius = egui::CornerRadius::same(6);
            widget.expansion = 0.0;
        }
        v.widgets.inactive.bg_fill = button;
        v.widgets.inactive.weak_bg_fill = v.widgets.inactive.bg_fill;
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, edge);
        v.widgets.hovered.bg_fill = hovered;
        v.widgets.hovered.weak_bg_fill = v.widgets.hovered.bg_fill;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, accent);
        v.widgets.active.bg_fill = v.selection.bg_fill;
        v.widgets.active.weak_bg_fill = v.selection.bg_fill;
        v.widgets.active.bg_stroke = v.selection.stroke;
        style.spacing.window_margin = egui::Margin::same(12);
        style.spacing.menu_margin = egui::Margin::same(7);
    }
    style.visuals = v;
    ctx.set_style(style);
}

/// A running real source and its picture.
struct Feed {
    colour_adjust: ColourAdjust,
    colour_fx: colour_fx::Effects,
    #[cfg(windows)]
    gpu_size: Option<[usize; 2]>,
    shared: Option<std::sync::Arc<media::Shared>>,
    tex: Option<egui::TextureHandle>,
    thumbnail_source: Option<media::Frame>,
    seen: u64,
    recorded: u64,
    /// Which source this reader was started for (a List moves on by changing it).
    key: String,
    /// Photos: the previous picture and when the fade to the new one started.
    prev_tex: Option<egui::TextureHandle>,
    fade_start: f64,
}

/// Photos change picture with a Fade of 500 ms (na::.ctor @0x14a6b8 fields c = 0, e = 500).
const PHOTO_FADE_MS: u32 = 500;

impl Drop for Feed {
    fn drop(&mut self) {
        if let Some(sh) = &self.shared {
            sh.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

type StreamGroup = (([u32; 2], f64), Vec<(usize, stream::Destination)>, stream::Output);

/// Video Delay (frmInput tVideoDelay): keeps the source picture for `seconds` at the chosen
/// resolution and shows the oldest frame that is old enough.
struct DelayFeed {
    /// (UI time the frame arrived, RGBA at `size`, width, height), oldest first.
    frames: std::collections::VecDeque<(f64, Vec<u8>, usize, usize)>,
    tex: Option<egui::TextureHandle>,
    /// Time of the frame currently in `tex`, so a repeat frame is not uploaded twice.
    last: f64,
    source: u32,
    seconds: f32,
    size: [usize; 2],
}

impl DelayFeed {
    fn new(size: [usize; 2]) -> Self {
        Self { frames: Default::default(), tex: None, last: -1.0, source: 0,
            seconds: 1.0, size }
    }

    /// Stores one frame of the source, scaled to the buffer resolution (nearest neighbour).
    fn capture(&mut self, rgba: &[u8], w: usize, h: usize, now: f64) {
        let [tw, th] = self.size;
        if w == 0 || h == 0 || tw == 0 || th == 0 { return; }
        if self.frames.back().is_some_and(|frame| now - frame.0 < 1.0 / 30.0) { return; }
        let mut scaled = vec![0u8; tw * th * 4];
        for y in 0..th {
            let sy = (y * h / th).min(h - 1);
            for x in 0..tw {
                let sx = (x * w / tw).min(w - 1);
                let s = (sy * w + sx) * 4;
                let d = (y * tw + x) * 4;
                scaled[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
            }
        }
        self.frames.push_back((now, scaled, tw, th));
        // Keep the buffer plus a second of slack: the frame shown is `seconds` old.
        let keep = f64::from(self.seconds) + 2.0;
        while self.frames.len() > 2 && self.frames[1].0 < now - keep {
            self.frames.pop_front();
        }
    }

    /// The frame that is at least `seconds` old, if the buffer is full enough.
    fn ready(&mut self, now: f64) -> Option<f64> {
        let target = now - f64::from(self.seconds);
        while self.frames.get(1).is_some_and(|frame| frame.0 <= target) {
            self.frames.pop_front();
        }
        self.frames.front().map(|frame| frame.0)
            .filter(|time| *time <= target)
    }
}

/// Applies a Telestrator stroke received from the web page to the first Telestrator input.
fn apply_telestroke(s: &mut Switcher, message: automation::StrokeMessage) {
    let Some(id) = s.inputs.iter()
        .find(|input| matches!(input.kind, Kind::Telestrator { .. }))
        .map(|input| input.id) else { return };
    let Some(input) = s.input_mut(id) else { return };
    let Kind::Telestrator { strokes } = &mut input.kind else { return };
    if message.clear {
        strokes.clear();
        return;
    }
    if message.undo {
        strokes.pop();
        return;
    }
    if message.points.is_empty() {
        return;
    }
    let points: Vec<[f32; 2]> = message.points.iter().take(4096)
        .map(|point| [point[0].clamp(0.0, 1.0), point[1].clamp(0.0, 1.0)]).collect();
    let colour = message.colour.unwrap_or([255, 60, 60]);
    let width = message.width.unwrap_or(0.006).clamp(0.001, 0.05);
    strokes.push(Telestroke { points, colour, width });
}

struct LastAutosave {
    startup: String,
    last: Instant,
    worker: Option<std::thread::JoinHandle<Result<(), String>>>,
    enabled: bool,
}

impl LastAutosave {
    fn new(s: &Switcher, enabled: bool) -> Self {
        Self { startup: s.preset_text(), last: Instant::now(), worker: None, enabled }
    }

    fn snapshot(&self, s: &Switcher) -> Result<Option<String>, String> {
        if !self.enabled || s.inputs.is_empty()
            || (s.inputs.len() == 2 && s.inputs.iter().all(|input| matches!(input.kind, Kind::Blank)))
            || (s.preset_file.is_none() && s.preset_text() == self.startup)
        {
            return Ok(None);
        }
        preset::encode(s).map(Some)
    }

    fn finish(&mut self, s: &mut Switcher) {
        if let Some(worker) = self.worker.take() {
            let result = worker.join().unwrap_or_else(|_| Err("Recovery writer stopped".into()));
            if let Err(error) = result { s.notice(&format!("Could not save Last: {error}")); }
        }
    }

    fn tick(&mut self, s: &mut Switcher) {
        if self.worker.as_ref().is_some_and(|worker| worker.is_finished()) { self.finish(s); }
        if self.worker.is_some() || self.last.elapsed() < std::time::Duration::from_secs(60) { return; }
        self.last = Instant::now();
        match self.snapshot(s) {
            Ok(Some(xml)) => {
                let path = Switcher::recovery_preset_path();
                self.worker = Some(std::thread::spawn(move || preset::write_file(&path, xml.as_bytes())));
            }
            Ok(None) => {}
            Err(error) => s.notice(&format!("Could not prepare Last: {error}")),
        }
    }

    fn flush(&mut self, s: &mut Switcher) {
        self.finish(s);
        if let Err(error) = self.snapshot(s).and_then(|xml| match xml {
            Some(xml) => preset::write_file(&Switcher::recovery_preset_path(), xml.as_bytes()),
            None => Ok(()),
        }) {
            s.notice(&format!("Could not save Last: {error}"));
        }
        self.last = Instant::now();
    }
}

struct App {
    s: Switcher,
    last_autosave: LastAutosave,
    applied_theme: InterfaceTheme,
    frame: u64,
    screenshot: Option<String>,
    shot_pending: bool,
    shot_recording: Option<f64>,
    next_record_frame: f64,
    record_format: ([u32; 2], f64),
    program: Arc<Mutex<ProgramCapture>>,
    #[cfg(windows)]
    pending_gpu: Vec<GpuUpload>,
    graphics_adapter: String,
    performance: performance::Monitor,
    video_clock: performance::VideoClock,
    media: std::collections::HashMap<u32, Feed>,
    /// Video Delay buffers, keyed by the delay input's id.
    delays: std::collections::HashMap<u32, DelayFeed>,
    audio_monitor: Option<audio::Monitor>,
    mixer: MixerControls,
    /// Recording slots 1 and 2 start and stop with Record.
    recordings: [Option<ComposedRecording>; 2],
    replay: ReplayCapture,
    multi_outputs: [Option<ComposedRecording>; OUTPUT_COUNT],
    multi_output_failed: [bool; OUTPUT_COUNT],
    decklink_outputs: [Option<(decklink::OutputConfig, decklink::OutputSink)>; OUTPUT_COUNT],
    decklink_settings_visible: bool,
    ndi_outputs: [Option<(f64, ndi::Sender)>; OUTPUT_COUNT],
    omt_outputs: [Option<((f64, u8), omt::Sender)>; OUTPUT_COUNT],
    srt_outputs: [Option<(([u32; 2], f64), stream::Output)>; OUTPUT_COUNT],
    stream_outputs: [Option<StreamGroup>; stream::COUNT],
    network_scan: Option<std::sync::mpsc::Receiver<[Result<Vec<String>, String>; 2]>>,
    network_thumbnail: Option<NetworkThumbnail>,
    thumbnail_attempts: std::collections::HashSet<String>,
    /// One recording per camera (MultiCorder).
    multi: std::collections::HashMap<u32, media::Recorder>,
    browse_rx: Option<std::sync::mpsc::Receiver<Option<String>>>,
    browse_kind: media::PickKind,
    /// Snapshot picture waiting for the Save dialog answer.
    snap_pending: Option<(Vec<u8>, usize, usize)>,
    snap_rx: Option<std::sync::mpsc::Receiver<Option<(String, u32)>>>,
    /// PhotoExportFilterIndex (config default 0 = PNG).
    photo_filter: u32,
    /// When the current Output recording file / each MultiCorder file was started.
    multi_started: std::collections::HashMap<u32, f64>,
    /// Contiguous audio for each MultiCorder input.
    multi_audio: std::collections::HashMap<u32, audio::AudioCursor>,
    /// Contiguous audio for each offscreen output group, by route and audio source.
    group_audio: Vec<((OutputRoute, u32), audio::AudioCursor)>,
    /// Contiguous audio for recordings captured from the window.
    window_audio: audio::AudioCursor,
    /// Preview waveform picture and when to refresh it.
    scopes: std::collections::HashMap<u8, egui::TextureHandle>,
    next_wave: f64,
    scope_frame: program::ScopeMailbox,
    scope_key: program::ScopeKey,
    scope_jobs: std::sync::mpsc::SyncSender<program::ScopeFrame>,
    scope_images: scopes::Mailbox,
    controls: controls::Controls,
}

#[derive(Default)]
struct ProgramCapture {
    #[cfg(windows)]
    input_bridge: Option<Result<dx_interop::Bridge, String>>,
    target: Option<program::Target>,
    error: Option<String>,
    scope_target: Option<program::Target>,
    scope_error: Option<String>,
}

#[cfg(windows)]
struct GpuUpload {
    picture: dx_interop::Picture,
    texture: egui::TextureHandle,
    resize: bool,
    effects: dx_interop::Effects,
    source: Arc<media::Shared>,
}

struct ComposedRecording {
    recorder: Arc<media::Recorder>,
    channel: usize,
    route_override: Option<OutputRoute>,
    audio_override: Option<u32>,
    started: f64,
}

struct NetworkThumbnail {
    link: String,
    shared: Arc<media::Shared>,
    result: std::sync::mpsc::Receiver<Option<egui::ColorImage>>,
    deadline: Instant,
}

impl Drop for NetworkThumbnail {
    fn drop(&mut self) {
        self.shared.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

struct ReplayTrack {
    recording: ComposedRecording,
    origin: Instant,
    angle: usize,
    file: String,
    worker: std::thread::JoinHandle<()>,
}

struct FinalizingTrack {
    angle: usize,
    file: String,
    error: Arc<Mutex<String>>,
    index: Option<media::ReplayIndex>,
    worker: std::thread::JoinHandle<()>,
}

struct FinalizingReplay {
    origin: Instant,
    start: u64,
    end: u64,
    tracks: Vec<FinalizingTrack>,
}

type ReplayResult = Result<std::path::PathBuf, String>;
type ReplayJob = std::sync::mpsc::Receiver<ReplayResult>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplayMotion { Still, Forward, Reverse }

struct IndexedReplay {
    angle: usize,
    event: Option<u64>,
    position: f64,
    range: Option<(u64, u64)>,
    direction: ReplayMotion,
    paused: bool,
    file: Option<String>,
    seek: Option<(u64, u64, f64)>,
    due: f64,
    take: Option<bool>,
    /// Transition length that replaces the session's Program transition.
    mix: Option<u32>,
}

#[derive(Clone, Copy)]
struct ReplayRequest {
    event: u64,
    angle: usize,
    speed: u8,
    program: bool,
    route: bool,
    mix: Option<u32>,
}

struct ReplayPlayback {
    job: ReplayJob,
    ready: Option<ReplayResult>,
    program: bool,
    speed: u8,
    mix: Option<u32>,
}

/// Highlights or playlist clips playing in order.
struct ReplayQueue {
    items: Vec<replay::QueueItem>,
    next: usize,
    channel: usize,
    program: bool,
    looped: bool,
    running: bool,
    issued: f64,
}

#[derive(Default)]
struct ReplayCapture {
    folder: Option<std::path::PathBuf>,
    session: replay::Session,
    clock: Option<(f64, u64)>,
    current: Vec<ReplayTrack>,
    finalizing: Vec<FinalizingReplay>,
    mark_in: Option<u64>,
    provisional: Vec<u64>,
    requested: [Option<ReplayRequest>; 2],
    playback: [Option<ReplayPlayback>; 2],
    sync_playback: bool,
    indexed: [Option<IndexedReplay>; 2],
    export: Option<ReplayJob>,
    close_after_recording: bool,
    queue: Option<ReplayQueue>,
    /// Highlight or clip last started on each channel.
    playing: [Option<u64>; 2],
}

impl ReplayCapture {
    fn frame_ms(&self, fallback: f32) -> f64 {
        1000.0 / f64::from(self.session.format.as_ref().map_or(fallback, |format| format.fps)).max(1.0)
    }

    fn position(&self, now: f64) -> u64 {
        self.clock.map_or(0, |(wall, start)| start + ((now - wall).max(0.0) * 1000.0).round() as u64)
    }

    fn mark_event(&mut self, start: u64, end: u64, now: u64) -> u64 {
        let mut angles = [false; 8];
        for segment in &self.finalizing {
            if segment.start < end && segment.end > start {
                for track in &segment.tracks { angles[track.angle] = true; }
            }
        }
        if self.current.first().is_some_and(|track| self.position(track.recording.started) < end)
            && now >= end
        {
            for track in &self.current { angles[track.angle] = true; }
        }
        let id = self.session.new_event_with_angles(start, end, angles);
        if angles.contains(&true) { self.provisional.push(id); }
        id
    }

    fn save(&self) -> Result<(), String> {
        self.session.save(self.folder.as_deref().ok_or("No replay session")?)
    }

    fn job(&self, event: u64, angle: usize, speed: u8, export: bool, start: Option<u64>) -> Result<ReplayJob, String> {
        let mut event = self.session.find(event)
            .filter(|event| event.angles.get(angle) == Some(&true)).ok_or("Select an angle available in this event")?;
        if let Some(start) = start {
            if !(event.start..event.end).contains(&start) { return Err("Cue is outside the event".into()); }
            event.start = start;
        }
        let folder = self.folder.clone().ok_or("Open a replay session first")?;
        let segments = self.session.segments.iter().filter(|segment| segment.start < event.end && segment.end > event.start).cloned().collect::<Vec<_>>();
        let native = self.session.native;
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let stamp = replay_stamp();
            let target = if native {
                media::storage_folder().join("Imported Replay").join(format!("session-{stamp}"))
            } else {
                folder.join(if export { "export" } else { "playback" })
            };
            let output = if export {
                target.join(format!("Event-{}-Camera-{}-{stamp}.mp4", event.id, angle + 1))
            } else {
                target.join(format!("Event-{}-{}-{}-Camera-{}-{speed}.mp4", event.id, event.start, event.end, angle + 1))
            };
            let pending = target.join(format!("render-{stamp}.mp4"));
            let result = std::fs::create_dir_all(target).map_err(|error| error.to_string()).and_then(|_| {
                if output.is_file() { return Ok(()); }
                replay::event_media(&folder, &segments, &event, angle, speed, &pending)?;
                if !output.is_file() {
                    std::fs::rename(&pending, &output).or_else(|error| if output.is_file() { Ok(()) } else { Err(error) })
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            });
            let _ = std::fs::remove_file(&pending);
            let _ = send.send(result.map(|_| output));
        });
        Ok(receive)
    }
}

fn poll_replay_job(job: &ReplayJob) -> Option<Result<std::path::PathBuf, String>> {
    match job.try_recv() {
        Ok(result) => Some(result),
        Err(std::sync::mpsc::TryRecvError::Empty) => None,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err("Replay worker stopped unexpectedly".into())),
    }
}

fn replay_stamp() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos()
}

impl ComposedRecording {
    fn new(folder: &str, name: &str, channel: usize, started: f64, opts: media::RecOpts) -> Self {
        let path = rec_dir(folder).join(format!("{}.{}", media::recording_name(name), opts.format.extension()));
        let recorder = Arc::new(media::Recorder::start_opts(path.to_string_lossy().into_owned(), opts));
        Self { recorder, channel, route_override: None, audio_override: None, started }
    }

    fn route(&self, settings: &Settings) -> OutputRoute {
        self.route_override.unwrap_or(settings.outputs[self.channel])
    }
}

/// Recording folder: the one chosen in a setup window, or Documents\LiveMixStorage.
fn rec_dir(folder: &str) -> std::path::PathBuf {
    let p = if folder.trim().is_empty() { media::storage_folder() } else { std::path::PathBuf::from(folder.trim()) };
    let _ = std::fs::create_dir_all(&p);
    p
}

fn safe_name(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect()
}

/// What a keyboard shortcut does (function names from the original's shortcut list).
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        locale::set(ctx, self.s.cfg.language);
        if self.applied_theme != self.s.cfg.theme {
            self.applied_theme = self.s.cfg.theme;
            apply_style(ctx, self.applied_theme);
        }
        let now = Instant::now();
        self.performance.update(now, || {
            self.program.lock().unwrap().target.as_ref().map_or(0, program::Target::transfer_bytes)
        });
        let render_started = Instant::now();
        let t = ctx.input(|i| i.time);
        // Project format and performance settings used by the engine and the camera readers.
        let rate = self.s.cfg.rate();
        set_output_fps(rate);
        media::AUTO_CAM_MAX_H.store(self.s.cfg.auto_cam_max_h, std::sync::atomic::Ordering::Relaxed);
        media::CAPTURE_LOW_LATENCY.store(self.s.cfg.low_latency_capture, std::sync::atomic::Ordering::Relaxed);
        let previous_picture = self.video_clock.picture_time;
        let video_due = self.video_clock.advance(t, f64::from(rate));
        if video_due {
            let dt = if self.frame == 0 { 1.0 / rate } else { (t - previous_picture).max(0.0) as f32 };
            self.s.tick(t, dt);
            self.s.ui_frame += 1;
        }
        let external_settings = self.s.settings_open && self.s.cfg.page == 6;
        if external_settings && !self.decklink_settings_visible {
            Self::refresh_external_devices(ctx.clone());
        }
        self.decklink_settings_visible = external_settings;
        let picture_time = self.video_clock.picture_time;
        self.frame += 1;
        if !self.s.automation.active() {
            if let Some(command) = self.s.remote.as_ref().and_then(automation::Remote::receive) {
                let _ = self.s.automation.start(&command);
            }
        }
        if let Some(stroke) = self.s.remote.as_ref().and_then(automation::Remote::receive_stroke) {
            apply_telestroke(&mut self.s, stroke);
        }
        automation::State::tick(&mut self.s, now);
        if let Some(remote) = &self.s.remote { remote.publish(&self.s, now); }
        self.handle_requests(ctx);
        self.update_media(ctx, video_due);
        self.update_levels();
        if self.s.show_mixer || self.s.cfg.master_meter || self.s.cfg.mv_audio_program.contains(&true) {
            self.s.program_audio_level = audio::stereo_peaks(&self.mix_audio(SRC_OUTPUT, Instant::now(), 960));
        }
        self.update_recording(t);
        if ctx.input(|input| input.viewport().close_requested())
            && (!self.replay.current.is_empty() || !self.replay.finalizing.is_empty())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.replay.close_after_recording = true;
            self.s.replay_arm = false;
        }
        self.update_replay(t);
        if self.replay.close_after_recording && self.replay.finalizing.is_empty() && self.replay.current.is_empty() {
            self.replay.close_after_recording = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.update_decklink_outputs();
        self.update_network_outputs();
        self.update_scopes(ctx, t);

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.s.fullscreen = [false; 2];
            for window in &mut self.s.external {
                window.fullscreen = false;
            }
        }
        self.controls.tick(ctx, &mut self.s, t);

        top_bar(ctx, &mut self.s);
        status_bar(ctx, &self.s, self.video_clock.fps, &self.performance.status);
        bottom_bar(ctx, &mut self.s, t);
        monitors(ctx, &mut self.s, picture_time);
        if self.s.show_mixer {
            let master = self.s.program_audio_level;
            let mut monitor = self.audio_monitor.is_some();
            mixer_panel(ctx, &mut self.s, master, &mut monitor, &mut self.mixer);
            if monitor && self.audio_monitor.is_none() {
                match audio::Monitor::start(&self.s.cfg.headphones_device) {
                    Ok(stream) => self.audio_monitor = Some(stream),
                    Err(error) => {
                        self.mixer.last_error = Some(error.clone());
                        self.s.notice(&format!("Audio monitor: {error}"));
                    }
                }
            } else if !monitor {
                self.audio_monitor = None;
            }
        }
        self.update_audio_monitor();
        inputs_panel(ctx, &mut self.s, picture_time);
        let encoder = if self.recordings.iter().all(Option::is_none) {
            "Start recording to identify the encoder".into()
        } else {
            self.recordings.iter().enumerate().filter_map(|(index, recording)| recording.as_ref()
                .map(|r| format!("{}: {}", index + 1, r.recorder.diagnostics()))).collect::<Vec<_>>().join("\n")
        };
        let program_path = {
            let capture = self.program.lock().unwrap();
            let report = if let Some(error) = &capture.error {
                format!("Window capture via system memory ({error})")
            } else if let Some(target) = &capture.target {
                let (frames, bytes) = target.readback_stats();
                let report = format!("{frames} frames submitted for OpenGL readback ({:.1} MiB)",
                    bytes as f64 / 1_048_576.0);
                #[cfg(windows)]
                let report = match target.interop_error() {
                    Some(reason) => format!("{report} | D3D11 interop: {reason}"),
                    None => report,
                };
                report
            } else {
                "Waiting for the first Program capture".into()
            };
            if let Some(target) = &capture.scope_target {
                format!("{report}\nScopes: {}", target.scope_backend())
            } else {
                report
            }
        };
        let mut readers: Vec<_> = self.media.values().filter_map(|feed| {
            feed.shared.as_ref()?.video_adapter.lock().ok()?.clone()
        }).collect();
        readers.sort_unstable();
        readers.dedup();
        let adapters = if readers.is_empty() { self.graphics_adapter.clone() } else {
            format!("{} | File reader devices: {}", self.graphics_adapter, readers.join(", "))
        };
        dialogs(ctx, &mut self.s, &adapters, &encoder, &program_path, &mut self.mixer, &mut self.controls, &mut self.last_autosave);
        self.last_autosave.tick(&mut self.s);
        self.controls.feedback(&self.s);
        self.audio_mixer_windows(ctx);
        for channel in 0..OUTPUT_COUNT {
            if self.s.external[channel].open {
                output_window(ctx, &mut self.s, picture_time, channel);
            }
        }
        for slot in 0..2 {
            if self.s.fullscreen[slot] {
                fullscreen_window(ctx, &mut self.s, picture_time, slot);
            } else {
                ctx.data_mut(|data| data.remove::<u32>(egui::Id::new(("fs-started", slot))));
            }
        }
        input_preview_window(ctx, &mut self.s, picture_time);

        self.handle_screenshots(ctx, t);
        self.finish_frame(ctx, t, render_started);
        if self.s.cfg.lock_refresh {
            // Synchronize Output with Refresh Rate: draw at the screen's refresh rate.
            ctx.request_repaint();
        } else {
            // Repaint at the next project deadline. UI events can still wake egui earlier.
            let predicted_dt = ctx.input(|i| i.predicted_dt);
            let remaining = (self.video_clock.next_deadline() - t - render_started.elapsed().as_secs_f64()).max(0.0) as f32;
            ctx.request_repaint_after_secs(remaining + predicted_dt);
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        self.last_autosave.flush(&mut self.s);
        for (_, _, sink) in self.stream_outputs.iter().flatten() { sink.stop(); }
        for (_, sink) in self.ndi_outputs.iter().flatten() { sink.stop(); }
        for (_, sink) in self.omt_outputs.iter().flatten() { sink.stop(); }
        for (_, sink) in self.srt_outputs.iter().flatten() { sink.stop(); }
        for (_, sink) in self.decklink_outputs.iter().flatten() {
            sink.stop();
        }
        if let Some(gl) = gl {
            let mut capture = self.program.lock().unwrap();
            #[cfg(windows)]
            if let Some(Ok(bridge)) = capture.input_bridge.take() {
                unsafe { bridge.destroy(gl) };
            }
            if let Some(target) = capture.target.take() {
                unsafe { target.destroy(gl) };
            }
            if let Some(target) = capture.scope_target.take() {
                unsafe { target.destroy(gl) };
            }
        }
    }
}

// ---------------------------------------------------------------- inputs

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn default_keyboard_has_the_templates_36_shortcuts() {
        assert_eq!(default_shortcuts().len(), 36);
    }

    #[test]
    fn recording_name_follows_the_original_pattern() {
        let n = media::recording_name("capture");
        // "capture - 06 November 2026 - 01-23-45 AM"
        assert!(n.starts_with("capture - "));
        assert!(n.ends_with(" AM") || n.ends_with(" PM"));
        assert_eq!(n.matches(" - ").count(), 2);
    }
}

#[cfg(test)]
mod position_tests {
    use super::*;

    #[test]
    fn half_zoom_with_pan_lands_in_the_top_left_quarter() {
        // MatrixPosition: scale by zoom, move by pan × half the size, Pan Y up.
        let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1920.0, 1080.0));
        let p = Position { zoom: 0.5, pan_x: -0.5, pan_y: 0.5, ..Position::default() };
        let q = place_rect(r, &p);
        assert_eq!(q, egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(960.0, 540.0)));
    }

    #[test]
    fn layers_are_cleared_when_their_input_closes() {
        let mut s = Switcher::demo();
        let (a, b) = (s.inputs[1].id, s.inputs[2].id);
        s.input_mut(a).unwrap().layers[0] = Layer { on: true, input: Some(b), place: Position::default(), ..Layer::default() };
        s.remove_input(b);
        assert_eq!(s.input(a).unwrap().layers[0], Layer::default());
    }

    #[test]
    fn telestrator_strokes_follow_the_web_page() {
        let mut s = Switcher::demo();
        let id = s.add(Input::new(0, "Tele", Kind::Telestrator { strokes: Vec::new() }));
        let message = |points: Vec<[f32; 2]>, width: Option<f32>| automation::StrokeMessage {
            points, colour: Some([1, 2, 3]), width, clear: false, undo: false,
        };
        apply_telestroke(&mut s, message(vec![[0.25, 0.5], [1.5, -1.0]], Some(0.01)));
        let strokes = match &s.input(id).unwrap().kind {
            Kind::Telestrator { strokes } => strokes.clone(),
            _ => panic!("not a Telestrator"),
        };
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points, vec![[0.25, 0.5], [1.0, 0.0]]);
        assert_eq!(strokes[0].colour, [1, 2, 3]);
        assert!((strokes[0].width - 0.01).abs() < 1e-6);
        apply_telestroke(&mut s, message(Vec::new(), Some(9.0)));
        apply_telestroke(&mut s, message(vec![[0.5, 0.5]], Some(9.0)));
        let Kind::Telestrator { strokes } = &s.input(id).unwrap().kind else { unreachable!() };
        assert_eq!(strokes.len(), 2);
        assert!((strokes[1].width - 0.05).abs() < 1e-6);
        apply_telestroke(&mut s, automation::StrokeMessage { undo: true, ..Default::default() });
        let Kind::Telestrator { strokes } = &s.input(id).unwrap().kind else { unreachable!() };
        assert_eq!(strokes.len(), 1);
        apply_telestroke(&mut s, automation::StrokeMessage { clear: true, ..Default::default() });
        let Kind::Telestrator { strokes } = &s.input(id).unwrap().kind else { unreachable!() };
        assert!(strokes.is_empty());
    }

    #[test]
    fn input_overlay_activates_replaces_and_clears_its_channel() {
        let mut s = Switcher::demo();
        let (a, b) = (s.inputs[0].id, s.inputs[1].id);
        let (preview, output) = (s.preview, s.output);
        s.press_input_overlay(1, a, false);
        assert_eq!(s.overlays[1].input, Some(a));
        assert!(s.overlays[1].on);
        assert!(!s.overlays[1].on_preview);
        s.press_input_overlay(1, b, true);
        assert_eq!(s.overlays[1].input, Some(b));
        assert!(s.overlays[1].on);
        assert!(s.overlays[1].on_preview);
        s.press_input_overlay(1, b, false);
        assert!(s.overlays[1].assigned.is_none());
        assert!(!s.overlays[1].on);
        assert!(!s.toggle_overlay_channel(1));
        assert_eq!((s.preview, s.output), (preview, output));
    }
}

#[cfg(test)]
mod recovery_and_replay_preview_tests {
    use super::*;

    #[test]
    fn automatic_copy_captures_unsaved_changes_and_skips_startup_demo() {
        let mut project = Switcher::demo();
        let autosave = LastAutosave::new(&project, true);
        assert!(autosave.snapshot(&project).unwrap().is_none());
        project.inputs[0].title = "Unsaved live title".into();
        let xml = autosave.snapshot(&project).unwrap().unwrap();
        let mut restored = Switcher::empty();
        preset::decode(&mut restored, std::path::Path::new("last.livemix"), &xml).unwrap();
        assert_eq!(restored.inputs[0].title, "Unsaved live title");
        project.inputs.clear();
        assert!(autosave.snapshot(&project).unwrap().is_none());
    }

    #[test]
    fn replay_preview_uses_the_selected_live_input_and_yields_to_playback() {
        let mut project = Switcher::demo();
        let cameras = [project.inputs[1].id + 1, project.inputs[2].id + 1];
        let channels = project.ensure_replay_inputs();
        project.replay_angles[1] = cameras[0];
        project.replay_angles[4] = cameras[1];
        project.replay_cameras = [4, 1];
        assert_eq!(project.replay_live_source(0), None);
        project.replay_setup.show_live_sources = true;
        assert_eq!(project.replay_live_source(0), Some(cameras[1]));
        assert_eq!(project.replay_live_source(1), Some(cameras[0]));
        project.input_mut(channels[0]).unwrap().playing = true;
        assert_eq!(project.replay_live_source(0), None);
        project.input_mut(channels[0]).unwrap().playing = false;
        project.replay_live = false;
        assert_eq!(project.replay_live_source(1), None);
    }
}
