// LiveMix — a live video production switcher.

mod model;
mod render;
mod ffmpeg_media;
mod replay;
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
#[cfg(windows)]
mod decklink;
mod scopes;
mod stream;
mod ndi;
mod omt;
mod srt;
mod audio;
mod dsp;
mod ptz;
mod automation;
mod midi;
#[cfg(windows)]
mod dx_interop;

use eframe::glow::HasContext;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, Vec2};
use model::*;
/// Photos: one tick per second (frmmain::g @0x24d8b0 on the 1000 ms timer → ozzzzzg::a → na::a
/// @0x14a98c), next picture every `d` = 2 ticks (na::.ctor @0x14a6b8) → 2 s per picture.
const PHOTO_SECS: f32 = 2.0;
#[allow(unused_imports)]
use model::FULL;
use render::*;

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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 900.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_title("LiveMix — Live Production"),
        ..Default::default()
    };
    eframe::run_native(
        "LiveMix",
        options,
        Box::new(move |cc| {
            let mut app = App::new(screenshot);
            apply_style(&cc.egui_ctx, app.s.cfg.theme);
            if let Some(gl) = &cc.gl {
                app.graphics_adapter = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
                app.performance.set_adapter(&app.graphics_adapter);
            }
            #[cfg(windows)]
            {
                let mut preference = app.s.cfg.video_adapter.clone();
                if preference.is_empty() {
                    if let Some(adapter) = media::video_adapter_names().into_iter()
                        .find(|adapter| media::renderer_matches_adapter(&app.graphics_adapter, adapter))
                    {
                        preference = adapter;
                    }
                }
                media::select_video_adapter(&preference);
            }
            Ok(Box::new(app))
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
    #[cfg(windows)]
    gpu_size: Option<[usize; 2]>,
    shared: Option<std::sync::Arc<media::Shared>>,
    tex: Option<egui::TextureHandle>,
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
    fn ready(&self, now: f64) -> Option<f64> {
        self.frames.front().map(|frame| frame.0)
            .filter(|time| *time <= now - f64::from(self.seconds))
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

struct App {
    s: Switcher,
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
    /// Preview waveform picture and when to refresh it.
    scopes: std::collections::HashMap<u8, egui::TextureHandle>,
    next_wave: f64,
    scope_frame: program::ScopeMailbox,
    scope_key: program::ScopeKey,
    scope_jobs: std::sync::mpsc::SyncSender<program::ScopeFrame>,
    scope_images: scopes::Mailbox,
    /// MIDI input for shortcuts (Settings > Shortcuts).
    midi: Option<midi::Input>,
    midi_selection: Vec<usize>,
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

fn update_network_thumbnails(ctx: &egui::Context, dialog: &mut AddDialog, enabled: bool,
    active: &mut Option<NetworkThumbnail>, attempted: &mut std::collections::HashSet<String>)
{
    if !enabled || !dialog.open || dialog.kind != AddKind::Desktop {
        *active = None;
        return;
    }
    if let Some(job) = active {
        match job.result.try_recv() {
            Ok(image) => {
                if let Some(image) = image {
                    let texture = ctx.load_texture(&job.link, image, egui::TextureOptions::LINEAR);
                    dialog.thumbnails.insert(job.link.clone(), texture);
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) if Instant::now() < job.deadline => return,
            Err(_) => {}
        }
        *active = None;
    }
    let link = dialog.ndi_sources.iter().map(|name| format!("ndi:{name}"))
        .chain(dialog.omt_sources.iter().map(|name| format!("omt:{name}")))
        .find(|link| attempted.insert(link.clone()));
    let Some(link) = link else { return; };
    let shared = Arc::new(media::Shared::default());
    let (send, result) = std::sync::mpsc::channel();
    let receiver = Arc::clone(&shared);
    let worker_link = link.clone();
    let repaint = ctx.clone();
    std::thread::spawn(move || {
        let opened = if let Some(name) = worker_link.strip_prefix("ndi:") {
            ndi::thumbnail(Arc::clone(&receiver), name)
        } else {
            omt::thumbnail(Arc::clone(&receiver), worker_link.strip_prefix("omt:").unwrap_or_default())
        };
        let frame = receiver.frame.lock().unwrap().take();
        let image = opened.ok().and(frame).and_then(|frame| {
            let image = image::RgbaImage::from_raw(frame.w as u32, frame.h as u32, frame.rgba)?;
            let image = image::imageops::thumbnail(&image, 160, 90);
            Some(egui::ColorImage::from_rgba_unmultiplied([image.width() as usize, image.height() as usize], image.as_raw()))
        });
        let _ = send.send(image);
        repaint.request_repaint();
    });
    *active = Some(NetworkThumbnail { link, shared, result, deadline: Instant::now() + std::time::Duration::from_secs(2) });
}

fn network_thumbnail_grid(ui: &mut egui::Ui, dialog: &mut AddDialog) {
    if dialog.ndi_sources.is_empty() && dialog.omt_sources.is_empty() { return; }
    let rows = dialog.ndi_sources.iter().map(|name| ("ndi", name)).chain(dialog.omt_sources.iter().map(|name| ("omt", name)));
    egui::ScrollArea::vertical().id_salt("network-thumbnails").max_height(175.0).show(ui, |ui| {
        for (protocol, name) in rows {
            let key = format!("{protocol}:{name}");
            ui.horizontal(|ui| {
                let clicked = if let Some(texture) = dialog.thumbnails.get(&key) {
                    ui.add(egui::Image::new(texture).fit_to_exact_size(Vec2::new(96.0, 54.0)).sense(Sense::click())).clicked()
                } else {
                    ui.allocate_exact_size(Vec2::new(96.0, 54.0), Sense::hover());
                    false
                };
                if clicked || ui.selectable_label(dialog.ndi_source.as_ref() == Some(&key), format!("{} · {name}", protocol.to_uppercase())).clicked() {
                    dialog.ndi_source = Some(key);
                }
            });
        }
    });
}

struct ReplayTrack {
    recording: ComposedRecording,
    angle: usize,
    file: String,
    worker: std::thread::JoinHandle<()>,
}

struct FinalizingTrack {
    angle: usize,
    file: String,
    error: Arc<Mutex<String>>,
    worker: std::thread::JoinHandle<()>,
}

struct FinalizingReplay {
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
    position: f64,
    range: Option<(u64, u64)>,
    direction: ReplayMotion,
    paused: bool,
    file: Option<String>,
    seek: Option<(u64, u64, f64)>,
    due: f64,
}

struct ReplayPlayback {
    job: ReplayJob,
    ready: Option<ReplayResult>,
    program: bool,
    speed: u8,
}

#[derive(Default)]
struct ReplayCapture {
    folder: Option<std::path::PathBuf>,
    session: replay::Session,
    clock: Option<(f64, u64)>,
    current: Vec<ReplayTrack>,
    finalizing: Vec<FinalizingReplay>,
    mark_in: Option<u64>,
    marks: Vec<(u64, u64)>,
    playback: [Option<ReplayPlayback>; 2],
    sync_playback: bool,
    indexed: [Option<IndexedReplay>; 2],
    export: Option<ReplayJob>,
    close_after_recording: bool,
}

impl ReplayCapture {
    fn frame_ms(&self, fallback: f32) -> f64 {
        1000.0 / f64::from(self.session.format.as_ref().map_or(fallback, |format| format.fps)).max(1.0)
    }

    fn position(&self, now: f64) -> u64 {
        self.clock.map_or(0, |(wall, start)| start + ((now - wall).max(0.0) * 1000.0).round() as u64)
    }

    fn save(&self) -> Result<(), String> {
        self.session.save(self.folder.as_deref().ok_or("No replay session")?)
    }

    fn job(&self, event: u64, angle: usize, speed: u8, export: bool) -> Result<ReplayJob, String> {
        let event = self.session.events.iter().find(|e| e.id == event)
            .filter(|event| event.angles.get(angle) == Some(&true)).cloned().ok_or("Select an angle available in this event")?;
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

impl App {
    fn new(screenshot: Option<String>) -> Self {
        let mut s = Switcher::demo();
        // Settings window options saved last time (defaults are the original's config defaults).
        s.load_settings();
        s.external[0].open = s.cfg.start_external;
        s.fullscreen[0] = s.cfg.start_fullscreen;
        if s.cfg.start_fullscreen2 { toggle_fullscreen(&mut s, 1); }
        // Testing aid: LIVEMIX_EXTRA_INPUTS=N adds N test cameras (to check the input grid).
        let extra: usize = std::env::var("LIVEMIX_EXTRA_INPUTS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
        for _ in 0..extra.min(200) {
            s.add_dialog.kind = AddKind::TestCamera;
            let _ = s.add_from_dialog();
        }
        // Testing aid: LIVEMIX_PHOTOS=<folder> adds a Photos input and puts it in Preview.
        if let Ok(folder) = std::env::var("LIVEMIX_PHOTOS") {
            s.add_dialog.kind = AddKind::Photos;
            s.add_dialog.folder = folder;
            if s.add_from_dialog().is_ok() {
                if let Some(i) = s.inputs.last_mut() {
                    i.playing = true;
                }
                s.preview = s.inputs.last().map(|i| i.id);
            }
        }
        s.add_dialog.kind = AddKind::Camera;
        // Testing aid: LIVEMIX_OPEN=settings / add opens that window at start.
        match std::env::var("LIVEMIX_OPEN").unwrap_or_default().as_str() {
            "settings" => s.settings_open = true,
            "decoders" => {
                s.settings_open = true;
                s.cfg.page = 4;
            }
            "recording" => s.rec_setup = true,
            "outputs" => {
                s.settings_open = true;
                s.cfg.page = 1;
            }
            "camera" => {
                // The first real camera, with its Input Settings → General open.
                if let Some(dev) = media::list_devices().into_iter().next() {
                    let id = s.add(Input::new(0, &dev.name, Kind::Device { link: dev.link.clone(), blackmagic: dev.blackmagic }));
                    s.settings_input = Some(id);
                    s.cfg.input_page = 0;
                }
            }
            "newpreset" => {
                // The New button pressed (External off): "save changes?" first, as the demo has
                // inputs; LIVEMIX_OPEN=newformat goes on to the New Preset window.
                s.external[0].open = false;
                s.press_new();
            }
            "newformat" => {
                s.external[0].open = false;
                s.press_new();
                s.new_step = NewStep::Format;
            }
            "tools" => {
                // Preview safe areas and waveform on.
                s.cfg.safe_type = 20;
                s.cfg.wave_type = 60;
            }
            "perf" => {
                s.settings_open = true;
                s.cfg.page = 3;
            }
            "setup" => {
                // Recording Setup and Streaming Settings (gear buttons) open.
                s.rec_setup = true;
                s.stream_setup = true;
            }
            "multicorder" => {
                s.mc_setup = true;
                s.playlist_setup = true;
            }
            "multiview" => {
                // Settings → Display with the MultiView 1 layout and its live picture.
                s.settings_open = true;
                s.cfg.page = 0;
            }
            "shrink" => {
                // Inputs 1, 4 and 6 shrunk to their title bars.
                for n in [0, 3, 5] {
                    if let Some(i) = s.inputs.get_mut(n) {
                        i.collapsed = true;
                    }
                }
            }
            "input" => {
                // Camera 1 (on Output) with Camera 2 as a picture-in-picture layer.
                let (a, b) = (s.inputs[1].id, s.inputs[2].id);
                if let Some(i) = s.input_mut(a) {
                    i.layers[0] = Layer { on: true, input: Some(b), place: Position { zoom: 0.3, pan_x: 0.6, pan_y: -0.6, ..Position::default() }, ..Layer::default() };
                }
                s.settings_input = Some(a);
                s.cfg.input_page = std::env::var("LIVEMIX_INPUT_PAGE").ok()
                    .and_then(|v| v.parse().ok()).unwrap_or(7);
            }
            "add" => {
                s.add_dialog.open = true;
                s.add_dialog.kind = AddKind::Colour;
            }
            "replay" if screenshot.is_some() => {
                s.ensure_replay_inputs();
                s.replay_open = true;
                s.replay_setup_open = true;
            }
            "audio" => {
                // Input Settings > Audio on a video input, to check the fader, meter and graphs.
                if let Some(i) = s.inputs.get_mut(1) {
                    i.kind = Kind::VideoFile { path: String::new() };
                }
                s.settings_input = s.inputs.get(1).map(|i| i.id);
                s.cfg.input_page = 4;
            }
            _ => {}
        }
        let (scope_jobs, scope_images) = scopes::worker();
        Self {
            applied_theme: s.cfg.theme,
            s,
            frame: 0,
            screenshot,
            shot_pending: false,
            shot_recording: None,
            next_record_frame: 0.0,
            record_format: ([1920, 1080], 30.0),
            program: Arc::default(),
            #[cfg(windows)]
            pending_gpu: Vec::new(),
            graphics_adapter: "OpenGL renderer unavailable".into(),
            performance: performance::Monitor::new(),
            video_clock: Default::default(),
            media: Default::default(),
            delays: Default::default(),
            audio_monitor: None,
            mixer: MixerControls::default(),
            recordings: Default::default(),
            replay: Default::default(),
            multi_outputs: Default::default(),
            multi_output_failed: [false; OUTPUT_COUNT],
            decklink_outputs: Default::default(),
            ndi_outputs: Default::default(),
            omt_outputs: Default::default(),
            srt_outputs: Default::default(),
            stream_outputs: Default::default(),
            network_scan: None,
            network_thumbnail: None,
            thumbnail_attempts: Default::default(),
            multi: Default::default(),
            browse_rx: None,
            browse_kind: media::PickKind::Video,
            snap_pending: None,
            snap_rx: None,
            photo_filter: 0,
            multi_started: Default::default(),
            scopes: Default::default(),
            next_wave: 0.0,
            scope_frame: Default::default(),
            scope_key: (None, 0, false),
            scope_jobs,
            scope_images,
            // No MIDI device until one is picked in Settings > Shortcuts.
            midi: None,
            midi_selection: Vec::new(),
        }
    }

    /// Starts/stops camera and file readers and turns their frames into pictures.
    fn update_media(&mut self, ctx: &egui::Context, video_due: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        let now = ctx.input(|i| i.time);
        let picture_time = self.video_clock.picture_time;
        let display_sources: Vec<_> = self.s.cfg.outputs.iter().zip(&self.s.external)
            .filter(|(_, window)| window.open).map(|(route, _)| route.source)
            .chain(self.s.fullscreen.into_iter().zip(self.s.cfg.fs_source).filter_map(|(open, source)| open.then_some(source)))
            .chain(self.composed_recordings().map(|r| r.route(&self.s.cfg).source))
            .chain(self.s.cfg.outputs.iter().enumerate().filter(|(channel, _)| {
                self.ndi_outputs[*channel].as_ref().is_some_and(|(_, sender)| sender.subscribed())
                    || self.omt_outputs[*channel].as_ref().is_some_and(|(_, sender)| sender.subscribed())
                    || self.s.srt_active[*channel]
            }).map(|(_, route)| route.source))
            .chain(self.s.cfg.streams.iter().zip(&self.s.stream_active).filter(|(_, active)| **active)
                .map(|(stream, _)| stream_route(&self.s, stream).source))
            .collect();
        #[cfg(windows)]
        let pending_gpu = &mut self.pending_gpu;
        let App { s, media: feeds, delays, multi, multi_started, .. } = self;
        let ids: std::collections::HashSet<u32> = s.inputs.iter().map(|i| i.id).collect();
        feeds.retain(|id, _| ids.contains(id));
        let multicorder = s.multicorder.is_some();
        let mc_split = s.cfg.mc_split_min as f64 * 60.0;
        if !multicorder {
            multi.clear();
            multi_started.clear();
        } else {
            // Inputs ticked off in the MultiCorder window stop recording; "New File Every"
            // closes a file once it is old enough (a new one starts with the next frame).
            multi.retain(|id, _| ids.contains(id) && !s.mc_off.contains(id) && !(mc_split > 0.0 && now - multi_started.get(id).copied().unwrap_or(now) >= mc_split));
        }
        let (mc_dir, mc_mbps, mc_fps, mc_hw, mc_format, mc_audio) = (
            s.cfg.mc_folder.clone(), s.cfg.mc_mbps, s.cfg.frame_rate(), s.cfg.hw_encoder, s.cfg.mc_codec, s.cfg.record_audio[2],
        );
        // Inputs seen big right now (Preview, Output, transitions, overlays and their layers).
        let skip = u64::from(s.cfg.preview_skip.max(1));
        let ui_frame = s.ui_frame;
        let mut live: std::collections::HashSet<u32> = [s.preview, s.output].into_iter().flatten().collect();
        live.extend(s.input_preview);
        if let Some(tr) = &s.trans {
            live.insert(tr.to);
            live.extend(tr.from);
        }
        live.extend(s.overlays.iter().filter(|c| c.on || c.level > 0.0).filter_map(|c| c.input));
        // A Video Delay keeps its source reading even while the source is not on screen.
        live.extend(s.inputs.iter().filter_map(|input| match &input.kind {
            Kind::VideoDelay { source, .. } => *source,
            _ => None,
        }).filter(|id| s.input(*id).is_some()));
        for source in display_sources {
            if source == SRC_MV1 || source == SRC_MV2 {
                live.extend(s.mv_resolve(usize::from(source == SRC_MV2)).into_iter()
                    .filter_map(|(_, code)| code.and_then(|code| code.checked_sub(1)))
                    .filter(|id| s.input(*id).is_some()));
            } else if let Some(id) = source.checked_sub(1).filter(|id| s.input(*id).is_some()) {
                live.insert(id);
            }
        }
        let tops: Vec<u32> = live.iter().copied().collect();
        for id in tops {
            if let Some(i) = s.input(id) {
                live.extend(i.layers.iter().filter(|l| l.on).filter_map(|l| l.input));
            }
        }
        for inp in &mut s.inputs {
            match &inp.kind {
                k if k.streamed() => {
                    // Which reader this input needs; a different key restarts the reader.
                    let (src, key, is_file, is_device) = match &inp.kind {
                        Kind::Device { link, .. } if link.starts_with("audio:") => (
                            media::Source::AudioDevice(link[6..].to_owned()), link.clone(), false, false,
                        ),
                        Kind::Device { link, .. } if link.starts_with("ndi:") => (
                            media::Source::Ndi(link[4..].to_owned()), link.clone(), false, true,
                        ),
                        Kind::Device { link, .. } if link.starts_with("omt:") => (
                            media::Source::Omt(link[4..].to_owned()), link.clone(), false, true,
                        ),
                        Kind::Device { link, .. } if link.starts_with("srt:") => {
                            let Ok(config) = serde_json::from_str(&link[4..]) else {
                                inp.status = "Invalid saved SRT configuration".into();
                                continue;
                            };
                            (media::Source::Srt(config), link.clone(), false, true)
                        }
                        // The chosen camera mode is part of the key: picking another restarts it.
                        Kind::Device { link, .. } if decklink::device_index(link).is_some() => (
                            media::Source::DeckLink(decklink::CaptureConfig {
                                device: decklink::device_index(link).unwrap(), port: inp.decklink_port, mode: inp.decklink_mode,
                            }),
                            format!("{link}|{}|{}", inp.decklink_port, inp.decklink_mode), false, true,
                        ),
                        Kind::Device { link, .. } => (
                            media::Source::Device(link.clone(), inp.cam_mode.clone()),
                            format!("{link}|{}|{}", inp.cam_mode.as_ref().map(|m| m.code()).unwrap_or_default(), s.cfg.low_latency_capture),
                            false,
                            true,
                        ),
                        Kind::VideoFile { path } => {
                            let (source, key) = file_decoder(&s.cfg, path);
                            (source, key, true, false)
                        },
                        Kind::AudioFile { path } => (media::Source::AudioFile(path.clone()), format!("audio-file:{path}"), true, false),
                        Kind::VideoList { paths, idx } => {
                            let p = paths.get(*idx).cloned().unwrap_or_default();
                            let (source, key) = file_decoder(&s.cfg, &p);
                            (source, format!("{idx}:{key}"), true, false)
                        }
                        Kind::Replay { path, .. } => (
                            media::Source::File(path.clone()), format!("replay:{path}"), true, false,
                        ),
                        Kind::Desktop => (media::Source::Desktop, "desktop".into(), false, false),
                        Kind::ImageSequence { folder } => (media::Source::Sequence(folder.clone(), output_fps()), folder.clone(), true, false),
                        Kind::Photos { folder } => (media::Source::Photos(folder.clone(), PHOTO_SECS), folder.clone(), true, false),
                        _ => continue,
                    };
                    let is_list = matches!(inp.kind, Kind::VideoList { .. });
                    if feeds.get(&inp.id).map_or(false, |f| f.key != key) {
                        // keep the last picture while the next list item starts
                        let old_tex = feeds.remove(&inp.id).and_then(|mut feed| {
                            #[cfg(windows)]
                            if feed.gpu_size.is_some() { return None; }
                            feed.tex.take()
                        });
                        feeds.insert(inp.id, Feed { shared: Some(media::start_for_gl(src.clone(), is_file && !inp.playing)),
                            tex: old_tex, seen: 0, recorded: 0, key: key.clone(), prev_tex: None, fade_start: 0.0,
                            colour_adjust: Default::default(),
                            #[cfg(windows)]
                            gpu_size: None,
                        });
                    }
                    let feed = feeds.entry(inp.id).or_insert_with(|| Feed { shared: Some(media::start_for_gl(src, is_file && !inp.playing)),
                        tex: None, seen: 0, recorded: 0, key, prev_tex: None, fade_start: 0.0,
                        colour_adjust: Default::default(),
                        #[cfg(windows)]
                        gpu_size: None,
                    });
                    let Some(sh) = feed.shared.clone() else { continue };
                    #[cfg(windows)]
                    sh.cpu_colour.store(inp.colour_adjust != ColourAdjust::default(), std::sync::atomic::Ordering::Release);
                    if feed.colour_adjust != inp.colour_adjust {
                        feed.colour_adjust = inp.colour_adjust;
                        if matches!(inp.kind, Kind::VideoFile { .. } | Kind::VideoList { .. }) && !inp.playing && inp.seek_ms.is_none() {
                            inp.seek_ms = Some(sh.pos_ms.load(Relaxed));
                        }
                    }
                    if let Ok(mut buffer) = sh.audio.try_lock() {
                        buffer.set_effects(&inp.audio_effects);
                    }
                    if is_file {
                        sh.precise_seek.store(matches!(inp.kind, Kind::Replay { .. }), Relaxed);
                        sh.paused.store(!inp.playing, Relaxed);
                        sh.looping.store(inp.loop_on && !is_list, Relaxed);
                        if inp.restart_req {
                            if let Kind::VideoList { idx, .. } = &mut inp.kind {
                                *idx = 0;
                            }
                            if matches!(inp.kind, Kind::VideoFile { .. }) {
                                inp.seek_ms = Some(0);
                            } else {
                                sh.restart.store(true, Relaxed);
                            }
                            inp.restart_req = false;
                        }
                        let seeking = inp.seek_ms.take();
                        if let Some(ms) = seeking {
                            sh.seek_ms.store(ms + 1, Relaxed);
                        }
                        inp.pos = sh.pos_ms.load(Relaxed) as f32 / 1000.0;
                        let d = sh.dur_ms.load(Relaxed) as f32 / 1000.0;
                        inp.duration = if d > 0.0 { d } else { inp.duration.max(inp.pos) };
                        if seeking.is_none() && sh.seek_ms.load(Relaxed) == 0 && sh.ended.load(Relaxed) {
                            let loop_on = inp.loop_on;
                            if let Kind::VideoList { paths, idx } = &mut inp.kind {
                                // List: next file; at the end start over when Loop is on.
                                if *idx + 1 < paths.len() {
                                    *idx += 1;
                                } else if loop_on {
                                    *idx = 0;
                                } else {
                                    inp.playing = false;
                                }
                            } else if !loop_on {
                                inp.playing = false;
                            }
                        }
                    }
                    let k = inp.key;
                    let key = if k.on { (1u64 << 40) | ((k.tolerance as u64) << 24) | ((k.colour[0] as u64) << 16) | ((k.colour[1] as u64) << 8) | k.colour[2] as u64 } else { 0 };
                    sh.key.store(key, Relaxed);
                    inp.status = sh.status.lock().map(|x| x.clone()).unwrap_or_default();
                    if is_device {
                        // Camera modes the device offers and the one in use (Input Settings).
                        if let Ok(m) = sh.modes.lock() {
                            if m.len() != inp.cam_modes.len() {
                                inp.cam_modes = m.clone();
                            }
                        }
                        inp.cam_current = sh.mode.lock().ok().and_then(|m| m.clone());
                    }
                    let seq = sh.seq.load(std::sync::atomic::Ordering::Acquire);
                    let recording_this = multicorder && is_device && !s.mc_off.contains(&inp.id);
                    // MultiCorder has its own source cadence, independent of display uploads.
                    if recording_this && seq != feed.recorded {
                        let frame = sh.frame.lock().ok().and_then(|slot| slot.as_ref()
                            .map(|fr| (fr.rgba.clone(), fr.w, fr.h)));
                        if let Some((rgba, w, h)) = frame {
                            feed.recorded = seq;
                            let rec = multi.entry(inp.id).or_insert_with(|| {
                                multi_started.insert(inp.id, now);
                                let p = rec_dir(&mc_dir).join(format!("{}.{}", media::recording_name(&safe_name(&inp.title)), mc_format.extension()));
                                media::Recorder::start_opts(p.to_string_lossy().to_string(), media::RecOpts { mbps: mc_mbps, size: None, fps: mc_fps, hw: mc_hw, format: mc_format, audio: mc_audio })
                            });
                            let at = Instant::now();
                            let count = (f64::from(audio::RATE) / f64::from(mc_fps)).ceil() as usize;
                            let packet = audio::mix([(&*sh, false, 1.0)], at, count);
                            rec.push_audio(rgba, w, h, at, packet);
                        }
                    }
                    // Preview Skip counts project pictures, even during rapid UI events.
                    let skip_now = feed.tex.is_some() && skip > 1 && !live.contains(&inp.id) && !recording_this && ui_frame % skip != 0;
                    if video_due && seq != feed.seen && !skip_now {
                        feed.seen = seq;
                        #[cfg(windows)]
                        if let Some(picture) = sh.gpu_frame.lock().unwrap().take()
                            .filter(|_| inp.colour_adjust == ColourAdjust::default())
                        {
                            let resize = feed.gpu_size != Some(picture.size);
                            if feed.gpu_size.is_none() {
                                feed.tex = Some(ctx.load_texture(format!("gpu-input-{}", inp.id),
                                    egui::ColorImage::new([1, 1], Color32::BLACK), egui::TextureOptions::LINEAR));
                            }
                            feed.gpu_size = Some(picture.size);
                            if let Some(texture) = &feed.tex {
                                pending_gpu.push(GpuUpload { picture, texture: texture.clone(), resize, source: sh.clone() });
                            }
                        }
                        let fr = sh.frame.lock().ok().and_then(|mut f| f.take());
                        if let Some(mut fr) = fr {
                            inp.colour_adjust.apply(&mut fr.rgba);
                            // Video Delay inputs that copy this one take the new picture.
                            for delay in delays.values_mut().filter(|delay| delay.source == inp.id) {
                                delay.capture(&fr.rgba, fr.w, fr.h, now);
                            }
                            #[cfg(windows)]
                            if feed.gpu_size.take().is_some() { feed.tex = None; }
                            let img = egui::ColorImage::from_rgba_unmultiplied([fr.w, fr.h], &fr.rgba);
                            let photos = matches!(inp.kind, Kind::Photos { .. });
                            if photos && feed.tex.is_some() {
                                // Keep the old picture and fade the new one in over it.
                                feed.prev_tex = feed.tex.take();
                                feed.fade_start = now;
                                feed.tex = Some(ctx.load_texture(format!("input-{}-{}", inp.id, seq), img, egui::TextureOptions::LINEAR));
                            } else {
                                match &mut feed.tex {
                                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                                    None => feed.tex = Some(ctx.load_texture(format!("input-{}", inp.id), img, egui::TextureOptions::LINEAR)),
                                }
                            }
                        }
                    }
                    inp.tex = feed.tex.as_ref().map(|t| (t.id(), t.size()));
                    #[cfg(windows)]
                    if let (Some(texture), Some(size)) = (&feed.tex, feed.gpu_size) {
                        inp.tex = Some((texture.id(), size));
                    }
                    // na::c @0x14aa30: step = mr::zzy(500 ms) per output frame; na::Render @0x14a75c adds it up to 255.
                    let frames = ((picture_time - feed.fade_start) * output_fps() as f64).floor() as f32;
                    let k = frames * step_for(PHOTO_FADE_MS) / FULL;
                    if k >= 1.0 {
                        feed.prev_tex = None;
                    }
                    inp.prev_tex = feed.prev_tex.as_ref().map(|t| (t.id(), t.size(), k.clamp(0.0, 1.0)));
                }
                Kind::ImageFile { path } => {
                    if feeds.get(&inp.id).is_none_or(|feed| feed.colour_adjust != inp.colour_adjust) {
                        let mut f = Feed { shared: None, tex: None, seen: 0, recorded: 0, key: String::new(), prev_tex: None, fade_start: 0.0,
                            colour_adjust: inp.colour_adjust,
                            #[cfg(windows)]
                            gpu_size: None,
                        };
                        match image::open(path) {
                            Ok(im) => {
                                let mut im = im.to_rgba8();
                                inp.colour_adjust.apply(im.as_mut());
                                let (w, h) = im.dimensions();
                                let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], im.as_raw());
                                f.tex = Some(ctx.load_texture(format!("input-{}", inp.id), ci, egui::TextureOptions::LINEAR));
                                inp.status.clear();
                            }
                            Err(e) => inp.status = format!("Can't open picture: {e}"),
                        }
                        feeds.insert(inp.id, f);
                    }
                    inp.tex = feeds.get(&inp.id).and_then(|f| f.tex.as_ref()).map(|t| (t.id(), t.size()));
                }
                _ => {}
            }
        }
        // Video Delay: show (and upload) the picture that is `seconds` old.
        delays.retain(|id, _| ids.contains(id));
        for inp in &mut s.inputs {
            let Kind::VideoDelay { source, seconds, resolution } = &inp.kind else { continue };
            let size = [resolution[0].max(1) as usize, resolution[1].max(1) as usize];
            let seconds = *seconds;
            let feed = delays.entry(inp.id).or_insert_with(|| DelayFeed::new(size));
            if feed.size != size {
                feed.size = size;
                feed.frames.clear();
                feed.last = -1.0;
            }
            feed.seconds = seconds;
            if let Some(source) = source { feed.source = *source; }
            match feed.ready(now) {
                Some(time) => {
                    if time != feed.last {
                        feed.last = time;
                        let img = {
                            let frame = &feed.frames[0];
                            egui::ColorImage::from_rgba_unmultiplied([frame.2, frame.3], &frame.1)
                        };
                        match &mut feed.tex {
                            Some(texture) => texture.set(img, egui::TextureOptions::LINEAR),
                            None => feed.tex = Some(ctx.load_texture(format!("delay-{}", inp.id), img,
                                egui::TextureOptions::LINEAR)),
                        }
                    }
                    inp.status = format!("Video Delay {seconds:.1} s");
                }
                None => inp.status = "Loading VideoDelay".into(),
            }
            inp.tex = feed.tex.as_ref().map(|texture| (texture.id(), texture.size()));
        }
    }

    fn composed_recordings(&self) -> impl Iterator<Item = &ComposedRecording> {
        self.recordings.iter().chain(&self.multi_outputs).filter_map(Option::as_ref)
            .chain(self.replay.current.iter().map(|track| &track.recording))
    }

    fn indexed_command(&mut self, action: replay::Action) {
        if self.replay.folder.is_none() { return; }
        let inputs = self.s.ensure_replay_inputs();
        let frame = self.replay.frame_ms(self.s.cfg.frame_rate());
        for channel in 0..2 {
            if self.s.replay_channel < 2 && self.s.replay_channel != channel || self.replay.playback[channel].is_some() {
                continue;
            }
            let angle = self.s.replay_cameras[channel];
            let event = self.s.replay_event.and_then(|id| self.replay.session.events.iter().find(|event| event.id == id));
            let previous = self.replay.indexed[channel].as_ref().filter(|cursor| cursor.angle == angle);
            let mut range = previous.map_or_else(|| event.map(|event| (event.start, event.end)), |cursor| cursor.range);
            let folder = self.replay.folder.as_deref().unwrap();
            let tail = self.replay.session.segments.iter().rev().find(|segment| segment.files[angle].as_deref()
                .is_some_and(|file| replay::direct_path(folder, file).is_some()));
            let position = previous.map(|cursor| {
                if cursor.direction == ReplayMotion::Forward {
                    if let Some(segment) = self.replay.session.segments.iter().find(|segment| {
                        cursor.file.is_some() && segment.files[angle] == cursor.file
                    }) {
                        let position = self.s.input(inputs[channel]).map_or(0.0, |input| input.pos);
                        return (segment.start as f64 + f64::from(position) * 1000.0).min(segment.end as f64);
                    }
                }
                cursor.position
            }).or_else(|| {
                let input = self.s.inputs.iter().find(|input| matches!(input.kind, Kind::Replay { channel: c, .. } if c == channel))?;
                event.map(|event| event.start as f64 + f64::from(input.pos) * f64::from(self.s.replay_speed[channel]) * 10.0)
            });
            let (point, direction) = match action {
                replay::Action::Live => {
                    range = None;
                    (tail.map(|segment| segment.start as f64), ReplayMotion::Forward)
                }
                replay::Action::Step(step) => (position.map(|point| point + f64::from(step) * frame), ReplayMotion::Still),
                replay::Action::Reverse if previous.is_some_and(|cursor| cursor.direction == ReplayMotion::Reverse) => (position, ReplayMotion::Still),
                replay::Action::Reverse => {
                    let point = position.filter(|point| range.is_none_or(|(start, _)| *point > start as f64 + frame))
                        .or_else(|| range.map(|(_, end)| end as f64 - frame));
                    (point, ReplayMotion::Reverse)
                }
                _ => return,
            };
            if let Some(position) = point.or_else(|| tail.map(|segment| segment.end as f64 - frame)) {
                let position = range.map_or(position, |(start, end)| position.clamp(start as f64, (end as f64 - frame).max(start as f64)));
                self.replay.indexed[channel] = Some(IndexedReplay {
                    angle, position, range, direction, paused: false, file: None, seek: None, due: 0.0,
                });
                if (self.s.replay_channel < 2 || channel == 0) && self.s.output != Some(inputs[channel]) {
                    self.s.preview = Some(inputs[channel]);
                }
            } else {
                self.s.replay_status = if self.replay.session.native {
                    "No directly seekable camera file. Use Play to prepare a native MPEG event.".into()
                } else { "No finalized video for this camera".into() };
            }
        }
    }

    fn update_indexed_replay(&mut self, now: f64) {
        use std::sync::atomic::Ordering::Relaxed;
        let Some(folder) = self.replay.folder.as_deref() else { return };
        let frame = self.replay.frame_ms(self.s.cfg.frame_rate());
        for (channel, transport) in self.replay.indexed.iter_mut().enumerate() {
            let Some(cursor) = transport else { continue };
            if cursor.paused { continue; }
            let Some(input) = self.s.inputs.iter_mut().find(|input| matches!(input.kind,
                Kind::Replay { channel: current, .. } if current == channel)) else { continue };
            let source = self.media.get(&input.id).and_then(|feed| feed.shared.as_ref());
            if let Some((baseline, target, sent)) = cursor.seek {
                let done = source.is_some_and(|source| source.seq.load(Relaxed) != baseline
                    && source.seek_ms.load(Relaxed) == 0 && source.pos_ms.load(Relaxed).abs_diff(target) <= frame.ceil() as u64);
                if !done {
                    if now - sent > 3.0 {
                        cursor.seek = None;
                        cursor.direction = ReplayMotion::Still;
                        self.s.replay_status = "Replay decoder could not seek to the requested frame".into();
                    }
                    continue;
                }
                cursor.seek = None;
                if cursor.direction != ReplayMotion::Reverse {
                    input.playing = cursor.direction == ReplayMotion::Forward;
                    continue;
                }
            } else if cursor.direction == ReplayMotion::Still && cursor.file.is_some() {
                continue;
            }
            if cursor.direction == ReplayMotion::Forward && cursor.file.is_some() {
                if let Some(segment) = self.replay.session.segments.iter().find(|segment| {
                    segment.files[cursor.angle].as_ref() == cursor.file.as_ref()
                }) {
                    if let Some(source) = source {
                        cursor.position = segment.start.saturating_add(source.pos_ms.load(Relaxed)).min(segment.end) as f64;
                        if !source.ended.load(Relaxed) && cursor.position < segment.end as f64 { continue; }
                    }
                    cursor.position = segment.end as f64;
                }
            } else if cursor.direction == ReplayMotion::Reverse && cursor.file.is_some() {
                if now < cursor.due || self.s.replay_speed[channel] == 0 { continue; }
                let next = cursor.position - frame;
                if next < cursor.range.map_or(0.0, |(start, _)| start as f64) - 0.01 {
                    cursor.direction = ReplayMotion::Still;
                    continue;
                }
                cursor.position = next.max(0.0);
            }
            let segments = &self.replay.session.segments;
            let has_angle = |segment: &&replay::Segment| segment.files[cursor.angle].as_deref()
                .is_some_and(|file| replay::direct_path(folder, file).is_some());
            let segment = segments.iter().filter(has_angle).find(|segment| {
                (segment.start as f64..segment.end as f64).contains(&cursor.position)
            }).or_else(|| {
                if cursor.direction == ReplayMotion::Forward {
                    segments.iter().filter(has_angle).find(|segment| segment.start as f64 >= cursor.position)
                } else {
                    segments.iter().filter(has_angle).rev().find(|segment| segment.end as f64 <= cursor.position + frame)
                }
            });
            let Some(segment) = segment else {
                if cursor.direction != ReplayMotion::Forward { cursor.direction = ReplayMotion::Still; }
                continue;
            };
            let file = segment.files[cursor.angle].as_ref().unwrap();
            let Some(path) = replay::direct_path(folder, file) else { continue };
            if cursor.direction == ReplayMotion::Forward && cursor.file.as_ref() == Some(file) { continue; }
            cursor.position = cursor.position.clamp(segment.start as f64, (segment.end as f64 - frame).max(segment.start as f64));
            let offset = (cursor.position - segment.start as f64).floor() as u64;
            let path = path.to_string_lossy().into_owned();
            let same_file = matches!(&input.kind, Kind::Replay { path: current, .. } if *current == path);
            let baseline = if same_file { source.map_or(0, |source| source.seq.load(Relaxed)) } else { 0 };
            input.kind = Kind::Replay { path, channel };
            input.playing = false;
            input.seek_ms = Some(offset);
            cursor.file = Some(file.clone());
            cursor.seek = Some((baseline, offset, now));
            cursor.due = now + frame / (10.0 * f64::from(self.s.replay_speed[channel].max(1)));
        }
    }


    fn update_replay(&mut self, now: f64) {
        if let Some(new_session) = self.s.replay_load.take() {
            if self.s.replay_arm || !self.replay.current.is_empty() || !self.replay.finalizing.is_empty()
                || self.replay.playback.iter().any(Option::is_some) || self.replay.export.is_some()
            {
                self.s.replay_status = "Stop recording and wait for replay jobs before changing sessions".into();
            } else {
                let folder = std::path::PathBuf::from(self.s.replay_setup.folder.trim());
                let opened = (|| -> Result<replay::Session, String> {
                    if folder.as_os_str().is_empty() { return Err("Select a Session Folder".into()); }
                    if new_session {
                        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
                        if std::fs::read_dir(&folder).map_err(|error| error.to_string())?.next().is_some() {
                            return Err("Choose an empty folder for a new replay session".into());
                        }
                        let camera_names = self.s.replay_angles.map(|source| {
                            if source == SRC_OUTPUT { "Program".into() }
                            else { source.checked_sub(1).and_then(|id| self.s.input(id)).map(|input| input.title.clone()).unwrap_or_default() }
                        });
                        let session = replay::Session { setup: self.s.replay_setup.clone(), camera_names, ..Default::default() };
                        session.save(&folder)?;
                        Ok(session)
                    } else {
                        replay::Session::open(&folder)
                    }
                })();
                match opened {
                    Ok(session) => {
                        let selected_folder = folder.to_string_lossy().into_owned();
                        self.s.replay_setup = session.setup.clone();
                        self.s.replay_setup.folder = selected_folder.clone();
                        self.s.replay_session = selected_folder;
                        self.s.replay_read_only = session.native;
                        for (index, name) in session.camera_names.iter().enumerate() {
                            if name.is_empty() && !new_session { self.s.replay_angles[index] = MV_AUTO; continue; }
                            if name.is_empty() { continue; }
                            self.s.replay_angles[index] = if name == "Program" { SRC_OUTPUT } else {
                                self.s.inputs.iter().find(|input| input.title == *name).map_or(MV_AUTO, |input| input.id + 1)
                            };
                        }
                        self.s.replay_event = session.events.first().map(|event| event.id);
                        self.s.replay_events = session.events.clone();
                        self.s.replay_status = if session.native && session.segments.is_empty() {
                            "vMix event list loaded read-only. No compatible recording files found.".into()
                        } else if session.native {
                            "vMix replay opened read-only".into()
                        } else { "Replay session ready".into() };
                        self.s.replay_setup_open = false;
                        self.s.ensure_replay_inputs();
                        for input in &mut self.s.inputs {
                            if let Kind::Replay { path, .. } = &mut input.kind {
                                path.clear();
                                input.playing = false;
                            }
                        }
                        self.replay = ReplayCapture { folder: Some(folder), session, ..Default::default() };
                    }
                    Err(error) => self.s.replay_status = error,
                }
            }
        }

        if matches!(self.s.replay_command, Some(replay::Action::Live | replay::Action::Step(_) | replay::Action::Reverse)) {
            let action = self.s.replay_command.take().unwrap();
            self.indexed_command(action);
        }
        let replay = &mut self.replay;
        let Some(folder) = replay.folder.clone() else {
            self.s.replay_arm = false;
            return;
        };
        if self.s.replay_session.is_empty() && !self.s.replay_arm && replay.finalizing.is_empty() && replay.current.is_empty() {
            self.replay = Default::default();
            return;
        }
        if replay.session.native { self.s.replay_arm = false; }
        let mut changed = false;
        if let Some(index) = replay.finalizing.iter().position(|segment| segment.tracks.iter().all(|track| track.worker.is_finished())) {
            let segment = replay.finalizing.remove(index);
            let mut files = std::array::from_fn(|_| None);
            for track in segment.tracks {
                let joined = track.worker.join().is_ok();
                let file = folder.join(&track.file);
                if joined && track.error.lock().unwrap().is_empty() && std::fs::metadata(&file).is_ok_and(|entry| entry.len() > 0) {
                    files[track.angle] = Some(track.file);
                } else {
                    let _ = std::fs::remove_file(file);
                    self.s.replay_status = format!("Camera {} failed to record", track.angle + 1);
                }
            }
            if files.iter().any(Option::is_some) && segment.end > segment.start {
                replay.session.segments.push(replay::Segment { start: segment.start, end: segment.end, files });
                replay.session.segments.sort_by_key(|segment| segment.start);
                changed = true;
            } else {
                self.s.replay_arm = false;
            }
        }

        let position = replay.position(now);
        if let Some(command) = self.s.replay_command.take() {
            match command {
                replay::Action::MarkIn if !replay.current.is_empty() => replay.mark_in = Some(position),
                replay::Action::CancelMark => replay.mark_in = None,
                replay::Action::MarkOut if !replay.current.is_empty() => {
                    if let Some(start) = replay.mark_in.take().filter(|start| *start < position) {
                        replay.marks.push((start, position));
                        self.s.replay_status = "Waiting for marked media to finalize…".into();
                    }
                }
                replay::Action::MarkLast(ms) if !replay.current.is_empty() => {
                    let take_start = replay.clock.map_or(position, |(_, start)| start);
                    let start = position.saturating_sub(ms).max(take_start);
                    if start < position {
                        replay.marks.push((start, position));
                        self.s.replay_status = "Waiting for marked media to finalize…".into();
                    }
                }
                replay::Action::SaveEvent(event) if !replay.session.native => {
                    if let Some(stored) = replay.session.events.iter_mut().find(|stored| stored.id == event.id) {
                        *stored = event;
                        changed = true;
                    }
                }
                replay::Action::DeleteEvent(id) if !replay.session.native => {
                    replay.session.events.retain(|event| event.id != id);
                    self.s.replay_event = replay.session.events.first().map(|event| event.id);
                    changed = true;
                }
                replay::Action::Play { event, program } => {
                    replay.sync_playback = self.s.replay_channel == 2;
                    for channel in 0..2 {
                        if (self.s.replay_channel < 2 && self.s.replay_channel != channel) || replay.playback[channel].is_some() {
                            continue;
                        }
                        let speed = self.s.replay_speed[channel];
                        if speed == 0 {
                            for input in &mut self.s.inputs {
                                if matches!(input.kind, Kind::Replay { channel: target, .. } if target == channel) { input.playing = false; }
                            }
                            continue;
                        }
                        match replay.job(event, self.s.replay_cameras[channel], speed, false) {
                            Ok(job) => {
                                replay.indexed[channel] = None;
                                let take = program && (self.s.replay_channel < 2 || channel == 0);
                                replay.playback[channel] = Some(ReplayPlayback { job, ready: None, program: take, speed });
                                self.s.replay_status = format!("Preparing Replay {}…", ["A", "B"][channel]);
                            }
                            Err(error) => self.s.replay_status = error,
                        }
                    }
                }
                replay::Action::Pause(channel) => {
                    let selected = |target: usize| channel == 2 || target == channel;
                    let resume = !replay.indexed.iter().enumerate().any(|(index, cursor)| selected(index)
                        && cursor.as_ref().is_some_and(|cursor| cursor.direction != ReplayMotion::Still && !cursor.paused))
                        && !self.s.inputs.iter().any(|input| matches!(input.kind, Kind::Replay { channel, .. }
                            if selected(channel)) && input.playing);
                    for (index, cursor) in replay.indexed.iter_mut().enumerate() {
                        if selected(index) {
                            if let Some(cursor) = cursor { cursor.paused = !resume; }
                        }
                    }
                    for input in &mut self.s.inputs {
                        if let Kind::Replay { channel, .. } = input.kind {
                            if selected(channel) {
                                input.playing = resume && replay.indexed[channel].as_ref()
                                    .is_none_or(|cursor| cursor.direction == ReplayMotion::Forward && cursor.seek.is_none());
                            }
                        }
                    }
                }
                replay::Action::Export { event, angle } if replay.export.is_none() => match replay.job(event, angle, 100, true) {
                    Ok(job) => { replay.export = Some(job); self.s.replay_status = "Exporting replay…".into(); }
                    Err(error) => self.s.replay_status = error,
                },
                _ => {}
            }
        }

        for job in replay.playback.iter_mut().flatten() {
            if job.ready.is_none() {
                job.ready = poll_replay_job(&job.job);
            }
        }
        let apply_playback = !replay.sync_playback || replay.playback.iter().flatten().all(|job| job.ready.is_some());
        for channel in 0..2 {
            let result = replay.playback[channel].as_mut().and_then(|job| {
                if !apply_playback { return None; }
                job.ready.take().map(|result| (result, job.program, job.speed))
            });
            if let Some((result, program, speed)) = result {
                replay.playback[channel] = None;
                match result {
                    Ok(path) => {
                        let id = self.s.ensure_replay_inputs()[channel];
                        if let Some(input) = self.s.input_mut(id) {
                            input.kind = Kind::Replay { path: path.to_string_lossy().into_owned(), channel };
                            input.playing = true;
                            input.pos = 0.0;
                        }
                        if program {
                            let mut transition = self.s.replay_setup.transition_ms;
                            if self.s.replay_setup.match_speed {
                                transition = transition.saturating_mul(100) / u32::from(speed.max(1));
                            }
                            self.s.transition_input(id, if transition == 0 { Effect::Cut } else { Effect::Fade }, transition);
                        } else if !replay.sync_playback || channel == 0 {
                            self.s.preview = Some(id);
                        }
                        self.s.replay_status = format!("Replay {} playing", ["A", "B"][channel]);
                    }
                    Err(error) => self.s.replay_status = error,
                }
            }
        }
        if let Some(result) = replay.export.as_ref().and_then(poll_replay_job) {
            replay.export = None;
            self.s.replay_status = match result {
                Ok(path) => format!("Exported to {}", path.display()),
                Err(error) => error,
            };
        }

        if self.s.replay_arm && self.program.lock().unwrap().error.is_some()
            && self.s.replay_angles.iter().any(|&source| !matches!(source, SRC_OUTPUT | MV_AUTO))
        {
            self.s.replay_arm = false;
            self.s.replay_status = "Multicamera replay requires offscreen capture".into();
        }
        if !replay.current.is_empty() && (!self.s.replay_arm || (
            replay.finalizing.len() < 2
                && now - replay.current[0].recording.started >= replay::SEGMENT_MS as f64 / 1000.0))
        {
            let start = replay.position(replay.current[0].recording.started);
            let tracks = replay.current.drain(..).map(|track| FinalizingTrack {
                angle: track.angle, file: track.file, error: track.recording.recorder.error.clone(), worker: track.worker,
            }).collect();
            replay.finalizing.push(FinalizingReplay { start, end: position, tracks });
        }

        if self.s.replay_arm && replay.clock.is_none() {
            let setup = &self.s.replay_setup;
            let format = replay::Format {
                size: setup.size.unwrap_or(self.record_format.0),
                fps: setup.fps.unwrap_or(self.record_format.1 as f32),
                codec: media::RecordFormat::parse(&setup.codec).key().to_owned(),
            };
            if replay.session.format.as_ref().is_some_and(|old| *old != format) {
                self.s.replay_arm = false;
                self.s.replay_status = "Recording format differs from this session. Restore it or create a new session.".into();
            } else {
                replay.session.format = Some(format);
            }
        }
        if self.s.replay_arm && replay.current.is_empty() {
            if replay.clock.is_none() {
                let last = replay.session.segments.last().map_or(0, |segment| segment.end);
                replay.clock = Some((now, last));
                replay.session.setup = self.s.replay_setup.clone();
                replay.session.camera_names = self.s.replay_angles.map(|source| {
                    if source == SRC_OUTPUT { "Program".into() }
                    else { source.checked_sub(1).and_then(|id| self.s.input(id)).map(|input| input.title.clone()).unwrap_or_default() }
                });
                changed = true;
            }
            let stamp = replay_stamp();
            let setup = &self.s.replay_setup;
            let [w, h] = setup.size.unwrap_or(self.record_format.0);
            let fps = setup.fps.unwrap_or(self.record_format.1 as f32);
            let options = media::RecOpts { mbps: setup.mbps.clamp(1, 100), size: Some((w, h)),
                fps, hw: self.s.cfg.hw_encoder, format: media::RecordFormat::parse(&setup.codec), audio: true };
            let timeline = Arc::new(std::sync::OnceLock::new());
            let mut seen = std::collections::HashSet::new();
            for (angle, source) in self.s.replay_angles.into_iter().enumerate() {
                if source == MV_AUTO || !seen.insert(source) { continue; }
                if source != SRC_OUTPUT && !self.s.input(source - 1).is_some_and(|input| !matches!(input.kind, Kind::Replay { .. })) { continue; }
                let file = format!("camera-{}-{stamp}.mp4", angle + 1);
                let route = OutputRoute { source, overlays: source == SRC_OUTPUT };
                let (recorder, worker) = media::Recorder::spawn_writer_with_timeline(
                    folder.join(&file).to_string_lossy().into_owned(), options, Some(timeline.clone()));
                let recording = ComposedRecording { recorder: Arc::new(recorder), channel: 0,
                    route_override: Some(route), audio_override: match setup.audio {
                        0 => None,
                        1 => Some(SRC_OUTPUT),
                        n => self.s.replay_angles.get(usize::from(n - 2)).copied().filter(|&source| source != MV_AUTO),
                    }, started: now };
                replay.current.push(ReplayTrack { recording, angle, file, worker });
            }
            if replay.current.is_empty() {
                self.s.replay_arm = false;
                self.s.replay_status = "Select at least one available camera".into();
            } else if self.s.replay_status == "Replay session ready" {
                self.s.replay_status = format!("Recording {} camera(s)", replay.current.len());
            }
        }
        if !self.s.replay_arm {
            if let Some(start) = replay.mark_in.take().filter(|start| *start < position) {
                replay.marks.push((start, position));
            }
            if replay.finalizing.is_empty() { replay.clock = None; }
        }

        let ready_until = replay.session.segments.last().map_or(0, |segment| segment.end);
        replay.marks.retain(|&(start, end)| {
            if end <= ready_until && !replay.finalizing.iter().any(|segment| segment.start < end && segment.end > start) {
                let id = replay.session.new_event(start, end);
                self.s.replay_event = Some(id);
                self.s.replay_status = format!("Event {id} saved");
                changed = true;
                false
            } else if !self.s.replay_arm && replay.finalizing.is_empty() {
                self.s.replay_status = "Marked range could not be recorded".into();
                false
            } else { true }
        });
        if changed && self.s.replay_setup.delete_old && replay.export.is_none() && replay.playback.iter().all(Option::is_none) {
            let mark = replay.mark_in.into_iter().chain(replay.marks.iter().map(|range| range.0))
                .chain(replay.indexed.iter().flatten().map(|cursor| cursor.position as u64)).min();
            replay.session.prune(&folder, ready_until.saturating_sub(self.s.replay_setup.buffer_seconds * 1000), mark);
        }
        if changed {
            self.s.replay_events = replay.session.events.clone();
            if let Err(error) = replay.save() {
                self.s.replay_status = format!("Could not save replay session: {error}");
            }
        }
        self.s.replay_marking = replay.mark_in.is_some();
        self.update_indexed_replay(now);
    }

    fn recording_error(&self, recording: &ComposedRecording) -> Option<String> {
        let error = recording.recorder.error.lock().unwrap();
        if !error.is_empty() {
            return Some(error.clone());
        }
        if self.program.lock().unwrap().error.is_some() && !self.can_capture_from_window(recording) {
            return Some(format!("Output {} requires offscreen capture", recording.channel + 1));
        }
        None
    }

    fn can_capture_from_window(&self, recording: &ComposedRecording) -> bool {
        recording.route(&self.s.cfg) == OutputRoute::default()
    }

    fn update_recording(&mut self, now: f64) {
        if self.composed_recordings().next().is_none() {
            let (w, h) = self.s.cfg.out_dims();
            let format = ([w, h], f64::from(self.s.cfg.frame_rate()));
            if self.record_format != format {
                self.record_format = format;
                self.next_record_frame = now;
            }
        }
        let ([w, h], fps) = self.record_format;
        let options = |mbps, format, hw, audio| media::RecOpts { mbps, size: Some((w, h)), fps: fps as f32, hw, format, audio };

        if self.s.rec.is_none() {
            if let Some(primary) = self.recordings[0].take() {
                self.s.notice(&format!("Recording stopped: {}", primary.recorder.path));
            }
            self.recordings[1] = None;
        } else {
            let c = &self.s.cfg;
            let split = f64::from(c.rec_split_min) * 60.0;
            if self.recordings[0].as_ref().is_some_and(|r| split > 0.0 && now - r.started >= split) {
                self.recordings = Default::default();
            }
            if self.recordings[0].is_none() {
                let name = safe_name(&c.rec_name);
                self.recordings[0] = Some(ComposedRecording::new(&c.rec_folder, &name, c.rec_source, now,
                    options(c.rec_mbps, c.rec_codec, c.hw_encoder, c.record_audio[0])));
                if c.rec_second {
                    self.recordings[1] = Some(ComposedRecording::new(&c.rec_folder, &format!("{name} - 2"), c.rec_second_source, now,
                        options(c.rec_second_mbps, c.rec_second_codec, c.hw_encoder, c.record_audio[1])));
                }
                let path = &self.recordings[0].as_ref().unwrap().recorder.path;
                self.s.notice(&format!("Recording to {path}"));
            }
            if let Some(error) = self.recordings[0].as_ref().and_then(|r| self.recording_error(r)) {
                self.s.rec = None;
                self.recordings = Default::default();
                self.s.notice(&error);
            } else if let Some(error) = self.recordings[1].as_ref().and_then(|r| self.recording_error(r)) {
                self.recordings[1] = None;
                self.s.notice(&format!("Recording 2: {error}"));
            }
        }

        let selected_camera = self.s.inputs.iter().any(|i| matches!(i.kind, Kind::Device { .. }) && !self.s.mc_off.contains(&i.id));
        if self.s.multicorder.is_some() && !selected_camera && !self.s.cfg.mc_outputs.contains(&true) {
            self.s.multicorder = None;
            self.s.notice("Select a camera or Output 1–4 in MultiCorder");
        }
        let split = f64::from(self.s.cfg.mc_split_min) * 60.0;
        for channel in 0..OUTPUT_COUNT {
            if self.s.multicorder.is_none() || !self.s.cfg.mc_outputs[channel] {
                self.multi_outputs[channel] = None;
                self.multi_output_failed[channel] = false;
                continue;
            }
            if self.multi_output_failed[channel] {
                continue;
            }
            if self.multi_outputs[channel].as_ref().is_some_and(|r| split > 0.0 && now - r.started >= split) {
                self.multi_outputs[channel] = None;
            }
            if self.multi_outputs[channel].is_none() {
                let c = &self.s.cfg;
                self.multi_outputs[channel] = Some(ComposedRecording::new(&c.mc_folder, &format!("MultiCorder Output {}", channel + 1), channel, now,
                    options(c.mc_mbps, c.mc_codec, c.hw_encoder, c.record_audio[2])));
            }
            if let Some(error) = self.multi_outputs[channel].as_ref().and_then(|r| self.recording_error(r)) {
                self.multi_outputs[channel] = None;
                self.multi_output_failed[channel] = true;
                self.s.notice(&format!("MultiCorder Output {}: {error}", channel + 1));
            }
        }
        for (id, recorder) in &self.multi {
            let error = std::mem::take(&mut *recorder.error.lock().unwrap());
            if !error.is_empty() {
                self.s.notice(&format!("MultiCorder Input {}: {error}", self.s.number_of(*id)));
            }
        }
    }

    fn advance_record_clock(&mut self, now: f64) {
        let fps = self.record_format.1;
        let elapsed = ((now - self.next_record_frame) * fps).floor() as u64;
        self.next_record_frame += (elapsed + 1) as f64 / fps;
        for recording in self.composed_recordings() {
            recording.recorder.note_dropped_frames(elapsed);
        }
    }

    fn update_scopes(&mut self, ctx: &egui::Context, now: f64) {
        let key = (self.s.preview, self.s.cfg.wave_type, self.s.cfg.vs_zoom);
        if key != self.scope_key {
            self.scope_key = key;
            self.next_wave = now;
            self.s.scope_tex.clear();
        }
        let frame = self.scope_frame.lock().unwrap().take();
        if let Some(frame) = frame.filter(|frame| frame.key == key && key.1 != 0) {
            let _ = self.scope_jobs.try_send(frame);
        }
        let images = self.scope_images.lock().unwrap().take();
        if let Some((_, images)) = images.filter(|(frame_key, _)| *frame_key == key && key.1 != 0) {
            let mut ids = Vec::new();
            for (kind, image) in images {
                let tex = self.scopes.entry(kind).or_insert_with(|| {
                    ctx.load_texture(format!("scope-{kind}"), egui::ColorImage::new([1, 1], Color32::TRANSPARENT), egui::TextureOptions::LINEAR)
                });
                tex.set(image, egui::TextureOptions::LINEAR);
                ids.push((kind, tex.id()));
            }
            self.s.scope_tex = ids;
        }
    }

    fn update_decklink_outputs(&mut self) {
        for channel in 0..OUTPUT_COUNT {
            let route = self.s.cfg.decklink_outputs[channel];
            let wanted = route.device.filter(|_| self.s.decklink_active[channel]).map(|device| decklink::OutputConfig {
                device, mode: route.mode, port: route.port,
                size: self.s.cfg.out_dims(), fps: self.s.cfg.frame_rate(), interlaced: self.s.cfg.interlaced,
            });
            let slot = &mut self.decklink_outputs[channel];
            if slot.as_ref().map(|(cfg, _)| *cfg) != wanted {
                if let Some((_, sink)) = slot {
                    sink.stop();
                    if !sink.finished() {
                        continue;
                    }
                }
                *slot = None;
                if let Some(cfg) = wanted {
                    *slot = Some((cfg, decklink::OutputSink::start(cfg)));
                }
            }
            if let Some((_, sink)) = slot {
                if let Some(error) = sink.error() {
                    self.s.decklink_status[channel] = error.clone();
                    self.s.decklink_active[channel] = false;
                    self.s.notice(&format!("DeckLink Output {}: {error}", channel + 1));
                } else {
                    self.s.decklink_status[channel] = if sink.frames() > 0 {
                        "Sending DeckLink output".into()
                    } else {
                        "Starting DeckLink output…".into()
                    };
                }
            } else if self.s.decklink_active[channel] {
                self.s.decklink_active[channel] = false;
            }
        }
    }

    fn update_network_outputs(&mut self) {
        let offscreen_error = self.program.lock().unwrap().error.clone();
        for (channel, slot) in self.ndi_outputs.iter_mut().enumerate() {
            let active = &mut self.s.cfg.ndi_outputs[channel];
            let status = &mut self.s.ndi_status[channel];
            if let Some(error) = offscreen_error.as_ref().filter(|_| *active) {
                *status = format!("NDI requires offscreen video: {error}");
                *active = false;
            }
            let fps = self.record_format.1;
            if slot.as_ref().is_some_and(|(rate, _)| !*active || *rate != fps) {
                slot.take().unwrap().1.stop();
            }
            if *active {
                let (_, sender) = slot.get_or_insert_with(|| {
                    (fps, ndi::Sender::start(format!("LiveMix Output {}", channel + 1), fps))
                });
                let (message, failed) = sender.status();
                *status = message;
                if failed { *active = false; }
            } else if status.starts_with("Sending") || status.starts_with("Starting") || status.starts_with("Waiting") {
                status.clear();
            }
        }
        for (channel, slot) in self.omt_outputs.iter_mut().enumerate() {
            let active = &mut self.s.cfg.omt_outputs[channel];
            let status = &mut self.s.omt_status[channel];
            let config = (self.record_format.1, self.s.cfg.omt_quality[channel]);
            if let Some(error) = offscreen_error.as_ref().filter(|_| *active) {
                *status = format!("OMT requires offscreen video: {error}");
                *active = false;
            }
            if slot.as_ref().is_some_and(|(previous, _)| !*active || *previous != config) {
                slot.take().unwrap().1.stop();
            }
            if *active {
                let (_, sender) = slot.get_or_insert_with(|| {
                    (config, omt::Sender::start(format!("LiveMix Output {}", channel + 1), config.0, config.1))
                });
                let (message, failed) = sender.status();
                *status = message;
                if failed { *active = false; }
            }
        }
        for (channel, slot) in self.srt_outputs.iter_mut().enumerate() {
            let active = &mut self.s.srt_active[channel];
            let status = &mut self.s.srt_status[channel];
            if let Some(error) = offscreen_error.as_ref().filter(|_| *active) {
                *status = format!("SRT requires offscreen video: {error}");
                *active = false;
            }
            if slot.as_ref().is_some_and(|(format, _)| !*active || *format != self.record_format) {
                slot.take().unwrap().1.stop();
            }
            if *active && slot.is_none() {
                let (size, fps) = self.record_format;
                match stream::Output::start_srt(&self.s.cfg.srt_outputs[channel], size, fps) {
                    Ok(output) => *slot = Some((self.record_format, output)),
                    Err(error) => { *status = error; *active = false; }
                }
            }
            if let Some((_, output)) = slot {
                let (message, failed) = output.status();
                *status = message;
                if failed { *active = false; }
            }
        }
        let mut groups: [Vec<(usize, stream::Destination)>; stream::COUNT] = Default::default();
        for (index, destination) in self.s.cfg.streams.iter().enumerate() {
            let status = &mut self.s.stream_status[index];
            if !self.s.stream_active[index] {
                if status.starts_with("Live") || status.starts_with("Connecting") { status.clear(); }
                continue;
            }
            let error = offscreen_error.as_ref().map(|error| format!("Streaming requires offscreen video: {error}"))
                .or_else(|| stream::publish_url(destination).err());
            if let Some(error) = error {
                *status = error;
                self.s.stream_active[index] = false;
                continue;
            }
            let leader = groups.iter().position(|group| group.first().is_some_and(|(_, first)| {
                (first.source, first.direct, first.quality, first.bitrate, first.hardware, first.hevc)
                    == (destination.source, destination.direct, destination.quality, destination.bitrate, destination.hardware, destination.hevc)
            })).unwrap_or(index);
            groups[leader].push((index, destination.clone()));
        }
        for (slot, group) in self.stream_outputs.iter_mut().zip(groups) {
            if slot.as_ref().is_some_and(|(format, previous, _)| *format != self.record_format || *previous != group) {
                slot.take().unwrap().2.stop();
            }
            if group.is_empty() { continue; }
            if slot.is_none() {
                let (size, fps) = self.record_format;
                let destinations: Vec<_> = group.iter().map(|(_, destination)| destination).collect();
                match stream::Output::start_group(&destinations, size, fps) {
                    Ok(output) => *slot = Some((self.record_format, group.clone(), output)),
                    Err(error) => {
                        for (index, _) in group {
                            self.s.stream_status[index] = error.clone();
                            self.s.stream_active[index] = false;
                        }
                        continue;
                    }
                }
            }
            if let Some((_, _, output)) = slot {
                let (message, failed) = output.status();
                for (index, _) in group {
                    self.s.stream_status[index] = message.clone();
                    if failed { self.s.stream_active[index] = false; }
                }
            }
        }
    }

    fn audio_input(&self, input: &Input) -> Option<(&media::Shared, bool, f32)> {
        let source = self.media.get(&input.id)?.shared.as_deref()?;
        let file = matches!(input.kind, Kind::VideoFile { .. } | Kind::AudioFile { .. } | Kind::VideoList { .. } | Kind::Replay { .. });
        let gain = if input.audio_on { input.volume / 100.0 } else { 0.0 };
        Some((source, file, gain))
    }

    /// Peak level of every audio input, kept for the mixer strips and the Input Settings meter.
    fn update_levels(&mut self) {
        let at = Instant::now();
        let levels: Vec<(u32, [f32; 2])> = self.s.inputs.iter().filter(|input| input.kind.has_audio())
            .map(|input| (input.id, self.audio_input(input).map_or([0.0; 2], |source| audio::input_peaks(source, at))))
            .collect();
        for (id, level) in levels {
            if let Some(input) = self.s.input_mut(id) {
                input.level = level;
            }
        }
    }

    fn mix_audio(&self, route: u32, at: Instant, frames: usize) -> audio::Packet {
        let master = matches!(route, SRC_OUTPUT | SRC_MV1 | SRC_MV2);
        let selected = match route {
            SRC_PREVIEW => self.s.preview,
            _ => route.checked_sub(1),
        };
        let gain = if master { self.master_gain() } else { 1.0 };
        audio::mix(self.s.inputs.iter().filter(|input| {
            if master { input.bus_m } else { Some(input.id) == selected }
        }).filter_map(|input| {
            let (source, file, level) = self.audio_input(input)?;
            let auto = if master { self.s.automatic_audio_gain(input) } else { 1.0 };
            Some((source, file, level * gain * auto))
        }), at, frames)
    }

    fn master_gain(&self) -> f32 {
        let fade = if self.s.ftb_audio { 1.0 - self.s.ftb_level / FULL } else { 1.0 };
        if self.s.master_on { self.s.master_volume / 100.0 * fade } else { 0.0 }
    }

    fn update_audio_monitor(&mut self) {
        let Some(monitor) = &self.audio_monitor else { return };
        let error = monitor.error.lock().unwrap().take();
        if let Some(error) = error {
            self.audio_monitor = None;
            self.mixer.last_error = Some(error.clone());
            self.s.notice(&format!("Audio monitor: {error}"));
            return;
        }
        let solo = self.s.inputs.iter().any(|input| input.solo && input.kind.has_audio());
        let bus_gain = if self.mixer.bus == 0 { self.master_gain() } else { 1.0 };
        let headphones = self.mixer.headphones / 100.0;
        let sources = self.s.inputs.iter().filter(|input| {
            if solo {
                return input.solo && input.kind.has_audio();
            }
            match self.mixer.bus {
                1 => input.bus_a,
                2 => input.bus_b,
                _ => input.bus_m,
            }
        }).filter_map(|input| {
            let (_, file, volume) = self.audio_input(input)?;
            let gain = if solo {
                if input.solo_pfl { 1.0 } else { volume }
            } else {
                volume * bus_gain * self.s.automatic_audio_gain(input)
            };
            Some((self.media.get(&input.id)?.shared.clone()?, file, gain * headphones))
        }).collect::<Vec<_>>().into();
        *monitor.sources.lock().unwrap() = sources;
    }

    /// Capture the Program when due and record the elapsed time at the end of drawing.
    fn finish_frame(&mut self, ctx: &egui::Context, now: f64, started: Instant) {
        let (available, scopes_available, pending) = {
            let state = self.program.lock().unwrap();
            (state.error.is_none(), state.scope_error.is_none(),
                state.target.as_ref().is_some_and(program::Target::has_pending)
                || state.scope_target.as_ref().is_some_and(program::Target::has_pending))
        };
        let mut frames = Vec::new();
        let any_output = self.decklink_outputs.iter().any(Option::is_some)
            || self.ndi_outputs.iter().flatten().any(|(_, sink)| sink.subscribed())
            || self.omt_outputs.iter().flatten().any(|(_, sink)| sink.subscribed())
            || self.srt_outputs.iter().any(Option::is_some)
            || self.stream_outputs.iter().any(Option::is_some)
            || self.composed_recordings().next().is_some();
        if available && now >= self.next_record_frame && any_output {
            self.advance_record_clock(now);
            let size = self.record_format.0;
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
            let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("program-offscreen"));
            let mut groups: Vec<(OutputRoute, u32, program::Destinations)> = Vec::new();
            for recording in self.composed_recordings() {
                let route = recording.route(&self.s.cfg);
                let audio_source = recording.audio_override.unwrap_or(route.source);
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, _)| *existing == route && *audio == audio_source) {
                    destinations.recorders.push(Arc::clone(&recording.recorder));
                } else {
                    groups.push((route, audio_source, program::Destinations { recorders: vec![Arc::clone(&recording.recorder)], ..Default::default() }));
                }
            }
            for (channel, (_, sink)) in self.decklink_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))) {
                let route = self.s.cfg.outputs[channel];
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, _)| *existing == route && *audio == route.source) {
                    destinations.decklinks.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { decklinks: vec![sink.clone()], ..Default::default() }));
                }
            }
            for (channel, (_, sink)) in self.ndi_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))).filter(|(_, (_, sink))| sink.subscribed()) {
                let route = self.s.cfg.outputs[channel];
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, _)| *existing == route && *audio == route.source) {
                    destinations.ndi.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { ndi: vec![sink.clone()], ..Default::default() }));
                }
            }
            for (channel, (_, sink)) in self.omt_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))).filter(|(_, (_, sink))| sink.subscribed()) {
                let route = self.s.cfg.outputs[channel];
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, _)| *existing == route && *audio == route.source) {
                    destinations.omt.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { omt: vec![sink.clone()], ..Default::default() }));
                }
            }
            for (channel, (_, sink)) in self.srt_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))) {
                let route = self.s.cfg.outputs[channel];
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, _)| *existing == route && *audio == route.source) {
                    destinations.streams.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { streams: vec![sink.clone()], ..Default::default() }));
                }
            }
            for (index, (_, _, sink)) in self.stream_outputs.iter().enumerate().filter_map(|(index, sink)| sink.as_ref().map(|sink| (index, sink))) {
                let route = stream_route(&self.s, &self.s.cfg.streams[index]);
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, _)| *existing == route && *audio == route.source) {
                    destinations.streams.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { streams: vec![sink.clone()], ..Default::default() }));
                }
            }
            let captured_at = Instant::now();
            for (route, audio_source, mut recorders) in groups {
                let count = (f64::from(audio::RATE) / self.record_format.1).ceil() as usize;
                recorders.audio = self.mix_audio(audio_source, captured_at, count);
                draw_src(&egui::Painter::new(ctx.clone(), layer, rect), rect, &self.s, now, route.source, route.overlays);
                let shapes = ctx.graphics_mut(|graphics| {
                    std::mem::take(graphics.entry(layer)).all_entries().cloned().collect()
                });
                frames.push((size, ctx.tessellate(shapes, 1.0), captured_at, recorders));
            }
        }
        if scopes_available && self.scope_key.1 != 0 && now >= self.next_wave {
            self.next_wave = now + 0.1;
            let size = [320, 180];
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
            let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("scopes-offscreen"));
            let painter = egui::Painter::new(ctx.clone(), layer, rect);
            if let Some(input) = self.scope_key.0.and_then(|id| self.s.input(id)) {
                draw_source(&painter, rect, input, 1.0, now, &self.s.inputs);
            }
            draw_overlay_channels(&painter, rect, &self.s, now, true);
            let shapes = ctx.graphics_mut(|graphics| {
                std::mem::take(graphics.entry(layer)).all_entries().cloned().collect()
            });
            frames.push((size, ctx.tessellate(shapes, 1.0), Instant::now(), program::Destinations {
                scope: Some((self.scope_key, Arc::clone(&self.scope_frame))), ..Default::default()
            }));
        }
        let program = Arc::clone(&self.program);
        let peak = Arc::clone(&self.performance.render_peak);
        #[cfg(windows)]
        let uploads = std::mem::take(&mut self.pending_gpu);
        ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::new("program-capture")))
            .add(egui::PaintCallback {
                rect: ctx.screen_rect(),
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_, painter| {
                    #[cfg(windows)]
                    if !uploads.is_empty() {
                        let mut state = program.lock().unwrap();
                        let bridge = state.input_bridge.get_or_insert_with(|| unsafe { dx_interop::Bridge::new(painter.gl()) });
                        for upload in &uploads {
                            let result = bridge.as_ref().map_err(String::as_str).and_then(|bridge| {
                                let texture = painter.texture(upload.texture.id()).ok_or("OpenGL input texture unavailable")?;
                                unsafe { bridge.import(painter.gl(), &upload.picture, texture, upload.resize) }
                                    .map_err(|_| "OpenGL could not import the decoded surface")
                            });
                            if result.is_err() {
                                use std::sync::atomic::Ordering::{Relaxed, Release};
                                upload.source.gpu_playback.store(false, Release);
                                let position = upload.source.pos_ms.load(Relaxed);
                                let _ = upload.source.seek_ms.compare_exchange(0, position.saturating_add(1), Release, Relaxed);
                            }
                        }
                    }
                    if pending || !frames.is_empty() {
                        let mut state = program.lock().unwrap();
                        let result = unsafe {
                            (|| {
                                if let Some(target) = state.target.as_mut() {
                                    target.collect(painter.gl())?;
                                }
                                if let Some(target) = state.scope_target.as_mut() {
                                    if let Err(error) = target.collect(painter.gl()) {
                                        state.scope_error = Some(error);
                                    }
                                }
                                for (size, meshes, captured_at, destinations) in &frames {
                                    if destinations.scope.is_some() {
                                        let capture = (|| {
                                            if state.scope_target.is_none() {
                                                state.scope_target = Some(program::Target::new(painter.gl())?);
                                            }
                                            state.scope_target.as_mut().unwrap().capture(painter, *size, meshes, *captured_at, destinations)
                                        })();
                                        if let Err(error) = capture { state.scope_error = Some(error); }
                                        continue;
                                    }
                                    if state.target.is_none() {
                                        state.target = Some(program::Target::new(painter.gl())?);
                                    }
                                    state.target.as_mut().unwrap().capture(painter, *size, meshes, *captured_at, destinations)?;
                                }
                                Ok::<(), String>(())
                            })()
                        };
                        if let Err(error) = result {
                            state.error = Some(error);
                            for (_, _, _, destinations) in &frames {
                                for recorder in &destinations.recorders { recorder.note_dropped_frames(1); }
                            }
                        }
                    }
                    peak.fetch_max(started.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
                })),
            });
    }

    /// Window pictures for screenshots, scopes, snapshots and recording fallback.
    fn handle_screenshots(&mut self, ctx: &egui::Context, now: f64) {
        let check_due = self.screenshot.is_some() && self.frame >= 120;
        let mut got = None;
        ctx.input(|i| {
            for e in &i.raw.events {
                if let egui::Event::Screenshot { image, .. } = e {
                    got = Some(image.clone());
                }
            }
        });
        let recorders: Vec<_> = if got.is_some() {
            self.shot_pending = false;
            let shot_at = self.shot_recording.take();
            self.composed_recordings()
                .filter(|r| shot_at.is_some_and(|at| r.started <= at) && self.can_capture_from_window(r))
                .map(|r| Arc::clone(&r.recorder)).collect()
        } else {
            Vec::new()
        };
        let record_due = self.composed_recordings().any(|r| self.can_capture_from_window(r)) && now >= self.next_record_frame
            && self.program.lock().unwrap().error.is_some();
        let want = record_due || (got.is_none() && (check_due || self.s.snapshot_req));
        if want && !self.shot_pending {
            self.shot_pending = true;
            self.shot_recording = record_due.then_some(now);
            if record_due {
                self.advance_record_clock(now);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let Some(img) = got else { return };
        let ppp = ctx.pixels_per_point();
        let [iw, ih] = img.size;
        let crop = |r: egui::Rect| -> (Vec<u8>, usize, usize) {
            let x0 = ((r.min.x * ppp).round().max(0.0) as usize).min(iw);
            let y0 = ((r.min.y * ppp).round().max(0.0) as usize).min(ih);
            let x1 = ((r.max.x * ppp).round().max(0.0) as usize).min(iw);
            let y1 = ((r.max.y * ppp).round().max(0.0) as usize).min(ih);
            let (w, h) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
            let mut out = Vec::with_capacity(w * h * 4);
            for y in y0..y1 {
                for x in x0..x1 {
                    let [r, g, b, _] = img.pixels[y * iw + x].to_srgba_unmultiplied();
                    out.extend_from_slice(&[r, g, b, 255]);
                }
            }
            (out, w, h)
        };
        let out_rect = self.s.output_rect;
        if out_rect.is_positive() && (self.s.snapshot_req || !recorders.is_empty()) {
            let (px, w, h) = crop(out_rect);
            if self.s.snapshot_req && w > 0 && h > 0 && self.snap_rx.is_none() {
                // Like the original (kb::c @0xe3850): a Save dialog with PNG/JPEG/BMP and the
                // name "<prefix>dd MMMM yyyy HH-mm-ss"; the chosen filter is remembered.
                self.s.snapshot_req = false;
                self.snap_pending = Some((px.clone(), w, h));
                self.snap_rx = Some(media::save_picture_dialog(media::snapshot_name("LiveMix Capture "), self.photo_filter));
            }
            if !recorders.is_empty() && w > 0 && h > 0 {
                let at = Instant::now();
                let count = (f64::from(audio::RATE) / self.record_format.1).ceil() as usize;
                let audio = self.mix_audio(SRC_OUTPUT, at, count);
                media::Recorder::push_group_audio(&recorders, px, w, h, at, false, audio);
            }
        }
        if check_due {
            if let Some(path) = self.screenshot.clone() {
                let (px, w, h) = crop(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(iw as f32 / ppp, ih as f32 / ppp)));
                if let Some(dir) = std::path::Path::new(&path).parent() {
                    if !dir.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                }
                if let Err(e) = image::save_buffer(&path, &px, w as u32, h as u32, image::ColorType::Rgba8) {
                    eprintln!("could not save screenshot: {e}");
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// File picker and device list requests from the Add Input dialog.
    fn handle_requests(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.snap_rx {
            if let Ok(res) = rx.try_recv() {
                self.snap_rx = None;
                if let (Some((path, idx)), Some((px, w, h))) = (res, self.snap_pending.take()) {
                    self.photo_filter = idx;
                    match media::save_picture(&path, &px, w, h) {
                        Ok(()) => self.s.notice(&format!("Snapshot saved: {path}")),
                        Err(e) => self.s.notice(&format!("Could not save snapshot: {e}")),
                    }
                }
                self.snap_pending = None;
            }
        }
        if let Some(kind) = self.s.browse_req.take() {
            if self.browse_rx.is_none() {
                self.browse_kind = kind;
                self.browse_rx = Some(media::browse(kind));
            }
        }
        if let Some(rx) = &self.browse_rx {
            if let Ok(res) = rx.try_recv() {
                let waiting = self.s.new_step == NewStep::WaitingForSave
                    && self.browse_kind == media::PickKind::PresetSave;
                if waiting {
                    self.s.new_step = NewStep::Idle;
                }
                if let Some(p) = res {
                    match (self.browse_kind, self.s.browse_for) {
                        (media::PickKind::PresetOpen, _) => self.s.load_preset_from(std::path::Path::new(&p)),
                        (media::PickKind::PresetSave, _) => {
                            if self.s.save_preset_to(std::path::Path::new(&p)) && waiting {
                                self.s.new_step = NewStep::Format;
                            }
                        }
                        // Recording Setup "Browse" / MultiCorder "Change Recording Folder".
                        (media::PickKind::Folder, 1) => self.s.cfg.rec_folder = p,
                        (media::PickKind::Folder, 2) => self.s.cfg.mc_folder = p,
                        (media::PickKind::Folder, 3) => self.s.replay_setup.folder = p,
                        (kind, _) => {
                            let d = &mut self.s.add_dialog;
                            match kind {
                                media::PickKind::VideoMany => d.list.extend(p.lines().map(String::from)),
                                media::PickKind::Folder => d.folder = p,
                                _ => d.path = p,
                            }
                            self.s.dialog_error.clear();
                        }
                    }
                    self.s.save_settings();
                }
                self.s.browse_for = 0;
                self.browse_rx = None;
            }
        }
        let d = &mut self.s.add_dialog;
        if d.open && d.kind == AddKind::Desktop && d.ndi_scan && self.network_scan.is_none() {
            d.ndi_scan = false;
            d.thumbnails.clear();
            self.thumbnail_attempts.clear();
            self.network_thumbnail = None;
            d.ndi_status = "Discovering NDI sources…".into();
            d.omt_status = "Discovering OMT sources…".into();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let results = std::thread::scope(|scope| {
                    let ndi = scope.spawn(ndi::discover);
                    let omt = omt::discover();
                    [ndi.join().unwrap_or_else(|_| Err("NDI discovery failed".into())), omt]
                });
                let _ = tx.send(results);
            });
            self.network_scan = Some(rx);
        }
        if let Some([ndi, omt]) = self.network_scan.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.network_scan = None;
            match ndi {
                Ok(names) => {
                    d.ndi_status = format!("{} NDI source(s) found", names.len());
                    d.ndi_sources = names;
                }
                Err(error) => d.ndi_status = error,
            }
            match omt {
                Ok(names) => {
                    d.omt_status = format!("{} OMT source(s) found", names.len());
                    d.omt_sources = names;
                }
                Err(error) => d.omt_status = error,
            }
        }
        update_network_thumbnails(ctx, d, self.s.cfg.network_thumbnails && self.network_scan.is_none(),
            &mut self.network_thumbnail, &mut self.thumbnail_attempts);
        if self.s.refresh_devices || (d.open && d.kind.uses_device() && !d.devices_loaded) {
            self.s.refresh_devices = false;
            let decklinks = decklink::refresh_devices();
            self.s.add_dialog.devices = decklinks.into_iter().filter(|device| device.capture)
                .map(|device| DeviceChoice { name: device.name, link: format!("decklink:{}", device.index), blackmagic: true })
                .chain(media::list_devices()
                .into_iter().chain(audio::devices())
                .map(|x| DeviceChoice { name: x.name, link: x.link, blackmagic: x.blackmagic }))
                .collect();
            self.s.add_dialog.devices_loaded = true;
            self.s.add_dialog.device = 0;
        }
    }
}

#[cfg(all(test, windows))]
#[test]
fn replay_marks_without_rotating_records_and_reopens_both_channels() {
    let mut app = App::new(None);
    app.s.cfg = Settings::default();
    app.s.cfg.hw_encoder = false;
    let folder = std::env::temp_dir().join(format!("livemix-replay-test-{}", replay_stamp()));
    app.s.replay_setup.folder = folder.to_string_lossy().into_owned();
    app.s.replay_load = Some(true);
    app.record_format = ([320, 180], 25.0);
    app.s.replay_angles[1] = app.s.inputs[0].id + 1;
    app.update_replay(0.0);
    app.s.replay_arm = true;
    app.update_replay(0.0);
    assert_eq!(app.replay.current.len(), 2);
    let original = app.replay.current[0].file.clone();
    let start = Instant::now();
    for index in 0..20 {
        for track in &app.replay.current {
            track.recording.recorder.push_audio([220, 60, 30, 255].repeat(320 * 180), 320, 180,
                start + std::time::Duration::from_millis(index * 40), audio::test_tone(1920));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    app.s.replay_command = Some(replay::Action::MarkIn);
    app.update_replay(0.08);
    app.s.replay_command = Some(replay::Action::MarkOut);
    app.update_replay(0.68);
    assert_eq!(app.replay.current[0].file, original);
    assert!(app.s.replay_events.is_empty());
    app.s.replay_arm = false;
    app.update_replay(0.8);
    let deadline = Instant::now() + std::time::Duration::from_secs(8);
    while app.s.replay_events.is_empty() && Instant::now() < deadline {
        app.update_replay(0.8);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(app.s.replay_events.len(), 1, "{}", app.s.replay_status);
    let event = &app.s.replay_events[0];
    assert_eq!((event.start, event.end), (80, 680));
    assert_eq!(event.angles[..2], [true, true]);
    let id = event.id;
    app.s.replay_load = Some(false);
    app.update_replay(0.8);
    assert_eq!(app.s.replay_event, Some(id));
    if crate::stream::command().arg("-version").output().is_ok_and(|output| output.status.success()) {
        app.s.replay_channel = 2;
        app.s.replay_cameras = [0, 1];
        app.s.replay_speed = [100, 50];
        app.s.replay_command = Some(replay::Action::Play { event: id, program: false });
        app.update_replay(0.8);
        let deadline = Instant::now() + std::time::Duration::from_secs(12);
        while app.replay.playback.iter().any(Option::is_some) && Instant::now() < deadline {
            app.update_replay(0.8);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        for channel in 0..2 {
            assert!(app.s.inputs.iter().any(|input| {
                matches!(&input.kind, Kind::Replay { channel: c, path } if *c == channel
                    && std::fs::metadata(path).is_ok_and(|entry| entry.len() > 1000))
            }), "{}", app.s.replay_status);
        }
    }
    app.s.replay_channel = 0;
    app.s.replay_command = Some(replay::Action::Live);
    app.update_replay(0.8);
    let id = app.s.preview.unwrap();
    assert!(matches!(&app.s.input(id).unwrap().kind, Kind::Replay { channel: 0, path } if path.ends_with(&original)));
    assert_eq!(app.s.input(id).unwrap().seek_ms, Some(0));
    app.s.replay_command = Some(replay::Action::Step(1));
    app.update_replay(0.81);
    assert_eq!(app.s.input(id).unwrap().seek_ms, Some(40));
    app.s.replay_command = Some(replay::Action::Reverse);
    app.update_replay(0.82);
    assert!(matches!(app.replay.indexed[0].as_ref().unwrap().direction, ReplayMotion::Reverse));
    std::fs::remove_dir_all(folder).unwrap();
}

/// What a keyboard shortcut does (function names from the original's shortcut list).
#[derive(Clone, Copy)]
enum ShortcutFn {
    PreviewInputPrevious,
    PreviewInputNext,
    /// Fade the given input (0 = Preview) to Output over `ms`.
    Fade(usize, u32),
    /// Cut the given input (0 = Preview) to Output.
    Cut(usize),
    /// Put input N in Preview.
    PreviewInput(usize),
}

/// The original's "Default Keyboard" shortcut template
/// The default keyboard layout contains 36 shortcuts.
fn default_keyboard() -> Vec<(egui::Key, ShortcutFn)> {
    use egui::Key::*;
    use ShortcutFn::*;
    let mut v = vec![
        (PageUp, PreviewInputPrevious),
        (PageDown, PreviewInputNext),
        (Period, Fade(0, 500)),
        (Num0, ShortcutFn::Cut(0)),
        (Num1, Fade(0, 1000)),
        (Num2, Fade(0, 2000)),
        (Num3, Fade(0, 3000)),
        (Num4, Fade(0, 4000)),
        (OpenBracket, PreviewInputPrevious),
        (CloseBracket, PreviewInputNext),
    ];
    for (n, k) in [Q, W, E, R, T, Y, U, I, O, P].into_iter().enumerate() {
        v.push((k, PreviewInput(n + 1)));
    }
    for (n, k) in [A, S, D, F, G, H, J, K, L].into_iter().enumerate() {
        v.push((k, ShortcutFn::Cut(n + 1)));
    }
    for (n, k) in [Z, X, C, V, B, N, M].into_iter().enumerate() {
        v.push((k, Fade(n + 1, 1000)));
    }
    v
}

/// Keyboard shortcuts (only when no text box is being typed in).
fn shortcuts(ctx: &egui::Context, s: &mut Switcher) {
    if ctx.wants_keyboard_input() {
        return;
    }
    let ctrl_s = ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::S));
    if ctrl_s {
        s.save_preset();
        return;
    }
    let pressed: Vec<ShortcutFn> = ctx.input(|i| {
        if i.modifiers.ctrl || i.modifiers.alt {
            return Vec::new();
        }
        default_keyboard().into_iter().filter(|(k, _)| i.key_pressed(*k)).map(|(_, f)| f).collect()
    });
    let id_of = |s: &Switcher, n: usize| s.inputs.get(n.wrapping_sub(1)).map(|i| i.id);
    for f in pressed {
        match f {
            ShortcutFn::PreviewInputPrevious => s.preview_step(-1),
            ShortcutFn::PreviewInputNext => s.preview_step(1),
            ShortcutFn::Fade(0, ms) => s.start_transition(Effect::Fade, ms),
            ShortcutFn::Cut(0) => s.start_transition(Effect::Cut, 0),
            ShortcutFn::Fade(n, ms) => {
                if let Some(id) = id_of(s, n) {
                    s.transition_input(id, Effect::Fade, ms);
                }
            }
            ShortcutFn::Cut(n) => {
                if let Some(id) = id_of(s, n) {
                    s.cut_direct(id);
                }
            }
            ShortcutFn::PreviewInput(n) => {
                if let Some(id) = id_of(s, n) {
                    s.preview = Some(id);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
        shortcuts(ctx, &mut self.s);

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
            if let Some(error) = &capture.error {
                format!("Window capture via system memory ({error})")
            } else if let Some(target) = &capture.target {
                let report = format!("{:.1} MiB submitted for OpenGL readback",
                    target.readback_bytes() as f64 / 1_048_576.0);
                #[cfg(windows)]
                let report = match target.interop_error() {
                    Some(reason) => format!("{report} | D3D11 interop: {reason}"),
                    None => report,
                };
                report
            } else {
                "Waiting for the first Program capture".into()
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
        dialogs(ctx, &mut self.s, &adapters, &encoder, &program_path, &mut self.mixer);
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
                unsafe { bridge.close() };
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

// ---------------------------------------------------------------- top bar

fn top_bar(ctx: &egui::Context, s: &mut Switcher) {
    // Top bar of the original: Preset | New Open Save Save As Last ... Fullscreen ... Pause Inputs, Settings, ?
    egui::TopBottomPanel::top("top")
        .exact_height(30.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(ctx.style().visuals.window_fill()).inner_margin(egui::Margin::symmetric(4, 3)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.add_sized([80.0, 24.0], egui::Label::new(RichText::new("Preset").strong().color(Color32::WHITE)));
                if bar_btn(ui, "New", s.new_step != NewStep::Idle).on_hover_text("Start a new blank preset with a video format you choose").clicked() {
                    s.press_new();
                }
                if bar_btn(ui, "Open", false).on_hover_text("Choose a project preset").clicked() {
                    if s.outputs_busy() {
                        s.notice(STOP_OUTPUTS_MSG);
                    } else {
                        s.browse_req = Some(media::PickKind::PresetOpen);
                    }
                }
                if bar_btn(ui, "Save", false).on_hover_text("Save changes to currently open Preset file (Ctrl+S)").clicked() {
                    s.save_preset();
                }
                if bar_btn(ui, "Save As", false).on_hover_text("Save to a new Preset file").clicked() {
                    s.browse_req = Some(media::PickKind::PresetSave);
                }
                if bar_btn(ui, "Last", false).on_hover_text("Open the last preset").clicked() {
                    if s.outputs_busy() {
                        s.notice(STOP_OUTPUTS_MSG);
                    } else {
                        s.load_last_preset();
                    }
                }
                let screen_w = ctx.screen_rect().width();
                let fs_x = (screen_w - 150.0) / 2.0;
                let cur = ui.cursor().min.x;
                if fs_x > cur {
                    ui.add_space(fs_x - cur);
                }
                for slot in 0..2 {
                    let label = if slot == 0 { "Fullscreen" } else { "Fullscreen 2" };
                    if bar_btn(ui, label, s.fullscreen[slot]).on_hover_text(src_name(s, s.cfg.fs_source[slot])).clicked() {
                        toggle_fullscreen(s, slot);
                    }
                }
                // External Output indicator beside Fullscreen (lit while the External window is on).
                let (r, resp) = ui.allocate_exact_size(Vec2::new(40.0, 24.0), Sense::click());
                let on = s.external[0].open || s.decklink_active[0];
                ui.painter().rect_filled(r, 2.0, if on { ACTIVE_RED } else { BTN_DARK });
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "EXT", egui::FontId::proportional(12.0), if on { Color32::WHITE } else { TEXT_DIM });
                if resp.on_hover_text(if on { "External Output is on — click to turn it off" } else { "External Output is off — click to turn it on" }).clicked() {
                    s.toggle_external_route(0);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if bar_btn(ui, "?", false).on_hover_text(
                        "Shortcuts (Default Keyboard): Q–P = input 1–10 to Preview · A–L = Cut input 1–9 · Z–M = Fade input 1–7 · . = Fade Preview 0.5 s · 0 = Cut Preview · 1–4 = Fade Preview 1–4 s · PgUp/PgDn or [ ] = previous/next input · Ctrl+S = save",
                    ).clicked() {
                        s.notice("Q–P Preview · A–L Cut · Z–M Fade · . Fade Preview · 0 Cut Preview · 1–4 Fade 1–4 s · PgUp/PgDn previous/next");
                    }
                    if bar_btn(ui, "Settings", s.settings_open).clicked() {
                        s.settings_open = !s.settings_open;
                    }
                    if bar_btn(ui, "Pause Inputs", false).on_hover_text("Pause all currently playing video inputs").clicked() {
                        for i in &mut s.inputs {
                            i.playing = false;
                        }
                    }
                });
            });
        });
}

/// Status line at the very bottom (format, FPS, render time, recording state, messages).
fn status_bar(ctx: &egui::Context, s: &Switcher, fps: Option<f64>, stats: &performance::Status) {
    egui::TopBottomPanel::bottom("status")
        .exact_height(22.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(ctx.style().visuals.window_fill()).inner_margin(egui::Margin::symmetric(8, 2)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(s.cfg.format_label()).strong().size(12.0).color(Color32::from_rgb(0x3c, 0xe0, 0x50)));
                // Alerts (Settings → Alerts, both off by default).
                if s.cfg.recording_alert && s.rec.is_none() {
                    ui.label(RichText::new(" NOT RECORDING ").strong().size(12.0).color(Color32::WHITE).background_color(ACTIVE_RED));
                }
                if s.cfg.ftb_alert && s.ftb_on {
                    ui.label(RichText::new(" FADE TO BLACK ").strong().size(12.0).color(Color32::WHITE).background_color(ACTIVE_RED));
                }
                ui.add_space(8.0);
                if s.external_active() {
                    ui.label(RichText::new("EX").size(12.0).color(TEXT));
                }
                ui.label(RichText::new("FPS:").strong().size(12.0).color(TEXT));
                ui.label(RichText::new(fps.map_or_else(|| "–".into(), |fps| format!("{fps:.0}"))).size(12.0).color(TEXT))
                    .on_hover_text("Measured project video cadence. Extra interface repaints are excluded.");
                ui.label(RichText::new("Render Time:").strong().size(12.0).color(TEXT));
                let render = stats.render_ms.map_or_else(|| "–".into(), |ms| format!("{ms} ms"));
                ui.label(RichText::new(render).size(12.0).color(TEXT))
                    .on_hover_text("Peak elapsed time from frame preparation through OpenGL draw submission, sampled once per second.");
                ui.label(RichText::new("GPU Mem:").strong().size(12.0).color(TEXT));
                ui.label(RichText::new(format!("{}%", stats.transfer_percent)).size(12.0).color(TEXT))
                    .on_hover_text(format!("Program transfer buffers: {:.1} MiB / {:.0} MiB budget",
                        stats.transfer_bytes as f64 / 1048576.0, stats.transfer_budget as f64 / 1048576.0));
                for (label, percent) in [("CPU LiveMix:", stats.process_cpu), ("Total:", stats.total_cpu)] {
                    ui.label(RichText::new(label).strong().size(12.0).color(TEXT));
                    let value = percent.map_or_else(|| "–".into(), |value| format!("{value}%"));
                    ui.label(RichText::new(value).size(12.0).color(TEXT));
                }
                for (on, tag) in [(s.rec.is_some(), "REC"), (s.multicorder.is_some(), "MULTICORDER")] {
                    if on {
                        ui.add_space(8.0);
                        ui.label(RichText::new(tag).strong().size(12.0).color(Color32::from_rgb(0xff, 0x40, 0x40)));
                    }
                }
                if !s.notice.is_empty() {
                    ui.add_space(12.0);
                    ui.label(RichText::new(&s.notice).size(12.0).color(Color32::from_rgb(0xe8, 0xc0, 0x50)));
                }
            });
        });
}

/// Dark top-bar button (#293038 in the original).
fn bar_btn(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(text).strong().size(13.0).color(Color32::WHITE))
        .fill(if active { HOVER } else { BTN_DARK })
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(if text.len() <= 2 { 24.0 } else { 90.0 }, 24.0));
    ui.add(b)
}

// ---------------------------------------------------------------- monitors

fn monitors(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    // Layout of the original: Preview and Output fill the width, the transition column sits
    // between them (reference screenshot).
    let screen = ctx.screen_rect();
    let center_w = 150.0;
    let mon_w = ((screen.width() - center_w) / 2.0).max(200.0);
    let max_h = (screen.height() - 30.0 - 44.0 - 22.0) * 0.55;
    let mon_h = (mon_w * 9.0 / 16.0).min(max_h - 26.0 - 24.0);
    let panel_h = mon_h + 26.0 + 24.0;
    egui::TopBottomPanel::top("monitors")
        .exact_height(panel_h)
        .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill))
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            ui.horizontal_top(|ui| {
                monitor(ui, s, t, false, mon_w, mon_h);
                center_column(ui, s, center_w, panel_h);
                monitor(ui, s, t, true, mon_w, mon_h);
            });
        });
}

/// Safe area guides over the Preview, drawn from the original's rules (model::safe_area_guides,
/// kzzzzzj::d @0xf6d78) on the picture's own size.
fn draw_safe_areas(p: &egui::Painter, r: egui::Rect, kind: u16) {
    let line = Stroke::new(1.0_f32, Color32::from_white_alpha(170));
    for g in safe_area_guides(kind, r.width(), r.height()) {
        p.line_segment([r.min + Vec2::new(g[0], g[1]), r.min + Vec2::new(g[2], g[3])], line);
    }
}

/// One scope in its place: black behind it (or the picture showing through when it lies over
/// the whole Preview), lines at 0/25/50/75/100 % for the waveforms (parade split in thirds), a
/// circle and cross for the vectorscope.
fn monitor(ui: &mut egui::Ui, s: &mut Switcher, t: f64, is_output: bool, w: f32, h: f32) {
    ui.vertical(|ui| {
        ui.set_width(w);
        let id = if is_output { s.output } else { s.preview };
        let title = id.and_then(|i| s.input(i)).map(|i| i.title.clone()).unwrap_or_else(|| "—".into());
        let num = id.map(|i| s.number_of(i)).unwrap_or(0);
        // Full-width title bar: orange for Preview, green for Output (as in the original).
        let (bar, _) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::hover());
        let col = if is_output { s.cfg.output_colour } else { s.cfg.preview_colour };
        ui.painter().rect_filled(bar, 0.0, col);
        let heading = if is_output { "Output" } else { "Preview" };
        let caption = if num > 0 { format!("{num} · {title}") } else { "—".into() };
        let left = bar.left_center() + Vec2::new(10.0, 0.0);
        let label = ui.painter().text(left, egui::Align2::LEFT_CENTER, heading, egui::FontId::proportional(14.0), Color32::WHITE);
        let right = bar.right() - if is_output { 94.0 } else { 67.0 };
        let clip = egui::Rect::from_min_max(egui::pos2(label.right() + 10.0, bar.top()), egui::pos2(right, bar.bottom()));
        ui.painter().with_clip_rect(clip).text(egui::pos2(clip.left(), left.y), egui::Align2::LEFT_CENTER,
            caption, egui::FontId::proportional(12.0), Color32::from_white_alpha(218));
        if !is_output {
            // Preview tools, right of the title bar as in the original: waveform and safe areas.
            let mut x = bar.right() - 6.0;
            for tool in 0..2 {
                let r = egui::Rect::from_min_size(egui::pos2(x - 24.0, bar.top() + 3.0), Vec2::new(24.0, 20.0));
                x = r.left() - 4.0;
                let resp = ui.interact(r, egui::Id::new(("preview-tool", tool)), Sense::click());
                let on = if tool == 0 { s.cfg.safe_type != 0 } else { s.cfg.wave_type != 0 };
                let p = ui.painter();
                p.rect_filled(r, 2.0, if on { Color32::from_black_alpha(140) } else if resp.hovered() { Color32::from_black_alpha(70) } else { Color32::TRANSPARENT });
                if tool == 0 {
                    // Safe area icon: a frame inside a frame.
                    p.rect_stroke(r.shrink2(Vec2::new(5.0, 4.0)), 0.0, Stroke::new(1.5_f32, Color32::WHITE), egui::StrokeKind::Inside);
                    p.rect_stroke(r.shrink2(Vec2::new(8.5, 7.0)), 0.0, Stroke::new(1.0_f32, Color32::WHITE), egui::StrokeKind::Inside);
                } else {
                    // Waveform icon: coloured bars.
                    for (k, c) in [Color32::from_rgb(0xff, 0x40, 0x40), Color32::from_rgb(0x40, 0xd0, 0x40), Color32::from_rgb(0x40, 0x90, 0xff)].iter().enumerate() {
                        let hgt = [9.0, 13.0, 7.0][k];
                        let b = egui::Rect::from_min_max(egui::pos2(r.left() + 6.0 + k as f32 * 4.5, r.bottom() - 4.0 - hgt), egui::pos2(r.left() + 9.5 + k as f32 * 4.5, r.bottom() - 4.0));
                        p.rect_filled(b, 0.0, *c);
                    }
                }
                // Click: the original's menus (frmmain::ShowSafeAreaMenu @0x250c70 drops the
                // "Safe Area" menu below the button; the waveform has its own list).
                let tip = if tool == 0 { "Safe Area: SMPTE/EBU, 90% / 80%, grids, Square, Vertical" } else { "Waveform Monitor: Waveform RGB, Waveform Y, Parade RGB, Vectorscope, alone or with the Preview" };
                let resp = resp.on_hover_text(tip);
                let pid = egui::Id::new(("preview-tool-menu", tool));
                if resp.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(pid));
                }
                let mut changed = false;
                egui::popup_below_widget(ui, pid, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
                    ui.set_min_width(170.0);
                    if tool == 0 {
                        ui.label(RichText::new("Safe Area").strong());
                        ui.separator();
                        for (name, v) in SAFE_AREAS {
                            changed |= ui.selectable_value(&mut s.cfg.safe_type, v, name).changed();
                        }
                    } else {
                        // hd::k @0x87cb4: "Waveform Monitor", the layouts, then the 2X zoom switch.
                        ui.label(RichText::new("Waveform Monitor").strong());
                        ui.separator();
                        for (name, v) in WAVEFORMS {
                            changed |= ui.selectable_value(&mut s.cfg.wave_type, v, name).changed();
                        }
                        ui.separator();
                        changed |= ui.checkbox(&mut s.cfg.vs_zoom, "Vectorscope 2X Zoom").changed();
                    }
                });
                if changed {
                    s.save_settings();
                }
            }
        }
        if is_output {
            let mut x = bar.right() - 8.0;
            for (on, tag) in [(s.ftb_on, "FTB"), (s.rec.is_some(), "REC")] {
                if on {
                    let g = ui.painter().layout_no_wrap(tag.into(), egui::FontId::proportional(11.0), Color32::WHITE);
                    let r = egui::Rect::from_min_size(egui::pos2(x - g.size().x - 8.0, bar.top() + 4.0), Vec2::new(g.size().x + 8.0, bar.height() - 8.0));
                    ui.painter().rect_filled(r, 2.0, ACTIVE_RED);
                    ui.painter().galley(r.min + Vec2::new(4.0, (r.height() - g.size().y) / 2.0), g, Color32::WHITE);
                    x = r.left() - 4.0;
                }
            }
        }
        let (area, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        ui.painter().rect_filled(area, 0.0, Color32::BLACK);
        // Keep the meter outside the video rectangle used for screenshot fallback.
        let show_master_meter = is_output && s.cfg.master_meter && area.width() >= 80.0;
        let mut video_area = area;
        if show_master_meter { video_area.max.x -= 28.0; }
        let rect = fit_16_9(video_area);
        let painter = ui.painter_at(rect);
        if is_output {
            s.output_rect = rect;
            draw_output(&painter, rect, s, t);
            if show_master_meter {
                let meter = egui::Rect::from_min_max(video_area.right_top(), area.right_bottom());
                picture_audio_meter(&ui.painter_at(meter), meter, s.program_audio_level);
            }
        } else {
            // Waveform Monitor layout (hc::a): where the picture and the scopes go.
            let (pic, scopes) = scope_layout(s.cfg.wave_type);
            let place = |f: [f32; 4]| egui::Rect::from_min_size(rect.min + Vec2::new(f[0] * rect.width(), f[1] * rect.height()), Vec2::new(f[2] * rect.width(), f[3] * rect.height()));
            if let Some(f) = pic {
                let pr = place(f);
                let pp = ui.painter_at(pr);
                if let Some(inp) = s.preview.and_then(|i| s.input(i)) {
                    draw_source(&pp, pr, inp, 1.0, t, &s.inputs);
                }
                // Overlays show on Preview as well (InputRenderFlags: overlays are only left out
                // with ExcludePreviewOverlays), so picking another input never hides them;
                // right-click overlays (ms::zzo @0x1301ac) show on Preview only.
                draw_overlay_channels(&pp, pr, s, t, true);
                if s.cfg.safe_type != 0 {
                    draw_safe_areas(&pp, pr, s.cfg.safe_type);
                }
            }
            for (kind, f) in &scopes {
                // Alone over the picture the scope keeps it visible underneath.
                let over = pic == Some([0.0, 0.0, 1.0, 1.0]);
                let tex = s.scope_tex.iter().find(|(k, _)| k == kind).map(|(_, t)| *t);
                scopes::draw(&painter, place(*f), tex, *kind, over);
            }
        }
        if resp.double_clicked() && is_output {
            s.external[0].open = true;
        }
        // PreviewClick (default False): clicking the Preview window runs Quick Play.
        if !is_output && s.cfg.preview_click && resp.clicked() {
            let (e, d) = (s.quickplay_effect, s.quickplay_ms);
            s.start_transition(e, d);
        }
        // transport row
        ui.horizontal(|ui| {
            ui.set_height(30.0);
            if let Some(inp) = id.and_then(|id| s.input_mut(id)).filter(|i| i.kind.playable()) {
                video_transport(ui, inp, w, area);
            }
        });
    });
}

fn video_transport(ui: &mut egui::Ui, inp: &mut Input, width: f32, picture: egui::Rect) {
    // ms::zzzo → mz::Restart → SetPosition, and ms::zzzl → StartVideo/PauseVideo.
    let file = matches!(inp.kind, Kind::VideoFile { .. } | Kind::VideoList { .. });
    let can_seek = inp.duration > 0.0 && (!inp.kind.streamed() || file);
    ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
    ui.add_space(5.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new((width - 108.0).max(1.0), 26.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    let track = rect.shrink2(Vec2::new(6.0_f32.min(rect.width() * 0.25), 0.0));
    let y = rect.center().y;
    if can_seek {
        let pointer = response.interact_pointer_pos()
            .filter(|_| response.clicked() || response.drag_started() || response.dragged());
        if let Some(pos) = pointer {
            let k = ((pos.x - track.left()) / track.width()).clamp(0.0, 1.0);
            inp.pos = inp.duration * k;
            if file {
                inp.seek_ms = Some((inp.pos as f64 * 1000.0).round() as u64);
            }
        }
        let fraction = (inp.pos / inp.duration).clamp(0.0, 1.0);
        for k in 0..=4 {
            let x = track.left() + track.width() * k as f32 / 4.0;
            p.line_segment([egui::pos2(x, y - 10.0), egui::pos2(x, y - 8.0)], Stroke::new(1.0_f32, TEXT_DIM));
        }
        p.line_segment([egui::pos2(track.left(), y), egui::pos2(track.right(), y)], Stroke::new(2.0_f32, BTN));
        let thumb = egui::Rect::from_center_size(egui::pos2(track.left() + fraction * track.width(), y), Vec2::new(8.0, 16.0));
        p.rect(thumb, 0.0, if response.hovered() { BTN_HOVER } else { BTN }, Stroke::new(1.0_f32, TEXT), egui::StrokeKind::Inside);
        let time = format!("{}  /  {}  /  {}", clock(inp.pos as f64), clock(inp.duration as f64),
            clock((inp.duration - inp.pos).max(0.0) as f64));
        let pos = picture.center_bottom() - Vec2::new(0.0, 4.0);
        let p = ui.painter_at(picture);
        let text = p.layout_no_wrap(time, egui::FontId::monospace(14.0), Color32::WHITE);
        let origin = pos - Vec2::new(text.size().x / 2.0, text.size().y);
        p.rect_filled(egui::Rect::from_min_size(origin, text.size()).expand2(Vec2::new(5.0, 2.0)), 0.0, Color32::from_black_alpha(180));
        p.galley(origin, text, Color32::WHITE);
        response.on_hover_text("Seek to a position in this input");
    }
    if ui.add_sized([48.0, 25.0], egui::Button::new(RichText::new("Restart").size(11.0)).fill(BTN))
        .on_hover_text("Return to the beginning").clicked()
    {
        inp.pos = 0.0;
        inp.seek_ms = None;
        inp.restart_req = true;
    }
    let (button, response) = ui.allocate_exact_size(Vec2::new(27.0, 25.0), Sense::click());
    let p = ui.painter();
    p.rect(button, 0.0, if response.hovered() { BTN_HOVER } else { BTN }, Stroke::new(1.0_f32, EDGE), egui::StrokeKind::Inside);
    let c = button.center();
    if inp.playing {
        for x in [-2.5, 2.5] {
            p.rect_filled(egui::Rect::from_center_size(c + Vec2::new(x, 0.0), Vec2::new(2.5, 10.0)), 0.0, TEXT);
        }
    } else {
        p.add(egui::Shape::convex_polygon(vec![c + Vec2::new(-3.0, -5.0), c + Vec2::new(5.0, 0.0), c + Vec2::new(-3.0, 5.0)], TEXT, Stroke::NONE));
    }
    if response.on_hover_text(if inp.playing { "Pause" } else { "Play" }).clicked() {
        if !inp.playing && inp.duration > 0.0 && inp.pos + 0.05 >= inp.duration {
            inp.restart_req = true;
        }
        inp.playing = !inp.playing;
    }
}

/// The transition column of the original: Quick Play, Cut, four transition buttons with a
/// drop-down (effect + duration), FTB, eight overlay channels, and the T-bar.
fn center_column(ui: &mut egui::Ui, s: &mut Switcher, w: f32, h: f32) {
    ui.allocate_ui_with_layout(Vec2::new(w, h), egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 6.0);
        ui.add_space(6.0);
        let bw = w - 28.0;
        let overlay_space = if s.cfg.show_overlays { 52.0 } else { 0.0 }
            + if s.cfg.production_clock { 26.0 } else { 0.0 };
        let bh = ((h - 6.0 - 7.0 * 6.0 - overlay_space - 70.0) / 7.0).clamp(24.0, 40.0);
        let busy = s.trans.is_some();
        if s.cfg.show_quickplay && ui
            .add_sized([bw, bh], egui::Button::new(RichText::new("Quick Play").strong()).fill(if busy { HOVER } else { BTN }))
            .on_hover_text("Transition from Preview to Output using the Quick Play effect")
            .clicked()
        {
            let (e, d) = (s.quickplay_effect, s.quickplay_ms);
            s.start_transition(e, d);
        }
        if ui.add_sized([bw, bh], egui::Button::new(RichText::new("Cut").strong())).on_hover_text("Cut (key 0)").clicked() {
            s.start_transition(Effect::Cut, 0);
        }
        let shown = if s.cfg.show_trans24 { 4 } else { 1 };
        for k in 0..shown {
            let tr = s.transitions[k];
            let title = if tr.stinger.is_some() { format!("Stinger {}", k + 1) } else { tr.effect.name().into() };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                ui.add_space(13.0);
                if ui
                    .add_sized([bw - 17.0, bh], egui::Button::new(RichText::new(title).strong()))
                    .on_hover_text(tr.stinger.and_then(|id| s.input(id)).map(|input| input.title.clone())
                        .unwrap_or_else(|| format!("{} — {} ms", tr.effect.name(), tr.ms)))
                    .clicked()
                {
                    s.start_transition_button(k);
                }
                let (r, resp) = ui.allocate_exact_size(Vec2::new(16.0, bh), Sense::click());
                ui.painter().rect_filled(r, 2.0, if resp.hovered() { BTN_HOVER } else { BTN });
                let c = r.center();
                ui.painter().add(egui::Shape::convex_polygon(vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(4.0, -2.0), c + Vec2::new(0.0, 3.0)], TEXT, Stroke::NONE));
                let pid = ui.make_persistent_id(("fx", k));
                if resp.clicked() {
                    ui.memory_mut(|m| m.toggle_popup(pid));
                }
                egui::popup_below_widget(ui, pid, &resp, egui::PopupCloseBehavior::CloseOnClickOutside, |ui| {
                    ui.set_min_width(170.0);
                    ui.horizontal(|ui| {
                        ui.label("Duration");
                        if tr.stinger.is_some() {
                            ui.label("From GT storyboard");
                        } else {
                            ui.add(egui::DragValue::new(&mut s.transitions[k].ms).range(1..=9999).speed(10.0).suffix(" ms"));
                        }
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for e in ALL_EFFECTS {
                            if ui.selectable_label(tr.stinger.is_none() && tr.effect == e, e.name()).clicked() {
                                s.transitions[k].stinger = None;
                                s.transitions[k].effect = e;
                            }
                        }
                        ui.separator();
                        ui.label("GT Stinger");
                        for input in s.inputs.iter().filter(|input| input.kind.gt_seconds(false).is_some() && input.kind.gt_seconds(true).is_some()) {
                            ui.selectable_value(&mut s.transitions[k].stinger, Some(input.id), &input.title);
                        }
                    });
                });
            });
        }
        let ftb = s.ftb_on;
        let flash = ftb && ((ui.input(|i| i.time) * 2.0) as i64 % 2 == 0);
        if s.cfg.show_ftb && ui
            .add_sized([bw, bh], egui::Button::new(RichText::new("FTB").strong()).fill(if flash { ACTIVE_RED } else if ftb { Color32::from_rgb(0x7a, 0x18, 0x18) } else { BTN }))
            .on_hover_text("Toggle fade to black on Output")
            .clicked()
        {
            s.ftb_on = !s.ftb_on;
        }
        if s.cfg.show_overlays {
            for row in (0..OVERLAY_CHANNELS).step_by(4) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.add_space(13.0);
                    for c in row..row + 4 {
                        let ch = &s.overlays[c];
                        let on = ch.on;
                        let tip = if on {
                            let name = ch.input.and_then(|id| s.input(id)).map_or("", |i| i.title.as_str());
                            format!("Turn Overlay {} off ({name})", c + 1)
                        } else if let Some((source, input)) = ch.assigned.and_then(|a| s.input(a.input).map(|i| (a, i))) {
                            let target = if source.on_preview { "Preview" } else { "Output" };
                            format!("Turn Overlay {} on: {} → {target}", c + 1, input.title)
                        } else {
                            format!("Overlay {}: assign a source on an input box first", c + 1)
                        };
                        let label = RichText::new((c + 1).to_string()).strong().color(if on { Color32::BLACK } else { TEXT });
                        let button = egui::Button::new(label).fill(if on { CYAN } else { BTN });
                        if ui.add_sized([(bw - 6.0) / 4.0, 20.0], button).on_hover_text(tip).clicked() {
                            s.toggle_overlay_channel(c);
                        }
                    }
                });
            }
        }
        if s.cfg.production_clock {
            let id = ui.make_persistent_id("production-clock");
            let now = ui.input(|input| input.time);
            let (mut start, mut elapsed) = ui.data(|data| data.get_temp::<(Option<f64>, f64)>(id)).unwrap_or((Some(0.0), 0.0));
            let shown = elapsed + start.map_or(0.0, |start| (now - start).max(0.0));
            let response = ui.add_sized([bw, 22.0], egui::Button::new(RichText::new(clock(shown)).monospace()))
                .on_hover_text("Production Clock · click to pause or resume · right-click to reset");
            if response.clicked() {
                if start.is_some() {
                    start = None;
                    elapsed = shown;
                } else {
                    start = Some(now);
                }
            }
            response.context_menu(|ui| {
                if ui.button("Reset Production Clock").clicked() {
                    elapsed = 0.0;
                    if start.is_some() { start = Some(now); }
                    ui.close_menu();
                }
            });
            ui.data_mut(|data| data.insert_temp(id, (start, elapsed)));
        }
        if s.cfg.show_tbar {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.add_space(13.0);
                let th = (ui.available_height() - 6.0).clamp(24.0, 72.0);
                tbar(ui, s, bw, th);
            });
        }
    });
}

/// T-Bar, sideways as in the original's main window: a grey handle slides along a white line,
/// left to right and back. Moving it all the way across completes the transition; the next
/// move goes the other way (ms::zzzzzze @0x133cc0 + hm::set_s @0x8ad04, 0–255 scale).
fn tbar(ui: &mut egui::Ui, s: &mut Switcher, w: f32, h: f32) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click_and_drag());
    let p = ui.painter();
    let handle_w = 24.0_f32.min(w / 4.0);
    // The handle's centre travels from half a handle in from the left to half in from the right.
    let (x0, x1) = (rect.left() + handle_w / 2.0, rect.right() - handle_w / 2.0);
    let mid = rect.center().y;
    p.line_segment([egui::pos2(rect.left(), mid), egui::pos2(rect.right(), mid)], Stroke::new(1.5_f32, Color32::from_white_alpha(220)));
    let v = s.tbar_value();
    let x = x0 + v * (x1 - x0);
    let handle = egui::Rect::from_center_size(egui::pos2(x, mid), Vec2::new(handle_w, h));
    p.rect_filled(handle, 5.0, if resp.hovered() || resp.dragged() { Color32::from_rgb(0x70, 0x78, 0x80) } else { Color32::from_rgb(0x55, 0x5c, 0x63) });
    // The original's T-bar position mark is blue (#0087FF), across the handle on the line.
    p.rect_filled(egui::Rect::from_center_size(egui::pos2(x, mid), Vec2::new(handle_w, 6.0)), 0.0, Color32::from_rgb(0x00, 0x87, 0xff));
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let nv = ((pos.x - x0) / (x1 - x0).max(1.0)).clamp(0.0, 1.0);
            s.set_tbar(nv);
        }
    }
    resp.on_hover_text("Manual fade using first transition effect (T-Bar) — drag left to right, then back");
}

fn draw_output(painter: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64) {
    draw_output_opts(painter, rect, s, t, true);
}

fn draw_overlay_channels(painter: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, preview: bool) {
    for ch in s.overlays.iter().filter(|c| preview || !c.on_preview) {
        if let Some(inp) = ch.input.and_then(|i| s.input(i)) {
            if ch.level > 0.0 {
                draw_overlay(painter, rect, inp, ch, t, &s.inputs);
            }
        }
    }
}

fn draw_output_opts(painter: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, overlays: bool) {
    match &s.trans {
        Some(tr) if tr.stinger.is_none() => {
            let a = tr.from.and_then(|i| s.input(i));
            let b = s.input(tr.to);
            draw_transition(painter, rect, a, b, tr.effect, tr.progress / FULL, t, &s.inputs);
        }
        _ => {
            if let Some(inp) = s.output.and_then(|i| s.input(i)) {
                draw_source(painter, rect, inp, 1.0, t, &s.inputs);
            }
        }
    }
    if overlays {
        draw_overlay_channels(painter, rect, s, t, false);
    }
    if let Some(tr) = &s.trans {
        if let Some(stinger) = &tr.stinger {
            if let Some(input) = s.input(stinger.input) {
                let cue = if tr.progress < stinger.cut {
                    gt::Cue::In((tr.progress / stinger.cut).clamp(0.0, 1.0))
                } else {
                    gt::Cue::Out(((tr.progress - stinger.cut) / (FULL - stinger.cut)).clamp(0.0, 1.0))
                };
                draw_gt_stinger(painter, rect, input, t, &s.inputs, cue);
            }
        }
    }
    if s.ftb_level > 0.0 {
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(s.ftb_level.clamp(0.0, FULL) as u8));
    }
}

/// The Output in its own window (External). Drag it to a second screen / projector and
/// double-click (or press Fullscreen) to fill that screen. Esc leaves fullscreen.
/// External window sizes offered in Settings → Outputs.
const EXT_SIZES: [(&str, [f32; 2]); 3] = [("960 x 540", [960.0, 540.0]), ("1280 x 720", [1280.0, 720.0]), ("1920 x 1080", [1920.0, 1080.0])];

/// Name of an output source code (Settings → Outputs "Source" column).
fn src_name(s: &Switcher, v: u32) -> String {
    match v {
        SRC_OUTPUT => "Output".into(),
        SRC_PREVIEW => "Preview".into(),
        SRC_MV1 => "MultiView 1".into(),
        SRC_MV2 => "MultiView 2".into(),
        MV_AUTO => "Auto (next input)".into(),
        n => s.input(n - 1).map(|i| format!("Input {} {}", s.number_of(i.id), i.title)).unwrap_or_else(|| "Output".into()),
    }
}

/// Every source an output can show: Output, Preview, MultiView 1–2 and each input.
fn src_choices(s: &Switcher, multiview: bool) -> Vec<(u32, String)> {
    let mut v: Vec<u32> = vec![SRC_OUTPUT, SRC_PREVIEW];
    if multiview {
        v.extend([SRC_MV1, SRC_MV2]);
    }
    v.extend(s.inputs.iter().map(|i| i.id + 1));
    v.into_iter().map(|c| (c, src_name(s, c))).collect()
}

/// Draws one output source: Output, Preview (with overlays as the Preview monitor shows them),
/// a MultiView or one input.
fn draw_src(p: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, src: u32, overlays: bool) {
    match src {
        SRC_OUTPUT => draw_output_opts(p, rect, s, t, overlays),
        SRC_MV1 | SRC_MV2 => draw_multiview(p, rect, s, t, if src == SRC_MV1 { 0 } else { 1 }),
        src => {
            let id = if src == SRC_PREVIEW { s.preview } else { Some(src - 1) };
            if let Some(inp) = id.and_then(|i| s.input(i)) {
                draw_source(p, rect, inp, 1.0, t, &s.inputs);
            }
            if overlays {
                draw_overlay_channels(p, rect, s, t, src == SRC_PREVIEW);
            }
        }
    }
}

/// Colour of a MultiView name strip that is neither in Preview nor on Output
/// (ozzzzzs::.cctor @0x17b8d8: Color.FromArgb(26, 60, 117), passed to qu as its third colour).
const MV_IDLE: Color32 = Color32::from_rgb(26, 60, 117);

/// A MultiView: several sources in one picture, each with a name strip underneath whose colour
/// is the tally — `qu::o` (@0x194c4c) pre-draws each strip in three states through `qu::p`
/// (@0x194cc8): the PreviewColour setting, the ActiveColour setting, or MV_IDLE
/// (colours handed over by ro/a::.ctor @0x1af984 from ozzzzzs::d / e / f).
fn draw_multiview(p: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, mv: usize) {
    p.rect_filled(rect, 0.0, Color32::BLACK);
    // Name strip under each picture (qu: title rect = box X, box Bottom, title height b).
    let title_h = mv_title_height(s.cfg.mv_layout[mv.min(1)]) * rect.height();
    for (b, src) in s.mv_resolve(mv) {
        let r = egui::Rect::from_min_size(
            rect.min + Vec2::new(b[0] * rect.width(), b[1] * rect.height()),
            Vec2::new(b[2] * rect.width(), b[3] * rect.height()),
        );
        let strip = egui::Rect::from_min_size(r.left_bottom(), Vec2::new(r.width(), title_h));
        let bp = p.with_clip_rect(r);
        bp.rect_filled(r, 0.0, Color32::from_gray(10));
        let Some(src) = src else {
            bp.rect_stroke(r, 0.0, Stroke::new(1.0_f32, Color32::from_gray(50)), egui::StrokeKind::Inside);
            continue;
        };
        // The Preview and Output boxes include the overlays, as the main window's monitors do.
        draw_src(&bp, r, s, t, src, src == SRC_OUTPUT || src == SRC_PREVIEW);
        let id = match src {
            SRC_OUTPUT => s.output,
            SRC_PREVIEW => s.preview,
            n => Some(n - 1),
        };
        if src == SRC_OUTPUT && s.cfg.mv_audio_program[mv.min(1)] {
            picture_audio_meter(&bp, r, s.program_audio_level);
        } else if src != SRC_OUTPUT && s.cfg.mv_audio_inputs[mv.min(1)] {
            if let Some(input) = id.and_then(|id| s.input(id)).filter(|input| input.kind.has_audio()) {
                picture_audio_meter(&bp, r, input.level);
            }
        }
        // Strip colour: Output (ActiveColour, DarkGreen) before Preview (PreviewColour,
        // DarkOrange); anything else MV_IDLE.
        let strip_col = if src == SRC_OUTPUT || (src != SRC_PREVIEW && id.is_some() && id == s.output) {
            s.cfg.output_colour
        } else if src == SRC_PREVIEW || (id.is_some() && id == s.preview) {
            s.cfg.preview_colour
        } else {
            MV_IDLE
        };
        // Names as qu::c / d / e / f set them: "Preview", "Output", otherwise the input's title.
        let label = match src {
            SRC_OUTPUT => "Output".to_string(),
            SRC_PREVIEW => "Preview".to_string(),
            n => s.input(n - 1).map(|i| format!("{} {}", s.number_of(i.id), i.title)).unwrap_or_default(),
        };
        let sp = p.with_clip_rect(strip);
        sp.rect_filled(strip, 0.0, strip_col);
        let fs = (title_h * 0.62).max(5.0);
        sp.text(strip.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(fs), Color32::WHITE);
    }
}

/// Screens of this computer as (left, top, width, height) in pixels, main screen first.
fn screens() -> Vec<[i32; 4]> {
    let mut v = media::monitors();
    if v.is_empty() {
        v.push([0, 0, 1920, 1080]);
    }
    v
}

/// The Fullscreen display (top bar "Fullscreen"): a borderless window covering the screen chosen
/// in Settings → Display, showing the Fullscreen source. Esc or a double-click closes it.
fn fullscreen_display(index: usize, count: usize) -> usize {
    if index < count { index } else { 0 }
}

fn toggle_fullscreen(s: &mut Switcher, slot: usize) {
    let monitors = screens();
    let other = 1 - slot;
    if !s.fullscreen[slot] && s.fullscreen[other]
        && fullscreen_display(s.cfg.fs_display[slot], monitors.len()) == fullscreen_display(s.cfg.fs_display[other], monitors.len())
    {
        s.notice("Choose a different monitor for each active Fullscreen output");
        return;
    }
    s.fullscreen[slot] = !s.fullscreen[slot];
}

fn fullscreen_window(ctx: &egui::Context, s: &mut Switcher, t: f64, slot: usize) {
    let id = egui::ViewportId::from_hash_of(("livemix-fullscreen", slot));
    let mons = screens();
    let m = mons[fullscreen_display(s.cfg.fs_display[slot], mons.len())];
    let ppp = ctx.pixels_per_point();
    let builder = egui::ViewportBuilder::default()
        .with_title(format!("LiveMix Fullscreen {}", slot + 1))
        .with_decorations(false)
        .with_position(egui::pos2(m[0] as f32 / ppp, m[1] as f32 / ppp))
        .with_inner_size(Vec2::new(m[2] as f32 / ppp, m[3] as f32 / ppp))
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    // Re-place the window when another screen is chosen.
    let key = egui::Id::new(("fs-display-last", slot));
    let last = ctx.data(|d| d.get_temp::<[i32; 4]>(key));
    if last != Some(m) {
        ctx.data_mut(|d| d.insert_temp(key, m));
        if last.is_some() {
            ctx.data_mut(|d| d.remove::<u32>(egui::Id::new(("fs-started", slot))));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Fullscreen(false));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::OuterPosition(egui::pos2(m[0] as f32 / ppp, m[1] as f32 / ppp)));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::InnerSize(Vec2::new(m[2] as f32 / ppp, m[3] as f32 / ppp)));
        }
    }
    let mut close = false;
    ctx.show_viewport_immediate(id, builder, |ctx, _class| {
        // Once the window sits on its screen, make it truly fullscreen there.
        let started = egui::Id::new(("fs-started", slot));
        let n = ctx.data(|d| d.get_temp::<u32>(started)).unwrap_or(0);
        if n < 3 {
            ctx.data_mut(|d| d.insert_temp(started, n + 1));
            if n == 2 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| {
            let r = ui.max_rect();
            let resp = ui.interact(r, egui::Id::new("fullscreen-area"), Sense::click());
            let out = fit_16_9(r);
            draw_src(&ui.painter_at(out), out, s, t, s.cfg.fs_source[slot], s.cfg.fs_overlays[slot]);
            if resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::None);
            }
            if resp.double_clicked() {
                close = true;
            }
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
            close = true;
        }
        if close {
            ctx.data_mut(|d| d.remove::<u32>(started));
        }
    });
    if close {
        s.fullscreen[slot] = false;
    }
}

/// The largest 16:9 rectangle centred in `r`.
fn fit_16_9(r: egui::Rect) -> egui::Rect {
    let mut w = r.width();
    let mut h = w * 9.0 / 16.0;
    if h > r.height() {
        h = r.height();
        w = h * 16.0 / 9.0;
    }
    egui::Rect::from_center_size(r.center(), Vec2::new(w, h))
}

fn output_window(ctx: &egui::Context, s: &mut Switcher, t: f64, channel: usize) {
    let id = egui::ViewportId::from_hash_of(("livemix-output", channel));
    let route = s.cfg.outputs[channel];
    let fullscreen = s.external[channel].fullscreen;
    let size = EXT_SIZES[s.cfg.ext_size.min(2) as usize].1;
    let builder = egui::ViewportBuilder::default()
        .with_title(format!("LiveMix External · Output {}", channel + 1))
        .with_inner_size(size)
        .with_fullscreen(fullscreen)
        .with_window_level(if s.cfg.ext_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal });
    let size_id = egui::Id::new(("ext-size-last", channel));
    let top_id = egui::Id::new(("ext-top-last", channel));
    let (hide_cursor, last_size) = (s.cfg.ext_hide_cursor, ctx.data(|d| d.get_temp::<u8>(size_id)));
    if last_size != Some(s.cfg.ext_size) {
        ctx.data_mut(|d| d.insert_temp(size_id, s.cfg.ext_size));
        if last_size.is_some() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::InnerSize(Vec2::new(size[0], size[1])));
        }
    }
    let last_top = ctx.data(|d| d.get_temp::<bool>(top_id));
    if last_top != Some(s.cfg.ext_on_top) {
        ctx.data_mut(|d| d.insert_temp(top_id, s.cfg.ext_on_top));
        if last_top.is_some() {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::WindowLevel(if s.cfg.ext_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal }));
        }
    }
    ctx.show_viewport_immediate(id, builder, |ctx, _class| {
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| {
            let r = ui.max_rect();
            let resp = ui.interact(r, egui::Id::new("output-window-area"), Sense::click());
            let out = fit_16_9(r);
            let p = ui.painter_at(out);
            draw_src(&p, out, s, t, route.source, route.overlays);
            // OutputHideCursor (default False).
            if hide_cursor && resp.hovered() {
                ctx.set_cursor_icon(egui::CursorIcon::None);
            }
            if resp.double_clicked() {
                s.external[channel].fullscreen = !fullscreen;
            }
            if resp.hovered() && !fullscreen {
                ui.painter().text(r.right_top() + Vec2::new(-8.0, 8.0), egui::Align2::RIGHT_TOP, "Double-click: fullscreen", egui::FontId::proportional(12.0), Color32::from_white_alpha(110));
            }
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            s.external[channel].fullscreen = false;
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            s.external[channel] = ExternalWindow::default();
        }
    });
}

// ---------------------------------------------------------------- inputs

fn input_preview_window(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    let Some(input) = s.input_preview.and_then(|id| s.input(id)) else { return };
    let title = format!("Input Preview · {}", input.title);
    let viewport = egui::ViewportId::from_hash_of("livemix-input-preview");
    let builder = egui::ViewportBuilder::default()
        .with_title(&title)
        .with_inner_size([800.0, 450.0])
        .with_min_inner_size([320.0, 180.0]);
    let mut open = true;
    ctx.show_viewport_immediate(viewport, builder, |ctx, class| {
        let draw = |ui: &mut egui::Ui, bounds| {
            let out = fit_16_9(bounds);
            let painter = ui.painter_at(out);
            if input.kind.transparent() {
                checker(&painter, out);
            }
            draw_source(&painter, out, input, 1.0, t, &s.inputs);
        };
        if class == egui::ViewportClass::Embedded {
            egui::Window::new(&title).id(egui::Id::new("livemix-input-preview"))
                .open(&mut open).default_width(640.0).show(ctx, |ui| {
                    let width = ui.available_width();
                    let (bounds, _) = ui.allocate_exact_size(Vec2::new(width, width * 9.0 / 16.0), Sense::hover());
                    draw(ui, bounds);
                });
        } else {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ctx, |ui| {
                draw(ui, ui.max_rect());
            });
            if ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
        }
    });
    if !open { s.input_preview = None; }
}

fn inputs_panel(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(egui::Margin::same(6)))
        .show(ctx, |ui| {
            // Category filter: coloured squares as in the original (first = all inputs).
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let square = |ui: &mut egui::Ui, col: Color32, sel: bool, tip: String| -> bool {
                    let (r, resp) = ui.allocate_exact_size(Vec2::new(28.0, 24.0), Sense::click());
                    ui.painter().rect_filled(r, 0.0, col);
                    if sel {
                        ui.painter().rect_stroke(r.shrink(1.0), 0.0, Stroke::new(2.0_f32, Color32::WHITE), egui::StrokeKind::Inside);
                    }
                    resp.on_hover_text(tip).clicked()
                };
                if square(ui, BTN, s.filter.is_none(), "Show all Inputs".into()) {
                    s.filter = None;
                }
                for (k, (name, col)) in CATEGORIES.iter().enumerate() {
                    if square(ui, *col, s.filter == Some(k as u8), format!("Show Inputs assigned to {name} category")) {
                        s.filter = Some(k as u8);
                    }
                }
            });
            let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, EDGE);
            ui.add_space(2.0);
            // Inputs are laid out as a grid: as many tiles per row as fit the width, then a new
            // row below; the list scrolls vertically when it gets taller than the panel.
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // ms::get_g @0x12e6c4: rows are tile height + 4 apart.
                // ms::zzzzzw @0x132d60 IL_00ad: every tile panel has Margin = Padding(2), so tiles
                // sit 2 px in from the list edge and 2 + 2 = 4 px apart.
                const TILE_MARGIN: f32 = 2.0;
                const GAP: f32 = TILE_MARGIN * 2.0;
                let (tile_w, tile_h) = tile_size(s);
                let avail = ui.available_width() - TILE_MARGIN * 2.0;
                let cols = (((avail + GAP) / (tile_w + GAP)).floor() as usize).max(1);
                // Visible inputs (category filter) with their input numbers; None = "Add Input" tile.
                let mut cells: Vec<Option<(u32, usize)>> = s
                    .inputs
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| s.filter.map_or(true, |f| i.category == Some(f)))
                    .map(|(n, i)| Some((i.id, n + 1)))
                    .collect();
                cells.push(None);
                // Tiles flow in input order, left to right, then on to the next row; every tile
                // keeps the full row height. A shrunk tile is a narrow upright strip (26 px wide),
                // so it stays in its place and the inputs after it just move up beside it.
                let _ = cols;
                let mut placed: Vec<(Option<(u32, usize)>, f32, f32, f32)> = Vec::new();
                let (mut x, mut y) = (TILE_MARGIN, 0.0_f32);
                for cell in &cells {
                    let w = match cell {
                        Some((id, _)) if s.input(*id).map(|i| i.collapsed).unwrap_or(false) => TILE_HEADER,
                        _ => tile_w,
                    };
                    if x > TILE_MARGIN && x + w > TILE_MARGIN + avail + 0.5 {
                        x = TILE_MARGIN;
                        y += tile_h + GAP;
                    }
                    placed.push((*cell, x, y, w));
                    x += w + GAP;
                }
                let total_h = y + tile_h;
                let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), total_h.max(1.0)), Sense::hover());
                let mut rects: Vec<(u32, egui::Rect)> = Vec::new();
                for (cell, x, y, w) in placed {
                    let r = egui::Rect::from_min_size(area.min + Vec2::new(x, y), Vec2::new(w, tile_h));
                    match cell {
                        Some((id, number)) => {
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("tile", id)));
                            tile(&mut child, s, id, number, t);
                            rects.push((id, r));
                        }
                        None => {
                            let r = egui::Rect::from_min_size(r.min, Vec2::new(tile_w, tile_h));
                            let resp = ui.interact(r, egui::Id::new("add-input-tile"), Sense::click());
                            let p = ui.painter();
                            p.rect_stroke(r.shrink(2.0), 4.0, Stroke::new(1.0_f32, EDGE), egui::StrokeKind::Inside);
                            p.text(r.center() - Vec2::new(0.0, 8.0), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(30.0), TEXT_DIM);
                            p.text(r.center() + Vec2::new(0.0, 18.0), egui::Align2::CENTER_CENTER, "Add Input", egui::FontId::proportional(12.0), TEXT_DIM);
                            if resp.clicked() {
                                s.add_dialog.open = true;
                            }
                        }
                    }
                }
                // Drag an input by its title bar and drop it on another input: it takes that
                // input's place (ms::zzzzzb @0x1324bc → ms::zzzzzc @0x132560).
                if let Some(from) = egui::DragAndDrop::payload::<u32>(ui.ctx()).map(|p| *p) {
                    if let Some(pos) = ui.ctx().pointer_latest_pos() {
                        let target = rects.iter().find(|(_, r)| r.contains(pos)).copied();
                        if let Some((to, r)) = target {
                            if to != from {
                                ui.painter().rect_stroke(r.expand(1.0), 0.0, Stroke::new(3.0_f32, CYAN), egui::StrokeKind::Outside);
                            }
                        }
                        let title = s.input(from).map(|i| format!("{}  {}", s.number_of(i.id), i.title)).unwrap_or_default();
                        let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("drag-ghost")));
                        let gr = egui::Rect::from_min_size(pos + Vec2::new(12.0, 8.0), Vec2::new(tile_w * 0.8, TILE_HEADER));
                        lp.rect_filled(gr, 2.0, s.cfg.preview_colour.gamma_multiply(0.85));
                        lp.text(gr.left_center() + Vec2::new(8.0, 0.0), egui::Align2::LEFT_CENTER, title, egui::FontId::proportional(13.0), Color32::WHITE);
                        if ui.input(|i| i.pointer.any_released()) {
                            if let Some((to, _)) = target {
                                s.move_input(from, to);
                            }
                            egui::DragAndDrop::clear_payload(ui.ctx());
                        }
                    }
                }
            });
        });
}

// Input tile layout of the original (ms::.cctor @0x12e000, ms::get_f @0x12e654):
// Title bar j = 26, picture = InputSize (208x156), button row k = 20.
const TILE_HEADER: f32 = 26.0;
const TILE_ROW: f32 = 20.0;
const BW_CLOSE: f32 = 42.0;
const BW_CUT: f32 = 33.0;
const BW_LOOP: f32 = 38.0;
const BW_AUDIO: f32 = 65.0;
const BW_ICON: f32 = 24.0;

/// (width, height) of one input tile from the InputSize setting.
fn tile_size(s: &Switcher) -> (f32, f32) {
    (s.cfg.input_w, TILE_HEADER + s.cfg.input_h + TILE_ROW * 3.0)
}

/// A flat tile button of a fixed width (the original's button widths).
fn tile_btn(ui: &mut egui::Ui, text: &str, w: f32, active: bool, enabled: bool, on_col: Color32) -> egui::Response {
    let col = if !enabled { TEXT_DIM.gamma_multiply(0.6) } else if active { Color32::BLACK } else { TEXT };
    let b = egui::Button::new(RichText::new(text).size(11.0).color(col))
        .fill(if active { on_col } else { BTN_DARK })
        .stroke(Stroke::NONE)
        .corner_radius(0)
        .min_size(Vec2::new(w, TILE_ROW));
    ui.add_enabled_ui(enabled, |ui| ui.add_sized([w, TILE_ROW], b)).inner
}

fn tile(ui: &mut egui::Ui, s: &mut Switcher, id: u32, number: usize, t: f64) {
    let (tile_w, tile_h) = tile_size(s);
    let (pic_w, pic_h) = (s.cfg.input_w, s.cfg.input_h);
    let show_numbers = s.cfg.input_numbers;
    let close_confirm = s.cfg.input_close_confirm;
    let Some(inp) = s.input(id) else { return };
    let is_out = s.output == Some(id);
    let is_prev = s.preview == Some(id);
    let border = if is_out { s.cfg.output_colour } else if is_prev { s.cfg.preview_colour } else { EDGE };
    let title = inp.title.clone();
    let cat = inp.category;
    let loop_on = inp.loop_on;
    let audio_on = inp.audio_on;
    let has_audio = inp.kind.has_audio();
    let playable = inp.kind.playable();
    let _ = border;
    let frame = egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(0));
    frame.show(ui, |ui| { ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);
        let shrunk = s.input(id).map(|i| i.collapsed).unwrap_or(false);
        // A shrunk input keeps its full height and becomes a narrow upright strip as wide as the
        // title bar is tall (26 px), so the inputs keep their rows and places.
        ui.set_width(if shrunk { TILE_HEADER } else { tile_w });
        ui.set_height(tile_h);
        // Header bar: green when on Output, orange when in Preview (as in the original).
        let hsize = if shrunk { Vec2::new(TILE_HEADER, tile_h) } else { Vec2::new(tile_w, TILE_HEADER) };
        let (hr, header) = ui.allocate_exact_size(hsize, Sense::click_and_drag());
        // Dragging the title bar carries the input (dropped on another input in inputs_panel).
        header.dnd_set_drag_payload(id);
        let hcol = if is_out { s.cfg.output_colour } else if is_prev { s.cfg.preview_colour } else { BTN };
        let close_rect = egui::Rect::from_min_size(
            if shrunk { egui::pos2(hr.left(), hr.bottom() - TILE_HEADER) }
            else { egui::pos2(hr.right() - TILE_HEADER, hr.top()) },
            Vec2::splat(TILE_HEADER),
        );
        let close = ui.interact(close_rect, ui.id().with(("input-close", id)), Sense::click())
            .on_hover_text("Close this Input");
        let p = ui.painter();
        p.rect_filled(hr, 0.0, hcol);
        if shrunk {
            if let Some(c) = cat {
                p.rect_filled(egui::Rect::from_min_size(hr.min, Vec2::new(hr.width(), 4.0)), 0.0, CATEGORIES[c as usize].1);
            }
            let mut y = hr.top() + 8.0;
            if show_numbers {
                let g = p.layout_no_wrap(number.to_string(), egui::FontId::proportional(13.0), Color32::WHITE);
                y += g.size().y;
                p.galley(egui::pos2(hr.center().x - g.size().x / 2.0, y - g.size().y), g, Color32::WHITE);
                y += 8.0;
            }
            // Title written top to bottom, cut to the strip's height.
            let room = (close_rect.top() - y - 6.0).max(0.0);
            let mut txt = title.clone();
            let mut g = p.layout_no_wrap(txt.clone(), egui::FontId::proportional(13.0), Color32::WHITE);
            while g.size().x > room && !txt.is_empty() {
                txt.pop();
                g = p.layout_no_wrap(format!("{txt}…"), egui::FontId::proportional(13.0), Color32::WHITE);
            }
            let x = hr.center().x + g.size().y / 2.0;
            p.add(egui::epaint::TextShape::new(egui::pos2(x, y), g, Color32::WHITE).with_angle(std::f32::consts::FRAC_PI_2));
        } else {
            if let Some(c) = cat {
                p.rect_filled(egui::Rect::from_min_size(hr.min, Vec2::new(4.0, hr.height())), 0.0, CATEGORIES[c as usize].1);
            }
            if show_numbers {
                p.text(hr.left_center() + Vec2::new(10.0, 0.0), egui::Align2::LEFT_CENTER, number.to_string(), egui::FontId::proportional(13.0), Color32::WHITE);
            }
        }
        let mut bx = close_rect.left() - 4.0;
        let mut badge = |txt: &str, col: Color32| {
            let g = p.layout_no_wrap(txt.into(), egui::FontId::proportional(9.0), Color32::BLACK);
            let r = egui::Rect::from_min_size(egui::pos2(bx - g.size().x - 6.0, hr.top() + 5.0), Vec2::new(g.size().x + 6.0, hr.height() - 10.0));
            p.rect_filled(r, 2.0, col);
            p.galley(r.min + Vec2::new(3.0, (r.height() - g.size().y) / 2.0), g, Color32::BLACK);
            bx = r.left() - 3.0;
        };
        if !shrunk {
            if let Some(Kind::Device { blackmagic: true, .. }) = s.input(id).map(|i| &i.kind) {
                badge("BMD", Color32::from_rgb(0xff, 0xb0, 0x30));
            }
            if s.input(id).map(|i| i.key.on).unwrap_or(false) {
                badge("KEY", Color32::from_rgb(0x50, 0xe0, 0x70));
            }
            let clip = egui::Rect::from_min_max(hr.min, egui::pos2(bx, hr.bottom()));
            p.with_clip_rect(clip).text(hr.left_center() + Vec2::new(if show_numbers { 32.0 } else { 10.0 }, 0.0), egui::Align2::LEFT_CENTER, &title, egui::FontId::proportional(13.0), Color32::WHITE);
        }
        if close.hovered() {
            p.rect_filled(close_rect, 0.0, Color32::from_rgb(0xc4, 0x35, 0x35));
        }
        let centre = close_rect.center();
        for slope in [-1.0, 1.0] {
            let d = Vec2::new(4.0, 4.0 * slope);
            p.line_segment([centre - d, centre + d], Stroke::new(1.5_f32, Color32::WHITE));
        }
        if close.clicked() {
            if close_confirm {
                s.confirm_close = Some(id);
                if let Some(i) = s.input_mut(id) {
                    i.collapsed = false;
                }
            } else {
                s.remove_input(id);
            }
            return;
        }
        // Title bar: click → Preview (ms::zx / ms::zy), double-click → shrink the tile to a
        // narrow upright strip (and back), right-click → menu. Double-click on the picture still
        // opens Input Settings.
        let collapsed = s.input(id).map(|i| i.collapsed).unwrap_or(false);
        if header.double_clicked() {
            if let Some(i) = s.input_mut(id) {
                i.collapsed = !i.collapsed;
            }
        } else if header.clicked() {
            s.preview = Some(id);
        }
        let header = header.on_hover_text(if collapsed {
            "Click: send to Preview · Double-click: show the whole input · Drag onto another input: move it there · Right-click: menu"
        } else {
            "Click: send to Preview · Double-click: shrink to a narrow strip · Drag onto another input: move it there · Right-click: menu"
        });
        header.context_menu(|ui| {
            if s.input(id).is_some_and(|i| matches!(i.kind, Kind::GtTitle { .. })) &&
                ui.button("Title Editor…").clicked()
            {
                s.cfg.input_page = 0;
                s.settings_input = Some(id);
                ui.close_menu();
            }
            if ui.button("Input Settings…").clicked() {
                s.settings_input = Some(id);
                ui.close_menu();
            }
            if ui.button(if collapsed { "Expand" } else { "Shrink" }).clicked() {
                if let Some(i) = s.input_mut(id) {
                    i.collapsed = !i.collapsed;
                }
                ui.close_menu();
            }
            ui.separator();
            ui.label("Category");
            if ui.button("None").clicked() {
                if let Some(i) = s.input_mut(id) {
                    i.category = None;
                }
                ui.close_menu();
            }
            for (k, (name, _)) in CATEGORIES.iter().enumerate() {
                if ui.button(*name).clicked() {
                    if let Some(i) = s.input_mut(id) {
                        i.category = Some(k as u8);
                    }
                    ui.close_menu();
                }
            }
        });
        if collapsed {
            // Shrunk: only the title bar shows.
            return;
        }
        // thumbnail
        let tw = pic_w;
        let th = pic_h;
        let (r, resp) = ui.allocate_exact_size(Vec2::new(tw, th), Sense::click());
        let painter = ui.painter_at(r);
        if let Some(inp) = s.input(id) {
            if inp.kind.transparent() {
                checker(&painter, r);
            } else {
                painter.rect_filled(r, 0.0, Color32::BLACK);
            }
            if s.cfg.no_input_effects {
                draw_plain(&painter, r, inp, t);
            } else {
                draw_source(&painter, r, inp, 1.0, t, &s.inputs);
            }
            if s.cfg.input_meter && inp.kind.has_audio() {
                picture_audio_meter(&painter, r, inp.level);
            }
            if playable {
                let k = (inp.pos / inp.duration.max(0.01)).clamp(0.0, 1.0);
                let bar = egui::Rect::from_min_size(egui::pos2(r.left(), r.bottom() - 3.0), Vec2::new(r.width() * k, 3.0));
                painter.rect_filled(bar, 0.0, Color32::from_rgb(0x40, 0xa0, 0xff));
            }
        }
        // ms::zzzzzp / ms::zx @0x12f944: a click on the picture puts the input in Preview (ms::zy);
        // a double-click opens its Input Settings (ms::zzzzzi → zzzzm → CogMouseUp).
        if resp.double_clicked() {
            s.settings_input = Some(id);
        } else if resp.clicked() {
            s.preview = Some(id);
        }
        resp.on_hover_text("Click: send to Preview · Double-click: Input Settings");

        // Playback, overlay assignment and audio controls use separate rows.
        if s.confirm_close == Some(id) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                ui.label(RichText::new("Close input?").color(Color32::WHITE).size(11.0));
                if tile_btn(ui, "YES", BW_CLOSE, true, true, Color32::from_rgb(0xff, 0x18, 0x18)).clicked() {
                    s.remove_input(id);
                }
                if tile_btn(ui, "No", BW_CUT, false, true, BTN).clicked() {
                    s.confirm_close = None;
                }
            });
        } else {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                ui.spacing_mut().button_padding.x = 1.0;
                let scale = ((tile_w - 3.0) / 153.0).min(1.0);
                if tile_btn(ui, "Preview", 48.0 * scale, s.input_preview == Some(id), true, s.cfg.preview_colour)
                    .on_hover_text("Open a larger preview of this input").clicked()
                {
                    s.input_preview = Some(id);
                    ui.ctx().send_viewport_cmd_to(egui::ViewportId::from_hash_of("livemix-input-preview"), egui::ViewportCommand::Focus);
                }
                if tile_btn(ui, "GO", 34.0 * scale, false, true, BTN).on_hover_text("Transition this Input directly to Output using the Quick Play effect").clicked() {
                    s.quick_play_input(id);
                }
                if tile_btn(ui, "Cut", BW_CUT * scale, false, true, BTN).on_hover_text("Switch this Input directly to Output").clicked() {
                    s.cut_direct(id);
                }
                if tile_btn(ui, "Loop", BW_LOOP * scale, loop_on, playable, Color32::from_rgb(0x50, 0x9a, 0xe0))
                    .on_hover_text("Enable looping of this Input when it has finished playing")
                    .clicked()
                {
                    if let Some(i) = s.input_mut(id) {
                        i.loop_on = !i.loop_on;
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                ui.spacing_mut().button_padding.x = 2.0;
                let width = (tile_w - (OVERLAY_CHANNELS - 1) as f32) / OVERLAY_CHANNELS as f32;
                for c in 0..OVERLAY_CHANNELS {
                    let assigned = s.overlays[c].assigned.filter(|source| source.input == id);
                    let target = if assigned.is_some_and(|source| source.on_preview) { "Preview" } else { "Output" };
                    let tip = if assigned.is_some() {
                        format!("Clear Overlay {} ({target})", c + 1)
                    } else {
                        format!("Activate Overlay {} (right-click activates on Preview)", c + 1)
                    };
                    let r = tile_btn(ui, &(c + 1).to_string(), width, assigned.is_some(), true, CYAN).on_hover_text(tip);
                    if r.secondary_clicked() {
                        s.press_input_overlay(c, id, true);
                    } else if r.clicked() {
                        s.press_input_overlay(c, id, false);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                if tile_btn(ui, "Audio", BW_AUDIO - 8.0, audio_on && has_audio, has_audio, Color32::from_rgb(0x4c, 0xc0, 0x50))
                    .on_hover_text("Turn on or off the audio on this Input")
                    .clicked()
                {
                    if let Some(i) = s.input_mut(id) {
                        i.audio_on = !i.audio_on;
                    }
                }
                let playing = s.input(id).map(|i| i.playing).unwrap_or(false);
                if tile_btn(ui, if playing { "||" } else { "▶" }, BW_ICON, playing, playable, Color32::from_rgb(0x50, 0x9a, 0xe0))
                    .on_hover_text("Play or pause this Input")
                    .clicked()
                {
                    if let Some(i) = s.input_mut(id) {
                        i.playing = !i.playing;
                    }
                }
                if tile_btn(ui, "Set", BW_ICON + 4.0, s.settings_input == Some(id), true, Color32::from_rgb(0x50, 0x9a, 0xe0))
                    .on_hover_text("Configure various Input settings")
                    .clicked()
                {
                    s.settings_input = Some(id);
                }
            });
        }
    }); });
}

// ---------------------------------------------------------------- mixer

#[derive(Clone, Copy)]
enum MixerSettings {
    Master,
    Input(u32),
}

struct MixerControls {
    bus: u8,
    headphones: f32,
    statistics: bool,
    settings: Option<MixerSettings>,
    last_error: Option<String>,
}

impl Default for MixerControls {
    fn default() -> Self {
        Self { bus: 0, headphones: 100.0, statistics: false, settings: None, last_error: None }
    }
}

impl App {
    fn audio_mixer_windows(&mut self, ctx: &egui::Context) {
        if self.mixer.statistics {
            egui::Window::new("Audio Engine")
                .open(&mut self.mixer.statistics)
                .default_width(500.0)
                .show(ctx, |ui| {
                    ui.label(format!("Status: {}", if self.audio_monitor.is_some() {
                        "Headphone monitoring active"
                    } else {
                        "Headphone monitoring off"
                    }));
                    ui.label(format!("Sample Rate: {} Hz (mixer)", audio::RATE));
                    ui.label(format!("Bit Rate: {} kb/s (16-bit stereo PCM)", audio::RATE * 16 * 2 / 1000));
                    if let Some(error) = &self.mixer.last_error {
                        ui.label(format!("Last monitor error: {error}"));
                    }
                    ui.separator();
                    egui::ScrollArea::both().max_height(240.0).show(ui, |ui| {
                        egui::Grid::new("audio-engine-inputs").striped(true).show(ui, |ui| {
                            for label in ["Input", "Peak L", "Peak R", "Buffered", "Clock resyncs"] {
                                ui.strong(label);
                            }
                            ui.end_row();
                            for input in self.s.inputs.iter().filter(|input| input.kind.has_audio()) {
                                ui.label(&input.title);
                                for peak in input.level {
                                    ui.label(if peak > 0.0 { format!("{:.1} dBFS", 20.0 * peak.log10()) } else { "−∞ dBFS".into() });
                                }
                                if let Some(buffer) = self.media.get(&input.id).and_then(|slot| slot.shared.as_ref())
                                    .and_then(|source| source.audio.try_lock().ok())
                                {
                                    let (frames, resyncs) = buffer.diagnostics();
                                    ui.label(format!("{:.0} ms", frames as f32 * 1000.0 / audio::RATE as f32));
                                    ui.label(resyncs.to_string());
                                } else {
                                    ui.label("—");
                                    ui.label("—");
                                }
                                ui.end_row();
                            }
                        });
                    });
                });
        }

        let Some(target) = self.mixer.settings else { return };
        let mut open = true;
        match target {
            MixerSettings::Master => {
                egui::Window::new("Audio Settings — Master").open(&mut open).show(ctx, |ui| {
                    ui.checkbox(&mut self.s.master_on, "Master audio");
                    ui.add(egui::Slider::new(&mut self.s.master_volume, 0.0..=100.0).text("Master volume").suffix("%"));
                    ui.add(egui::Slider::new(&mut self.mixer.headphones, 0.0..=100.0).text("Headphone volume").suffix("%"));
                });
            }
            MixerSettings::Input(id) => {
                if let Some(input) = self.s.input_mut(id) {
                    egui::Window::new(format!("Audio Settings — {}", input.title))
                        .id(egui::Id::new(("mixer-audio-settings", id)))
                        .open(&mut open)
                        .default_width(490.0)
                        .default_height(215.0)
                        .show(ctx, |ui| {
                            input_audio(ui, input, (ctx.screen_rect().height() - 110.0).clamp(100.0, 215.0));
                        });
                } else {
                    open = false;
                }
            }
        }
        if !open {
            self.mixer.settings = None;
        }
    }
}

fn picture_audio_meter(painter: &egui::Painter, picture: egui::Rect, levels: [f32; 2]) {
    let height = (picture.height() * 0.72).min(128.0);
    if height < 26.0 || picture.width() < 28.0 {
        return;
    }
    let width = (picture.width() * 0.035).clamp(5.0, 9.0);
    let origin = picture.right_bottom() - Vec2::new(2.0 * width + 5.0, height + 5.0);
    let backing = egui::Rect::from_min_size(origin - Vec2::splat(2.0), Vec2::new(2.0 * width + 5.0, height + 4.0));
    painter.rect_filled(backing, 1.0, Color32::from_black_alpha(215));
    for (index, level) in levels.into_iter().enumerate() {
        let lane = egui::Rect::from_min_size(origin + Vec2::new(index as f32 * (width + 1.0), 0.0),
            Vec2::new(width, height));
        render::meter(painter, lane, meter_fraction(level));
    }
}

fn meter_fraction(level: f32) -> f32 {
    ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn headphone_knob(ui: &mut egui::Ui, volume: &mut f32) {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(23.0), Sense::click_and_drag());
    if response.dragged() {
        *volume = (*volume - ui.input(|input| input.pointer.delta().y)).clamp(0.0, 100.0);
    }
    if response.has_focus() {
        let step = ui.input(|input| {
            i32::from(input.key_pressed(egui::Key::ArrowUp)) - i32::from(input.key_pressed(egui::Key::ArrowDown))
        });
        *volume = (*volume + step as f32).clamp(0.0, 100.0);
    }
    if response.double_clicked() {
        *volume = 100.0;
    }
    let center = rect.center();
    let angle = egui::lerp(0.75..=2.25, *volume / 100.0) * std::f32::consts::PI;
    let pointer = Vec2::new(angle.cos(), angle.sin());
    let style = ui.style().interact(&response);
    ui.painter().circle(center, 10.0, style.bg_fill, style.bg_stroke);
    ui.painter().line_segment([center, center + pointer * 7.5], Stroke::new(1.5_f32, TEXT));
    response.on_hover_text(format!("Headphones {:.0}% · Drag up or down", *volume));
}


fn mixer_panel(ctx: &egui::Context, s: &mut Switcher, master: [f32; 2], monitor: &mut bool, controls: &mut MixerControls) {
    let width = (ctx.available_rect().width() / 3.0).clamp(260.0, 760.0);
    egui::SidePanel::right("mixer")
        .default_width(width)
        .width_range(240.0..=ctx.available_rect().width() * 0.55)
        .frame(egui::Frame::new().fill(ctx.style().visuals.window_fill()).inner_margin(egui::Margin::same(3)))
        .show(ctx, |ui| {
            let follow_id = ui.make_persistent_id("follow-input-category");
            let mut follow = ui.data(|data| data.get_temp::<bool>(follow_id)).unwrap_or(false);
            ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 18.0), egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(" Audio Mixer ").strong()
                    .color(Color32::WHITE).background_color(Color32::from_rgb(0, 100, 0))))
                    .context_menu(|ui| {
                        if ui.checkbox(&mut follow, "Follow input categories").changed() {
                            ui.data_mut(|data| data.insert_temp(follow_id, follow));
                            ui.close_menu();
                        }
                    });
            });
            let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 2.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, Color32::from_rgb(0, 100, 0));
            let height = (ui.available_height() - 10.0).clamp(146.0, 190.0);
            egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if mixer_section(ui, "OUTPUTS", height) {
                        let open = mixer_strip(ui, "Master", &mut s.master_volume, &mut s.master_on,
                            master, None, height, Some((&mut *monitor, &mut *controls)));
                        if open {
                            controls.settings = Some(MixerSettings::Master);
                        }
                    }
                    if mixer_section(ui, "INPUTS", height) {
                        let category = if follow { s.filter } else { None };
                        for (index, input) in s.inputs.iter_mut().enumerate()
                            .filter(|(_, i)| i.kind.has_audio() && category.is_none_or(|c| i.category == Some(c)))
                        {
                            let name = format!("{} {}", index + 1, input.title);
                            let strip = MixerInputControls {
                                buses: [&mut input.bus_m, &mut input.bus_a, &mut input.bus_b],
                                solo: &mut input.solo,
                                solo_pfl: &mut input.solo_pfl,
                                auto: &mut input.auto_mix_audio,
                            };
                            if mixer_strip(ui, &name, &mut input.volume, &mut input.audio_on, input.level,
                                Some(strip), height, None)
                            {
                                controls.settings = Some(MixerSettings::Input(input.id));
                            }
                        }
                    }
                });
            });
        });
}

fn mixer_section(ui: &mut egui::Ui, name: &str, height: f32) -> bool {
    let id = ui.make_persistent_id(name);
    let mut open = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(true);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(22.0, height), Sense::click());
    if response.on_hover_text(format!("{} {name}", if open { "Collapse" } else { "Expand" })).clicked() {
        open = !open;
        ui.data_mut(|data| data.insert_temp(id, open));
    }
    let p = ui.painter();
    p.rect_filled(rect, 0.0, if open { Color32::from_rgb(0, 103, 169) } else { BTN });
    let galley = p.layout_no_wrap(name.to_owned(), egui::FontId::proportional(11.0), Color32::WHITE);
    let pos = rect.center() + Vec2::new(-galley.size().y / 2.0, galley.size().x / 2.0);
    p.add(egui::epaint::TextShape::new(pos, galley, Color32::WHITE)
        .with_angle(-std::f32::consts::FRAC_PI_2));
    open
}

struct MixerInputControls<'a> {
    buses: [&'a mut bool; 3],
    solo: &'a mut bool,
    solo_pfl: &'a mut bool,
    auto: &'a mut bool,
}

fn mixer_strip(ui: &mut egui::Ui, name: &str, volume: &mut f32, on: &mut bool, levels: [f32; 2],
    input: Option<MixerInputControls<'_>>, height: f32, monitor: Option<(&mut bool, &mut MixerControls)>) -> bool
{
    let master = input.is_none();
    let mut input = input;
    let mut monitor = monitor;
    let width = 98.0;
    egui::Frame::new().fill(Color32::from_rgb(13, 18, 23)).inner_margin(2).show(ui, |ui| ui.vertical(|ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
        let header = if master { Color32::from_rgb(0, 100, 0) } else { BTN };
        let (title, _) = ui.allocate_exact_size(Vec2::new(width, 17.0), Sense::hover());
        ui.painter().rect_filled(title, 0.0, header);
        ui.scope_builder(egui::UiBuilder::new().max_rect(title).layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight)), |ui| {
            ui.add(egui::Label::new(RichText::new(name).size(11.0).strong().color(Color32::WHITE))
                .truncate()).on_hover_text(name);
        });
        let meter_height = height - 48.0;
        let mut settings = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.add_space(13.0);
                settings = ui.add_sized([24.0, 24.0], egui::Button::new("⚙"))
                    .on_hover_text("Audio settings").clicked();
                if let Some((_, controls)) = monitor.as_mut() {
                    ui.add_space((meter_height - 90.0).max(0.0));
                    if ui.add_sized([24.0, 24.0], egui::Button::new("i")).on_hover_text("Audio Engine statistics").clicked() {
                        controls.statistics = true;
                    }
                } else if let Some(input) = input.as_mut() {
                    ui.add_space((meter_height - 113.0).max(0.0));
                    let solo = ui.add_sized([24.0, 19.0], egui::Button::new("S").selected(*input.solo))
                        .on_hover_text("Solo in headphones");
                    solo.context_menu(|ui| {
                        if ui.checkbox(input.solo_pfl, "Solo PFL (pre-fader)").changed() {
                            ui.close_menu();
                        }
                    });
                    if solo.clicked() {
                        *input.solo = !*input.solo;
                    }
                    if ui.add_sized([24.0, 19.0], egui::Button::new("↔").selected(*input.auto))
                        .on_hover_text("Automatically mix audio with Program").clicked()
                    {
                        *input.auto = !*input.auto;
                    }
                }
                if ui.add_sized([24.0, 24.0], egui::Button::new(if *on { "♪" } else { "×" })
                    .fill(if *on { Color32::from_rgb(0, 100, 0) } else { BTN }))
                    .on_hover_text(if *on { "Mute" } else { "Unmute" }).clicked()
                {
                    *on = !*on;
                }
            });
            let (scale, _) = ui.allocate_exact_size(Vec2::new(8.0, meter_height), Sense::hover());
            let p = ui.painter();
            for tick in 0..=10 {
                let y = egui::lerp((scale.top() + 6.0)..=(scale.bottom() - 6.0), tick as f32 / 10.0);
                let length = if tick % 5 == 0 { 6.0 } else { 3.0 };
                p.hline((scale.right() - length)..=scale.right(), y, Stroke::new(1.0_f32, TEXT_DIM));
            }
            ui.spacing_mut().slider_width = meter_height;
            ui.spacing_mut().slider_rail_height = 3.0;
            ui.add(egui::Slider::new(volume, 0.0..=100.0).vertical().show_value(false)
                .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.48 }))
                .on_hover_text(format!("Volume {:.0}%", *volume));
            for (label, level) in ["Left", "Right"].into_iter().zip(levels) {
                let (r, hover) = ui.allocate_exact_size(Vec2::new(10.0, meter_height), Sense::hover());
                let db = 20.0 * level.log10();
                render::meter(ui.painter(), r, meter_fraction(level));
                hover.on_hover_text(if level > 0.0 { format!("{label} {db:.1} dBFS") } else { format!("{label} −∞ dBFS") });
            }
        });
        ui.horizontal(|ui| {
            if let Some(input) = input {
                for ((name, bus), enabled) in [("M", "Master"), ("A", "Bus A"), ("B", "Bus B")].into_iter().zip(input.buses) {
                    if ui.add_sized([25.0, 18.0], egui::Button::new(name).selected(*enabled))
                        .on_hover_text(format!("Route to {bus}")).clicked() { *enabled = !*enabled; }
                }
            } else if let Some((monitor, controls)) = monitor {
                if ui.add_sized([25.0, 18.0], egui::Button::new("HP").selected(*monitor))
                    .on_hover_text("Headphone monitoring through the system output").clicked()
                {
                    *monitor = !*monitor;
                }
                headphone_knob(ui, &mut controls.headphones);
                egui::ComboBox::from_id_salt("audio-monitor-bus").width(35.0)
                    .selected_text(["M", "A", "B"][usize::from(controls.bus)])
                    .show_ui(ui, |ui| {
                        for (index, label) in ["Master", "A", "B"].into_iter().enumerate() {
                            ui.selectable_value(&mut controls.bus, index as u8, label);
                        }
                    });
            }
        });
        settings
    }).inner).inner
}

// ---------------------------------------------------------------- bottom bar

fn bottom_bar(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    egui::TopBottomPanel::bottom("bottom")
        .exact_height(40.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(ctx.style().visuals.window_fill()).inner_margin(egui::Margin::symmetric(2, 4)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if big_btn(ui, "Add Input", false, None).on_hover_text("Add a new Input such as a Video, Camera or Image").clicked() {
                    s.add_dialog.open = true;
                }
                ui.add_space(14.0);
                let red = ACTIVE_RED;
                if gear_btn(ui, s.rec_setup, "Configure Recording").clicked() {
                    s.rec_setup = !s.rec_setup;
                }
                if big_btn(ui, &timer_label("Record", s.rec, t), s.rec.is_some(), Some(red)).on_hover_text("Start or stop Recording").clicked() {
                    // frmmain::zzo @0x1ab0c: start at once; stopping asks first (RecordingConfirm).
                    if s.rec.is_some() {
                        if s.cfg.recording_confirm {
                            s.confirm_stop = Some("Recording");
                        } else {
                            s.rec = None;
                        }
                    } else {
                        s.rec = Some(t);
                    }
                }
                ui.add_space(14.0);
                external_menu(ui, s);
                if big_btn(ui, "External", s.external[0].open || s.decklink_active[0], Some(red)).on_hover_text("Start or stop External Output 1").clicked() {
                    s.toggle_external_route(0);
                }
                ui.add_space(14.0);
                if gear_btn(ui, s.stream_setup, "Configure Streaming settings").clicked() {
                    s.stream_setup = !s.stream_setup;
                }
                let streaming = s.stream_active.contains(&true);
                let status = s.stream_status.iter().enumerate().filter(|(_, status)| !status.is_empty())
                    .map(|(i, status)| format!("Stream {} · {status}", i + 1)).collect::<Vec<_>>().join("\n");
                if big_btn(ui, "Stream", streaming, Some(red))
                    .on_hover_text(if status.is_empty() { "Start or stop all configured streams".into() } else { status })
                    .clicked()
                {
                    if streaming {
                        s.stream_active.fill(false);
                    } else if s.cfg.streams.iter().all(|dest| dest.url.trim().is_empty()) {
                        s.stream_setup = true;
                    } else {
                        for (active, dest) in s.stream_active.iter_mut().zip(&s.cfg.streams) {
                            *active = !dest.url.trim().is_empty();
                        }
                    }
                }
                ui.add_space(14.0);
                if gear_btn(ui, s.mc_setup, "Configure recording of multiple camera inputs").clicked() {
                    s.mc_setup = !s.mc_setup;
                }
                if big_btn(ui, &timer_label("MultiCorder", s.multicorder, t), s.multicorder.is_some(), Some(red)).on_hover_text("Start or Stop MultiCorder").clicked() {
                    if s.multicorder.is_some() {
                        if s.cfg.recording_confirm {
                            s.confirm_stop = Some("MultiCorder");
                        } else {
                            s.multicorder = None;
                        }
                    } else {
                        s.multicorder = Some(t);
                    }
                }
                ui.add_space(14.0);
                if gear_btn(ui, s.playlist_setup, "Configure a PlayList of Inputs").clicked() {
                    s.playlist_setup = !s.playlist_setup;
                }
                if big_btn(ui, "PlayList", s.playlist.is_some(), Some(HOVER)).on_hover_text("Start or Stop a PlayList of Inputs").clicked() {
                    s.playlist = if s.playlist.is_some() { None } else { Some(t) };
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if big_btn(ui, "Snapshot", false, None).on_hover_text("Take image snapshot of Output (saved in Pictures\\LiveMix)").clicked() {
                        s.snapshot_req = true;
                    }
                    if big_btn(ui, "Replay", s.replay_arm, None).clicked() {
                        s.replay_open = true;
                        s.replay_setup_open = s.replay_session.is_empty();
                    }
                    if big_btn(ui, "Audio Mixer", s.show_mixer, Some(Color32::from_rgb(0x00, 0x64, 0x00))).on_hover_text("Show or hide audio mixer pane").clicked() {
                        s.show_mixer = !s.show_mixer;
                    }
                    if big_btn(ui, "Overlay", s.overlay_dialog, None).on_hover_text("Configure overlay settings").clicked() {
                        s.overlay_dialog = !s.overlay_dialog;
                    }
                });
            });
        });
}

fn timer_label(name: &str, v: Option<f64>, t: f64) -> String {
    match v {
        Some(st) => format!("{name}  {}", clock(t - st)),
        None => name.to_string(),
    }
}

// ---------------------------------------------------------------- input settings window

/// The original's Input Settings sections, in its order (frmInputSettings.tMenu.*), plus the
/// Audio page: vMix treats audio in the separate Audio Mixer, but Inside Settings it is
/// reachable from General > Audio Settings, so it lives here as its own section.
const INPUT_PAGES: [&str; 13] = [
    "General",
    "Colour Adjust",
    "Colour Key",
    "Colour Correction",
    "Audio",
    "Effects",
    "Position",
    "Layers",
    "Triggers",
    "Tally Lights",
    "PTZ",
    "Advanced",
    "Telestrator",
];
/// Sections built in this version (indexes into INPUT_PAGES).
const INPUT_PAGES_BUILT: [usize; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

/// cboAspectRatio entries: value 0 keeps the source's own shape.
const ASPECTS: [(f32, &str); 9] = [
    (0.0, "Default"),
    (16.0 / 9.0, "16:9"),
    (4.0 / 3.0, "4:3"),
    (16.0 / 10.0, "16:10"),
    (21.0 / 9.0, "21:9"),
    (2.35, "2.35:1"),
    (1.85, "1.85:1"),
    (1.0, "1:1"),
    (9.0 / 16.0, "9:16"),
];

fn aspect_name(v: f32) -> &'static str {
    ASPECTS.iter().find(|(a, _)| (a - v).abs() < 0.001).map_or("Custom", |(_, n)| n)
}

fn input_ptz(ui: &mut egui::Ui, input: &mut Input) {
    use ptz::Ptz;
    let key = egui::Id::new(("ptz-result", input.id));
    let result: Arc<Mutex<Option<Result<(), String>>>> = ui.data_mut(|data| {
        data.get_temp_mut_or_insert_with(key, || Arc::new(Mutex::new(Some(Ok(()))))).clone()
    });
    let state = result.lock().unwrap().clone();
    ui.label("Camera IP address");
    ui.text_edit_singleline(&mut input.ptz_address);
    ui.label("VISCA over UDP uses port 52381 by default.");
    let mut preset = ui.data_mut(|data| data.get_temp::<u8>(key.with("preset")).unwrap_or(0));
    let mut command = None;
    ui.add_enabled_ui(state.is_some() && !input.ptz_address.trim().is_empty(), |ui| {
        egui::Grid::new(key.with("movement")).show(ui, |ui| {
            ui.label("");
            if ui.button("↑").clicked() { command = Some(Ptz::Move { pan: 0, tilt: 1 }); }
            ui.end_row();
            if ui.button("←").clicked() { command = Some(Ptz::Move { pan: -1, tilt: 0 }); }
            if ui.button("Home").clicked() { command = Some(Ptz::Home); }
            if ui.button("→").clicked() { command = Some(Ptz::Move { pan: 1, tilt: 0 }); }
            ui.end_row();
            ui.label("");
            if ui.button("↓").clicked() { command = Some(Ptz::Move { pan: 0, tilt: -1 }); }
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.button("Zoom in").clicked() { command = Some(Ptz::ZoomIn); }
            if ui.button("Zoom out").clicked() { command = Some(Ptz::ZoomOut); }
        });
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut preset).range(0..=15).prefix("Preset "));
            if ui.button("Recall").clicked() { command = Some(Ptz::Recall(preset)); }
            if ui.button("Store").clicked() { command = Some(Ptz::Store(preset)); }
        });
    });
    ui.data_mut(|data| data.insert_temp(key.with("preset"), preset));
    if let Some(command) = command {
        *result.lock().unwrap() = None;
        let address = input.ptz_address.clone();
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            *result.lock().unwrap() = Some(ptz::send(&address, command));
            ctx.request_repaint();
        });
    }
    match state {
        None => { ui.label("Sending…"); }
        Some(Err(error)) => { ui.colored_label(Color32::LIGHT_RED, error); }
        Some(Ok(())) => {}
    }
}

/// Camera capture options (grid rows): Resolution, Frame Rate and Video Format, picked from the
/// modes the camera itself offers; "Auto" takes the best one up to the Performance limit.
/// A new choice restarts the camera with that mode.
fn camera_mode_rows(ui: &mut egui::Ui, inp: &mut Input) {
    let modes = inp.cam_modes.clone();
    let cur = inp.cam_current.clone();
    ui.label("Resolution");
    if modes.is_empty() {
        ui.label(RichText::new("Reading the camera's formats…").color(TEXT_DIM));
        ui.end_row();
        return;
    }
    let chosen = inp.cam_mode.clone();
    // What the three lists show: the chosen mode, or (Auto) the one in use.
    let shown = chosen.clone().or(cur.clone());
    let res_name = |w: u32, h: u32| format!("{w} x {h}");
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    for m in &modes {
        if !sizes.contains(&(m.w, m.h)) {
            sizes.push((m.w, m.h));
        }
    }
    let mut pick: Option<Option<media::CamMode>> = None;
    let res_text = match &chosen {
        None => format!("Auto ({})", cur.as_ref().map(|m| res_name(m.w, m.h)).unwrap_or_default()),
        Some(m) => res_name(m.w, m.h),
    };
    egui::ComboBox::from_id_salt(("cam-res", inp.id)).width(200.0).selected_text(res_text).show_ui(ui, |ui| {
        if ui.selectable_label(chosen.is_none(), "Auto").clicked() {
            pick = Some(None);
        }
        for (w, h) in &sizes {
            let sel = chosen.as_ref().map_or(false, |m| (m.w, m.h) == (*w, *h));
            if ui.selectable_label(sel, res_name(*w, *h)).clicked() {
                // Highest frame rate at that size, keeping the current format when possible.
                let fmt = shown.as_ref().map(|m| m.format.clone()).unwrap_or_default();
                let mut c: Vec<&media::CamMode> = modes.iter().filter(|m| (m.w, m.h) == (*w, *h)).collect();
                c.sort_by(|a, b| b.fps().partial_cmp(&a.fps()).unwrap_or(std::cmp::Ordering::Equal).then((b.format == fmt).cmp(&(a.format == fmt))));
                pick = c.first().map(|m| Some((*m).clone()));
            }
        }
    });
    ui.end_row();
    if let Some(s) = &shown {
        ui.label("Frame Rate");
        let mut rates: Vec<&media::CamMode> = modes.iter().filter(|m| (m.w, m.h) == (s.w, s.h)).collect();
        rates.dedup_by(|a, b| (a.fps() - b.fps()).abs() < 0.001);
        let rate_name = |m: &media::CamMode| {
            let f = m.fps();
            if (f - f.round()).abs() < 0.01 { format!("{} fps", f.round() as i64) } else { format!("{f:.2} fps") }
        };
        egui::ComboBox::from_id_salt(("cam-fps", inp.id)).width(200.0).selected_text(rate_name(s)).show_ui(ui, |ui| {
            let mut seen: Vec<String> = Vec::new();
            for m in modes.iter().filter(|m| (m.w, m.h) == (s.w, s.h)) {
                let n = rate_name(m);
                if seen.contains(&n) {
                    continue;
                }
                seen.push(n.clone());
                if ui.selectable_label(chosen.is_some() && n == rate_name(s), n).clicked() {
                    let same_fmt = modes.iter().find(|x| (x.w, x.h) == (s.w, s.h) && (x.fps() - m.fps()).abs() < 0.001 && x.format == s.format);
                    pick = Some(Some(same_fmt.unwrap_or(m).clone()));
                }
            }
        });
        let _ = rates;
        ui.end_row();
        ui.label("Video Format");
        egui::ComboBox::from_id_salt(("cam-fmt", inp.id)).width(200.0).selected_text(s.format.clone()).show_ui(ui, |ui| {
            for m in modes.iter().filter(|m| (m.w, m.h) == (s.w, s.h) && (m.fps() - s.fps()).abs() < 0.001) {
                if ui.selectable_label(chosen.as_ref() == Some(m), m.format.clone()).clicked() {
                    pick = Some(Some(m.clone()));
                }
            }
        });
        ui.end_row();
        ui.label("In use");
        ui.label(RichText::new(cur.as_ref().map(|m| m.label()).unwrap_or_else(|| "—".into())).color(TEXT_DIM));
        ui.end_row();
    }
    if let Some(p) = pick {
        if p != inp.cam_mode {
            inp.cam_mode = p;
            inp.cam_current = None;
        }
    }
}

fn input_general(ui: &mut egui::Ui, inp: &mut Input, others: &[(usize, u32, String)], page: &mut usize,
    project: ([u32; 2], f32)) {
    egui::Grid::new("in-general").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut inp.title);
        ui.end_row();
        ui.label("Type");
        let kind = match &inp.kind {
            Kind::Device { link, .. } if link.starts_with("ndi:") => "NDI",
            Kind::Device { link, .. } if link.starts_with("omt:") => "OMT",
            Kind::Device { link, .. } if link.starts_with("srt:") => "SRT",
            Kind::Device { blackmagic: true, .. } => "Camera (Blackmagic)",
            Kind::Device { .. } => "Camera",
            Kind::VideoFile { .. } => "Video",
            Kind::VideoList { .. } => "List",
            Kind::Replay { .. } => "Instant Replay",
            Kind::ImageFile { .. } => "Image",
            Kind::Photos { .. } => "Photos",
            Kind::ImageSequence { .. } => "Image Sequence",
            Kind::VideoDelay { .. } => "Video Delay",
            Kind::Telestrator { .. } => "Telestrator",
            Kind::Desktop => "Desktop Capture",
            Kind::Colour(_) => "Colour",
            Kind::Bars => "Colour Bars",
            Kind::Blank => "Transparent",
            Kind::Title { .. } => "Title",
            Kind::GtTitle { .. } => "GT Title",
            Kind::Clock => "Production Clock",
            Kind::AudioFile { .. } => "Audio",
            Kind::Camera(_) => "Test Camera",
            Kind::Video { .. } => "Test Video",
            Kind::Image(_) => "Test Image",
        };
        ui.label(kind);
        ui.end_row();
        let file = match &inp.kind {
            Kind::VideoFile { path } | Kind::AudioFile { path } | Kind::ImageFile { path } | Kind::GtTitle { path, .. }
            | Kind::Replay { path, .. } => Some(path.clone()),
            Kind::Photos { folder } | Kind::ImageSequence { folder } => Some(folder.clone()),
            _ => None,
        };
        if let Some(f) = file {
            ui.label("File");
            ui.label(RichText::new(f).size(11.0).color(TEXT_DIM));
            ui.end_row();
        }
        let decklink = match &inp.kind {
            Kind::Device { link, .. } => decklink::device_index(link),
            _ => None,
        };
        if let Some(device) = decklink {
            ui.label("Connector");
            decklink_connector(ui, "decklink-capture-port", &mut inp.decklink_port);
            ui.end_row();
            ui.label("Video format");
            decklink_mode_picker(ui, "input-decklink-mode", device, false, &mut inp.decklink_mode);
            ui.end_row();
            ui.label("Detected mode");
            ui.label(inp.cam_current.as_ref().map(|mode| mode.label()).unwrap_or_else(|| inp.status.clone()));
            ui.end_row();
        } else if let Kind::Device { link, .. } = &inp.kind {
            if let Some(name) = link.strip_prefix("ndi:").or_else(|| link.strip_prefix("omt:")) {
                ui.label("Source");
                ui.label(name);
                ui.end_row();
                ui.label("Signal");
                ui.label(&inp.status);
                ui.end_row();
            } else if link.starts_with("srt:") {
                ui.label("Signal");
                ui.label(&inp.status);
                ui.end_row();
            } else {
                camera_mode_rows(ui, inp);
            }
        }
        if let Kind::Title { text, sub } = &mut inp.kind {
            ui.label("Text");
            ui.text_edit_singleline(text);
            ui.end_row();
            ui.label("Line 2");
            ui.text_edit_singleline(sub);
            ui.end_row();
        }
        if let Kind::Colour(c) = &mut inp.kind {
            ui.label("Colour");
            ui.color_edit_button_srgba(c);
            ui.end_row();
        }
        // frmInput tVideoDelay: source, buffer seconds and resolution, with the buffer state.
        if let Kind::VideoDelay { source, seconds, resolution } = &mut inp.kind {
            ui.label("Source");
            let current = source.and_then(|id| others.iter().find(|(_, i, _)| *i == id))
                .map(|(n, _, title)| format!("{n} {title}"))
                .unwrap_or_else(|| "Choose an input".into());
            egui::ComboBox::from_id_salt(("delay-src", inp.id)).width(220.0).selected_text(current)
                .show_ui(ui, |ui| {
                    for (number, id, title) in others {
                        ui.selectable_value(source, Some(*id), format!("{number} {title}"));
                    }
                });
            ui.end_row();
            ui.label("Seconds");
            ui.add(egui::DragValue::new(seconds).range(1.0..=120.0).suffix(" s").speed(0.1));
            ui.end_row();
            ui.label("Resolution");
            let [w, h] = *resolution;
            egui::ComboBox::from_id_salt(("delay-res", inp.id)).width(140.0)
                .selected_text(format!("{w}x{h}")).show_ui(ui, |ui| {
                    for [w, h] in DELAY_RESOLUTIONS {
                        ui.selectable_value(resolution, [w, h], format!("{w}x{h}"));
                    }
                });
            ui.end_row();
            ui.label("Buffer");
            ui.label(RichText::new(format!("about {:.0} MB", f64::from(w * h * 4)
                * f64::from(*seconds) / 1_048_576.0)).color(TEXT_DIM));
            ui.end_row();
            ui.label("Status");
            ui.label(if inp.status.is_empty() { "Loading VideoDelay" } else { &inp.status });
            ui.end_row();
        }
    });
    ui.add_space(4.0);
    if let Kind::GtTitle { document, texts, .. } = &mut inp.kind {
        if let Some(document) = document {
            ui.heading("Title Editor");
            for (field, default) in &document.fields {
                ui.label(format!("{field}.Text"));
                let mut value = texts.get(field).unwrap_or(default).clone();
                if ui.add(egui::TextEdit::multiline(&mut value).desired_rows(1)).changed() {
                    let now = ui.input(|i| i.time);
                    let previous = inp.gt_change.as_ref().filter(|change| {
                        matches!(document.data_cue((now - change.started) as f32, &change.field), Some(gt::Cue::DataOut(_, _)))
                    }).map(|change| change.previous.clone()).unwrap_or_else(|| texts.clone());
                    inp.gt_change = Some(gt::DataChange { started: now, field: field.clone(), previous });
                    if &value == default {
                        texts.remove(field);
                    } else {
                        texts.insert(field.clone(), value);
                    }
                }
            }
            if !document.unsupported.is_empty() {
                ui.label("This template uses unsupported GT features")
                    .on_hover_text(document.unsupported.iter().cloned().collect::<Vec<_>>().join(", "));
            }
        }
        if !inp.status.is_empty() {
            ui.colored_label(Color32::LIGHT_RED, &inp.status);
        }
        if ui.button("Reload GT Title").clicked() {
            if let Err(error) = inp.reload_gt() {
                inp.status = error;
            }
        }
    }
    // Source information (txtSourceAspectRatio / txtSourceFrameRate / txtSourceResolution /
    // txtSourceDeinterlacing in frmInputSettings pGeneral).
    let unavailable = || "Not available".to_string();
    let (source_aspect, source_rate, source_res) = match (&inp.cam_current, inp.tex) {
        (Some(mode), _) if mode.w > 0 && mode.h > 0 => (
            format!("{:.3}:1", mode.w as f32 / mode.h as f32),
            format!("{:.2} fps", mode.fps()),
            format!("{} x {}", mode.w, mode.h),
        ),
        (_, Some((_, [w, h]))) if w > 0 && h > 0 => (
            format!("{:.3}:1", w as f32 / h as f32), unavailable(), format!("{w} x {h}"),
        ),
        // Pictures the app draws itself (colour, bars, titles, test inputs) are produced at the
        // project size.
        _ => {
            let ([w, h], fps) = project;
            (format!("{:.3}:1", w as f32 / h.max(1) as f32), format!("{fps:.2} fps"),
                format!("{w} x {h}"))
        }
    };
    ui.add_space(4.0);
    egui::Grid::new(("in-source-info", inp.id)).num_columns(2).spacing([12.0, 3.0]).show(ui, |ui| {
        ui.label(RichText::new("Source Aspect Ratio").size(11.0));
        ui.label(RichText::new(source_aspect).size(11.0).color(TEXT_DIM));
        ui.end_row();
        ui.label(RichText::new("Frame Rate").size(11.0));
        ui.label(RichText::new(source_rate).size(11.0).color(TEXT_DIM));
        ui.end_row();
        ui.label(RichText::new("Resolution").size(11.0));
        ui.label(RichText::new(source_res).size(11.0).color(TEXT_DIM));
        ui.end_row();
        ui.label(RichText::new("Deinterlacing").size(11.0));
        ui.label(RichText::new(if inp.deinterlace { "Blend" } else { "None" }).size(11.0).color(TEXT_DIM));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        // cmdAudioSettings: opens the Audio section of this window.
        if ui.add_enabled(inp.kind.has_audio(), egui::Button::new("Audio Settings")).clicked() {
            *page = 4;
        }
        if ui.add_enabled(false, egui::Button::new("Properties"))
            .on_disabled_hover_text("Source properties are provided by the capture device driver.")
            .clicked() {}
    });
    if !inp.status.is_empty() {
        ui.label(RichText::new(&inp.status).size(11.0).color(TEXT_DIM));
    }
    ui.separator();
    // chkMirror / chkSharpen / chkDeinterlace, the transition options and chkFlatten.
    setting_toggle(ui, &mut inp.mirror, "Mirror");
    setting_toggle(ui, &mut inp.sharpen, "Sharpen").on_hover_text("Sharpen capture inputs");
    setting_toggle(ui, &mut inp.deinterlace, "Deinterlace Blend").on_hover_text("Blend fields of interlaced sources");
    if inp.kind.playable() {
        setting_toggle(ui, &mut inp.loop_on, "Loop");
    }
    setting_toggle(ui, &mut inp.play_active, "Automatically Play with Transition");
    setting_toggle(ui, &mut inp.pause_preview, "Automatically Pause after Transition");
    setting_toggle(ui, &mut inp.restart_active, "Automatically Restart with Transition");
    setting_toggle(ui, &mut inp.auto_mix_audio, "Automatically mix audio");
    setting_toggle(ui, &mut inp.flatten, "Flatten Layers").on_hover_text("Render the layers into one picture");
    ui.horizontal(|ui| {
        ui.label("Aspect Ratio");
        egui::ComboBox::from_id_salt(("in-aspect", inp.id)).width(140.0)
            .selected_text(aspect_name(inp.aspect)).show_ui(ui, |ui| {
                for (value, name) in ASPECTS {
                    ui.selectable_value(&mut inp.aspect, value, name);
                }
            });
        ui.label("Category");
        let cur = inp.category.map(|c| CATEGORIES[c as usize].0).unwrap_or("None");
        egui::ComboBox::from_id_salt("in-cat").selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut inp.category, None, "None");
            for (k, (name, _)) in CATEGORIES.iter().enumerate() {
                ui.selectable_value(&mut inp.category, Some(k as u8), *name);
            }
        });
    });
    ui.horizontal(|ui| {
        ui.label("Mouse Click Action");
        let names = ["Open Input Settings", "Preview", "Cut to Program"];
        egui::ComboBox::from_id_salt(("click-action", inp.id)).width(160.0)
            .selected_text(names[usize::from(inp.click_action).min(names.len() - 1)])
            .show_ui(ui, |ui| {
                for (value, name) in names.iter().enumerate() {
                    ui.selectable_value(&mut inp.click_action, value as u8, *name);
                }
            });
    });
}

/// Telestrator (frmInput tTelestrator): the original only points at its web page; here the page
/// also draws directly over the Output picture.
fn input_telestrator(ui: &mut egui::Ui, inp: &mut Input, all: &[Input], t: f64, output: Option<u32>, web: bool) {
    let Kind::Telestrator { strokes } = &mut inp.kind else {
        ui.label(RichText::new("The Telestrator section draws on a Telestrator input. Add one from Add Input.").color(TEXT_DIM));
        return;
    };
    if web {
        ui.label(RichText::new(format!("Web Controller: http://{}/telestrator/", automation::WEB_ADDRESS))
            .color(Color32::from_rgb(0x40, 0xb0, 0x50)));
    } else {
        ui.label(RichText::new("Please enable Telestrator under Settings - Web Controller first")
            .color(Color32::from_rgb(0xb0, 0x30, 0x30)));
    }
    let pen = egui::Id::new(("tel-pen", inp.id));
    let mut colour = ui.data(|data| data.get_temp::<[u8; 3]>(pen).unwrap_or([255, 60, 60]));
    let mut width = ui.data(|data| data.get_temp::<f32>(pen.with("width")).unwrap_or(0.006));
    ui.horizontal(|ui| {
        ui.label("Colour");
        for c in [[255u8, 60, 60], [60, 220, 60], [60, 120, 255], [255, 220, 60], [255, 255, 255], [230, 60, 220]] {
            let colour32 = Color32::from_rgb(c[0], c[1], c[2]);
            let stroke = if colour == c { Stroke::new(2.0_f32, Color32::WHITE) } else { Stroke::NONE };
            if ui.add(egui::Button::new("      ").fill(colour32).stroke(stroke)).clicked() {
                colour = c;
            }
        }
        ui.label("Width");
        ui.add(egui::Slider::new(&mut width, 0.001..=0.02).show_value(false));
        if ui.button("Undo").clicked() {
            strokes.pop();
        }
        if ui.button("Clear").clicked() {
            strokes.clear();
        }
    });
    ui.data_mut(|data| {
        data.insert_temp(pen, colour);
        data.insert_temp(pen.with("width"), width);
    });
    // Draw over the Output picture (the input being drawn on, before overlays).
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().clamp(1.0, 480.0), 270.0), Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    if let Some(out) = output.and_then(|id| all.iter().find(|input| input.id == id)) {
        draw_source(&painter, rect, out, 1.0, t, all);
    } else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "No Output",
            egui::FontId::proportional(16.0), Color32::GRAY);
    }
    draw_strokes(&painter, rect, strokes, 1.0);
    let normalise = |pos: egui::Pos2| [
        (pos.x - rect.left()) / rect.width().max(1.0),
        (pos.y - rect.top()) / rect.height().max(1.0),
    ];
    if let Some(pos) = response.interact_pointer_pos().filter(|pos| rect.contains(*pos)) {
        if response.drag_started() || response.clicked() {
            strokes.push(Telestroke { points: vec![normalise(pos)], colour, width });
        } else if response.dragged() {
            if let Some(stroke) = strokes.last_mut() {
                let point = normalise(pos);
                if stroke.points.last().is_none_or(|last| {
                    (last[0] - point[0]).abs() + (last[1] - point[1]).abs() > 0.0005
                }) {
                    stroke.points.push(point);
                }
            }
        }
    }
    ui.label(RichText::new("Drag on the picture to draw. The Telestrator is a transparent layer: add it as an Overlay over Program.")
        .size(11.0).color(TEXT_DIM));
}

fn input_colour_key(ui: &mut egui::Ui, inp: &mut Input) {
    if !inp.kind.keyable() {
        ui.label(RichText::new("Colour keying works on cameras, video files, lists and desktop capture.").color(TEXT_DIM));
        return;
    }
    setting_toggle(ui, &mut inp.key.on, "Chroma Key (remove this colour)");
    ui.horizontal(|ui| {
        ui.label("Key colour");
        ui.color_edit_button_srgb(&mut inp.key.colour);
        if ui.small_button("Green").clicked() {
            inp.key.colour = [0, 177, 64];
        }
        if ui.small_button("Blue").clicked() {
            inp.key.colour = [0, 71, 187];
        }
    });
    ui.horizontal(|ui| {
        ui.label("Tolerance");
        ui.add(egui::Slider::new(&mut inp.key.tolerance, 5..=150));
        if ui.small_button("Reset").on_hover_text("Restore the default colour key settings").clicked() {
            inp.key = Key::default();
        }
    });
    // Auto Chroma Key presets (frmInputSettings::zp / zq): key on, slider at 0.7 / 0.8 of its range.
    ui.horizontal(|ui| {
        ui.label("Auto Chroma Key Presets");
        for (n, f) in [(1, 0.7f32), (2, 0.8f32)] {
            if ui.small_button(format!("{n}")).clicked() {
                inp.key.on = true;
                inp.key.tolerance = (150.0 * f).round() as u8;
            }
        }
    });
    ui.label(RichText::new("Tip: assign the keyed input to an Overlay (1–8) or a Layer over a background.").size(11.0).color(TEXT_DIM));
}

/// A 256-step grey ramp passed through the input's Colour Adjust/Correction settings: the
/// original's colour bars preview (pColor / pColorCorrection).
fn colour_ramp(ui: &mut egui::Ui, adj: ColourAdjust) {
    let mut px = vec![0u8; 256 * 4];
    for (i, chunk) in px.chunks_exact_mut(4).enumerate() {
        chunk.copy_from_slice(&[i as u8, i as u8, i as u8, 255]);
    }
    adj.apply(&mut px);
    let width = ui.available_width().min(430.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::hover());
    let p = ui.painter_at(rect);
    let slices = 128;
    for k in 0..slices {
        let x0 = rect.left() + rect.width() * k as f32 / slices as f32;
        let x1 = rect.left() + rect.width() * (k + 1) as f32 / slices as f32;
        let i = (k * 2).min(255) * 4;
        let c = Color32::from_rgb(px[i], px[i + 1], px[i + 2]);
        p.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())), 0.0, c);
    }
    p.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, Color32::from_gray(60)), egui::StrokeKind::Inside);
}

/// frmInputSettings Colour Adjust: Brightness, Contrast and Saturation.
fn input_colour_adjust(ui: &mut egui::Ui, inp: &mut Input) {
    let adj = &mut inp.colour_adjust;
    ui.add(egui::Slider::new(&mut adj.brightness, -1.0..=1.0).text("Brightness"));
    ui.add(egui::Slider::new(&mut adj.contrast, -1.0..=1.0).text("Contrast"));
    ui.add(egui::Slider::new(&mut adj.saturation, -1.0..=1.0).text("Saturation"));
    ui.add(egui::Slider::new(&mut adj.alpha, -1.0..=1.0).text("Alpha"));
    ui.horizontal(|ui| {
        if ui.button("Reset").clicked() {
            *adj = ColourAdjust::default();
        }
        ui.label(RichText::new("Applies to capture, video, image and list inputs.").size(11.0).color(TEXT_DIM));
    });
    ui.add_space(6.0);
    colour_ramp(ui, *adj);
}

/// frmInputSettings Colour Correction: per-channel gains, black/white stretch and the
/// 0-255 / 16-235 level presets.
fn input_colour_correction(ui: &mut egui::Ui, inp: &mut Input) {
    let adj = &mut inp.colour_adjust;
    egui::Grid::new(("colour-correct", inp.id)).num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
        ui.label("Red");
        ui.add(egui::Slider::new(&mut adj.red, -1.0..=1.0).show_value(true));
        ui.end_row();
        ui.label("Green");
        ui.add(egui::Slider::new(&mut adj.green, -1.0..=1.0));
        ui.end_row();
        ui.label("Blue");
        ui.add(egui::Slider::new(&mut adj.blue, -1.0..=1.0));
        ui.end_row();
        ui.label("Alpha");
        ui.add(egui::Slider::new(&mut adj.alpha, -1.0..=1.0));
        ui.end_row();
        ui.label("Saturation");
        ui.add(egui::Slider::new(&mut adj.saturation, -1.0..=1.0));
        ui.end_row();
        ui.label("Black Stretch");
        ui.add(egui::Slider::new(&mut adj.black, -0.5..=0.5));
        ui.end_row();
        ui.label("White Stretch");
        ui.add(egui::Slider::new(&mut adj.white, -0.5..=0.5));
        ui.end_row();
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("Auto").on_hover_text("Basic auto correct").clicked() {
            *adj = ColourAdjust::default();
        }
        if ui.button("Auto White Balance").on_hover_text("Neutralise the colour channels").clicked() {
            adj.red = 0.0;
            adj.green = 0.0;
            adj.blue = 0.0;
        }
        if ui.button("Reset").clicked() {
            *adj = ColourAdjust::default();
        }
        if ui.button("0-255").on_hover_text("Full range levels").clicked() {
            adj.black = 0.0;
            adj.white = 0.0;
        }
        if ui.button("16-235").on_hover_text("Studio range levels").clicked() {
            adj.black = 0.125;
            adj.white = -0.157;
        }
    });
    ui.add_space(4.0);
    setting_toggle(ui, &mut inp.rec601, "Rec. 601 to 709").on_hover_text("Correct SD colour space sources");
    setting_toggle(ui, &mut inp.premultiplied, "Premultiplied Alpha");
    ui.add_space(6.0);
    colour_ramp(ui, *adj);
}

/// Effects: the original's pEffects page (chkUseSourceRenderEffects, the effect list and its
/// properties). The original loads render-effect plug-ins; this version lists the render
/// effects it implements, with quick controls and links to their full pages.
fn input_effects(ui: &mut egui::Ui, inp: &mut Input, page: &mut usize) {
    let sel_id = egui::Id::new(("effects-sel", inp.id));
    let mut sel: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0).min(5);
    setting_toggle(ui, &mut inp.effects_source, "Use Source Settings")
        .on_hover_text("Use the render effects configured on the source input.");
    if inp.effects_source {
        ui.label(RichText::new("The input uses the source's render effects; clear the box to edit its own.")
            .size(11.0).color(TEXT_DIM));
    }
    ui.separator();
    let names = ["Mirror", "Sharpen", "Deinterlace Blend", "Colour Correction", "Colour Key", "Position / Crop"];
    egui::ScrollArea::vertical().max_height(142.0).id_salt(("effects-list", inp.id)).show(ui, |ui| {
        for (k, name) in names.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.selectable_label(sel == k, RichText::new(*name).strong()).clicked() {
                    sel = k;
                }
                match k {
                    0 => { setting_toggle(ui, &mut inp.mirror, "On"); }
                    1 => { setting_toggle(ui, &mut inp.sharpen, "On"); }
                    2 => { setting_toggle(ui, &mut inp.deinterlace, "On"); }
                    3 => {
                        let on = inp.colour_adjust != ColourAdjust::default();
                        ui.label(RichText::new(if on { "Configured" } else { "Off" }).size(10.0).color(TEXT_DIM));
                    }
                    4 => { setting_toggle(ui, &mut inp.key.on, "On"); }
                    _ => {
                        let on = inp.place != Position::default();
                        ui.label(RichText::new(if on { "Configured" } else { "Default" }).size(10.0).color(TEXT_DIM));
                    }
                }
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(sel_id, sel));
    ui.separator();
    ui.label(RichText::new(format!("{} properties", names[sel])).strong());
    match sel {
        0 => {
            setting_toggle(ui, &mut inp.mirror, "Mirror the picture horizontally");
            ui.label(RichText::new("Also on General (chkMirror).").size(10.0).color(TEXT_DIM));
        }
        1 => {
            setting_toggle(ui, &mut inp.sharpen, "Sharpen the picture");
            ui.label(RichText::new("Also on General (chkSharpen).").size(10.0).color(TEXT_DIM));
        }
        2 => {
            setting_toggle(ui, &mut inp.deinterlace, "Deinterlace Blend");
            ui.label(RichText::new("Blends the fields of interlaced sources; also on General.")
                .size(10.0).color(TEXT_DIM));
        }
        3 => {
            ui.add(egui::Slider::new(&mut inp.colour_adjust.brightness, -1.0..=1.0).text("Brightness"));
            ui.add(egui::Slider::new(&mut inp.colour_adjust.contrast, -1.0..=1.0).text("Contrast"));
            ui.add(egui::Slider::new(&mut inp.colour_adjust.saturation, -1.0..=1.0).text("Saturation"));
            if ui.button("Open Colour Correction…").clicked() {
                *page = 3;
            }
        }
        4 => {
            ui.horizontal(|ui| {
                setting_toggle(ui, &mut inp.key.on, "Enable");
                ui.color_edit_button_srgb(&mut inp.key.colour);
                ui.label("Key colour");
            });
            ui.add(egui::Slider::new(&mut inp.key.tolerance, 0..=255).text("Tolerance"));
            if ui.button("Open Colour Key…").clicked() {
                *page = 2;
            }
        }
        _ => {
            ui.label(format!("Zoom {:.2}, Pan X {:.2}, Pan Y {:.2}", inp.place.zoom, inp.place.pan_x, inp.place.pan_y));
            ui.label(RichText::new("Crop is edited on the Position page, together with the preview.")
                .size(10.0).color(TEXT_DIM));
            ui.horizontal(|ui| {
                if ui.button("Open Position…").clicked() {
                    *page = 6;
                }
                if ui.button("Reset position").clicked() {
                    inp.place = Position::default();
                }
            });
        }
    }
    ui.add_space(6.0);
    ui.label(RichText::new("Render-effect plug-ins from the original are not available; the effects above are the ones this version renders.")
        .size(10.0).color(TEXT_DIM));
}

/// Graphic equaliser: 10 bands, drag a node to set its gain (dsp::EQ_BANDS).
fn eq_graph(ui: &mut egui::Ui, eq: &mut [f32; 10]) {
    let width = ui.available_width().clamp(260.0, 460.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 132.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_rgb(0x0d, 0x10, 0x14));
    let plot = rect.shrink2(Vec2::new(10.0, 8.0));
    let y_of = |db: f32| plot.center().y - db / 18.0 * plot.height() / 2.0;
    for (db, strong) in [(-18.0, false), (-12.0, false), (-6.0, false), (0.0, true),
        (6.0, false), (12.0, false), (18.0, false)]
    {
        let y = y_of(db);
        p.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            Stroke::new(1.0_f32, Color32::from_gray(if strong { 80 } else { 38 })));
        p.text(egui::pos2(plot.left() + 2.0, y - 1.0), egui::Align2::LEFT_BOTTOM,
            format!("{db:+.0}"), egui::FontId::proportional(8.0), Color32::from_gray(130));
    }
    let n = eq.len();
    let x_of = |i: usize| plot.left() + (i as f32 + 0.5) / n as f32 * plot.width();
    for (i, gain) in eq.iter().enumerate() {
        p.line_segment([egui::pos2(x_of(i), plot.top()), egui::pos2(x_of(i), plot.bottom())],
            Stroke::new(1.0_f32, Color32::from_gray(26)));
        let _ = gain;
    }
    let points: Vec<egui::Pos2> = eq.iter().enumerate().map(|(i, g)| egui::pos2(x_of(i), y_of(*g))).collect();
    p.add(egui::Shape::line(points.clone(), Stroke::new(2.0_f32, Color32::from_rgb(0x50, 0x9a, 0xe0))));
    for (i, g) in eq.iter().enumerate() {
        p.circle_filled(egui::pos2(x_of(i), y_of(*g)), 4.0, Color32::from_rgb(0x9a, 0xd0, 0xff));
        p.text(egui::pos2(x_of(i), plot.bottom() + 2.0), egui::Align2::CENTER_TOP,
            if *g == 0.0 { String::new() } else { format!("{g:+.1}") },
            egui::FontId::proportional(8.0), Color32::from_gray(150));
    }
    if (resp.dragged() || resp.clicked()) && resp.interact_pointer_pos().is_some() {
        let pos = resp.interact_pointer_pos().unwrap();
        let t = ((pos.x - plot.left()) / plot.width()).clamp(0.0, 0.999);
        let index = (t * n as f32) as usize;
        let db = ((plot.center().y - pos.y) / (plot.height() / 2.0) * 18.0).clamp(-18.0, 18.0);
        eq[index] = (db * 10.0).round() / 10.0;
    }
}

/// Noise gate and compressor transfer curve (input dBFS across, output dBFS up).
fn dynamics_graph(ui: &mut egui::Ui, fx: &mut crate::audio::Effects) {
    let width = ui.available_width().clamp(260.0, 460.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 118.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 3.0, Color32::from_rgb(0x0d, 0x10, 0x14));
    let plot = rect.shrink2(Vec2::new(10.0, 8.0));
    let x_of = |db: f32| plot.left() + (db + 60.0) / 60.0 * plot.width();
    let y_of = |db: f32| plot.top() + (-db) / 60.0 * plot.height();
    for db in [-60.0, -48.0, -36.0, -24.0, -12.0, 0.0] {
        p.line_segment([egui::pos2(x_of(db), plot.top()), egui::pos2(x_of(db), plot.bottom())],
            Stroke::new(1.0_f32, Color32::from_gray(30)));
        p.text(egui::pos2(x_of(db) + 2.0, plot.bottom() - 1.0), egui::Align2::LEFT_BOTTOM,
            format!("{db:.0}"), egui::FontId::proportional(7.5), Color32::from_gray(120));
    }
    for db in [-60.0, -40.0, -20.0, 0.0] {
        p.line_segment([egui::pos2(plot.left(), y_of(db)), egui::pos2(plot.right(), y_of(db))],
            Stroke::new(1.0_f32, Color32::from_gray(30)));
    }
    // Unity reference.
    p.line_segment([egui::pos2(x_of(-60.0), y_of(-60.0)), egui::pos2(x_of(0.0), y_of(0.0))],
        Stroke::new(1.0_f32, Color32::from_gray(60)));
    let gate = fx.gate_threshold;
    let threshold = fx.comp_threshold;
    let ratio = fx.comp_ratio.max(1.0);
    let gain = fx.gain_db;
    let out_db = |db: f32| {
        if db <= gate {
            return -60.0;
        }
        let mut out = db;
        if db > threshold {
            out = threshold + (db - threshold) / ratio;
        }
        (out + gain).clamp(-60.0, 0.0)
    };
    let points: Vec<egui::Pos2> = (0..=120).map(|k| {
        let db = -60.0 + k as f32 * 0.5;
        egui::pos2(x_of(db), y_of(out_db(db)))
    }).collect();
    p.add(egui::Shape::line(points, Stroke::new(2.0_f32, Color32::from_rgb(0x4c, 0xc0, 0x60))));
    for (db, colour, label) in [(gate.clamp(-60.0, 0.0), Color32::from_rgb(0xd0, 0x60, 0x60), "Gate"),
        (threshold.clamp(-60.0, 0.0), Color32::from_rgb(0xe8, 0xc8, 0x40), "Comp")]
    {
        p.line_segment([egui::pos2(x_of(db), plot.top()), egui::pos2(x_of(db), plot.bottom())],
            Stroke::new(1.0_f32, colour));
        p.text(egui::pos2(x_of(db) + 2.0, plot.top() + 1.0), egui::Align2::LEFT_TOP, label,
            egui::FontId::proportional(7.5), colour);
    }
}

fn input_audio_general(ui: &mut egui::Ui, inp: &mut Input) {
    if !inp.kind.has_audio() {
        ui.label(RichText::new("This input has no audio. Video, camera, list, audio file and stream inputs do.")
            .color(TEXT_DIM));
        return;
    }
    let levels = if inp.audio_on { inp.level } else { [0.0; 2] };
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(Vec2::new(46.0, 152.0), egui::Layout::top_down(egui::Align::Center), |ui| {
            ui.label(RichText::new("Volume").size(10.0).color(TEXT_DIM));
            ui.spacing_mut().slider_width = 112.0;
            ui.add(egui::Slider::new(&mut inp.volume, 0.0..=100.0).vertical().show_value(true));
        });
        for level in levels {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(10.0, 136.0), Sense::hover());
            render::meter(ui.painter(), rect, meter_fraction(level));
        }
        let (scale_rect, _) = ui.allocate_exact_size(Vec2::new(30.0, 136.0), Sense::hover());
        {
            let p = ui.painter_at(scale_rect);
            for db in [0.0f32, -6.0, -12.0, -18.0, -24.0, -36.0, -48.0] {
                let k = (db / 60.0 + 1.0).clamp(0.0, 1.0);
                let y = scale_rect.bottom() - k * scale_rect.height();
                p.text(egui::pos2(scale_rect.left() + 2.0, y), egui::Align2::LEFT_CENTER,
                    format!("{db:.0}"), egui::FontId::proportional(8.0), Color32::from_gray(140));
            }
            p.text(egui::pos2(scale_rect.left() + 2.0, scale_rect.top() - 2.0), egui::Align2::LEFT_BOTTOM,
                "dB", egui::FontId::proportional(8.0), Color32::from_gray(100));
        }
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let b = egui::Button::new(RichText::new(if inp.audio_on { "Audio On" } else { "Muted" })
                    .size(11.0).color(Color32::BLACK))
                    .fill(if inp.audio_on { Color32::from_rgb(0x4c, 0xc0, 0x50) } else { Color32::from_rgb(0xd0, 0x40, 0x40) });
                if ui.add(b).clicked() {
                    inp.audio_on = !inp.audio_on;
                }
                ui.label(RichText::new(format!("{:.0}", inp.volume)).size(11.0).color(TEXT_DIM));
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new("Mix").size(10.0).color(TEXT_DIM));
                for (label, value) in [("M", &mut inp.bus_m), ("A", &mut inp.bus_a), ("B", &mut inp.bus_b)] {
                    let b = egui::Button::new(RichText::new(label).size(10.0)
                        .color(if *value { Color32::BLACK } else { TEXT }))
                        .fill(if *value { Color32::from_rgb(0x50, 0x9a, 0xe0) } else { BTN })
                        .min_size(Vec2::new(20.0, 18.0));
                    if ui.add(b).clicked() {
                        *value = !*value;
                    }
                }
            });
            let fx = &mut inp.audio_effects;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Channel").size(10.0).color(TEXT_DIM));
                ui.selectable_value(&mut fx.channel, 0, "Stereo");
                ui.selectable_value(&mut fx.channel, 1, "Left");
                ui.selectable_value(&mut fx.channel, 2, "Right");
            });
            ui.horizontal(|ui| {
                ui.label("Gain");
                ui.add(egui::DragValue::new(&mut fx.gain_db).range(-24.0..=24.0).speed(0.2).suffix(" dB"));
                ui.label("Delay");
                ui.add(egui::DragValue::new(&mut fx.delay_ms).range(0.0..=1000.0).speed(1.0).suffix(" ms"));
            });
            ui.horizontal(|ui| {
                ui.label("Pan");
                ui.add(egui::Slider::new(&mut fx.pan, -1.0..=1.0).show_value(false));
                let label = if fx.pan.abs() < 0.01 { "C".to_string() }
                    else if fx.pan < 0.0 { format!("L{:.0}", -fx.pan * 100.0) }
                    else { format!("R{:.0}", fx.pan * 100.0) };
                ui.label(RichText::new(label).size(10.0).color(TEXT_DIM));
                if ui.small_button("Centre").clicked() {
                    fx.pan = 0.0;
                }
            });
            setting_toggle(ui, &mut inp.auto_mix_audio, "Automatically mix audio");
            setting_toggle(ui, &mut inp.solo_pfl, "Solo PFL (pre-fader)");
        });
    });
}

fn input_audio(ui: &mut egui::Ui, input: &mut Input, page_height: f32) {
    let id = ui.make_persistent_id(("audio-settings-page", input.id));
    let mut page = ui.data(|data| data.get_temp::<usize>(id)).unwrap_or_default();
    ui.horizontal_wrapped(|ui| {
        for (index, name) in ["General", "EQ", "Compressor", "Noise Gate"].into_iter().enumerate() {
            ui.selectable_value(&mut page, index, name);
        }
    });
    ui.data_mut(|data| data.insert_temp(id, page));
    ui.separator();
    egui::ScrollArea::vertical().id_salt(page).max_height(page_height).show(ui, |ui| match page {
        1 => input_audio_eq(ui, input),
        2 => {
            dynamics_graph(ui, &mut input.audio_effects);
            ui.add(egui::Slider::new(&mut input.audio_effects.comp_threshold, -60.0..=0.0).text("Threshold").suffix(" dBFS"));
            ui.add(egui::Slider::new(&mut input.audio_effects.comp_ratio, 1.0..=20.0).text("Ratio").suffix(":1"));
            if ui.button("Reset").clicked() {
                let defaults = audio::Effects::default();
                input.audio_effects.comp_threshold = defaults.comp_threshold;
                input.audio_effects.comp_ratio = defaults.comp_ratio;
            }
        }
        3 => {
            ui.add(egui::Slider::new(&mut input.audio_effects.gate_threshold, -90.0..=0.0).text("Threshold").suffix(" dBFS"));
            if ui.button("Reset").clicked() {
                input.audio_effects.gate_threshold = audio::Effects::default().gate_threshold;
            }
        }
        _ => input_audio_general(ui, input),
    });
}

fn input_audio_eq(ui: &mut egui::Ui, inp: &mut Input) {
    eq_graph(ui, &mut inp.audio_effects.eq);
    egui::Grid::new(("eq-bands", inp.id)).num_columns(4).spacing([8.0, 2.0]).show(ui, |ui| {
        for (index, frequency) in dsp::EQ_BANDS.into_iter().enumerate() {
            ui.label(RichText::new(format!("{frequency:.0}")).size(9.0).color(TEXT_DIM));
            ui.add(egui::DragValue::new(&mut inp.audio_effects.eq[index]).range(-18.0..=18.0)
                .speed(0.2).prefix("").suffix(" dB"));
            if index % 2 == 1 {
                ui.end_row();
            }
        }
    });
    if ui.button("Reset EQ").clicked() {
        inp.audio_effects.eq = audio::Effects::default().eq;
    }
}


/// Tally Lights: the input's own tally colour, plus the original's notes.
fn input_tally(ui: &mut egui::Ui, inp: &mut Input, number: usize) {
    ui.label(format!("Tally Number: {number}"));
    ui.label(RichText::new("The tally colour is used by Tally Lights and the Web Controller.")
        .size(11.0).color(TEXT_DIM));
    let mut custom = inp.tally_colour.is_some();
    let mut colour = inp.tally_colour.unwrap_or([0xff, 0x9a, 0x1a]);
    if setting_toggle(ui, &mut custom, "Use a custom tally colour").changed() {
        inp.tally_colour = custom.then_some(colour);
    }
    ui.horizontal(|ui| {
        ui.add_enabled_ui(custom, |ui| {
            if ui.color_edit_button_srgb(&mut colour).changed() {
                inp.tally_colour = Some(colour);
            }
            ui.label("Colour");
        });
    });
    ui.add_space(6.0);
    ui.label(RichText::new("For information on other Tally Light options, see the Tally Lights tab in Settings.")
        .size(11.0).color(TEXT_DIM));
    ui.label(RichText::new("Arduino Tally Lights").strong());
    ui.label(RichText::new("Serial tally data is sent on the COM port chosen under Settings > Tally Lights.")
        .size(11.0).color(TEXT_DIM));
}

/// Advanced: video Delay, Flatten Layers, Mouse Click Action and the input's identity.
fn input_advanced(ui: &mut egui::Ui, inp: &mut Input, number: usize) {
    ui.label(RichText::new(format!("Input {number} - ID {}", inp.id)));
    ui.horizontal(|ui| {
        ui.label("Delay");
        ui.add(egui::DragValue::new(&mut inp.delay_frames).range(0..=300).suffix(" frames"));
        if ui.button("Apply").clicked() {
            inp.restart_req = true;
        }
    });
    ui.label(RichText::new("Applied when the input restarts.").size(10.0).color(TEXT_DIM));
    ui.separator();
    ui.label(RichText::new("Filters: None").color(TEXT_DIM));
}


/// Zoom, Pan X/Y and Crop X1/Y1/X2/Y2 with Reset buttons (frmInputSettings Position page).
fn position_editor(ui: &mut egui::Ui, salt: &str, p: &mut Position) {
    egui::Grid::new(("posgrid", salt)).num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, name: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, reset: f32| {
            ui.label(name);
            ui.add(egui::Slider::new(v, range).step_by(0.01).fixed_decimals(2));
            if ui.small_button("Reset").clicked() {
                *v = reset;
            }
            ui.end_row();
        };
        // Zoom and Pan use the original's integer trackbars (ozzzzzq): zoom −3990…4000 with
        // /4000 below the centre and /1000 above, pan ±2000 / 1000.
        let track = |ui: &mut egui::Ui, name: &str, v: &mut f32, range: (i32, i32),
                     to: fn(f32) -> i32, from: fn(i32) -> f32, reset: f32| {
            ui.label(name);
            let mut t = to(*v);
            ui.horizontal(|ui| {
                if ui.add(egui::Slider::new(&mut t, range.0..=range.1).show_value(false)).changed() {
                    *v = from(t);
                }
                let mut shown = *v;
                if ui.add(egui::DragValue::new(&mut shown).speed(0.01).fixed_decimals(3)).changed() {
                    *v = from(to(shown));
                }
            });
            if ui.small_button("Reset").clicked() {
                *v = reset;
            }
            ui.end_row();
        };
        track(ui, "Zoom:", &mut p.zoom, crate::model::ZOOM_SLIDER, crate::model::zoom_to_slider, crate::model::zoom_from_slider, 1.0);
        track(ui, "Pan X:", &mut p.pan_x, crate::model::PAN_SLIDER, crate::model::pan_to_slider, crate::model::pan_from_slider, 0.0);
        track(ui, "Pan Y:", &mut p.pan_y, crate::model::PAN_SLIDER, crate::model::pan_to_slider, crate::model::pan_from_slider, 0.0);
        row(ui, "Crop X1:", &mut p.crop[0], 0.0..=1.0, 0.0);
        row(ui, "Crop Y1:", &mut p.crop[1], 0.0..=1.0, 0.0);
        row(ui, "Crop X2:", &mut p.crop[2], 0.0..=1.0, 1.0);
        row(ui, "Crop Y2:", &mut p.crop[3], 0.0..=1.0, 1.0);
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("Quick:").size(11.0).color(TEXT_DIM));
        for (name, z, x, y) in [
            ("Full", 1.0, 0.0, 0.0),
            ("Top left", 0.5, -0.5, 0.5),
            ("Top right", 0.5, 0.5, 0.5),
            ("Bottom left", 0.5, -0.5, -0.5),
            ("Bottom right", 0.5, 0.5, -0.5),
            ("Picture-in-picture", 0.3, 0.6, -0.6),
        ] {
            if ui.small_button(name).clicked() {
                p.zoom = z;
                p.pan_x = x;
                p.pan_y = y;
            }
        }
    });
}

/// The 10 layers of an input plus the Layer Designer: a live preview where the selected layer
/// can be moved (drag), scaled (corner handle or wheel), cropped (edge handles) and given a
/// border, like the original's LayerToolbar + LayerDesigner (Position / Cropping / Border).
fn layers_editor(ui: &mut egui::Ui, inp: &mut Input, others: &[(usize, u32, String)], all: &[Input], t: f64) {
    let sel_id = egui::Id::new(("layer-sel", inp.id));
    let mode_id = egui::Id::new(("layer-mode", inp.id));
    let drag_id = egui::Id::new(("layer-drag", inp.id));
    let mut sel: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0).min(LAYERS - 1);
    let mut mode: u8 = ui.data(|d| d.get_temp(mode_id))
        .or_else(|| std::env::var("LIVEMIX_LAYER_MODE").ok().and_then(|value| value.parse().ok()))
        .unwrap_or(0).min(2);

    ui.label(RichText::new("Each layer draws another input on top of this one (layer 1 first, layer 10 on top).")
        .size(11.0).color(TEXT_DIM));
    egui::ScrollArea::vertical().max_height(140.0).id_salt(("layer-list", inp.id)).show(ui, |ui| {
        for k in 0..LAYERS {
            ui.horizontal(|ui| {
                if ui.selectable_label(sel == k, RichText::new(format!("Layer {}", k + 1)).strong()).clicked() {
                    sel = k;
                }
                let l = &mut inp.layers[k];
                setting_toggle(ui, &mut l.on, "On");
                let cur = l.input.and_then(|id| others.iter().find(|o| o.1 == id))
                    .map(|o| format!("{} {}", o.0, o.2)).unwrap_or_else(|| "None".into());
                egui::ComboBox::from_id_salt(("layer-in", inp.id, k)).width(210.0)
                    .selected_text(cur).show_ui(ui, |ui| {
                        if ui.selectable_label(l.input.is_none(), "None").clicked() {
                            l.input = None;
                            l.on = false;
                        }
                        for (n, oid, title) in others {
                            if ui.selectable_label(l.input == Some(*oid), format!("{n} {title}")).clicked() {
                                l.input = Some(*oid);
                                l.on = true;
                            }
                        }
                    });
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(sel_id, sel));
    ui.separator();

    let has = inp.layers[sel].input.is_some() && inp.layers[sel].on;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Layer {}", sel + 1)).strong());
        ui.selectable_value(&mut mode, 0, "Position");
        ui.selectable_value(&mut mode, 1, "Cropping");
        ui.selectable_value(&mut mode, 2, "Border");
    });
    ui.data_mut(|d| d.insert_temp(mode_id, mode));
    if !has {
        ui.label(RichText::new("Choose another input for this layer to edit it here.").size(11.0).color(TEXT_DIM));
        return;
    }

    // Live preview: the input itself with its layers, then the selection outlines.
    let width = ui.available_width().clamp(280.0, 520.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, width * 9.0 / 16.0), Sense::click_and_drag());
    let p = ui.painter_at(rect);
    render::checker(&p, rect);
    render::draw_source(&p.with_clip_rect(rect), rect, inp, 1.0, t, all);
    p.rect_stroke(rect, 0.0, Stroke::new(1.0_f32, Color32::from_gray(70)), egui::StrokeKind::Inside);

    let l = inp.layers[sel];
    let lr = render::place_rect(rect, &l.place);
    let cr = render::crop_rect(lr, &l.place);
    let accent = Color32::from_rgb(0x50, 0x9a, 0xe0);
    p.rect_stroke(cr, 0.0, Stroke::new(1.5_f32, accent), egui::StrokeKind::Outside);
    if mode == 1 {
        for (x, y) in [(cr.left(), cr.top()), (cr.right(), cr.top()), (cr.left(), cr.bottom()), (cr.right(), cr.bottom())] {
            p.circle_filled(egui::pos2(x, y), 3.5, accent);
        }
    } else {
        let h = egui::Rect::from_center_size(cr.right_bottom(), Vec2::splat(9.0));
        p.rect_filled(h, 1.0, accent);
    }

    // Which part of the layer the drag started on: 0 = move, 1..4 = crop edges, 5 = resize.
    let mut drag: u8 = ui.data(|d| d.get_temp(drag_id)).unwrap_or(0);
    if resp.drag_started() {
        drag = 0;
        if let Some(pos) = resp.interact_pointer_pos() {
            if mode == 1 {
                let x1 = lr.left() + l.place.crop[0] * lr.width();
                let x2 = lr.left() + l.place.crop[2] * lr.width();
                let y1 = lr.top() + l.place.crop[1] * lr.height();
                let y2 = lr.top() + l.place.crop[3] * lr.height();
                let near = |a: f32, b: f32| (a - b).abs() < 12.0;
                drag = if near(pos.x, x1) { 1 } else if near(pos.x, x2) { 2 }
                    else if near(pos.y, y1) { 3 } else if near(pos.y, y2) { 4 } else { 0 };
            } else if (pos - cr.right_bottom()).length() < 16.0 {
                drag = 5;
            }
        }
        ui.data_mut(|d| d.insert_temp(drag_id, drag));
    }
    if resp.dragged() {
        let pointer = resp.interact_pointer_pos();
        let delta = resp.drag_delta();
        let l = &mut inp.layers[sel];
        if mode == 1 {
            if let Some(pos) = pointer {
                let nx = ((pos.x - lr.left()) / lr.width().max(1.0)).clamp(0.0, 1.0);
                let ny = ((pos.y - lr.top()) / lr.height().max(1.0)).clamp(0.0, 1.0);
                match drag {
                    1 => l.place.crop[0] = nx.min(l.place.crop[2]),
                    2 => l.place.crop[2] = nx.max(l.place.crop[0]),
                    3 => l.place.crop[1] = ny.min(l.place.crop[3]),
                    4 => l.place.crop[3] = ny.max(l.place.crop[1]),
                    _ => {}
                }
            }
        } else if drag == 5 {
            if let Some(pos) = pointer {
                let old = render::place_rect(rect, &l.place);
                let dx = (pos.x - old.center().x).abs() * 2.0 / rect.width().max(1.0);
                let dy = (pos.y - old.center().y).abs() * 2.0 / rect.height().max(1.0);
                let zoom = dx.max(dy).clamp(0.05, 4.0);
                let size = rect.size() * zoom;
                let center = old.min + size / 2.0;
                l.place.zoom = zoom;
                l.place.pan_x = 2.0 * (center.x - rect.center().x) / rect.width().max(1.0);
                l.place.pan_y = -2.0 * (center.y - rect.center().y) / rect.height().max(1.0);
            }
        } else {
            l.place.pan_x = (l.place.pan_x + 2.0 * delta.x / rect.width().max(1.0)).clamp(-4.0, 4.0);
            l.place.pan_y = (l.place.pan_y - 2.0 * delta.y / rect.height().max(1.0)).clamp(-4.0, 4.0);
        }
    }
    if resp.hovered() && mode != 1 {
        let scroll = ui.input_mut(|i| {
            let scroll = i.smooth_scroll_delta.y;
            i.smooth_scroll_delta.y = 0.0;
            scroll
        });
        if scroll.abs() > 0.1 {
            let l = &mut inp.layers[sel];
            l.place.zoom = (l.place.zoom * (1.0 + scroll * 0.002)).clamp(0.05, 4.0);
        }
    }

    let l = &mut inp.layers[sel];
    ui.add_space(4.0);
    match mode {
        1 => {
            ui.label(RichText::new("Drag the crop handles in the preview, or set the edges.").size(10.0).color(TEXT_DIM));
            egui::Grid::new(("layer-crop", inp.id, sel)).num_columns(4).spacing([8.0, 3.0]).show(ui, |ui| {
                for (label, index) in [("Left", 0usize), ("Top", 1), ("Right", 2), ("Bottom", 3)] {
                    ui.label(label);
                    let mut percent = l.place.crop[index] * 100.0;
                    if ui.add(egui::DragValue::new(&mut percent).range(0.0..=100.0).speed(0.2).suffix(" %")).changed() {
                        l.place.crop[index] = (percent / 100.0).clamp(0.0, 1.0);
                    }
                    if index % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            if ui.button("Reset crop").clicked() {
                l.place.crop = [0.0, 0.0, 1.0, 1.0];
            }
        }
        2 => {
            setting_toggle(ui, &mut l.border, "Border");
            ui.horizontal(|ui| {
                ui.label("Colour");
                ui.color_edit_button_srgb(&mut l.border_colour);
                ui.label("Width");
                let mut percent = l.border_width * 100.0;
                if ui.add(egui::Slider::new(&mut percent, 0.0..=15.0).suffix(" %")).changed() {
                    l.border_width = percent / 100.0;
                }
            });
            ui.label(RichText::new("The border follows the layer's cropped rectangle.").size(10.0).color(TEXT_DIM));
        }
        _ => {
            let mut x = (0.5 + l.place.pan_x / 2.0) * 1920.0;
            let mut y = (0.5 - l.place.pan_y / 2.0) * 1080.0;
            let mut w = l.place.zoom * 1920.0;
            let mut h = l.place.zoom * 1080.0;
            egui::Grid::new(("layer-pos", inp.id, sel)).num_columns(4).spacing([8.0, 3.0]).show(ui, |ui| {
                ui.label("X");
                if ui.add(egui::DragValue::new(&mut x).range(-1920.0..=3840.0).speed(1.0).suffix(" px")).changed() {
                    l.place.pan_x = 2.0 * x / 1920.0 - 1.0;
                }
                ui.label("Y");
                if ui.add(egui::DragValue::new(&mut y).range(-1080.0..=2160.0).speed(1.0).suffix(" px")).changed() {
                    l.place.pan_y = 1.0 - 2.0 * y / 1080.0;
                }
                ui.end_row();
                ui.label("W");
                if ui.add(egui::DragValue::new(&mut w).range(1.0..=7680.0).speed(1.0).suffix(" px")).changed() {
                    l.place.zoom = (w / 1920.0).clamp(0.05, 4.0);
                }
                ui.label("H");
                if ui.add(egui::DragValue::new(&mut h).range(1.0..=4320.0).speed(1.0).suffix(" px")).changed() {
                    l.place.zoom = (h / 1080.0).clamp(0.05, 4.0);
                }
                ui.end_row();
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Align").size(10.0).color(TEXT_DIM));
                for (name, px, py) in [
                    ("TL", -1.0f32, 1.0f32), ("T", 0.0, 1.0), ("TR", 1.0, 1.0),
                    ("L", -1.0, 0.0), ("C", 0.0, 0.0), ("R", 1.0, 0.0),
                    ("BL", -1.0, -1.0), ("B", 0.0, -1.0), ("BR", 1.0, -1.0),
                ] {
                    if ui.small_button(name).clicked() {
                        l.place.pan_x = px * (1.0 - l.place.zoom);
                        l.place.pan_y = py * (1.0 - l.place.zoom);
                    }
                }
                if ui.small_button("Full").clicked() {
                    l.place = Position::default();
                }
            });
        }
    }
    ui.label(RichText::new("Drag to move, scroll to zoom, corner handle to scale.").size(10.0).color(TEXT_DIM));
}

// ---------------------------------------------------------------- settings window

/// The original's Settings sections, in its order (language file frmSettings.tMenu.*).
const SETTINGS_PAGES: [&str; 17] = [
    "Display",
    "Outputs / NDI / OMT / SRT",
    "Options",
    "Performance",
    "Decoders",
    "Recording",
    "External Output",
    "Audio",
    "Audio Outputs",
    "Web Controller",
    "Tally Lights",
    "Shortcuts",
    "Activators",
    "Scripting",
    "Alerts",
    "About",
    "Streaming",
];
/// Sections built in this version (indexes into SETTINGS_PAGES).
const SETTINGS_BUILT: [usize; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 13, 14, 15, 16];

fn settings_navigation(ui: &mut egui::Ui, pages: &[&str], built: &[usize], groups: &[(&str, &[usize])],
    selected: &mut usize, max_height: f32)
{
    egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
        for (group, entries) in groups {
            ui.label(RichText::new(*group).strong().color(TEXT_DIM));
            for &index in *entries {
                let label = if built.contains(&index) { RichText::new(pages[index]) }
                    else { RichText::new(pages[index]).color(TEXT_DIM.gamma_multiply(0.7)).italics() };
                ui.selectable_value(selected, index, label);
            }
            ui.add_space(7.0);
        }
    });
}

/// Settings table for the same routes exposed by the External cog.
fn output_routes(ui: &mut egui::Ui, s: &mut Switcher) {
    let choices = src_choices(s, true);
    egui::Grid::new("output-routes").spacing([9.0, 6.0]).show(ui, |ui| {
        for label in ["Output", "Source", "Overlays", "NDI", "OMT", "SRT", "Local window"] {
            ui.strong(label);
        }
        ui.end_row();
        for channel in 0..OUTPUT_COUNT {
            ui.label(format!("Output {}", channel + 1));
            let source = s.cfg.outputs[channel].source;
            let title = src_name(s, source);
            let route = &mut s.cfg.outputs[channel];
            egui::ComboBox::from_id_salt(("output-route", channel)).width(160.0)
                .selected_text(title).show_ui(ui, |ui| {
                    for (value, name) in &choices {
                        ui.selectable_value(&mut route.source, *value, name);
                    }
                });
            ui.checkbox(&mut route.overlays, "");
            if ui.checkbox(&mut s.cfg.ndi_outputs[channel], "").on_hover_text(&s.ndi_status[channel]).changed() {
                s.ndi_status[channel].clear();
            }
            if ui.checkbox(&mut s.cfg.omt_outputs[channel], "").on_hover_text(&s.omt_status[channel]).changed() {
                s.omt_status[channel].clear();
            }
            if ui.button(if s.srt_active[channel] { "On…" } else { "Off…" })
                .on_hover_text(&s.srt_status[channel]).clicked() {
                s.srt_setup = Some(channel);
            }
            if ui.button(if s.external[channel].open { "Close" } else { "Open" }).clicked() {
                s.toggle_external(channel);
            }
            ui.end_row();
        }
    });
}

fn omt_quality(ui: &mut egui::Ui, quality: &mut u8) -> bool {
    let values = [(0, "Default"), (1, "Low"), (50, "Medium"), (100, "High")];
    let before = *quality;
    let label = values.iter().find(|(value, _)| *value == before).map_or("Default", |(_, name)| *name);
    egui::ComboBox::from_id_salt("omt-quality").selected_text(format!("OMT Quality · {label}")).show_ui(ui, |ui| {
        for (value, name) in values { ui.selectable_value(quality, value, name); }
    });
    *quality != before
}

fn srt_connection_rows(ui: &mut egui::Ui, config: &mut srt::Connection) {
    ui.label("Mode");
    egui::ComboBox::from_id_salt("srt-mode").selected_text(if config.listen { "Listener" } else { "Caller" })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut config.listen, false, "Caller");
            ui.selectable_value(&mut config.listen, true, "Listener");
        });
    ui.end_row();
    if !config.listen {
        ui.label("Hostname");
        ui.text_edit_singleline(&mut config.host);
        ui.end_row();
    }
    ui.label("Port");
    ui.add(egui::DragValue::new(&mut config.port).range(1..=65535));
    ui.end_row();
    ui.label("Latency");
    ui.add(egui::DragValue::new(&mut config.latency_ms).range(20..=8000).suffix(" ms"));
    ui.end_row();
    ui.label("Passphrase");
    ui.add(egui::TextEdit::singleline(&mut config.passphrase).password(true));
    ui.end_row();
    if !config.passphrase.is_empty() {
        ui.label("Key length");
        egui::ComboBox::from_id_salt("srt-key-bytes").selected_text(format!("{} bit", u16::from(config.key_bytes) * 8))
            .show_ui(ui, |ui| for bytes in [16, 24, 32] {
                ui.selectable_value(&mut config.key_bytes, bytes, format!("{} bit", u16::from(bytes) * 8));
            });
        ui.end_row();
    }
    ui.label("Stream ID");
    ui.add_enabled(!config.listen, egui::TextEdit::singleline(&mut config.stream_id));
    ui.end_row();
}

fn srt_output_dialog(ctx: &egui::Context, s: &mut Switcher) {
    let Some(channel) = s.srt_setup else { return };
    let mut open = true;
    let mut close = false;
    egui::Window::new(format!("SRT Settings · Output {}", channel + 1))
        .open(&mut open).collapsible(false).show(ctx, |ui| {
            let cfg = &mut s.cfg.srt_outputs[channel];
            let active = &mut s.srt_active[channel];
            ui.add_enabled_ui(!*active, |ui| {
                egui::Grid::new("srt-output").spacing([12.0, 6.0]).show(ui, |ui| {
                    srt_connection_rows(ui, &mut cfg.connection);
                    ui.label("Video codec");
                    egui::ComboBox::from_id_salt("srt-codec").selected_text(if cfg.hevc { "HEVC" } else { "H.264" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg.hevc, false, "H.264");
                            ui.selectable_value(&mut cfg.hevc, true, "HEVC");
                        });
                    ui.end_row();
                    ui.label("Video bitrate");
                    ui.add(egui::DragValue::new(&mut cfg.video_kbps).range(100..=100_000).suffix(" kbps"));
                    ui.end_row();
                    ui.label("AAC bitrate");
                    ui.add(egui::DragValue::new(&mut cfg.audio_kbps).range(32..=384).suffix(" kbps"));
                    ui.end_row();
                    ui.label("Keyframe interval");
                    ui.add(egui::DragValue::new(&mut cfg.gop_seconds).range(1..=10).suffix(" s"));
                    ui.end_row();
                    ui.label("Encoder");
                    ui.checkbox(&mut cfg.hardware, stream::HARDWARE_ENCODER_LABEL);
                    ui.end_row();
                });
            });
            ui.label(RichText::new("Progressive video with stereo AAC.").size(11.0).color(TEXT_DIM));
            ui.label(&s.srt_status[channel]);
            ui.horizontal(|ui| {
                if ui.button(if *active { "Stop SRT" } else { "Start SRT" }).clicked() {
                    *active = !*active;
                    s.srt_status[channel].clear();
                }
                close = ui.button("Save and Close").clicked();
            });
        });
    if !open || close {
        s.srt_setup = None;
        s.save_settings();
    }
}

fn external_menu(ui: &mut egui::Ui, s: &mut Switcher) {
    let button = egui::Button::new("").fill(BTN_DARK).min_size(Vec2::new(38.0, 30.0));
    let menu = egui::menu::menu_custom_button(ui, button, |ui| external_menu_items(ui, s));
    gear_icon(ui.painter(), menu.response.rect.center(), 8.0, Color32::WHITE, BTN_DARK);
    menu.response.on_hover_text("Configure Output 1–4, NDI, OMT and SRT")
        .context_menu(|ui| external_menu_items(ui, s));
}

fn external_menu_items(ui: &mut egui::Ui, s: &mut Switcher) {
    let choices = src_choices(s, true);
    for channel in 0..OUTPUT_COUNT {
        let name = src_name(s, s.cfg.outputs[channel].source);
        ui.menu_button(format!("Output {} · {name}", channel + 1), |ui| {
            if ui.button(if s.external[channel].open { "Close local window" } else { "Open local window" }).clicked() {
                s.toggle_external(channel);
                ui.close_menu();
            }
            if s.cfg.decklink_outputs[channel].device.is_some() {
                if ui.button(if s.decklink_active[channel] { "Stop DeckLink output" } else { "Start DeckLink output" }).clicked() {
                    s.toggle_external_route(channel);
                    ui.close_menu();
                }
                if !s.decklink_status[channel].is_empty() {
                    ui.label(&s.decklink_status[channel]);
                }
            }
            if ui.checkbox(&mut s.cfg.outputs[channel].overlays, "Overlays").changed() {
                s.save_settings();
            }
            if ui.checkbox(&mut s.cfg.ndi_outputs[channel], "NDI").changed() {
                s.ndi_status[channel].clear();
                s.save_settings();
            }
            if !s.ndi_status[channel].is_empty() { ui.label(&s.ndi_status[channel]); }
            if ui.checkbox(&mut s.cfg.omt_outputs[channel], "OMT").changed() {
                s.omt_status[channel].clear();
                s.save_settings();
            }
            if !s.omt_status[channel].is_empty() { ui.label(&s.omt_status[channel]); }
            if omt_quality(ui, &mut s.cfg.omt_quality[channel]) { s.save_settings(); }
            if ui.button(if s.srt_active[channel] { "SRT On…" } else { "SRT Off…" }).clicked() {
                s.srt_setup = Some(channel);
                ui.close_menu();
            }
            ui.menu_button("Source", |ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (value, name) in &choices {
                        if ui.selectable_value(&mut s.cfg.outputs[channel].source, *value, name).clicked() {
                            s.save_settings();
                            ui.close_menu();
                        }
                    }
                });
            });
        });
    }
    ui.separator();
    for (page, label) in [(1, "Outputs / NDI / OMT / SRT…"), (6, "External Output Settings…")] {
        if ui.button(label).clicked() {
            s.settings_open = true;
            s.cfg.page = page;
            ui.close_menu();
        }
    }
}

fn decklink_connector(ui: &mut egui::Ui, id: impl std::hash::Hash, port: &mut u32) {
    egui::ComboBox::from_id_salt(id).selected_text(decklink::connection_name(*port)).show_ui(ui, |ui| {
        for (label, value) in decklink::CONNECTIONS {
            ui.selectable_value(port, value, label);
        }
    });
}

fn decklink_mode_picker(ui: &mut egui::Ui, id: &str, device: usize, playback: bool, mode: &mut u32) {
    let modes_key = ui.id().with((id, device));
    let modes = ui.data(|d| d.get_temp::<Vec<decklink::Mode>>(modes_key)).unwrap_or_else(|| {
        let modes = decklink::display_modes(device, playback);
        ui.data_mut(|d| d.insert_temp(modes_key, modes.clone()));
        modes
    });
    let auto = if playback { "Auto (project format)" } else { "Auto (detect signal)" };
    let selected = modes.iter().find(|m| m.mode == *mode).map(|m| m.label())
        .unwrap_or_else(|| if *mode == 0 { auto.into() } else { "Unavailable mode".into() });
    egui::ComboBox::from_id_salt(id).selected_text(selected).show_ui(ui, |ui| {
        ui.selectable_value(mode, 0, auto);
        for m in &modes {
            ui.selectable_value(mode, m.mode, m.label());
        }
    });
}

fn decklink_output_settings(ui: &mut egui::Ui, s: &mut Switcher) {
    ui.strong("DeckLink output");
    let key = ui.id().with("decklink-output-channel");
    let mut channel = ui.data(|d| d.get_temp::<usize>(key)).unwrap_or(0).min(OUTPUT_COUNT - 1);
    ui.horizontal(|ui| {
        for index in 0..OUTPUT_COUNT {
            ui.selectable_value(&mut channel, index, format!("Output {}", index + 1));
        }
    });
    ui.data_mut(|d| d.insert_temp(key, channel));
    if ui.button("Refresh DeckLink devices").clicked() {
        let devices = decklink::refresh_devices();
        for device in devices {
            let modes = ui.id().with(("decklink-output-mode", device.index));
            ui.data_mut(|d| d.remove::<Vec<decklink::Mode>>(modes));
        }
    }
    let devices = decklink::devices_cached();
    let route = &mut s.cfg.decklink_outputs[channel];
    let before = route.device;
    let name = before.map_or_else(|| "None".into(), |index| {
        devices.iter().find(|d| d.index == index).map(|d| d.name.clone()).unwrap_or_else(|| format!("Device {} unavailable", index + 1))
    });
    egui::ComboBox::from_id_salt("decklink-output-device").selected_text(name).show_ui(ui, |ui| {
        ui.selectable_value(&mut route.device, None, "None");
        for device in devices.iter().filter(|device| device.playback) {
            ui.selectable_value(&mut route.device, Some(device.index), &device.name);
        }
    });
    if before != route.device {
        route.mode = 0;
        route.port = 0;
        s.decklink_active[channel] = false;
        s.decklink_status[channel].clear();
    }
    if let Some(device) = route.device {
        ui.horizontal(|ui| {
            if cfg!(windows) {
                ui.label("Connector");
                decklink_connector(ui, "decklink-output-port", &mut route.port);
            }
            ui.label("Mode");
            decklink_mode_picker(ui, "decklink-output-mode", device, true, &mut route.mode);
        });
        if cfg!(target_os = "macos") {
            ui.label("Set the playback connector in Blackmagic Desktop Video Setup.");
        }
        if ui.button(if s.decklink_active[channel] { "Stop DeckLink output" } else { "Start DeckLink output" }).clicked() {
            s.toggle_external_route(channel);
        }
        ui.label(&s.decklink_status[channel]);
    } else if !devices.iter().any(|device| device.playback) {
        ui.label("No DeckLink playback device detected.");
        if cfg!(target_os = "macos") {
            ui.label("Install Desktop Video and use a DeckLink-enabled FFmpeg build, then refresh.");
        }
    }
}

/// MultiView 1 / 2 (frmSettings cmdMultiView1/2, lblMultiViewLayout "MultiView Layout",
/// cmdMVInputs "Customise Layout"): layout, what each box shows, and a live picture of it.
fn multiview_settings(ui: &mut egui::Ui, s: &mut Switcher) {
    let tab_id = egui::Id::new("mv-tab");
    let mut mv = ui.data(|d| d.get_temp::<usize>(tab_id)).unwrap_or(0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("MultiView:").strong());
        for k in 0..2 {
            if ui.selectable_label(mv == k, format!("MultiView {}", k + 1)).clicked() {
                mv = k;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(tab_id, mv));
    ui.horizontal(|ui| {
        ui.label("MultiView Layout:");
        let cur = s.cfg.mv_layout[mv] as usize;
        egui::ComboBox::from_id_salt(("mv-layout", mv)).selected_text(MV_LAYOUTS[cur.min(MV_LAYOUTS.len() - 1)]).show_ui(ui, |ui| {
            for (k, name) in MV_LAYOUTS.iter().enumerate() {
                if ui.selectable_label(k == cur, *name).clicked() && k != cur {
                    s.cfg.mv_layout[mv] = k as u8;
                    // A new layout starts from its own default boxes.
                    s.cfg.mv_boxes[mv].clear();
                }
            }
        });
        if ui.button("Reset boxes").on_hover_text("Back to the layout's default: Preview, Output and the inputs in list order").clicked() {
            s.cfg.mv_boxes[mv].clear();
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut s.cfg.mv_audio_program[mv], "Program audio meter");
        ui.checkbox(&mut s.cfg.mv_audio_inputs[mv], "Input audio meters");
    });
    ui.horizontal_top(|ui| {
        // Live picture of this MultiView.
        let (r, _) = ui.allocate_exact_size(Vec2::new(256.0, 144.0), Sense::hover());
        let t = ui.input(|i| i.time);
        draw_multiview(&ui.painter_at(r), r, s, t, mv);
        // Customise Layout: one choice per box.
        ui.vertical(|ui| {
            ui.label(RichText::new("Customise Layout").strong());
            let defaults = mv_layout_boxes(s.cfg.mv_layout[mv]);
            let mut choices = vec![(MV_AUTO, src_name(s, MV_AUTO))];
            choices.extend(src_choices(s, false));
            egui::ScrollArea::vertical().id_salt(("mv-boxes", mv)).max_height(144.0).show(ui, |ui| {
                egui::Grid::new(("mv-grid", mv)).num_columns(2).spacing([8.0, 3.0]).show(ui, |ui| {
                    for (k, (_, def)) in defaults.iter().enumerate() {
                        let cur = s.cfg.mv_boxes[mv].get(k).copied().unwrap_or(*def);
                        ui.label(format!("Box {}", k + 1));
                        let mut v = cur;
                        egui::ComboBox::from_id_salt(("mv-box", mv, k)).width(190.0).selected_text(src_name(s, cur)).show_ui(ui, |ui| {
                            for (c, name) in &choices {
                                ui.selectable_value(&mut v, *c, name.as_str());
                            }
                        });
                        if v != cur {
                            let b = &mut s.cfg.mv_boxes[mv];
                            while b.len() <= k {
                                let d = defaults[b.len()].1;
                                b.push(d);
                            }
                            b[k] = v;
                        }
                        ui.end_row();
                    }
                });
            });
        });
    });
    ui.label(RichText::new("Show a MultiView by choosing it as the Source of Fullscreen or External.").size(11.0).color(TEXT_DIM));
}

fn edit_tally_colour(ui: &mut egui::Ui, colour: &mut Color32) {
    let mut rgb = [colour.r(), colour.g(), colour.b()];
    if ui.color_edit_button_srgb(&mut rgb).changed() {
        *colour = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    }
}

fn settings_page(ui: &mut egui::Ui, s: &mut Switcher, graphics_adapter: &str, encoder: &str, program_path: &str) {
    let page = s.cfg.page.min(SETTINGS_PAGES.len() - 1);
    ui.label(RichText::new(SETTINGS_PAGES[page]).strong().size(15.0).color(Color32::WHITE));
    ui.add_space(6.0);
    match page {
        0 => {
            // Display
            egui::Grid::new("set-display").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                // Project format (frmSettings lblOutputFormat / lblOutputSize / lblSystem).
                ui.label("Output Size:");
                let (w, h) = s.cfg.out_dims();
                egui::ComboBox::from_id_salt("out-size").selected_text(format!("{w}x{h}")).show_ui(ui, |ui| {
                    for (k, (w, h)) in OUTPUT_SIZES.iter().enumerate() {
                        ui.selectable_value(&mut s.cfg.out_size, k as u8, format!("{w}x{h}"));
                    }
                });
                ui.end_row();
                ui.label("Master Frame Rate:");
                ui.horizontal(|ui| {
                    let cur = FRAME_RATES[(s.cfg.out_rate as usize).min(FRAME_RATES.len() - 1)].0;
                    egui::ComboBox::from_id_salt("out-rate").selected_text(cur).show_ui(ui, |ui| {
                        for (k, (name, _)) in FRAME_RATES.iter().enumerate() {
                            ui.selectable_value(&mut s.cfg.out_rate, k as u8, *name);
                        }
                    });
                    setting_toggle(ui, &mut s.cfg.interlaced, "Interlaced");
                });
                ui.end_row();
                ui.label("Output Format:");
                ui.label(RichText::new(format!("{}  —  recordings {}x{} at {:.3} frames/s", s.cfg.format_label(), w, h, s.cfg.frame_rate())).color(Color32::from_rgb(0x3c, 0xe0, 0x50)));
                ui.end_row();
                ui.label("Input Size:");
                let cur = format!("{}x{}", s.cfg.input_w as u32, s.cfg.input_h as u32);
                egui::ComboBox::from_id_salt("inputsize").selected_text(cur).show_ui(ui, |ui| {
                    for (w, h) in [(160.0, 120.0), (184.0, 138.0), (208.0, 156.0), (240.0, 180.0), (280.0, 210.0), (320.0, 240.0)] {
                        let sel = s.cfg.input_w == w && s.cfg.input_h == h;
                        if ui.selectable_label(sel, format!("{}x{}", w as u32, h as u32)).clicked() {
                            s.cfg.input_w = w;
                            s.cfg.input_h = h;
                        }
                    }
                });
                ui.end_row();
                ui.label("Preview Colour:");
                ui.horizontal(|ui| {
                    edit_tally_colour(ui, &mut s.cfg.preview_colour);
                    if ui.small_button("Reset").clicked() { s.cfg.preview_colour = PREVIEW; }
                });
                ui.end_row();
                ui.label("Output Colour:");
                ui.horizontal(|ui| {
                    edit_tally_colour(ui, &mut s.cfg.output_colour);
                    if ui.small_button("Reset").clicked() { s.cfg.output_colour = OUTPUT; }
                });
                ui.end_row();
                ui.label("Theme:");
                egui::ComboBox::from_id_salt("interface-theme")
                    .selected_text(s.cfg.theme.label())
                    .show_ui(ui, |ui| {
                        for theme in InterfaceTheme::ALL { ui.selectable_value(&mut s.cfg.theme, theme, theme.label()); }
                    });
                ui.end_row();
            });
            ui.add_space(6.0);
            let fullscreen_tab = ui.id().with("fullscreen-tab");
            let mut slot = ui.data(|data| data.get_temp::<usize>(fullscreen_tab)).unwrap_or(0);
            ui.horizontal(|ui| {
                for index in 0..2 {
                    ui.selectable_value(&mut slot, index, format!("Fullscreen {}", index + 1));
                }
            });
            ui.data_mut(|data| data.insert_temp(fullscreen_tab, slot));
            let mons = screens();
            let choices = src_choices(s, true);
            ui.push_id(("fullscreen-settings", slot), |ui| {
                    egui::Grid::new("set-fs").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label("Source");
                        let label = src_name(s, s.cfg.fs_source[slot]);
                        egui::ComboBox::from_id_salt("source").width(235.0).selected_text(label).show_ui(ui, |ui| {
                            for (value, name) in &choices {
                                ui.selectable_value(&mut s.cfg.fs_source[slot], *value, name);
                            }
                        });
                        ui.end_row();
                        ui.label("Monitor");
                        let current = fullscreen_display(s.cfg.fs_display[slot], mons.len());
                        let label = |k: usize| format!("Screen {} · {}×{}{}", k + 1, mons[k][2], mons[k][3],
                            if k == 0 { " (main)" } else { "" });
                        egui::ComboBox::from_id_salt("monitor").width(235.0).selected_text(label(current)).show_ui(ui, |ui| {
                            for index in 0..mons.len() {
                                if ui.selectable_label(index == current, label(index)).clicked() {
                                    let other = 1 - slot;
                                    if s.fullscreen[slot] && s.fullscreen[other]
                                        && fullscreen_display(s.cfg.fs_display[other], mons.len()) == index
                                    {
                                        s.notice("This monitor is already used by the other Fullscreen output");
                                    } else {
                                        s.cfg.fs_display[slot] = index;
                                    }
                                }
                            }
                        });
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        setting_toggle(ui, &mut s.cfg.fs_overlays[slot], "Overlays");
                        let action = if s.fullscreen[slot] { "Turn off" } else { "Turn on" };
                        if ui.button(action).clicked() { toggle_fullscreen(s, slot); }
                    });
            });
            ui.separator();
            multiview_settings(ui, s);
            ui.separator();
            setting_toggle(ui, &mut s.cfg.input_numbers, "Show Input Numbers");
            setting_toggle(ui, &mut s.cfg.production_clock, "Show Production Clock");
            setting_toggle(ui, &mut s.cfg.master_meter, "Show Master Audio Meter in Main Window");
            setting_toggle(ui, &mut s.cfg.input_meter, "Show Audio Meter next to Inputs");
            ui.separator();
            ui.label(RichText::new("Transition Display:").strong());
            ui.horizontal_wrapped(|ui| {
                setting_toggle(ui, &mut s.cfg.show_quickplay, "QuickPlay");
                setting_toggle(ui, &mut s.cfg.show_trans24, "Transitions 2-4");
                setting_toggle(ui, &mut s.cfg.show_ftb, "FTB");
                setting_toggle(ui, &mut s.cfg.show_tbar, "T-Bar");
                setting_toggle(ui, &mut s.cfg.show_overlays, "Overlay");
            });
        }
        2 => {
            // Options
            egui::Grid::new("set-options").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("QuickPlay Transition:");
                egui::ComboBox::from_id_salt("qp").selected_text(s.quickplay_effect.name()).show_ui(ui, |ui| {
                    for e in ALL_EFFECTS {
                        ui.selectable_value(&mut s.quickplay_effect, e, e.name());
                    }
                });
                ui.end_row();
                ui.label("Duration:");
                ui.add(egui::DragValue::new(&mut s.quickplay_ms).range(1..=9999).speed(10.0).suffix(" Milliseconds"));
                ui.end_row();
                ui.label("Fade To Black Duration:");
                ui.add(egui::DragValue::new(&mut s.ftb_ms).range(1..=9999).speed(10.0).suffix(" Milliseconds"));
                ui.end_row();
                ui.label("PlayList time per Input:");
                ui.add(egui::DragValue::new(&mut s.playlist_secs).range(1.0..=600.0).suffix(" s"));
                ui.end_row();
            });
            ui.add_space(4.0);
            setting_toggle(ui, &mut s.ftb_audio, "Fade To Black includes Audio");
            setting_toggle(ui, &mut s.pause_preview, "Automatically Pause Preview Input after Transition");
            setting_toggle(ui, &mut s.play_active, "Automatically Play Output Input with Transition");
            setting_toggle(ui, &mut s.restart_active, "Automatically Restart Output Input with Transition");
            setting_toggle(ui, &mut s.cfg.preview_click, "Activate QuickPlay by clicking Preview Window");
            setting_toggle(ui, &mut s.cfg.recording_confirm, "Display confirmation for Record, External, Stream and MultiCorder buttons");
            setting_toggle(ui, &mut s.cfg.input_close_confirm, "Display confirmation for Input Close button");
            ui.separator();
            setting_toggle(ui, &mut s.cfg.start_maximised, "Start Maximised");
            setting_toggle(ui, &mut s.cfg.start_fullscreen, "Start Fullscreen 1");
            setting_toggle(ui, &mut s.cfg.start_fullscreen2, "Start Fullscreen 2");
            setting_toggle(ui, &mut s.cfg.start_external, "Start External Output");
        }
        3 => {
            // Performance (names from frmSettings: lblPreviewSkip, chkLockBackBuffer,
            // chkDisableInputEffects; hardware encoder = recording key "HardwareAcceleration").
            egui::Grid::new("set-perf").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                ui.label("Preview Skip:");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("perf-skip").selected_text(if s.cfg.preview_skip <= 1 { "None".to_string() } else { format!("Every {} frames", s.cfg.preview_skip) }).show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.cfg.preview_skip, 1, "None");
                        for n in [2u8, 3, 4] {
                            ui.selectable_value(&mut s.cfg.preview_skip, n, format!("Every {n} frames"));
                        }
                    });
                    ui.label(RichText::new("input boxes redraw less often").size(11.0).color(TEXT_DIM));
                });
                ui.end_row();
                ui.label("Camera Auto mode up to:");
                egui::ComboBox::from_id_salt("perf-cam").selected_text(format!("{}p", s.cfg.auto_cam_max_h)).show_ui(ui, |ui| {
                    for h in [480u32, 720, 1080, 2160] {
                        ui.selectable_value(&mut s.cfg.auto_cam_max_h, h, format!("{h}p"));
                    }
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            setting_toggle(ui, &mut s.cfg.network_thumbnails, "Show NDI/OMT thumbnails when browsing sources");
            if cfg!(windows) {
                setting_toggle(ui, &mut s.cfg.low_latency_capture, "Low Latency Capture")
                    .on_hover_text("Request the Media Foundation low-latency mode for local camera inputs.");
            }
            setting_toggle(ui, &mut s.cfg.lock_refresh, "Synchronize interface with display refresh")
                .on_hover_text("The video clock always follows the Master Frame Rate.");
            setting_toggle(ui, &mut s.cfg.no_input_effects, "Disable Effects on Input Previews (input boxes skip position, crop and layers)");
            setting_toggle(ui, &mut s.cfg.hw_encoder, "Use Hardware Encoder for recordings (when available for the selected codec)");
            ui.separator();
            ui.label(format!("Graphics adapter: {graphics_adapter}"));
            #[cfg(windows)]
            {
                let adapters = ui.data_mut(|data| {
                    data.get_temp_mut_or_insert_with(egui::Id::new("video-adapters"), media::video_adapter_names).clone()
                });
                let label = match s.cfg.video_adapter.as_str() {
                    "" => "Automatic",
                    "@high" => "High performance",
                    name => name,
                };
                egui::ComboBox::from_id_salt("video-adapter").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.cfg.video_adapter, String::new(), "Automatic");
                    ui.selectable_value(&mut s.cfg.video_adapter, "@high".into(), "High performance");
                    for name in adapters {
                        ui.selectable_value(&mut s.cfg.video_adapter, name.clone(), name);
                    }
                });
                ui.label("Restart LiveMix to apply the D3D11 adapter to file decoding and hardware recording. Set the OpenGL adapter in Windows Graphics settings.");
            }
            ui.label(format!("Program capture: {program_path}"));
            ui.label(format!("Program encoder: {encoder}"));
            ui.add_space(4.0);
            ui.label(RichText::new("Lower camera modes (Input Settings > General) and a lower Output Size also save work.").size(11.0).color(TEXT_DIM));
        }
        4 => {
            ui.strong("File decoding");
            if cfg!(windows) {
                setting_toggle(ui, &mut s.cfg.ffmpeg_all_files, "Use FFmpeg for all video files");
                ui.add_enabled_ui(!s.cfg.ffmpeg_all_files, |ui| {
                    ui.label("Use FFmpeg for these extensions");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.cfg.ffmpeg_extensions)
                            .hint_text("*.mov, *.qt, *.mxf, *.mp4").desired_width(230.0));
                        if ui.button("Common formats").clicked() {
                            s.cfg.ffmpeg_extensions = "*.mov,*.qt,*.mxf,*.mp4".into();
                        }
                    });
                    ui.label("Other video files use Windows Media Foundation.");
                });
            } else {
                ui.label("Video files use FFmpeg on macOS.");
            }
            ui.separator();
            ui.label("FFmpeg deinterlacing");
            let methods = ["Off", "BWDIF", "YADIF"];
            egui::ComboBox::from_id_salt("decoder-deinterlace")
                .selected_text(methods[usize::from(s.cfg.decoder_deinterlace.min(2))])
                .show_ui(ui, |ui| {
                    for (index, method) in methods.into_iter().enumerate() {
                        ui.selectable_value(&mut s.cfg.decoder_deinterlace, index as u8, method);
                    }
                });
            ui.label(RichText::new("Applies to interlaced video files and playlists. FFmpeg and FFprobe are required.").color(TEXT_DIM));
        }
        5 => {
            ui.label("Default recording folder:");
            ui.label(RichText::new(media::storage_folder().display().to_string()).color(TEXT_DIM));
            if ui.button("Recording Setup…").clicked() {
                s.rec_setup = true;
            }
            if ui.button("MultiCorder Setup…").clicked() {
                s.mc_setup = true;
            }
        }
        1 => {
            output_routes(ui, s);
            ui.label(RichText::new("NDI, OMT and SRT use the selected video route.").size(11.0).color(TEXT_DIM));
        }
        6 => {
            decklink_output_settings(ui, s);
            ui.separator();
            ui.strong("Local External windows");
            ui.label("Drag a window to a second screen and double-click to enter fullscreen. Press Esc to leave fullscreen.");
            ui.add_space(6.0);
            egui::Grid::new("set-ext").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Window Size:");
                egui::ComboBox::from_id_salt("ext-size").selected_text(EXT_SIZES[s.cfg.ext_size.min(2) as usize].0).show_ui(ui, |ui| {
                    for (k, (name, _)) in EXT_SIZES.iter().enumerate() {
                        ui.selectable_value(&mut s.cfg.ext_size, k as u8, *name);
                    }
                });
                ui.end_row();
            });
            ui.add_space(4.0);
            setting_toggle(ui, &mut s.cfg.ext_on_top, "On Top");
            setting_toggle(ui, &mut s.cfg.ext_hide_cursor, "Hide Cursor");
            setting_toggle(ui, &mut s.cfg.start_external, "Open Output 1 at startup");
            if ui.button("Configure Output 1–4…").clicked() {
                s.cfg.page = 1;
            }
        }
        7 => {
            // Audio
            setting_toggle(ui, &mut s.ftb_audio, "Fade To Black includes Audio");
            setting_toggle(ui, &mut s.cfg.master_meter, "Show Master Audio Meter in Main Window");
            setting_toggle(ui, &mut s.cfg.input_meter, "Show Audio Meter next to Inputs");
            setting_toggle(ui, &mut s.cfg.default_auto_mix, "Automatically mix new inputs");
            ui.horizontal(|ui| {
                ui.label("Default Bus for New Inputs");
                for (index, label) in ["Master", "A", "B"].into_iter().enumerate() {
                    ui.selectable_value(&mut s.cfg.default_audio_bus, index, label);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Default Input Audio Delay");
                ui.add(egui::DragValue::new(&mut s.cfg.default_audio_delay_ms).range(0..=1000).suffix(" ms"));
            });
        }
        8 => {
            ui.label("Headphones");
            let label = if s.cfg.headphones_device.is_empty() { "System default" } else { &s.cfg.headphones_device };
            egui::ComboBox::from_id_salt("headphones-device").selected_text(label).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.cfg.headphones_device, String::new(), "System default");
                for name in audio::output_devices() {
                    ui.selectable_value(&mut s.cfg.headphones_device, name.clone(), name);
                }
            });
            ui.label("The selection applies the next time headphone monitoring starts.");
        }
        11 => {
            // Shortcuts
            ui.label("Default keyboard shortcuts");
            egui::Grid::new("set-shortcuts").num_columns(2).spacing([16.0, 3.0]).show(ui, |ui| {
                for (k, f) in [
                    ("Q W E R T Y U I O P", "Input 1–10 to Preview"),
                    ("A S D F G H J K L", "Cut input 1–9 to Output"),
                    ("Z X C V B N M", "Fade input 1–7 to Output (1 s)"),
                    (".", "Fade Preview to Output (0.5 s)"),
                    ("0", "Cut Preview to Output"),
                    ("1 2 3 4", "Fade Preview over 1, 2, 3, 4 s"),
                    ("PageUp / PageDown, [ ]", "Previous / next input to Preview"),
                    ("Ctrl+S", "Save preset"),
                    ("Esc", "Leave fullscreen"),
                ] {
                    ui.label(RichText::new(k).strong());
                    ui.label(f);
                    ui.end_row();
                }
            });
        }
        14 => {
            // Alerts
            ui.label(RichText::new("No Recording Alert").strong());
            setting_toggle(ui, &mut s.cfg.recording_alert, "Display alert on UI when NOT Recording");
            ui.add_space(4.0);
            ui.label(RichText::new("Fade To Black Alert").strong());
            setting_toggle(ui, &mut s.cfg.ftb_alert, "Display alert on UI when Fade To Black (FTB) enabled");
        }
        9 => {
            if s.remote.is_some() {
                ui.hyperlink(format!("http://{}", automation::WEB_ADDRESS));
                if ui.button("Stop web controller").clicked() { s.remote = None; }
            } else if ui.button("Start local web controller").clicked() {
                match automation::Remote::start() {
                    Ok(remote) => s.remote = Some(remote),
                    Err(error) => s.notice(&format!("Web controller: {error}")),
                }
            }
        }
        13 => {
            ui.label("Commands: Preview 2, Cut, Fade 500, Wait 1000. One command per line.");
            ui.add(egui::TextEdit::multiline(&mut s.script).code_editor().desired_rows(9));
            ui.horizontal(|ui| {
                if ui.button("Run").clicked() {
                    if let Err(error) = s.automation.start(&s.script) { s.notice(&error); }
                }
                if ui.add_enabled(s.automation.active(), egui::Button::new("Stop script")).clicked() { s.automation.stop(); }
            });
        }
        15 => {
            ui.label(RichText::new("LiveMix").strong().size(16.0));
            ui.label("A live video production switcher.");
            ui.add_space(4.0);
            ui.label(format!("Inputs: {}", s.inputs.len()));
            let backend = if cfg!(windows) { "Media Foundation" } else { "AVFoundation" };
            ui.label(format!("Video devices: {backend} · Blackmagic driver {}", if media::blackmagic_driver_installed() { "available" } else { "not found" }));
        }
        16 => {
            if ui.button("Open RTMP streaming setup").clicked() { s.stream_setup = true; }
            if ui.button("Configure NDI, OMT and SRT outputs").clicked() { s.cfg.page = 1; }
        }
        _ => {
            ui.label(RichText::new("This section is unavailable in this build.").color(TEXT_DIM));
        }
    }
}

// ---------------------------------------------------------------- dialogs

/// Streaming destinations offered in Streaming Settings (frmStreaming "Destination"); each fills
/// in its usual RTMP address.
const STREAM_DESTS: [(&str, &str); 4] = [
    ("Custom RTMP Server", ""),
    ("YouTube RTMPS", "rtmps://a.rtmps.youtube.com/live2"),
    ("Facebook", "rtmps://live-api-s.facebook.com:443/rtmp/"),
    ("Twitch", "rtmp://live.twitch.tv/app"),
];
fn file_decoder(settings: &Settings, path: &str) -> (media::Source, String) {
    let extension = std::path::Path::new(path).extension().and_then(|extension| extension.to_str());
    let ffmpeg = cfg!(target_os = "macos") || settings.ffmpeg_all_files
        || extension.is_some_and(|extension| settings.ffmpeg_extensions.split([',', ';', ' ']).any(|item| {
            extension.eq_ignore_ascii_case(item.trim().trim_start_matches(['*', '.']))
        }));
    if ffmpeg {
        let deinterlace = settings.decoder_deinterlace;
        (media::Source::FfmpegFile(path.to_owned(), deinterlace), format!("ffmpeg:{deinterlace}:{path}"))
    } else {
        (media::Source::File(path.to_owned()), path.to_owned())
    }
}

fn stream_route(s: &Switcher, destination: &stream::Destination) -> OutputRoute {
    let configured = s.cfg.outputs[destination.source];
    if let Some(source) = destination.direct {
        if matches!(source, SRC_OUTPUT | SRC_PREVIEW | SRC_MV1 | SRC_MV2)
            || source.checked_sub(1).is_some_and(|id| s.input(id).is_some())
        {
            return OutputRoute { source, overlays: matches!(source, SRC_OUTPUT | SRC_PREVIEW) };
        }
    }
    configured
}

/// The original's "New File Every" list: None, 1, 5, 10, 15, 20, 30, 60 (minutes).
fn new_file_every(ui: &mut egui::Ui, id: &str, v: &mut u32) -> bool {
    let name = |m: u32| if m == 0 { "None".to_string() } else { m.to_string() };
    let before = *v;
    egui::ComboBox::from_id_salt(id).width(80.0).selected_text(name(*v)).show_ui(ui, |ui| {
        for m in media::NEW_FILE_EVERY {
            ui.selectable_value(v, m, name(m));
        }
    });
    *v != before
}

fn setting_toggle(ui: &mut egui::Ui, value: &mut bool, label: impl Into<egui::WidgetText>) -> egui::Response {
    let label = label.into();
    let mut response = ui.horizontal(|ui| {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(32.0, 18.0), egui::Sense::click());
        let text = ui.add(egui::Label::new(label.clone()).sense(egui::Sense::click()));
        (rect, response.union(text))
    }).inner;
    if response.1.clicked() {
        *value = !*value;
        response.1.mark_changed();
    }
    response.1.widget_info(|| egui::WidgetInfo::selected(
        egui::WidgetType::Checkbox, ui.is_enabled(), *value, label.text(),
    ));
    let track = response.0;
    let style = ui.style().interact(&response.1);
    let color = if *value { ui.visuals().selection.bg_fill } else { style.bg_fill };
    ui.painter().rect(track, 9.0, color, style.bg_stroke, egui::StrokeKind::Inside);
    let progress = ui.ctx().animate_bool(response.1.id, *value);
    let x = egui::lerp(track.left() + 9.0..=track.right() - 9.0, progress);
    ui.painter().circle_filled(egui::pos2(x, track.center().y), 5.0, style.fg_stroke.color);
    response.1
}

fn recording_groups() -> &'static [(&'static str, &'static [media::RecordFormat])] {
    use media::RecordFormat::*;
    const WMV: &[media::RecordFormat] = if cfg!(windows) { &[Wmv9, Wmv] } else { &[Wmv] };
    &[
        ("AVI", &[Mjpeg, MagicYuv]), ("MPEG-2", &[Mpeg2Ts, Mpeg2Ps]),
        ("WMV", WMV), ("WMV Streaming", &[]), ("MP4", &[H264, Hevc]),
        ("FFmpeg", &[ProRes, DnxHr, DnxHrMxf]), ("VMX AVI", &[]), ("MKV", &[H264Mkv, HevcMkv]),
    ]
}

fn recording_format_tabs(ui: &mut egui::Ui, format: &mut media::RecordFormat) -> bool {
    let before = *format;
    ui.horizontal_wrapped(|ui| {
        for &(name, choices) in recording_groups() {
            let active = choices.contains(format);
            let button = egui::Button::new(name).selected(active).min_size(Vec2::new(48.0, 24.0));
            let response = ui.add_enabled(!choices.is_empty(), button);
            if response.clicked() && !active {
                *format = choices[0];
            }
            if choices.is_empty() {
                response.on_disabled_hover_text("LiveMix has no encoder for this profile.");
            }
        }
    });
    *format != before
}

fn recording_profile(ui: &mut egui::Ui, id: &str, format: &mut media::RecordFormat) -> bool {
    let before = *format;
    let choices = recording_groups().iter().find(|(_, choices)| choices.contains(format)).unwrap().1;
    let label = |format: media::RecordFormat| format!("{} · .{}", format.codec(), format.extension());
    egui::ComboBox::from_id_salt(id).width(176.0).selected_text(label(*format)).show_ui(ui, |ui| {
        for &codec in choices {
            ui.selectable_value(format, codec, label(codec));
        }
    });
    *format != before
}

/// Setup windows opened by the gear buttons: Recording Setup, Streaming Settings, MultiCorder
/// and PlayList.
fn setup_windows(ctx: &egui::Context, s: &mut Switcher) {
    let folder_line = |ui: &mut egui::Ui, folder: &str| {
        let full = if folder.trim().is_empty() { media::storage_folder().display().to_string() } else { folder.to_string() };
        // Long paths keep their end (the folder name) so the button beside them stays visible.
        let chars: Vec<char> = full.chars().collect();
        let shown = if chars.len() > 34 { format!("…{}", chars[chars.len() - 33..].iter().collect::<String>()) } else { full.clone() };
        ui.label(RichText::new(shown).color(TEXT_DIM)).on_hover_text(full);
    };
    let mut changed = false;

    if s.rec_setup {
        let mut open = true;
        egui::Window::new("Recording Setup").open(&mut open).collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).default_width(590.0).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            let tab_id = ui.id().with("recording-slot");
            let mut secondary = ui.data(|d| d.get_temp::<bool>(tab_id)).unwrap_or(false);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut secondary, false, "Recording 1");
                ui.selectable_value(&mut secondary, true, "Recording 2");
                if secondary {
                    changed |= setting_toggle(ui, &mut s.cfg.rec_second, "Enabled").changed();
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_id, secondary));
            let codec = if secondary { &mut s.cfg.rec_second_codec } else { &mut s.cfg.rec_codec };
            changed |= recording_format_tabs(ui, codec);
            ui.separator();
            let output_names: Vec<_> = s.cfg.outputs.iter().enumerate()
                .map(|(n, route)| format!("Output {} · {}", n + 1, src_name(s, route.source))).collect();
            let name = safe_name(&s.cfg.rec_name);
            egui::Grid::new("rec-setup").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                ui.label("Filename:");
                ui.horizontal(|ui| {
                    folder_line(ui, &s.cfg.rec_folder);
                    if ui.button("Browse").clicked() {
                        s.browse_for = 1;
                        s.browse_req = Some(media::PickKind::Folder);
                    }
                    if !s.cfg.rec_folder.is_empty() && ui.small_button("Default").clicked() {
                        s.cfg.rec_folder.clear();
                    }
                });
                ui.end_row();
                ui.label("Name:");
                changed |= ui.add(egui::TextEdit::singleline(&mut s.cfg.rec_name).desired_width(200.0)).changed();
                ui.end_row();
                ui.label("");
                let (source, codec, mbps) = if secondary {
                    (&mut s.cfg.rec_second_source, &mut s.cfg.rec_second_codec, &mut s.cfg.rec_second_mbps)
                } else {
                    (&mut s.cfg.rec_source, &mut s.cfg.rec_codec, &mut s.cfg.rec_mbps)
                };
                let filename = if secondary { format!("{name} - 2") } else { name.clone() };
                ui.label(RichText::new(format!("{}.{}", media::recording_name(&filename), codec.extension())).size(11.0).color(TEXT_DIM));
                ui.end_row();
                ui.label("Source:");
                let before = *source;
                egui::ComboBox::from_id_salt("rec-source").width(240.0)
                    .selected_text(&output_names[*source]).show_ui(ui, |ui| {
                        for (index, name) in output_names.iter().enumerate() {
                            ui.selectable_value(source, index, name);
                        }
                    });
                changed |= *source != before;
                ui.end_row();
                ui.label("Video codec");
                changed |= recording_profile(ui, "rec-profile", codec);
                ui.end_row();
                ui.label("Video bitrate");
                changed |= ui.add_enabled(codec.ffmpeg_profile(false).bitrate,
                    egui::DragValue::new(mbps).range(1..=100).suffix(" Mbps")).changed();
                ui.end_row();
                ui.label("Audio track");
                changed |= setting_toggle(ui, &mut s.cfg.record_audio[usize::from(secondary)],
                    format!("{} · Stereo", codec.audio_codec())).changed();
                ui.end_row();
                ui.label("New File Every:");
                ui.horizontal(|ui| {
                    changed |= new_file_every(ui, "rec-restart", &mut s.cfg.rec_split_min);
                    ui.label("Minutes");
                });
                ui.end_row();
            });
            setting_toggle(ui, &mut s.cfg.recording_confirm, "Ask before stopping (Display confirmation for Record, External, Stream and MultiCorder buttons)");
            ui.label(RichText::new("Both slots share the folder and split interval.").size(11.0).color(TEXT_DIM));
            ui.horizontal(|ui| {
                if ui.button("  OK  ").clicked() {
                    s.rec_setup = false;
                }
            });
        });
        if !open {
            s.rec_setup = false;
        }
        if !s.rec_setup {
            changed = true;
        }
    }

    if s.stream_setup {
        let mut open = true;
        let sources = src_choices(s, true);
        egui::Window::new("Streaming Settings").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            ui.horizontal(|ui| {
                for slot in 0..stream::COUNT {
                    ui.selectable_value(&mut s.stream_slot, slot, format!("Stream {}", slot + 1));
                }
            });
            let index = s.stream_slot;
            let active = &mut s.stream_active[index];
            let destination = &mut s.cfg.streams[index];
            ui.add_enabled_ui(!*active, |ui| egui::Grid::new("stream-setup").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                ui.label("Destination");
                let cur = (destination.provider as usize).min(STREAM_DESTS.len() - 1);
                egui::ComboBox::from_id_salt("stream-dest").width(240.0).selected_text(STREAM_DESTS[cur].0).show_ui(ui, |ui| {
                    for (k, (name, url)) in STREAM_DESTS.iter().enumerate() {
                        if ui.selectable_label(k == cur, *name).clicked() {
                            destination.provider = k as u8;
                            if !url.is_empty() { destination.url = url.to_string(); }
                        }
                    }
                });
                ui.end_row();
                ui.label("URL");
                ui.add(egui::TextEdit::singleline(&mut destination.url).desired_width(320.0));
                ui.end_row();
                ui.label("Stream Name or Key");
                ui.add(egui::TextEdit::singleline(&mut destination.key).password(true).desired_width(320.0));
                ui.end_row();
                ui.label("Resolution");
                let q = usize::from(destination.quality).min(stream::QUALITIES.len() - 1);
                egui::ComboBox::from_id_salt("stream-q").width(240.0).selected_text(stream::QUALITIES[q].0).show_ui(ui, |ui| {
                    for (k, (label, [w, h], mbps)) in stream::QUALITIES.iter().enumerate() {
                        ui.selectable_value(&mut destination.quality, k as u8, format!("{label} · {w}×{h} · {mbps} Mbps"));
                    }
                });
                ui.end_row();
                ui.label("Video bitrate");
                ui.horizontal(|ui| {
                    let mut bitrate = destination.bitrate.unwrap_or(stream::QUALITIES[q].2);
                    if ui.add(egui::DragValue::new(&mut bitrate).range(1..=100).suffix(" Mbps")).changed() {
                        destination.bitrate = Some(bitrate);
                    }
                    if ui.small_button("Preset").clicked() { destination.bitrate = None; }
                });
                ui.end_row();
                ui.label("Video codec");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut destination.hevc, false, "H.264");
                    ui.selectable_value(&mut destination.hevc, true, "H.265 (HEVC)");
                });
                ui.end_row();
                ui.label("Source");
                let label = destination.direct.and_then(|id| sources.iter().find(|(source, _)| *source == id))
                    .map(|(_, name)| name.clone()).unwrap_or_else(|| format!("Output {}", destination.source + 1));
                egui::ComboBox::from_id_salt("stream-source").selected_text(label).show_ui(ui, |ui| {
                    for channel in 0..OUTPUT_COUNT {
                        if ui.selectable_label(destination.direct.is_none() && destination.source == channel, format!("Output {}", channel + 1)).clicked() {
                            destination.direct = None;
                            destination.source = channel;
                        }
                    }
                    ui.separator();
                    for (source, name) in &sources {
                        ui.selectable_value(&mut destination.direct, Some(*source), name);
                    }
                });
                ui.end_row();
                ui.label("Encoder");
                setting_toggle(ui, &mut destination.hardware, stream::HARDWARE_ENCODER_LABEL);
                ui.end_row();
            }));
            ui.label(RichText::new("FFmpeg · RTMP/RTMPS · stereo AAC").size(11.0).color(TEXT_DIM));
            ui.label(&s.stream_status[index]);
            ui.horizontal(|ui| {
                if ui.add_enabled(*active || !destination.url.trim().is_empty(),
                    egui::Button::new(if *active { "Stop" } else { "Start" })).clicked() {
                    *active = !*active;
                }
                if ui.button("Save and Close").clicked() { s.stream_setup = false; }
            });
        });
        if !open {
            s.stream_setup = false;
        }
        if !s.stream_setup {
            changed = true;
        }
    }

    if s.mc_setup {
        let mut open = true;
        let t = ctx.input(|i| i.time);
        egui::Window::new("MultiCorder").open(&mut open).collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).default_width(620.0).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            let cams: Vec<(u32, String)> = s.inputs.iter().filter(|i| matches!(i.kind, Kind::Device { .. })).map(|i| (i.id, format!("{} {}", s.number_of(i.id), i.title))).collect();
            let running = s.multicorder.is_some();
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    changed |= recording_format_tabs(ui, &mut s.cfg.mc_codec);
                    egui::Grid::new("mc-setup").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
                        ui.label("Video codec");
                        changed |= recording_profile(ui, "mc-profile", &mut s.cfg.mc_codec);
                        ui.end_row();
                        ui.label("Video bitrate");
                        changed |= ui.add_enabled(s.cfg.mc_codec.ffmpeg_profile(false).bitrate,
                            egui::DragValue::new(&mut s.cfg.mc_mbps).range(1..=100).suffix(" Mbps")).changed();
                        ui.end_row();
                        ui.label("Audio track");
                        changed |= setting_toggle(ui, &mut s.cfg.record_audio[2],
                            format!("{} · Stereo", s.cfg.mc_codec.audio_codec())).changed();
                        ui.end_row();
                        ui.label("New File Every:");
                        ui.horizontal(|ui| {
                            changed |= new_file_every(ui, "mc-restart", &mut s.cfg.mc_split_min);
                            ui.label("Minutes");
                        });
                        ui.end_row();
                    });
                });
                if ui.add_sized([86.0, 78.0], egui::Button::new(if running { "Stop" } else { "Start" }).fill(BTN)).clicked() {
                    if running && s.cfg.recording_confirm {
                        s.confirm_stop = Some("MultiCorder");
                    } else {
                        s.multicorder = if running { None } else { Some(t) };
                    }
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(240.0).min_scrolled_height(146.0).show(ui, |ui| {
                egui::Grid::new("mc-sources").num_columns(2).min_col_width(240.0)
                    .striped(true).spacing([8.0, 8.0]).show(ui, |ui| {
                    ui.strong("Input");
                    ui.strong("Folder");
                    ui.end_row();
                    for (id, name) in cams {
                        let mut on = !s.mc_off.contains(&id);
                        if setting_toggle(ui, &mut on, name).changed() {
                            if on { s.mc_off.remove(&id); } else { s.mc_off.insert(id); }
                        }
                        folder_line(ui, &s.cfg.mc_folder);
                        ui.end_row();
                    }
                    for channel in 0..OUTPUT_COUNT {
                        let source = src_name(s, s.cfg.outputs[channel].source);
                        changed |= setting_toggle(ui, &mut s.cfg.mc_outputs[channel], format!("Output {} · {source}", channel + 1)).changed();
                        folder_line(ui, &s.cfg.mc_folder);
                        ui.end_row();
                    }
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Change Recording Folder").clicked() {
                    s.browse_for = 2;
                    s.browse_req = Some(media::PickKind::Folder);
                }
                if ui.button("Close").clicked() {
                    s.mc_setup = false;
                }
            });
        });
        if !open {
            s.mc_setup = false;
        }
        if !s.mc_setup {
            changed = true;
        }
    }

    if s.playlist_setup {
        let mut open = true;
        let t = ctx.input(|i| i.time);
        egui::Window::new("PlayList").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label("Ticked inputs go to Output in list order with a Cut; a video plays its own length, other inputs the Duration:");
            ui.add_space(4.0);
            let items: Vec<(u32, String)> = s.inputs.iter().map(|i| (i.id, format!("{} {}", s.number_of(i.id), i.title))).collect();
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                for (id, name) in items {
                    let mut on = !s.playlist_off.contains(&id);
                    if ui.checkbox(&mut on, name).changed() {
                        if on {
                            s.playlist_off.remove(&id);
                        } else {
                            s.playlist_off.insert(id);
                        }
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Duration:");
                ui.add(egui::DragValue::new(&mut s.playlist_secs).range(1.0..=600.0).suffix(" s"));
                ui.checkbox(&mut s.cfg.playlist_loop, "Loop");
            });
            ui.horizontal(|ui| {
                let running = s.playlist.is_some();
                if ui.button(if running { "Stop" } else { "Start" }).clicked() {
                    s.playlist = if running { None } else { Some(t - s.playlist_secs as f64) };
                }
                if ui.button("Close").clicked() {
                    s.playlist_setup = false;
                }
            });
        });
        if !open {
            s.playlist_setup = false;
        }
        if !s.playlist_setup {
            changed = true;
        }
    }
    if changed && !(s.rec_setup || s.stream_setup || s.mc_setup || s.playlist_setup) {
        s.save_settings();
    }
}

/// The New button's two windows: "save changes?" (frmCloseConfirm without closing: Save,
/// Discard changes, Cancel) and "New Preset" (frmNewPreset: Resolution and Frame Rate set to the
/// current format — frmNewPreset::d @0x259698 — then OK / Cancel).
fn new_preset_windows(ctx: &egui::Context, s: &mut Switcher) {
    match s.new_step {
        NewStep::Idle | NewStep::WaitingForSave => {}
        NewStep::SaveChanges => {
            let mut next = None;
            egui::Window::new("LiveMix").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.add_space(4.0);
                // sr::c passes the preset's name ("Untitled" when never saved) to the question.
                ui.label(RichText::new(s.preset_name.clone()).strong());
                ui.label("Do you wish to save changes to the current preset?");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("  Save  ").clicked() {
                        next = Some(1);
                    }
                    if ui.button("Discard changes").clicked() {
                        next = Some(2);
                    }
                    if ui.button("Cancel").clicked() {
                        next = Some(0);
                    }
                });
            });
            match next {
                Some(1) => {
                    // Go on only when the preset was saved (sr::c → sr::zw).
                    s.new_step = if s.save_preset() {
                        NewStep::Format
                    } else if s.browse_req == Some(media::PickKind::PresetSave) {
                        NewStep::WaitingForSave
                    } else {
                        NewStep::Idle
                    };
                }
                Some(2) => s.new_step = NewStep::Format,
                Some(_) => s.new_step = NewStep::Idle,
                None => {}
            }
        }
        NewStep::Format => {
            let mut ok = false;
            let mut cancel = false;
            egui::Window::new("New Preset").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label("Create a new blank preset with the following video format");
                ui.add_space(8.0);
                let (size, rate, il) = s.new_format;
                egui::Grid::new("new-preset").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Resolution");
                    let (w, h) = OUTPUT_SIZES[(size as usize).min(OUTPUT_SIZES.len() - 1)];
                    egui::ComboBox::from_id_salt("np-res").width(160.0).selected_text(format!("{w} x {h}")).show_ui(ui, |ui| {
                        // The original's New Preset sizes (ss::e @0x1c9128).
                        for k in NEW_PRESET_SIZES {
                            let (w, h) = OUTPUT_SIZES[k as usize];
                            if ui.selectable_label(k == size, format!("{w} x {h}")).clicked() {
                                s.new_format.0 = k;
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("Frame Rate");
                    let cur = NEW_PRESET_RATES.iter().find(|r| r.1 == rate && r.2 == il).map(|r| r.0).unwrap_or("59.94i");
                    egui::ComboBox::from_id_salt("np-rate").width(160.0).selected_text(cur).show_ui(ui, |ui| {
                        for (name, r, i) in NEW_PRESET_RATES {
                            if ui.selectable_label(r == rate && i == il, name).clicked() {
                                s.new_format.1 = r;
                                s.new_format.2 = i;
                            }
                        }
                    });
                    ui.end_row();
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("   OK   ").clicked() {
                        ok = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
            });
            if ok {
                let (size, rate, il) = s.new_format;
                s.new_step = NewStep::Idle;
                s.new_preset_with(size, rate, il);
            } else if cancel {
                s.new_step = NewStep::Idle;
            }
        }
    }
}

fn replay_windows(ctx: &egui::Context, s: &mut Switcher) {
    use replay::Action;
    if s.replay_setup_open {
        let choices: Vec<_> = std::iter::once((MV_AUTO, "Off".into()))
            .chain(std::iter::once((SRC_OUTPUT, "Program".into())))
            .chain(s.inputs.iter().filter(|input| !matches!(input.kind, Kind::Replay { .. }))
                .map(|input| (input.id + 1, input.title.clone()))).collect();
        egui::Window::new("Instant Replay Setup").open(&mut s.replay_setup_open).default_width(570.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
                ui.add_enabled_ui(!s.replay_arm, |ui| {
                    let setup = &mut s.replay_setup;
                    ui.horizontal(|ui| {
                        ui.label("Session Folder");
                        ui.add(egui::TextEdit::singleline(&mut setup.folder).desired_width(280.0));
                        if ui.button("Browse…").clicked() {
                            s.browse_for = 3;
                            s.browse_req = Some(media::PickKind::Folder);
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("New Session").clicked() { s.replay_load = Some(true); }
                        if ui.button("Open Session").clicked() { s.replay_load = Some(false); }
                        if !s.replay_session.is_empty() {
                            ui.label(RichText::new("Recording uses the open session").color(TEXT_DIM));
                        }
                    });
                    ui.separator();
                    ui.strong("Recording Format");
                    ui.horizontal_wrapped(|ui| {
                        let size_label = setup.size.map_or("Project size".into(), |[w, h]| format!("{w}×{h}"));
                        egui::ComboBox::from_id_salt("replay-size").selected_text(size_label).show_ui(ui, |ui| {
                            ui.selectable_value(&mut setup.size, None, "Project size");
                            for [w, h] in [[1280, 720], [1920, 1080], [3840, 2160]] {
                                ui.selectable_value(&mut setup.size, Some([w, h]), format!("{w}×{h}"));
                            }
                        });
                        let rate = setup.fps.map_or("Project FPS".into(), |fps| format!("{fps} fps"));
                        egui::ComboBox::from_id_salt("replay-fps").selected_text(rate).show_ui(ui, |ui| {
                            ui.selectable_value(&mut setup.fps, None, "Project FPS");
                            for fps in [25.0, 29.97, 30.0, 50.0, 59.94, 60.0] {
                                ui.selectable_value(&mut setup.fps, Some(fps), format!("{fps} fps"));
                            }
                        });
                        egui::ComboBox::from_id_salt("replay-codec").selected_text(media::RecordFormat::parse(&setup.codec).codec())
                            .show_ui(ui, |ui| {
                                for format in [media::RecordFormat::H264, media::RecordFormat::Hevc] {
                                    ui.selectable_value(&mut setup.codec, format.key().to_owned(), format.codec());
                                }
                            });
                        ui.add(egui::DragValue::new(&mut setup.mbps).range(1..=100).suffix(" Mbps"));
                    });
                    ui.separator();
                    ui.strong("Cameras");
                    egui::Grid::new("replay-cameras").num_columns(4).show(ui, |ui| {
                        for (index, source) in s.replay_angles.iter_mut().enumerate() {
                            ui.label(format!("Camera {}", index + 1));
                            let label = choices.iter().find(|(id, _)| id == source).map_or("Unavailable", |(_, label)| label.as_str());
                            egui::ComboBox::from_id_salt(("replay-angle-source", index)).width(125.0)
                                .selected_text(label).show_ui(ui, |ui| {
                                    for (value, label) in &choices { ui.selectable_value(source, *value, label); }
                                });
                            if index % 2 == 1 { ui.end_row(); }
                        }
                    });
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Playback Audio Source");
                        let label = match setup.audio { 0 => "Follow camera".into(), 1 => "Master".into(), n => format!("Camera {}", n - 1) };
                        egui::ComboBox::from_id_salt("replay-audio-source").selected_text(label).show_ui(ui, |ui| {
                            ui.selectable_value(&mut setup.audio, 0, "Follow camera");
                            ui.selectable_value(&mut setup.audio, 1, "Master");
                            for angle in 0..8 {
                                if s.replay_angles[angle] != MV_AUTO {
                                    ui.selectable_value(&mut setup.audio, angle as u8 + 2, format!("Camera {}", angle + 1));
                                }
                            }
                        });
                        ui.label("Program transition");
                        ui.add(egui::DragValue::new(&mut setup.transition_ms).range(0..=5000).suffix(" ms"));
                        ui.checkbox(&mut setup.match_speed, "Match Replay Speed");
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label("Recording Buffer");
                        ui.add(egui::DragValue::new(&mut setup.buffer_seconds).range(10..=3600).suffix(" s"));
                        ui.checkbox(&mut setup.delete_old, "Delete old unmarked video");
                    });
                    let cameras = s.replay_angles.iter().filter(|&&source| source != MV_AUTO).count();
                    let mb = cameras as u64 * u64::from(setup.mbps) * setup.buffer_seconds / 8;
                    ui.label(format!("Estimated rolling video storage: {mb} MB"));
                });
            });
            ui.label(&s.replay_status);
        });
    }

    if !s.replay_open { return; }
    egui::Window::new("Instant Replay").open(&mut s.replay_open).default_width(670.0).show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Setup…").clicked() { s.replay_setup_open = true; }
            if ui.add_enabled(!s.replay_session.is_empty() && !s.replay_read_only, egui::Button::new(if s.replay_arm { "Stop Recording" } else { "Record" })).clicked() {
                s.replay_arm = !s.replay_arm;
            }
            if ui.add_enabled(s.replay_arm && !s.replay_marking, egui::Button::new("Mark In")).clicked() {
                s.replay_command = Some(Action::MarkIn);
            }
            if ui.add_enabled(s.replay_arm && s.replay_marking, egui::Button::new("Mark Out")).clicked() {
                s.replay_command = Some(Action::MarkOut);
            }
            if s.replay_marking && ui.button("Cancel Mark").clicked() { s.replay_command = Some(Action::CancelMark); }
            for seconds in [5, 10, 20] {
                if ui.add_enabled(s.replay_arm, egui::Button::new(format!("Last {seconds}s"))).clicked() {
                    s.replay_command = Some(Action::MarkLast(seconds * 1000));
                }
            }
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for (channel, name) in ["A", "B", "AB"].into_iter().enumerate() {
                ui.selectable_value(&mut s.replay_channel, channel, name);
            }
            if ui.button("Pause / Resume").clicked() { s.replay_command = Some(Action::Pause(s.replay_channel)); }
            if ui.button("Live buffer").clicked() { s.replay_command = Some(Action::Live); }
            if ui.button("−1 frame").clicked() { s.replay_command = Some(Action::Step(-1)); }
            if ui.button("+1 frame").clicked() { s.replay_command = Some(Action::Step(1)); }
            if ui.button("Reverse").clicked() { s.replay_command = Some(Action::Reverse); }
        });
        for channel in 0..2 {
            if s.replay_channel < 2 && s.replay_channel != channel { continue; }
            ui.horizontal_wrapped(|ui| {
                ui.strong(["A", "B"][channel]);
                egui::ComboBox::from_id_salt(("replay-playback-camera", channel))
                    .selected_text(format!("Camera {}", s.replay_cameras[channel] + 1)).show_ui(ui, |ui| {
                        for angle in 0..8 { ui.selectable_value(&mut s.replay_cameras[channel], angle, format!("Camera {}", angle + 1)); }
                    });
                for speed in [25, 33, 50, 75, 100] {
                    ui.selectable_value(&mut s.replay_speed[channel], speed, format!("{speed}%"));
                }
                ui.add(egui::DragValue::new(&mut s.replay_speed[channel]).range(0..=100).suffix("%"));
            });
        }
        ui.separator();
        ui.strong("Events");
        egui::ScrollArea::both().max_height(155.0).show(ui, |ui| {
            egui::Grid::new("replay-events").striped(true).show(ui, |ui| {
                for title in ["ID", "In", "Out", "Duration", "Speed", "1", "2", "3", "4", "5", "6", "7", "8", "Description"] {
                    ui.strong(title);
                }
                ui.end_row();
                for event in &s.replay_events {
                    let row = ui.selectable_label(s.replay_event == Some(event.id), event.id.to_string());
                    if row.clicked() {
                        s.replay_event = Some(event.id);
                        for channel in 0..2 {
                            if s.replay_channel < 2 && channel != s.replay_channel { continue; }
                            s.replay_speed[channel] = event.speed;
                            if !event.angles[s.replay_cameras[channel]] {
                                s.replay_cameras[channel] = event.angles.iter().position(|&enabled| enabled).unwrap_or(0);
                            }
                        }
                    }
                    if row.double_clicked() { s.replay_command = Some(Action::Play { event: event.id, program: false }); }
                    for value in [event.start, event.end, event.end.saturating_sub(event.start)] {
                        ui.label(format!("{:.3}", value as f64 / 1000.0));
                    }
                    ui.label(format!("{}%", event.speed));
                    for &angle in &event.angles { ui.label(if angle { "●" } else { "·" }); }
                    ui.label(&event.description);
                    ui.end_row();
                }
            });
        });
        if let Some(event) = s.replay_events.iter_mut().find(|event| Some(event.id) == s.replay_event) {
            let channel = s.replay_channel.min(1);
            ui.horizontal_wrapped(|ui| {
                let title = ui.add_enabled(!s.replay_read_only, egui::TextEdit::singleline(&mut event.description).desired_width(170.0));
                let speed = ui.add_enabled(!s.replay_read_only, egui::DragValue::new(&mut event.speed).range(1..=100).prefix("Event ").suffix("%"));
                if title.changed() || speed.changed() { s.replay_command = Some(Action::SaveEvent(event.clone())); }
                if ui.button("Play").clicked() { s.replay_command = Some(Action::Play { event: event.id, program: false }); }
                if ui.button("Play to Program").clicked() { s.replay_command = Some(Action::Play { event: event.id, program: true }); }
                if ui.button("Export").clicked() {
                    s.replay_command = Some(Action::Export { event: event.id, angle: s.replay_cameras[channel] });
                }
                if ui.add_enabled(!s.replay_read_only, egui::Button::new("Delete")).clicked() { s.replay_command = Some(Action::DeleteEvent(event.id)); }
            });
        }
        ui.label(&s.replay_status);
    });
}

fn dialogs(ctx: &egui::Context, s: &mut Switcher, graphics_adapter: &str, encoder: &str, program_path: &str, mixer: &mut MixerControls) {
    replay_windows(ctx, s);
    if let Some(report) = &s.import_report {
        let mut open = true;
        let mut close = false;
        egui::Window::new("Project Import Report").open(&mut open).default_width(560.0).show(ctx, |ui| {
            ui.label(report.path.display().to_string());
            ui.label(format!("{} input slots · {} unsupported types · {} missing asset references",
                report.inputs, report.unsupported.len(), report.missing.len()));
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                if !report.unsupported.is_empty() {
                    ui.heading("Unsupported inputs replaced by blanks");
                    for input in &report.unsupported { ui.label(input); }
                }
                if !report.missing.is_empty() {
                    ui.heading("Missing assets");
                    for (input, asset) in &report.missing {
                        ui.label(RichText::new(input).strong());
                        ui.label(asset);
                    }
                }
            });
            close = ui.button("Done").clicked();
        });
        if !open || close { s.import_report = None; }
    }
    srt_output_dialog(ctx, s);
    new_preset_windows(ctx, s);
    setup_windows(ctx, s);
    // RecordingConfirm (config default True): "Stop Recording?" Yes / No (ConfirmRecording @0x250d10).
    if let Some(kind) = s.confirm_stop {
        egui::Window::new("Confirm")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("Stop {kind}?")).size(15.0).color(Color32::WHITE));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([80.0, 26.0], egui::Button::new("Yes")).clicked() {
                        if kind == "Recording" {
                            s.rec = None;
                        } else {
                            s.multicorder = None;
                        }
                        s.confirm_stop = None;
                    }
                    if ui.add_sized([80.0, 26.0], egui::Button::new("No")).clicked() {
                        s.confirm_stop = None;
                    }
                });
            });
    }
    if s.add_dialog.open {
        let mut open = true;
        let mut done = false;
        egui::Window::new("Input Select").open(&mut open).collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).default_width(650.0).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            let body_height = (ctx.screen_rect().height() - 145.0).clamp(240.0, 510.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    // The original's input list (frmInput.tMenu), then this app's extras.
                    ui.set_width(170.0);
                    egui::ScrollArea::vertical().id_salt("input-select-types").max_height(body_height).show(ui, |ui| {
                        for (n, k) in ADD_KINDS.into_iter().enumerate() {
                            if n == ORIGINAL_ADD_KINDS {
                                ui.separator();
                                ui.label(RichText::new("Extras").size(10.0).color(TEXT_DIM));
                            }
                            let label = if k.available() {
                                RichText::new(k.label())
                            } else {
                                RichText::new(k.label()).color(TEXT_DIM.gamma_multiply(0.7)).italics()
                            };
                            let active = s.add_dialog.kind == k;
                            if ui.add_sized([156.0, 19.0], egui::Button::new(label)
                                .fill(if active { OUTPUT } else { BTN_DARK })).clicked()
                            {
                                s.add_dialog.device = 0;
                                s.add_dialog.kind = k;
                                if k == AddKind::Desktop { s.add_dialog.ndi_scan = true; }
                                s.dialog_error.clear();
                            }
                        }
                    });
                });
                ui.add_space(10.0);
                ui.vertical(|ui| {
                    ui.set_width(440.0);
                    ui.set_min_height(body_height);
                    ui.label(RichText::new(s.add_dialog.kind.label()).strong());
                    ui.label(RichText::new(s.add_dialog.kind.help()).color(TEXT_DIM).size(11.0));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(&mut s.add_dialog.name);
                    });
                    match s.add_dialog.kind {
                        AddKind::Title => {
                            ui.horizontal(|ui| {
                                ui.label("GT Title");
                                ui.text_edit_singleline(&mut s.add_dialog.path);
                                if ui.button("Browse…").clicked() {
                                    s.browse_req = Some(media::PickKind::Title);
                                }
                            });
                        }
                        AddKind::Colour => {
                            // The original's Colour tab: Colour / Transparent / Colour Bars.
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut s.add_dialog.colour_mode, 0, "Colour");
                                ui.radio_value(&mut s.add_dialog.colour_mode, 1, "Transparent");
                                ui.radio_value(&mut s.add_dialog.colour_mode, 2, "Colour Bars");
                            });
                            if s.add_dialog.colour_mode == 0 {
                                ui.horizontal(|ui| {
                                    ui.label("Colour");
                                    ui.color_edit_button_srgba(&mut s.add_dialog.colour);
                                });
                            }
                        }
                        AddKind::List => {
                            ui.horizontal(|ui| {
                                if ui.button("Add…").clicked() {
                                    s.browse_req = Some(media::PickKind::VideoMany);
                                }
                                if ui.button("Shuffle").clicked() {
                                    // simple shuffle (the original's "Shuffle" button)
                                    let n = s.add_dialog.list.len();
                                    let mut seed = (ui.input(|i| i.time) * 1000.0) as u64 | 1;
                                    for i in (1..n).rev() {
                                        seed ^= seed << 13;
                                        seed ^= seed >> 7;
                                        seed ^= seed << 17;
                                        s.add_dialog.list.swap(i, (seed % (i as u64 + 1)) as usize);
                                    }
                                }
                                if ui.button("Clear").clicked() {
                                    s.add_dialog.list.clear();
                                }
                            });
                            let mut remove = None;
                            egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                                for (k, p) in s.add_dialog.list.iter().enumerate() {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("Remove").clicked() {
                                            remove = Some(k);
                                        }
                                        let name = std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                                        ui.label(RichText::new(format!("{}. {}", k + 1, name)).size(11.0));
                                    });
                                }
                            });
                            if let Some(k) = remove {
                                s.add_dialog.list.remove(k);
                            }
                            if s.add_dialog.list.is_empty() {
                                ui.label(RichText::new("No videos yet — press Add…").size(11.0).color(TEXT_DIM));
                            }
                        }
                        AddKind::ImageSequence | AddKind::Photos => {
                            ui.horizontal(|ui| {
                                ui.label("Folder");
                                ui.add(egui::TextEdit::singleline(&mut s.add_dialog.folder).desired_width(190.0));
                                if ui.button("Browse…").clicked() {
                                    s.browse_req = Some(media::PickKind::Folder);
                                }
                            });
                        }
                        AddKind::Desktop => {
                            let d = &mut s.add_dialog;
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("ndi-source").width(255.0)
                                    .selected_text(d.ndi_source.as_deref().map(|link| &link[4..]).unwrap_or("Desktop Capture (main screen)"))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut d.ndi_source, None, "Desktop Capture (main screen)");
                                        for source in &d.ndi_sources {
                                            ui.selectable_value(&mut d.ndi_source, Some(format!("ndi:{source}")), format!("NDI · {source}"));
                                        }
                                        for source in &d.omt_sources {
                                            ui.selectable_value(&mut d.ndi_source, Some(format!("omt:{source}")), format!("OMT · {source}"));
                                        }
                                    });
                                if ui.button("Refresh").clicked() { d.ndi_scan = true; }
                            });
                            ui.label(RichText::new(&d.ndi_status).size(11.0).color(TEXT_DIM));
                            ui.label(RichText::new(&d.omt_status).size(11.0).color(TEXT_DIM));
                            if s.cfg.network_thumbnails {
                                network_thumbnail_grid(ui, d);
                            }
                        }
                        AddKind::StreamSrt => {
                            egui::Grid::new("srt-input-connection").spacing([12.0, 6.0]).show(ui, |ui| {
                                srt_connection_rows(ui, &mut s.add_dialog.srt);
                            });
                        }
                        AddKind::TestVideo => {
                            ui.horizontal(|ui| {
                                ui.label("Length (s)");
                                ui.add(egui::DragValue::new(&mut s.add_dialog.length).range(1.0..=3600.0));
                            });
                        }
                        AddKind::VideoDelay => {
                            // frmInput tVideoDelay: Source, buffer Seconds, Resolution.
                            let choices: Vec<(u32, usize, String)> = s.inputs.iter().enumerate()
                                .map(|(n, i)| (i.id, n + 1, i.title.clone())).collect();
                            let d = &mut s.add_dialog;
                            ui.horizontal(|ui| {
                                ui.label("Source");
                                let current = d.delay_source.and_then(|id| choices.iter().find(|c| c.0 == id))
                                    .map(|c| format!("{} {}", c.1, c.2)).unwrap_or_else(|| "Choose an input".into());
                                egui::ComboBox::from_id_salt("delay-source").width(240.0).selected_text(current)
                                    .show_ui(ui, |ui| {
                                        for (id, number, title) in &choices {
                                            ui.selectable_value(&mut d.delay_source, Some(*id),
                                                format!("{number} {title}"));
                                        }
                                    });
                            });
                            if choices.is_empty() {
                                ui.label(RichText::new("Add the input the delay should copy first.").size(11.0)
                                    .color(Color32::from_rgb(0xff, 0xc8, 0x60)));
                            }
                            ui.horizontal(|ui| {
                                ui.label("Seconds");
                                ui.add(egui::Slider::new(&mut d.delay_seconds, 1.0..=120.0).suffix(" s"));
                            });
                            let [w, h] = DELAY_RESOLUTIONS[d.delay_resolution.min(DELAY_RESOLUTIONS.len() - 1)];
                            ui.horizontal(|ui| {
                                ui.label("Resolution");
                                egui::ComboBox::from_id_salt("delay-resolution").width(140.0)
                                    .selected_text(format!("{w}x{h}")).show_ui(ui, |ui| {
                                        for (index, [w, h]) in DELAY_RESOLUTIONS.into_iter().enumerate() {
                                            ui.selectable_value(&mut d.delay_resolution, index, format!("{w}x{h}"));
                                        }
                                    });
                            });
                            ui.label(RichText::new("The source picture is stored in memory at the chosen resolution, so longer delays and higher resolutions use more RAM.")
                                .size(11.0).color(TEXT_DIM));
                            if let Some(source) = d.delay_source {
                                ui.label(RichText::new(format!("Buffer: {:.1} s at {w}x{h} (about {:.0} MB).",
                                    d.delay_seconds, f64::from(w * h * 4) * f64::from(d.delay_seconds) / 1_048_576.0))
                                    .size(11.0).color(TEXT_DIM));
                                let _ = source;
                            }
                        }
                        AddKind::Telestrator => {
                            if s.remote.is_some() {
                                ui.label(RichText::new(format!("Web Controller: enabled at http://{}/telestrator/",
                                    automation::WEB_ADDRESS)).size(11.0).color(Color32::from_rgb(0x7c, 0xe0, 0x8a)));
                            } else {
                                ui.label(RichText::new("Please enable Telestrator under Settings - Web Controller first")
                                    .size(11.0).color(Color32::from_rgb(0xff, 0x9a, 0x9a)));
                            }
                            ui.label(" Click OK to add a Telestrator input.");
                            ui.label(RichText::new("The Telestrator is a transparent drawing layer. Add it as an Overlay over Program; draw from the Web Controller page or from Input Settings.")
                                .size(11.0).color(TEXT_DIM));
                        }
                        AddKind::Camera | AddKind::Blackmagic | AddKind::AudioInput => {
                            let list = s.dialog_devices();
                            if s.add_dialog.kind == AddKind::Blackmagic {
                                let ok = media::blackmagic_driver_installed();
                                ui.label(
                                    RichText::new(if ok { "Blackmagic Desktop Video driver: installed" } else { "Blackmagic Desktop Video driver: not found (needed for DeckLink / Intensity / UltraStudio)" })
                                        .size(11.0)
                                        .color(if ok { Color32::from_rgb(0x7c, 0xe0, 0x8a) } else { Color32::from_rgb(0xff, 0xc8, 0x60) }),
                                );
                                if cfg!(target_os = "macos") {
                                    ui.label("DeckLink requires a DeckLink-enabled FFmpeg build (LIVEMIX_DECKLINK_FFMPEG).");
                                }
                            }
                            ui.horizontal(|ui| {
                                ui.label("Device");
                                let previous = s.add_dialog.device;
                                let sel = list.get(s.add_dialog.device).map(|d| d.name.clone()).unwrap_or_else(|| "— none found —".into());
                                egui::ComboBox::from_id_salt("device").width(200.0).selected_text(sel).show_ui(ui, |ui| {
                                    for (k, d) in list.iter().enumerate() {
                                        let label = if decklink::device_index(&d.link).is_some() { format!("{}  (DeckLink)", d.name) }
                                            else { d.name.clone() };
                                        ui.selectable_value(&mut s.add_dialog.device, k, label);
                                    }
                                });
                                if previous != s.add_dialog.device {
                                    s.add_dialog.decklink_mode = 0;
                                    s.add_dialog.decklink_port = 0;
                                }
                                if ui.button("Refresh").clicked() {
                                    s.refresh_devices = true;
                                }
                            });
                            if let Some(device) = list.get(s.add_dialog.device).and_then(|d| decklink::device_index(&d.link)) {
                                ui.horizontal(|ui| {
                                    ui.label("Connector");
                                    decklink_connector(ui, "add-decklink-connector", &mut s.add_dialog.decklink_port);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Video format");
                                    decklink_mode_picker(ui, "add-decklink-mode", device, false, &mut s.add_dialog.decklink_mode);
                                });
                            }
                            if s.add_dialog.devices_loaded {
                                ui.label(RichText::new(format!("{} device(s) found on this computer", list.len())).size(11.0).color(TEXT_DIM));
                            }
                        }
                        AddKind::VideoFile | AddKind::ImageFile | AddKind::AudioFile => {
                            ui.horizontal(|ui| {
                                ui.label("File");
                                ui.add(egui::TextEdit::singleline(&mut s.add_dialog.path).desired_width(190.0));
                                if ui.button("Browse…").clicked() {
                                    s.browse_req = Some(match s.add_dialog.kind {
                                        AddKind::VideoFile => media::PickKind::Video,
                                        AddKind::AudioFile => media::PickKind::Audio,
                                        _ => media::PickKind::Image,
                                    });
                                }
                            });
                        }
                        _ => {}
                    }
                    if let Some(index) = s.add_dialog.kind.history_group() {
                        if !s.cfg.recent_media[index].is_empty() {
                            egui::ComboBox::from_id_salt(("recent-media", index)).width(270.0)
                                .selected_text("Recent media").show_ui(ui, |ui| {
                                    for path in &s.cfg.recent_media[index] {
                                        let name = std::path::Path::new(path).file_name().unwrap_or_default().to_string_lossy();
                                        if ui.selectable_label(false, name).on_hover_text(path).clicked() {
                                            if index == 2 { s.add_dialog.folder = path.clone(); }
                                            else { s.add_dialog.path = path.clone(); }
                                        }
                                    }
                                });
                        }
                    }
                    if !s.dialog_error.is_empty() {
                        ui.label(RichText::new(&s.dialog_error).size(11.0).color(Color32::from_rgb(0xff, 0x9a, 0x9a)));
                    }
                });
            });
            ui.separator();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Cancel").clicked() {
                    s.add_dialog.open = false;
                    s.dialog_error.clear();
                }
                if ui.add_enabled(s.add_dialog.kind.available(), egui::Button::new("  OK  ")).clicked() {
                    done = true;
                }
            });
        });
        if done {
            match s.add_from_dialog() {
                Ok(()) => {
                    if s.add_dialog.kind == AddKind::Replay {
                        s.replay_open = true;
                    }
                    if let Some(index) = s.add_dialog.kind.history_group() {
                        let path = if index == 2 { &s.add_dialog.folder } else { &s.add_dialog.path };
                        let path = path.trim();
                        if !path.is_empty() {
                            let history = &mut s.cfg.recent_media[index];
                            history.retain(|entry| entry != path);
                            history.insert(0, path.to_owned());
                            history.truncate(16);
                            s.save_settings();
                        }
                    }
                    s.add_dialog.open = false;
                    s.dialog_error.clear();
                    s.add_dialog.path.clear();
                }
                Err(e) => s.dialog_error = e,
            }
        }
        if !open {
            s.add_dialog.open = false;
            s.dialog_error.clear();
        }
    }

    // Input Settings: the original's sections (frmInputSettings.tMenu.*). Opened by the input's
    // Set button, its right-click menu, or a double-click on its title or picture.
    if let Some(id) = s.settings_input {
        let mut open = true;
        let mut close = false;
        // Other inputs, for the Layers section (number, id, title).
        let others: Vec<(usize, u32, String)> =
            s.inputs.iter().enumerate().filter(|(_, i)| i.id != id).map(|(n, i)| (n + 1, i.id, i.title.clone())).collect();
        let number = s.number_of(id);
        let mut page = s.cfg.input_page.min(INPUT_PAGES.len() - 1);
        // Snapshot of the input list for the Layers preview (draw_source resolves the layers).
        let all: Vec<Input> = s.inputs.clone();
        let t = ctx.input(|i| i.time);
        let page_height = (ctx.screen_rect().height() - 150.0).clamp(160.0, 320.0);
        let clip = s.position_clipboard;
        let output = s.output;
        let project = { let (w, h) = s.cfg.out_dims(); ([w, h], s.cfg.frame_rate()) };
        let web = s.remote.is_some();
        let mut copy_pos = None;
        if let Some(inp) = s.input_mut(id) {
            egui::Window::new(format!("Input Settings — {} {}", number, inp.title))
                .id(egui::Id::new("input-settings"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size([760.0, page_height + 58.0])
                .min_width(540.0)
                .min_height(235.0)
                .max_height((ctx.screen_rect().height() - 30.0).max(340.0))
                .default_pos(ctx.screen_rect().center() - Vec2::new(380.0, (page_height + 58.0) / 2.0))
                .show(ctx, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::splat(8.0);
                    ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), page_height + 24.0),
                        egui::Layout::left_to_right(egui::Align::Min), |ui| {
                        ui.vertical(|ui| {
                            ui.set_width(150.0);
                            settings_navigation(ui, &INPUT_PAGES, &INPUT_PAGES_BUILT, &[
                                ("Source", &[0, 12]), ("Picture", &[1, 2, 3, 5, 6, 7]),
                                ("Sound", &[4]), ("Automation", &[8, 9, 10, 11]),
                            ], &mut page, page_height);
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            let width = ui.available_width().clamp(340.0, 900.0);
                            ui.set_width(width);
                            ui.label(RichText::new(INPUT_PAGES[page]).strong().size(15.0).color(Color32::WHITE));
                            ui.add_space(6.0);
                            let height = page_height;
                            egui::ScrollArea::vertical().id_salt(("input-properties", id, page))
                                .auto_shrink([false, false]).max_height(height).show(ui, |ui| match page {
                                0 => input_general(ui, inp, &others, &mut page, project),
                                1 => input_colour_adjust(ui, inp),
                                2 => input_colour_key(ui, inp),
                                3 => input_colour_correction(ui, inp),
                                4 => {
                                    if inp.kind.has_audio() && ui.button("Audio Settings…").clicked() {
                                        mixer.settings = Some(MixerSettings::Input(inp.id));
                                    }
                                    input_audio_general(ui, inp);
                                }
                                5 => input_effects(ui, inp, &mut page),
                                6 => {
                                    position_editor(ui, "pos", &mut inp.place);
                                    ui.horizontal(|ui| {
                                        if ui.button("Copy").on_hover_text("Copy this position").clicked() {
                                            copy_pos = Some(inp.place);
                                        }
                                        if ui.add_enabled(clip.is_some(), egui::Button::new("Paste")).clicked() {
                                            if let Some(p) = clip {
                                                inp.place = p;
                                            }
                                        }
                                        if ui.button("Reset All").clicked() {
                                            inp.place = Position::default();
                                        }
                                    });
                                }
                                7 => layers_editor(ui, inp, &others, &all, t),
                                8 => {
                                    ui.label("Run when this input goes to Program");
                                    ui.add(egui::TextEdit::multiline(&mut inp.on_program).code_editor().desired_rows(6));
                                    ui.label("Commands: Preview 2, Cut, Fade 500, Wait 1000.");
                                }
                                9 => input_tally(ui, inp, number),
                                10 => input_ptz(ui, inp),
                                11 => input_advanced(ui, inp, number),
                                12 => input_telestrator(ui, inp, &all, t, output, web),
                                _ => {
                                    ui.label(RichText::new("This section of the original is not available in this version yet.").color(TEXT_DIM));
                                }
                            });
                        });
                    });
                    ui.separator();
                    ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 24.0),
                        egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    });
                });
        } else {
            open = false;
        }
        s.cfg.input_page = page;
        if let Some(p) = copy_pos {
            s.position_clipboard = Some(p);
        }
        if !open || close {
            s.settings_input = None;
        }
    }

    if s.settings_open {
        let mut open = true;
        let mut close = false;
        // Keep the window inside the display: the page area shrinks first.
        let screen = ctx.screen_rect();
        let page_height = (screen.height() - 150.0).clamp(160.0, 320.0);
        let page_width = (screen.width() - 250.0).clamp(340.0, 500.0);
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([page_width + 210.0, page_height + 58.0])
            .min_width(520.0)
            .max_height((screen.height() - 40.0).max(300.0))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(8.0);
                ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), page_height),
                    egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    // The original's sections (frmSettings.tMenu.*).
                    ui.vertical(|ui| {
                        ui.set_width(150.0);
                        settings_navigation(ui, &SETTINGS_PAGES, &SETTINGS_BUILT, &[
                            ("Project", &[0, 2, 3, 4]), ("Outputs", &[1, 6, 5, 16]),
                            ("Audio", &[7, 8]), ("Control", &[11, 13, 9, 10, 12]),
                            ("Application", &[14, 15]),
                        ], &mut s.cfg.page, page_height);
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_width(page_width);
                        egui::ScrollArea::vertical().id_salt("settings-page")
                            .auto_shrink([false, false]).max_height(page_height).show(ui, |ui| {
                            settings_page(ui, s, graphics_adapter, encoder, program_path);
                        });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("  OK  ").clicked() {
                        close = true;
                    }
                    ui.label(RichText::new("Settings are saved in Documents\\LiveMixStorage").size(11.0).color(TEXT_DIM));
                });
            });
        if !open || close {
            s.settings_open = false;
            s.save_settings();
        }
    }

    if s.overlay_dialog {
        let mut open = true;
        egui::Window::new("Overlay Settings").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            egui::Grid::new("ov").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label(RichText::new("Number").strong());
                ui.label(RichText::new("Effect").strong());
                ui.label(RichText::new("Effect Duration").strong());
                ui.label(RichText::new("Assigned Input").strong());
                ui.end_row();
                for c in 0..OVERLAY_CHANNELS {
                    ui.label(format!("{}", c + 1));
                    egui::ComboBox::from_id_salt(("ovfx", c)).selected_text(s.overlays[c].effect.name()).show_ui(ui, |ui| {
                        for e in OVERLAY_EFFECTS {
                            ui.selectable_value(&mut s.overlays[c].effect, e, e.name());
                        }
                    });
                    ui.add(egui::DragValue::new(&mut s.overlays[c].ms).range(1..=9999).speed(10.0).suffix(" ms"));
                    let name = s.overlays[c].assigned.and_then(|source| s.input(source.input))
                        .map_or("—", |i| i.title.as_str());
                    ui.label(name);
                    ui.end_row();
                }
            });
        });
        if !open {
            s.overlay_dialog = false;
        }
    }
}

// ---------------------------------------------------------------- widgets

/// The original's gear ("cog") setup button that sits left of Record, External, Stream,
/// MultiCorder and PlayList (frmmain cmdRecSetup, cmdExternal, cmdStreamingSetup,
/// cmdMultiCorderSetup, cmdPlayListSetup).
fn gear_btn(ui: &mut egui::Ui, open: bool, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(38.0, 30.0), Sense::click());
    let p = ui.painter();
    let fill = if open { HOVER } else if resp.hovered() { BTN } else { BTN_DARK };
    p.rect_filled(r, 0.0, fill);
    gear_icon(p, r.center(), 8.0, Color32::WHITE, fill);
    resp.on_hover_text(tip)
}

/// A cog wheel: eight teeth round a ring with a hole.
fn gear_icon(p: &egui::Painter, c: egui::Pos2, radius: f32, col: Color32, bg: Color32) {
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::TAU / 8.0;
        let dir = Vec2::new(a.cos(), a.sin());
        let side = Vec2::new(-dir.y, dir.x) * radius * 0.28;
        let (inner, outer) = (c + dir * radius * 0.6, c + dir * radius * 1.12);
        p.add(egui::Shape::convex_polygon(vec![inner - side, outer - side * 0.8, outer + side * 0.8, inner + side], col, Stroke::NONE));
    }
    p.circle_filled(c, radius * 0.82, col);
    p.circle_filled(c, radius * 0.36, bg);
}

fn big_btn(ui: &mut egui::Ui, text: &str, active: bool, active_col: Option<Color32>) -> egui::Response {
    let fill = if active { active_col.unwrap_or(HOVER) } else { BTN_DARK };
    let b = egui::Button::new(RichText::new(text).strong().size(13.0).color(Color32::WHITE)).fill(fill).stroke(Stroke::NONE).min_size(Vec2::new(116.0, 30.0));
    ui.add(b)
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    #[test]
    fn default_keyboard_has_the_templates_36_shortcuts() {
        assert_eq!(default_keyboard().len(), 36);
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
