// LiveMix — a live video production switcher (own rebuild).
// Rules and numbers come from MECHANICS.md / mechanics.json.

mod model;
mod render;
mod media;
mod program;
mod performance;
mod gt;

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
            apply_style(&cc.egui_ctx);
            let mut app = App::new(screenshot);
            if let Some(gl) = &cc.gl {
                app.graphics_adapter = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
                app.performance.set_adapter(&app.graphics_adapter);
            }
            Ok(Box::new(app))
        }),
    )
}

fn apply_style(ctx: &egui::Context) {
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
    ctx.set_visuals(v);
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = Vec2::new(4.0, 4.0);
    style.spacing.button_padding = Vec2::new(6.0, 3.0);
    ctx.set_style(style);
}

/// A running real source and its picture.
struct Feed {
    shared: Option<std::sync::Arc<media::Shared>>,
    tex: Option<egui::TextureHandle>,
    seen: u64,
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

struct App {
    s: Switcher,
    frame: u64,
    screenshot: Option<String>,
    shot_pending: bool,
    shot_recording: Option<f64>,
    shot_wave: bool,
    next_record_frame: f64,
    record_format: ([u32; 2], f64),
    program: Arc<Mutex<ProgramCapture>>,
    graphics_adapter: String,
    performance: performance::Monitor,
    last_time: f64,
    fps: f32,
    media: std::collections::HashMap<u32, Feed>,
    /// Recording slots 1 and 2 start and stop with Record.
    recordings: [Option<ComposedRecording>; 2],
    multi_outputs: [Option<ComposedRecording>; OUTPUT_COUNT],
    multi_output_failed: [bool; OUTPUT_COUNT],
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
}

#[derive(Default)]
struct ProgramCapture {
    target: Option<program::Target>,
    error: Option<String>,
}

struct ComposedRecording {
    recorder: Arc<media::Recorder>,
    channel: usize,
    started: f64,
}

impl ComposedRecording {
    fn new(folder: &str, name: &str, channel: usize, started: f64, opts: media::RecOpts) -> Self {
        let path = rec_dir(folder).join(format!("{}.{}", media::recording_name(name), opts.format.extension()));
        let recorder = Arc::new(media::Recorder::start_opts(path.to_string_lossy().into_owned(), opts));
        Self { recorder, channel, started }
    }
}

/// Scope picture of a picture for the Waveform Monitor: SCOPE_RGB — red, green and blue levels
/// of every column laid over each other; SCOPE_Y — brightness (BT.709); SCOPE_PARADE — red,
/// green and blue side by side; SCOPE_VECTOR — colour (Cb across, Cr up) round the centre of a
/// square picture, twice as large with "Vectorscope 2X Zoom". Levels run 0 % at the bottom to
/// 100 % at the top.
#[allow(non_snake_case)]
fn waveform_image(px: &[u8], w: usize, h: usize, kind: u8, vs_zoom: bool) -> egui::ColorImage {
    let vector = kind == SCOPE_VECTOR;
    let (W, H): (usize, usize) = if vector { (128, 128) } else { (256, 128) };
    // counts[channel][row * W + col]; channel 3 = white (luma / vectorscope).
    let mut counts = vec![vec![0u32; W * H]; 4];
    let step = ((w * h) / 120_000).max(1);
    let level_row = |v: f32| H - 1 - ((v / 255.0).clamp(0.0, 1.0) * (H - 1) as f32).round() as usize;
    let vs_scale = if vs_zoom { 2.0 } else { 1.0 };
    let mut i = 0;
    while i < w * h {
        let (x, p) = (i % w, i * 4);
        let (r, g, b) = (px[p] as f32, px[p + 1] as f32, px[p + 2] as f32);
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let kind = match kind {
            SCOPE_RGB => 1,
            SCOPE_PARADE => 3,
            SCOPE_VECTOR => 4,
            _ => 2,
        };
        match kind {
            1 => {
                let col = x * W / w;
                for (c, v) in [r, g, b].into_iter().enumerate() {
                    counts[c][level_row(v) * W + col] += 1;
                }
            }
            3 => {
                // Three thirds: red, green, blue.
                let third = W / 3;
                for (c, v) in [r, g, b].into_iter().enumerate() {
                    let col = c * third + x * third / w;
                    counts[c][level_row(v) * W + col] += 1;
                }
            }
            4 => {
                let cb = (b - luma) / 1.8556;
                let cr = (r - luma) / 1.5748;
                let col = (W as f32 / 2.0 + cb / 255.0 * H as f32 * 0.9 * vs_scale).clamp(0.0, W as f32 - 1.0) as usize;
                let row = (H as f32 / 2.0 - cr / 255.0 * H as f32 * 0.9 * vs_scale).clamp(0.0, H as f32 - 1.0) as usize;
                counts[3][row * W + col] += 1;
            }
            _ => {
                counts[3][level_row(luma) * W + x * W / w] += 1;
            }
        }
        i += step;
    }
    let per_col = ((w * h / step) as f32 / W as f32).max(1.0);
    let alpha = |c: u32| ((c as f32 / per_col * 40.0).sqrt().min(1.0) * 255.0) as u8;
    let mut img = egui::ColorImage::new([W, H], Color32::TRANSPARENT);
    for k in 0..W * H {
        let (cr, cg, cb, cw) = (counts[0][k], counts[1][k], counts[2][k], counts[3][k]);
        if cw > 0 {
            img.pixels[k] = Color32::from_rgba_unmultiplied(170, 255, 170, alpha(cw).max(60));
        } else if cr + cg + cb > 0 {
            let lit = |c: u32| if c > 0 { 255 } else { 40 };
            img.pixels[k] = Color32::from_rgba_unmultiplied(lit(cr), lit(cg), lit(cb), alpha(cr.max(cg).max(cb)).max(60));
        }
    }
    img
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
        s.fullscreen = s.cfg.start_fullscreen;
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
                    i.layers[0] = Layer { on: true, input: Some(b), place: Position { zoom: 0.3, pan_x: 0.6, pan_y: -0.6, ..Position::default() } };
                }
                s.settings_input = Some(a);
                s.cfg.input_page = 6;
            }
            "add" => {
                s.add_dialog.open = true;
                s.add_dialog.kind = AddKind::Colour;
            }
            _ => {}
        }
        Self {
            s,
            frame: 0,
            screenshot,
            shot_pending: false,
            shot_recording: None,
            shot_wave: false,
            next_record_frame: 0.0,
            record_format: ([1920, 1080], 30.0),
            program: Arc::default(),
            graphics_adapter: "OpenGL renderer unavailable".into(),
            performance: performance::Monitor::new(),
            last_time: 0.0,
            fps: 30.0,
            media: Default::default(),
            recordings: Default::default(),
            multi_outputs: Default::default(),
            multi_output_failed: [false; OUTPUT_COUNT],
            multi: Default::default(),
            browse_rx: None,
            browse_kind: media::PickKind::Video,
            snap_pending: None,
            snap_rx: None,
            photo_filter: 0,
            multi_started: Default::default(),
            scopes: Default::default(),
            next_wave: 0.0,
        }
    }

    /// Starts/stops camera and file readers and turns their frames into pictures.
    fn update_media(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering::Relaxed;
        let now = ctx.input(|i| i.time);
        let display_sources: Vec<_> = self.s.cfg.outputs.iter().zip(&self.s.external)
            .filter(|(_, window)| window.open).map(|(route, _)| route.source)
            .chain(self.s.fullscreen.then_some(self.s.cfg.fs_source))
            .chain(self.composed_recordings().map(|r| self.s.cfg.outputs[r.channel].source))
            .collect();
        let App { s, media: feeds, multi, multi_started, .. } = self;
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
        let (mc_dir, mc_mbps, mc_fps, mc_hw, mc_format) = (
            s.cfg.mc_folder.clone(), s.cfg.mc_mbps, s.cfg.frame_rate(), s.cfg.hw_encoder, s.cfg.mc_codec,
        );
        // Inputs seen big right now (Preview, Output, transitions, overlays and their layers).
        let skip = u64::from(s.cfg.preview_skip.max(1));
        let ui_frame = s.ui_frame;
        let mut live: std::collections::HashSet<u32> = [s.preview, s.output].into_iter().flatten().collect();
        if let Some(tr) = &s.trans {
            live.insert(tr.to);
            live.extend(tr.from);
        }
        live.extend(s.overlays.iter().filter(|c| c.on || c.level > 0.0).filter_map(|c| c.input));
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
                        // The chosen camera mode is part of the key: picking another restarts it.
                        Kind::Device { link, .. } => (
                            media::Source::Device(link.clone(), inp.cam_mode.clone()),
                            format!("{link}|{}", inp.cam_mode.as_ref().map(|m| m.code()).unwrap_or_default()),
                            false,
                            true,
                        ),
                        Kind::VideoFile { path } => (media::Source::File(path.clone()), path.clone(), true, false),
                        Kind::VideoList { paths, idx } => {
                            let p = paths.get(*idx).cloned().unwrap_or_default();
                            (media::Source::File(p.clone()), format!("{idx}:{p}"), true, false)
                        }
                        Kind::Desktop => (media::Source::Desktop, "desktop".into(), false, false),
                        Kind::ImageSequence { folder } => (media::Source::Sequence(folder.clone(), output_fps()), folder.clone(), true, false),
                        Kind::Photos { folder } => (media::Source::Photos(folder.clone(), PHOTO_SECS), folder.clone(), true, false),
                        _ => continue,
                    };
                    let is_list = matches!(inp.kind, Kind::VideoList { .. });
                    if feeds.get(&inp.id).map_or(false, |f| f.key != key) {
                        // keep the last picture while the next list item starts
                        let old_tex = feeds.remove(&inp.id).and_then(|mut f| f.tex.take());
                        feeds.insert(inp.id, Feed { shared: Some(media::start(src.clone())), tex: old_tex, seen: 0, key: key.clone(), prev_tex: None, fade_start: 0.0 });
                    }
                    let feed = feeds.entry(inp.id).or_insert_with(|| Feed { shared: Some(media::start(src)), tex: None, seen: 0, key, prev_tex: None, fade_start: 0.0 });
                    let Some(sh) = feed.shared.clone() else { continue };
                    if is_file {
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
                    let seq = sh.seq.load(Relaxed);
                    // Preview Skip: an input that isn't on screen big gets its new picture only
                    // every Nth interface frame (MultiCorder still records every frame).
                    let recording_this = multicorder && is_device && !s.mc_off.contains(&inp.id);
                    let skip_now = skip > 1 && !live.contains(&inp.id) && !recording_this && ui_frame % skip != 0;
                    if seq != feed.seen && !skip_now {
                        feed.seen = seq;
                        let fr = sh.frame.lock().ok().and_then(|mut f| f.take());
                        if let Some(fr) = fr {
                            if recording_this {
                                let title = safe_name(&inp.title);
                                let rec = multi.entry(inp.id).or_insert_with(|| {
                                    multi_started.insert(inp.id, now);
                                    let p = rec_dir(&mc_dir).join(format!("{}.{}", media::recording_name(&title), mc_format.extension()));
                                    // Each camera keeps its own picture size.
                                    media::Recorder::start_opts(p.to_string_lossy().to_string(), media::RecOpts { mbps: mc_mbps, size: None, fps: mc_fps, hw: mc_hw, format: mc_format })
                                });
                                rec.push(fr.rgba.clone(), fr.w, fr.h);
                            }
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
                    // na::c @0x14aa30: step = mr::zzy(500 ms) per output frame; na::Render @0x14a75c adds it up to 255.
                    let frames = ((now - feed.fade_start) * output_fps() as f64).floor() as f32;
                    let k = frames * step_for(PHOTO_FADE_MS) / FULL;
                    if k >= 1.0 {
                        feed.prev_tex = None;
                    }
                    inp.prev_tex = feed.prev_tex.as_ref().map(|t| (t.id(), t.size(), k.clamp(0.0, 1.0)));
                }
                Kind::ImageFile { path } => {
                    if !feeds.contains_key(&inp.id) {
                        let mut f = Feed { shared: None, tex: None, seen: 0, key: String::new(), prev_tex: None, fade_start: 0.0 };
                        match image::open(path) {
                            Ok(im) => {
                                let im = im.to_rgba8();
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
    }

    fn composed_recordings(&self) -> impl Iterator<Item = &ComposedRecording> {
        self.recordings.iter().chain(&self.multi_outputs).filter_map(Option::as_ref)
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
        self.s.cfg.outputs[recording.channel] == OutputRoute::default()
    }

    fn update_recording(&mut self, now: f64) {
        if self.composed_recordings().next().is_none() {
            let (w, h) = self.s.cfg.out_dims();
            self.record_format = ([w, h], f64::from(self.s.cfg.frame_rate()));
            self.next_record_frame = now;
        }
        let ([w, h], fps) = self.record_format;
        let options = |mbps, format, hw| media::RecOpts { mbps, size: Some((w, h)), fps: fps as f32, hw, format };

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
                    options(c.rec_mbps, c.rec_codec, c.hw_encoder)));
                if c.rec_second {
                    self.recordings[1] = Some(ComposedRecording::new(&c.rec_folder, &format!("{name} - 2"), c.rec_second_source, now,
                        options(c.rec_second_mbps, c.rec_second_codec, c.hw_encoder)));
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
                    options(c.mc_mbps, c.mc_codec, c.hw_encoder)));
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

    /// Capture the Program when due and record the elapsed time at the end of drawing.
    fn finish_frame(&mut self, ctx: &egui::Context, now: f64, started: Instant) {
        let (available, pending) = {
            let state = self.program.lock().unwrap();
            (state.error.is_none(), state.target.as_ref().is_some_and(program::Target::has_pending))
        };
        let mut frames = Vec::new();
        if available && now >= self.next_record_frame && self.composed_recordings().next().is_some() {
            self.advance_record_clock(now);
            let size = self.record_format.0;
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(size[0] as f32, size[1] as f32));
            let layer = egui::LayerId::new(egui::Order::Background, egui::Id::new("program-offscreen"));
            let mut groups: Vec<(OutputRoute, Vec<Arc<media::Recorder>>)> = Vec::new();
            for recording in self.composed_recordings() {
                let route = self.s.cfg.outputs[recording.channel];
                if let Some((_, recorders)) = groups.iter_mut().find(|(existing, _)| *existing == route) {
                    recorders.push(Arc::clone(&recording.recorder));
                } else {
                    groups.push((route, vec![Arc::clone(&recording.recorder)]));
                }
            }
            let captured_at = Instant::now();
            for (route, recorders) in groups {
                draw_src(&egui::Painter::new(ctx.clone(), layer, rect), rect, &self.s, now, route.source, route.overlays);
                let shapes = ctx.graphics_mut(|graphics| {
                    std::mem::take(graphics.entry(layer)).all_entries().cloned().collect()
                });
                frames.push((size, ctx.tessellate(shapes, 1.0), captured_at, recorders));
            }
        }
        let program = Arc::clone(&self.program);
        let peak = Arc::clone(&self.performance.render_peak);
        ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::new("program-capture")))
            .add(egui::PaintCallback {
                rect: ctx.screen_rect(),
                callback: Arc::new(eframe::egui_glow::CallbackFn::new(move |_, painter| {
                    if pending || !frames.is_empty() {
                        let mut state = program.lock().unwrap();
                        let result = unsafe {
                            (|| {
                                if let Some(target) = state.target.as_mut() {
                                    target.collect(painter.gl())?;
                                }
                                for (size, meshes, captured_at, recorders) in &frames {
                                    if state.target.is_none() {
                                        state.target = Some(program::Target::new(painter.gl())?);
                                    }
                                    state.target.as_mut().unwrap().capture(painter, *size, meshes, *captured_at, recorders)?;
                                }
                                Ok::<(), String>(())
                            })()
                        };
                        if let Err(error) = result {
                            state.error = Some(error);
                            for (_, _, _, recorders) in &frames {
                                for recorder in recorders { recorder.note_dropped_frames(1); }
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
        let sample_wave = got.is_some() && std::mem::take(&mut self.shot_wave);
        let record_due = self.composed_recordings().any(|r| self.can_capture_from_window(r)) && now >= self.next_record_frame
            && self.program.lock().unwrap().error.is_some();
        // Waveform: a fresh look at the Preview five times a second.
        let wave_due = self.s.cfg.wave_type != 0 && now >= self.next_wave;
        let want = record_due || (got.is_none() && (check_due || self.s.snapshot_req || wave_due));
        if want && !self.shot_pending {
            self.shot_pending = true;
            self.shot_recording = record_due.then_some(now);
            self.shot_wave = wave_due;
            if wave_due {
                self.next_wave = now + 0.2;
            }
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
        let pv = self.s.preview_rect;
        if sample_wave && self.s.cfg.wave_type != 0 && pv.is_positive() {
            let (px, w, h) = crop(pv);
            if w > 0 && h > 0 {
                // One picture per scope the chosen layout shows.
                let mut ids = Vec::new();
                for (kind, _) in scope_layout(self.s.cfg.wave_type).1 {
                    let img = waveform_image(&px, w, h, kind, self.s.cfg.vs_zoom);
                    let tex = self.scopes.entry(kind).or_insert_with(|| ctx.load_texture(format!("preview-scope-{kind}"), egui::ColorImage::new([1, 1], Color32::TRANSPARENT), egui::TextureOptions::LINEAR));
                    tex.set(img, egui::TextureOptions::LINEAR);
                    ids.push((kind, tex.id()));
                }
                self.s.scope_tex = ids;
            }
        }
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
                media::Recorder::push_group(&recorders, px, w, h, Instant::now(), false);
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
    fn handle_requests(&mut self) {
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
                if let Some(p) = res {
                    match (self.browse_kind, self.s.browse_for) {
                        // Recording Setup "Browse" / MultiCorder "Change Recording Folder".
                        (media::PickKind::Folder, 1) => self.s.cfg.rec_folder = p,
                        (media::PickKind::Folder, 2) => self.s.cfg.mc_folder = p,
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
        let d = &self.s.add_dialog;
        if self.s.refresh_devices || (d.open && d.kind.uses_device() && !d.devices_loaded) {
            self.s.refresh_devices = false;
            self.s.add_dialog.devices = media::list_devices()
                .into_iter()
                .map(|x| DeviceChoice { name: x.name, link: x.link, blackmagic: x.blackmagic })
                .collect();
            self.s.add_dialog.devices_loaded = true;
            self.s.add_dialog.device = 0;
        }
    }
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
/// (shortcuts\templates\Default Keyboard.vMixShortcutTemplate, 36 shortcuts).
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
        let now = Instant::now();
        self.performance.update(now, || {
            self.program.lock().unwrap().target.as_ref().map_or(0, program::Target::transfer_bytes)
        });
        let render_started = Instant::now();
        let t = ctx.input(|i| i.time);
        let dt = if self.frame == 0 { 1.0 / 60.0 } else { (t - self.last_time).max(0.0) as f32 };
        self.last_time = t;
        if dt > 0.0 {
            self.fps = self.fps * 0.95 + (1.0 / dt.max(0.001)) * 0.05;
        }
        // Project format and performance settings used by the engine and the camera readers.
        set_output_fps(self.s.cfg.rate());
        media::AUTO_CAM_MAX_H.store(self.s.cfg.auto_cam_max_h, std::sync::atomic::Ordering::Relaxed);
        self.s.tick(t, dt);
        self.frame += 1;
        self.s.ui_frame = self.frame;
        self.handle_requests();
        self.update_media(ctx);
        self.update_recording(t);

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.s.fullscreen = false;
            for window in &mut self.s.external {
                window.fullscreen = false;
            }
        }
        shortcuts(ctx, &mut self.s);

        top_bar(ctx, &mut self.s, self.fps);
        status_bar(ctx, &self.s, self.fps, &self.performance.status);
        bottom_bar(ctx, &mut self.s, t);
        monitors(ctx, &mut self.s, t);
        if self.s.show_mixer {
            mixer_panel(ctx, &mut self.s, t);
        }
        // Filled in again by the Preview input's box when it is drawn.
        self.s.preview_tile_rect = egui::Rect::NOTHING;
        inputs_panel(ctx, &mut self.s, t);
        let encoder = if self.recordings.iter().all(Option::is_none) {
            "Start recording to identify the encoder".into()
        } else {
            self.recordings.iter().enumerate().filter_map(|(index, recording)| recording.as_ref()
                .map(|r| format!("{}: {}", index + 1, r.recorder.diagnostics()))).collect::<Vec<_>>().join("\n")
        };
        let program_path = self.program.lock().unwrap().error.as_ref()
            .map_or_else(|| "Dedicated OpenGL texture".into(), |e| format!("Window capture fallback ({e})"));
        dialogs(ctx, &mut self.s, &self.graphics_adapter, &encoder, &program_path);
        for channel in 0..OUTPUT_COUNT {
            if self.s.external[channel].open {
                output_window(ctx, &mut self.s, t, channel);
            }
        }
        if self.s.fullscreen {
            fullscreen_window(ctx, &mut self.s, t);
        }

        self.handle_screenshots(ctx, t);
        self.finish_frame(ctx, t, render_started);
        if self.s.cfg.lock_refresh {
            // Synchronize Output with Refresh Rate: draw at the screen's refresh rate.
            ctx.request_repaint();
        } else {
            // egui subtracts its predicted frame time from delayed repaint requests.
            let predicted_dt = ctx.input(|i| i.predicted_dt);
            ctx.request_repaint_after_secs(1.0 / output_fps() + predicted_dt);
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let (Some(gl), Some(target)) = (gl, self.program.lock().unwrap().target.take()) {
            unsafe { target.destroy(gl) };
        }
    }
}

// ---------------------------------------------------------------- top bar

fn top_bar(ctx: &egui::Context, s: &mut Switcher, fps: f32) {
    // Top bar of the original: Preset | New Open Save Save As Last ... Fullscreen ... Pause Inputs, Settings, ?
    let _ = fps;
    egui::TopBottomPanel::top("top")
        .exact_height(30.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(WINDOW).inner_margin(egui::Margin::symmetric(4, 3)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.add_sized([80.0, 24.0], egui::Label::new(RichText::new("Preset").strong().color(Color32::WHITE)));
                if bar_btn(ui, "New", s.new_step != NewStep::Idle).on_hover_text("Start a new blank preset with a video format you choose").clicked() {
                    s.press_new();
                }
                if bar_btn(ui, "Open", false).on_hover_text("Open the saved preset").clicked() {
                    if s.outputs_busy() {
                        s.notice(STOP_OUTPUTS_MSG);
                    } else {
                        s.load_preset();
                    }
                }
                if bar_btn(ui, "Save", false).on_hover_text("Save changes to currently open Preset file (Ctrl+S)").clicked() {
                    s.save_preset();
                }
                if bar_btn(ui, "Save As", false).on_hover_text("Save to a new Preset file").clicked() {
                    s.save_preset();
                }
                if bar_btn(ui, "Last", false).on_hover_text("Open the last preset").clicked() {
                    if s.outputs_busy() {
                        s.notice(STOP_OUTPUTS_MSG);
                    } else {
                        s.load_preset();
                    }
                }
                let screen_w = ctx.screen_rect().width();
                let fs_x = (screen_w - 150.0) / 2.0;
                let cur = ui.cursor().min.x;
                if fs_x > cur {
                    ui.add_space(fs_x - cur);
                }
                let fs_tip = |ui: &mut egui::Ui| {
                    ui.label(format!(
                        "Toggle on or off the Fullscreen Output display — shows {} on screen {} (change both in Settings > Display)",
                        src_name(s, s.cfg.fs_source),
                        (s.cfg.fs_display as usize).min(screens().len() - 1) + 1
                    ));
                };
                if bar_btn(ui, "Fullscreen", s.fullscreen).on_hover_ui(fs_tip).clicked() {
                    s.fullscreen = !s.fullscreen;
                }
                // External Output indicator beside Fullscreen (lit while the External window is on).
                let (r, resp) = ui.allocate_exact_size(Vec2::new(40.0, 24.0), Sense::click());
                let on = s.external[0].open;
                ui.painter().rect_filled(r, 2.0, if on { ACTIVE_RED } else { BTN_DARK });
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "EXT", egui::FontId::proportional(12.0), if on { Color32::WHITE } else { TEXT_DIM });
                if resp.on_hover_text(if on { "External Output is on — click to turn it off" } else { "External Output is off — click to turn it on" }).clicked() {
                    s.toggle_external(0);
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
fn status_bar(ctx: &egui::Context, s: &Switcher, fps: f32, stats: &performance::Status) {
    egui::TopBottomPanel::bottom("status")
        .exact_height(22.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(WINDOW).inner_margin(egui::Margin::symmetric(8, 2)))
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
                ui.label(RichText::new(format!("{:.0}", fps.min(999.0))).size(12.0).color(TEXT));
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
        .frame(egui::Frame::new().fill(PANEL))
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
fn draw_scope(p: &egui::Painter, r: egui::Rect, tex: Option<egui::TextureId>, kind: u8, over: bool) {
    let p = p.with_clip_rect(r);
    p.rect_filled(r, 0.0, if over { Color32::from_black_alpha(110) } else { Color32::BLACK });
    let faint = Stroke::new(1.0_f32, Color32::from_white_alpha(45));
    let plot = r.shrink2(Vec2::new(4.0, 6.0));
    if kind == SCOPE_VECTOR {
        let sq = egui::Rect::from_center_size(plot.center(), Vec2::splat(plot.width().min(plot.height())));
        p.circle_stroke(sq.center(), sq.width() / 2.0 * 0.9, faint);
        p.line_segment([sq.center_top(), sq.center_bottom()], faint);
        p.line_segment([sq.left_center(), sq.right_center()], faint);
        if let Some(t) = tex {
            p.image(t, sq, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
    } else {
        let plot = egui::Rect::from_min_max(plot.min + Vec2::new(22.0, 0.0), plot.max);
        for k in 0..=4 {
            let y = plot.bottom() - plot.height() * k as f32 / 4.0;
            p.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)], faint);
            p.text(egui::pos2(plot.left() - 4.0, y), egui::Align2::RIGHT_CENTER, format!("{}", k * 25), egui::FontId::proportional(9.0), Color32::from_white_alpha(150));
        }
        if kind == SCOPE_PARADE {
            for k in 1..3 {
                let x = plot.left() + plot.width() * k as f32 / 3.0;
                p.line_segment([egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())], faint);
            }
        }
        if let Some(t) = tex {
            p.image(t, plot, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
        }
    }
    let name = match kind {
        SCOPE_RGB => "Waveform RGB",
        SCOPE_Y => "Waveform Y",
        SCOPE_PARADE => "Parade RGB",
        _ => "Vectorscope",
    };
    p.text(r.right_top() + Vec2::new(-6.0, 4.0), egui::Align2::RIGHT_TOP, name, egui::FontId::proportional(10.0), Color32::from_white_alpha(150));
    if tex.is_none() {
        p.text(r.center(), egui::Align2::CENTER_CENTER, "…", egui::FontId::proportional(14.0), Color32::from_white_alpha(150));
    }
}

fn monitor(ui: &mut egui::Ui, s: &mut Switcher, t: f64, is_output: bool, w: f32, h: f32) {
    ui.vertical(|ui| {
        ui.set_width(w);
        let id = if is_output { s.output } else { s.preview };
        let title = id.and_then(|i| s.input(i)).map(|i| i.title.clone()).unwrap_or_else(|| "—".into());
        let num = id.map(|i| s.number_of(i)).unwrap_or(0);
        // Full-width title bar: orange for Preview, green for Output (as in the original).
        let (bar, _) = ui.allocate_exact_size(Vec2::new(w, 26.0), Sense::hover());
        let col = if is_output { OUTPUT } else { PREVIEW };
        ui.painter().rect_filled(bar, 0.0, col);
        let num_txt = if num > 0 { num.to_string() } else { "–".into() };
        ui.painter().text(bar.left_center() + Vec2::new(10.0, 0.0), egui::Align2::LEFT_CENTER, num_txt, egui::FontId::proportional(13.0), Color32::WHITE);
        ui.painter().text(bar.left_center() + Vec2::new(32.0, 0.0), egui::Align2::LEFT_CENTER, &title, egui::FontId::proportional(13.0), Color32::WHITE);
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
        // The picture keeps 16:9 inside the black monitor area.
        let mut pw = w;
        let mut ph = w * 9.0 / 16.0;
        if ph > h {
            ph = h;
            pw = h * 16.0 / 9.0;
        }
        let rect = egui::Rect::from_center_size(area.center(), Vec2::new(pw, ph));
        let painter = ui.painter_at(rect);
        if is_output {
            s.output_rect = rect;
            draw_output(&painter, rect, s, t);
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
            // The scopes measure the picture itself when nothing covers it (the "+ Preview"
            // layouts), otherwise the Preview input's own box picture.
            s.preview_rect = match pic {
                Some(f) if f != [0.0, 0.0, 1.0, 1.0] => place(f),
                Some(_) if scopes.is_empty() => rect,
                _ => s.preview_tile_rect,
            };
            for (kind, f) in &scopes {
                // Alone over the picture the scope keeps it visible underneath.
                let over = pic == Some([0.0, 0.0, 1.0, 1.0]);
                let tex = s.scope_tex.iter().find(|(k, _)| k == kind).map(|(_, t)| *t);
                draw_scope(&painter, place(*f), tex, *kind, over);
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
        let overlay_space = if s.cfg.show_overlays { 52.0 } else { 0.0 };
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
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                ui.add_space(13.0);
                if ui
                    .add_sized([bw - 17.0, bh], egui::Button::new(RichText::new(tr.effect.name()).strong()))
                    .on_hover_text(format!("{} — {} ms", tr.effect.name(), tr.ms))
                    .clicked()
                {
                    s.start_transition(tr.effect, tr.ms);
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
                        ui.add(egui::DragValue::new(&mut s.transitions[k].ms).range(1..=9999).speed(10.0).suffix(" ms"));
                    });
                    ui.separator();
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for e in ALL_EFFECTS {
                            ui.selectable_value(&mut s.transitions[k].effect, e, e.name());
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
        Some(tr) => {
            let a = tr.from.and_then(|i| s.input(i));
            let b = s.input(tr.to);
            draw_transition(painter, rect, a, b, tr.effect, tr.progress / FULL, t, &s.inputs);
        }
        None => {
            if let Some(inp) = s.output.and_then(|i| s.input(i)) {
                draw_source(painter, rect, inp, 1.0, t, &s.inputs);
            }
        }
    }
    if overlays {
        draw_overlay_channels(painter, rect, s, t, false);
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
        // Strip colour: Output (ActiveColour, DarkGreen) before Preview (PreviewColour,
        // DarkOrange); anything else MV_IDLE.
        let strip_col = if src == SRC_OUTPUT || (src != SRC_PREVIEW && id.is_some() && id == s.output) {
            OUTPUT
        } else if src == SRC_PREVIEW || (id.is_some() && id == s.preview) {
            PREVIEW
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
fn fullscreen_window(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    let id = egui::ViewportId::from_hash_of("livemix-fullscreen");
    let mons = screens();
    let m = mons[(s.cfg.fs_display as usize).min(mons.len() - 1)];
    let ppp = ctx.pixels_per_point();
    let builder = egui::ViewportBuilder::default()
        .with_title("LiveMix Fullscreen")
        .with_decorations(false)
        .with_position(egui::pos2(m[0] as f32 / ppp, m[1] as f32 / ppp))
        .with_inner_size(Vec2::new(m[2] as f32 / ppp, m[3] as f32 / ppp))
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    // Re-place the window when another screen is chosen.
    let key = egui::Id::new("fs-display-last");
    let last = ctx.data(|d| d.get_temp::<u8>(key));
    if last != Some(s.cfg.fs_display) {
        ctx.data_mut(|d| d.insert_temp(key, s.cfg.fs_display));
        if last.is_some() {
            ctx.data_mut(|d| d.remove::<u32>(egui::Id::new("fs-started")));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Fullscreen(false));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::OuterPosition(egui::pos2(m[0] as f32 / ppp, m[1] as f32 / ppp)));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::InnerSize(Vec2::new(m[2] as f32 / ppp, m[3] as f32 / ppp)));
        }
    }
    let mut close = false;
    ctx.show_viewport_immediate(id, builder, |ctx, _class| {
        // Once the window sits on its screen, make it truly fullscreen there.
        let started = egui::Id::new("fs-started");
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
            draw_src(&ui.painter_at(out), out, s, t, s.cfg.fs_source, s.cfg.fs_overlays);
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
        s.fullscreen = false;
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

fn inputs_panel(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(6)))
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
                        lp.rect_filled(gr, 2.0, PREVIEW.gamma_multiply(0.85));
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
    let border = if is_out { OUTPUT_BRIGHT } else if is_prev { PREVIEW } else { EDGE };
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
        let hcol = if is_out { OUTPUT } else if is_prev { PREVIEW } else { BTN };
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
        if s.preview == Some(id) {
            // The scopes measure this picture when the Preview monitor is covered by them.
            s.preview_tile_rect = r;
        }
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
                if tile_btn(ui, "Preview", 48.0 * scale, is_prev, true, PREVIEW).on_hover_text("Send this Input to Preview").clicked() {
                    s.preview = Some(id);
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
                        format!("Assigned to Overlay {} ({target}). Use the centre button to turn it on or off.", c + 1)
                    } else {
                        format!("Assign this input to Overlay {} (right-click assigns to Preview)", c + 1)
                    };
                    let r = tile_btn(ui, &(c + 1).to_string(), width, assigned.is_some(), true, CYAN).on_hover_text(tip);
                    if r.secondary_clicked() {
                        s.assign_overlay(c, id, true);
                    } else if r.clicked() {
                        s.assign_overlay(c, id, false);
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

fn mixer_panel(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    egui::SidePanel::right("mixer")
        .exact_width(330.0)
        .resizable(false)
        .frame(egui::Frame::new().fill(WINDOW).inner_margin(egui::Margin::same(6)))
        .show(ctx, |ui| {
            ui.label(RichText::new("Audio Mixer").strong().color(Color32::WHITE));
            ui.add_space(2.0);
            egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    // master strip
                    let master_level = s.master_level(t);
                    strip(ui, "Master", &mut s.master_volume, &mut s.master_on, master_level, None);
                    ui.separator();
                    let ids: Vec<u32> = s.inputs.iter().filter(|i| i.kind.has_audio()).map(|i| i.id).collect();
                    for id in ids {
                        let level = s.input_level(id, t);
                        let num = s.number_of(id);
                        if let Some(i) = s.input_mut(id) {
                            let name = format!("{} {}", num, i.title);
                            let mut buses = [i.bus_m, i.bus_a, i.bus_b];
                            strip(ui, &name, &mut i.volume, &mut i.audio_on, level, Some(&mut buses));
                            i.bus_m = buses[0];
                            i.bus_a = buses[1];
                            i.bus_b = buses[2];
                        }
                    }
                });
            });
        });
}

fn strip(ui: &mut egui::Ui, name: &str, vol: &mut f32, on: &mut bool, level: f32, buses: Option<&mut [bool; 3]>) {
    ui.allocate_ui_with_layout(Vec2::new(66.0, ui.available_height()), egui::Layout::top_down(egui::Align::Center), |ui| {
        ui.set_width(66.0);
        let mut short: String = name.chars().take(10).collect();
        if name.chars().count() > 10 {
            short.push('…');
        }
        ui.label(RichText::new(short).size(10.5).color(TEXT));
        let h = (ui.available_height() - 90.0).clamp(80.0, 260.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().slider_width = h;
            ui.add(egui::Slider::new(vol, 0.0..=100.0).vertical().show_value(false));
            let (r, _) = ui.allocate_exact_size(Vec2::new(14.0, h), Sense::hover());
            meter(ui.painter(), r, if *on { level } else { 0.0 });
        });
        ui.label(RichText::new(format!("{:.0}", vol)).size(10.0).color(TEXT_DIM));
        let b = egui::Button::new(RichText::new(if *on { "On" } else { "Muted" }).size(10.5).color(Color32::BLACK))
            .fill(if *on { Color32::from_rgb(0x4c, 0xc0, 0x50) } else { Color32::from_rgb(0xd0, 0x40, 0x40) })
            .min_size(Vec2::new(56.0, 18.0));
        if ui.add(b).clicked() {
            *on = !*on;
        }
        if let Some(bs) = buses {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (k, n) in ["M", "A", "B"].iter().enumerate() {
                    let b = egui::Button::new(RichText::new(*n).size(10.0).color(if bs[k] { Color32::BLACK } else { TEXT }))
                        .fill(if bs[k] { Color32::from_rgb(0x50, 0x9a, 0xe0) } else { BTN })
                        .min_size(Vec2::new(18.0, 16.0));
                    if ui.add(b).on_hover_text("Send this Inputs audio to this bus").clicked() {
                        bs[k] = !bs[k];
                    }
                }
            });
        }
    });
}

// ---------------------------------------------------------------- bottom bar

fn bottom_bar(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    egui::TopBottomPanel::bottom("bottom")
        .exact_height(40.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(WINDOW).inner_margin(egui::Margin::symmetric(2, 4)))
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
                if big_btn(ui, "External", s.external[0].open, Some(red)).on_hover_text("Start or stop External Output 1").clicked() {
                    s.toggle_external(0);
                }
                ui.add_space(14.0);
                if gear_btn(ui, s.stream_setup, "Configure Streaming settings").clicked() {
                    s.stream_setup = !s.stream_setup;
                }
                if big_btn(ui, "Stream", false, None).on_hover_text("Start or stop Streaming (sending is not built yet — the settings are kept)").clicked() {
                    s.notice("Streaming (RTMP/SRT) is not built yet in this version — use Record for now");
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

/// The original's Input Settings sections, in its order (frmInputSettings.tMenu.*).
const INPUT_PAGES: [&str; 11] = [
    "General",
    "Colour Adjust",
    "Colour Key",
    "Colour Correction",
    "Effects",
    "Position",
    "Layers",
    "Triggers",
    "Tally Lights",
    "PTZ",
    "Advanced",
];
/// Sections built in this version.
const INPUT_PAGES_BUILT: [usize; 4] = [0, 2, 5, 6];

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

fn input_general(ui: &mut egui::Ui, inp: &mut Input) {
    egui::Grid::new("in-general").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        ui.text_edit_singleline(&mut inp.title);
        ui.end_row();
        ui.label("Type");
        let kind = match &inp.kind {
            Kind::Device { blackmagic: true, .. } => "Camera (Blackmagic)",
            Kind::Device { .. } => "Camera",
            Kind::VideoFile { .. } => "Video",
            Kind::VideoList { .. } => "List",
            Kind::ImageFile { .. } => "Image",
            Kind::Photos { .. } => "Photos",
            Kind::ImageSequence { .. } => "Image Sequence",
            Kind::Desktop => "Desktop Capture",
            Kind::Colour(_) => "Colour",
            Kind::Bars => "Colour Bars",
            Kind::Blank => "Transparent",
            Kind::Title { .. } => "Title",
            Kind::GtTitle { .. } => "GT Title",
            Kind::Clock => "Production Clock",
            Kind::Camera(_) => "Test Camera",
            Kind::Video { .. } => "Test Video",
            Kind::Image(_) => "Test Image",
        };
        ui.label(kind);
        ui.end_row();
        let file = match &inp.kind {
            Kind::VideoFile { path } | Kind::ImageFile { path } | Kind::GtTitle { path, .. } => Some(path.clone()),
            Kind::Photos { folder } | Kind::ImageSequence { folder } => Some(folder.clone()),
            _ => None,
        };
        if let Some(f) = file {
            ui.label("File");
            ui.label(RichText::new(f).size(11.0).color(TEXT_DIM));
            ui.end_row();
        }
        if matches!(inp.kind, Kind::Device { .. }) {
            camera_mode_rows(ui, inp);
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
    });
    ui.add_space(4.0);
    if let Kind::GtTitle { document, texts, .. } = &mut inp.kind {
        if let Some(document) = document {
            ui.heading("Title Editor");
            for (field, default) in &document.fields {
                ui.label(format!("{field}.Text"));
                let mut value = texts.get(field).unwrap_or(default).clone();
                if ui.add(egui::TextEdit::multiline(&mut value).desired_rows(1)).changed() {
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
    if inp.kind.playable() {
        ui.checkbox(&mut inp.loop_on, "Loop");
    }
    if inp.kind.has_audio() {
        ui.horizontal(|ui| {
            ui.checkbox(&mut inp.audio_on, "Audio");
            ui.add(egui::Slider::new(&mut inp.volume, 0.0..=100.0).text("Volume"));
        });
    }
    ui.horizontal(|ui| {
        ui.label("Category");
        let cur = inp.category.map(|c| CATEGORIES[c as usize].0).unwrap_or("None");
        egui::ComboBox::from_id_salt("in-cat").selected_text(cur).show_ui(ui, |ui| {
            ui.selectable_value(&mut inp.category, None, "None");
            for (k, (name, _)) in CATEGORIES.iter().enumerate() {
                ui.selectable_value(&mut inp.category, Some(k as u8), *name);
            }
        });
    });
    if !inp.status.is_empty() {
        ui.add_space(4.0);
        ui.label(RichText::new(&inp.status).size(11.0).color(TEXT_DIM));
    }
}

fn input_colour_key(ui: &mut egui::Ui, inp: &mut Input) {
    if !inp.kind.keyable() {
        ui.label(RichText::new("Colour keying works on cameras, video files, lists and desktop capture.").color(TEXT_DIM));
        return;
    }
    ui.checkbox(&mut inp.key.on, "Chroma Key (remove this colour)");
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
        if ui.small_button("Reset").on_hover_text("Reset like the original (frmInputSettings::zo)").clicked() {
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

/// Zoom, Pan X/Y and Crop X1/Y1/X2/Y2 with Reset buttons (frmInputSettings Position page).
fn position_editor(ui: &mut egui::Ui, salt: &str, p: &mut Position) {
    egui::Grid::new(("posgrid", salt)).num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
        let mut row = |ui: &mut egui::Ui, name: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, reset: f32| {
            ui.label(name);
            ui.add(egui::Slider::new(v, range).step_by(0.01).fixed_decimals(2));
            if ui.small_button("Reset").clicked() {
                *v = reset;
            }
            ui.end_row();
        };
        row(ui, "Zoom:", &mut p.zoom, 0.0..=5.0, 1.0);
        row(ui, "Pan X:", &mut p.pan_x, -2.0..=2.0, 0.0);
        row(ui, "Pan Y:", &mut p.pan_y, -2.0..=2.0, 0.0);
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

/// The 10 layers of an input: each shows another input on top, with its own position.
fn layers_editor(ui: &mut egui::Ui, inp: &mut Input, others: &[(usize, u32, String)]) {
    ui.label(RichText::new("Each layer draws another input on top of this one (layer 1 first, layer 10 on top).").size(11.0).color(TEXT_DIM));
    let sel_id = ui.make_persistent_id("layer-sel");
    let mut sel: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0);
    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
        for (k, l) in inp.layers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut sel, k, RichText::new(format!("Layer {}", k + 1)).strong());
                ui.checkbox(&mut l.on, "On");
                let cur = l
                    .input
                    .and_then(|id| others.iter().find(|o| o.1 == id))
                    .map(|o| format!("{} {}", o.0, o.2))
                    .unwrap_or_else(|| "None".into());
                egui::ComboBox::from_id_salt(("layer-in", k)).width(200.0).selected_text(cur).show_ui(ui, |ui| {
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
    ui.label(RichText::new(format!("Layer {} position", sel + 1)).strong());
    if let Some(l) = inp.layers.get_mut(sel) {
        position_editor(ui, &format!("layer{sel}"), &mut l.place);
    }
}

// ---------------------------------------------------------------- settings window

/// The original's Settings sections, in its order (language file frmSettings.tMenu.*).
const SETTINGS_PAGES: [&str; 16] = [
    "Display",
    "Outputs / NDI / SRT",
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
];
/// Sections built in this version (indexes into SETTINGS_PAGES).
const SETTINGS_BUILT: [usize; 10] = [0, 1, 2, 3, 5, 6, 7, 11, 14, 15];

/// Settings table for the same routes exposed by the External cog.
fn output_routes(ui: &mut egui::Ui, s: &mut Switcher) {
    let choices = src_choices(s, true);
    egui::Grid::new("output-routes").spacing([9.0, 6.0]).show(ui, |ui| {
        for label in ["Output", "Source", "Overlays", "NDI", "SRT", "Local window"] {
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
            for protocol in ["NDI", "SRT"] {
                ui.add_enabled(false, egui::Button::new("Off"))
                    .on_disabled_hover_text(format!("{protocol} transmission is unavailable in this build"));
            }
            if ui.button(if s.external[channel].open { "Close" } else { "Open" }).clicked() {
                s.toggle_external(channel);
            }
            ui.end_row();
        }
    });
}

fn external_menu(ui: &mut egui::Ui, s: &mut Switcher) {
    let button = egui::Button::new("").fill(BTN_DARK).min_size(Vec2::new(38.0, 30.0));
    let menu = egui::menu::menu_custom_button(ui, button, |ui| external_menu_items(ui, s));
    gear_icon(ui.painter(), menu.response.rect.center(), 8.0, Color32::WHITE, BTN_DARK);
    menu.response.on_hover_text("Configure Output 1–4, NDI and SRT")
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
            if ui.checkbox(&mut s.cfg.outputs[channel].overlays, "Overlays").changed() {
                s.save_settings();
            }
            for protocol in ["NDI", "SRT"] {
                ui.add_enabled(false, egui::Button::new(format!("{protocol} Off")))
                    .on_disabled_hover_text("Network transmission is unavailable in this build");
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
    for (page, label) in [(1, "Outputs / NDI / SRT…"), (6, "External Window Settings…")] {
        if ui.button(label).clicked() {
            s.settings_open = true;
            s.cfg.page = page;
            ui.close_menu();
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
                    ui.checkbox(&mut s.cfg.interlaced, "Interlaced");
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
                ui.label(RichText::new("DarkOrange").color(PREVIEW));
                ui.end_row();
                ui.label("Output Colour:");
                ui.label(RichText::new("DarkGreen").color(Color32::from_rgb(0x30, 0xc0, 0x30)));
                ui.end_row();
                ui.label("Theme:");
                ui.label("Charcoal");
                ui.end_row();
            });
            ui.add_space(6.0);
            // Fullscreen (frmSettings lblFullscreen / lblOutputScreen "Display:").
            ui.label(RichText::new("Fullscreen:").strong());
            egui::Grid::new("set-fs").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label("Source:");
                let cur = src_name(s, s.cfg.fs_source);
                let choices = src_choices(s, true);
                egui::ComboBox::from_id_salt("fs-src").width(240.0).selected_text(cur).show_ui(ui, |ui| {
                    for (v, name) in choices {
                        ui.selectable_value(&mut s.cfg.fs_source, v, name);
                    }
                });
                ui.end_row();
                ui.label("Display:");
                let mons = screens();
                let cur = (s.cfg.fs_display as usize).min(mons.len() - 1);
                let mname = |k: usize, m: &[i32; 4]| format!("Screen {} — {}x{}{}", k + 1, m[2], m[3], if k == 0 { " (main)" } else { "" });
                egui::ComboBox::from_id_salt("fs-display").width(240.0).selected_text(mname(cur, &mons[cur])).show_ui(ui, |ui| {
                    for (k, m) in mons.iter().enumerate() {
                        if ui.selectable_label(k == cur, mname(k, m)).clicked() {
                            s.cfg.fs_display = k as u8;
                        }
                    }
                });
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut s.cfg.fs_overlays, "Overlays");
                if ui.button(if s.fullscreen { "Turn Fullscreen off" } else { "Turn Fullscreen on" }).clicked() {
                    s.fullscreen = !s.fullscreen;
                }
                ui.label(RichText::new("Esc or a double-click leaves it").size(11.0).color(TEXT_DIM));
            });
            ui.separator();
            multiview_settings(ui, s);
            ui.separator();
            ui.checkbox(&mut s.cfg.input_numbers, "Show Input Numbers");
            ui.checkbox(&mut s.cfg.master_meter, "Show Master Audio Meter in Main Window");
            ui.checkbox(&mut s.cfg.input_meter, "Show Audio Meter next to Inputs");
            ui.separator();
            ui.label(RichText::new("Transition Display:").strong());
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut s.cfg.show_quickplay, "QuickPlay");
                ui.checkbox(&mut s.cfg.show_trans24, "Transitions 2-4");
                ui.checkbox(&mut s.cfg.show_ftb, "FTB");
                ui.checkbox(&mut s.cfg.show_tbar, "T-Bar");
                ui.checkbox(&mut s.cfg.show_overlays, "Overlay");
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
            ui.checkbox(&mut s.ftb_audio, "Fade To Black includes Audio");
            ui.checkbox(&mut s.pause_preview, "Automatically Pause Preview Input after Transition");
            ui.checkbox(&mut s.play_active, "Automatically Play Output Input with Transition");
            ui.checkbox(&mut s.restart_active, "Automatically Restart Output Input with Transition");
            ui.checkbox(&mut s.cfg.preview_click, "Activate QuickPlay by clicking Preview Window");
            ui.checkbox(&mut s.cfg.recording_confirm, "Display confirmation for Record, External, Stream and MultiCorder buttons");
            ui.checkbox(&mut s.cfg.input_close_confirm, "Display confirmation for Input Close button");
            ui.separator();
            ui.checkbox(&mut s.cfg.start_maximised, "Start Maximised");
            ui.checkbox(&mut s.cfg.start_fullscreen, "Start Fullscreen display");
            ui.checkbox(&mut s.cfg.start_external, "Start External Output");
        }
        3 => {
            // Performance (names from frmSettings: lblPreviewSkip, chkLockBackBuffer,
            // chkDisableInputEffects; hardware encoder = recording key "HardwareAcceleration").
            egui::Grid::new("set-perf").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
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
            ui.checkbox(&mut s.cfg.lock_refresh, "Synchronize Output with Refresh Rate (off: the window redraws at the Master Frame Rate)");
            ui.checkbox(&mut s.cfg.no_input_effects, "Disable Effects on Input Previews (input boxes skip position, crop and layers)");
            ui.checkbox(&mut s.cfg.hw_encoder, "Use Hardware Encoder for recordings (when available for the selected codec)");
            ui.separator();
            ui.label(format!("Graphics adapter: {graphics_adapter}"));
            ui.label(format!("Program capture: {program_path}"));
            ui.label(format!("Program encoder: {encoder}"));
            ui.add_space(4.0);
            ui.label(RichText::new("Lower camera modes (Input Settings > General) and a lower Output Size also save work.").size(11.0).color(TEXT_DIM));
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
            ui.label(RichText::new("Each output can open a separate local window. NDI and SRT transmission are unavailable in this build.").size(11.0).color(TEXT_DIM));
        }
        6 => {
            ui.label("These options apply to all four local External windows.");
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
            ui.checkbox(&mut s.cfg.ext_on_top, "On Top");
            ui.checkbox(&mut s.cfg.ext_hide_cursor, "Hide Cursor");
            ui.checkbox(&mut s.cfg.start_external, "Open Output 1 at startup");
            if ui.button("Configure Output 1–4…").clicked() {
                s.cfg.page = 1;
            }
        }
        7 => {
            // Audio
            ui.checkbox(&mut s.ftb_audio, "Fade To Black includes Audio");
            ui.checkbox(&mut s.cfg.master_meter, "Show Master Audio Meter in Main Window");
            ui.checkbox(&mut s.cfg.input_meter, "Show Audio Meter next to Inputs");
            ui.label(RichText::new("Real sound capture, devices, ASIO and VST are not available yet.").size(11.0).color(TEXT_DIM));
        }
        11 => {
            // Shortcuts
            ui.label("Keyboard shortcuts (the original's \"Default Keyboard\" template):");
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
            ui.checkbox(&mut s.cfg.recording_alert, "Display alert on UI when NOT Recording");
            ui.add_space(4.0);
            ui.label(RichText::new("Fade To Black Alert").strong());
            ui.checkbox(&mut s.cfg.ftb_alert, "Display alert on UI when Fade To Black (FTB) enabled");
        }
        15 => {
            ui.label(RichText::new("LiveMix").strong().size(16.0));
            ui.label("A live video production switcher.");
            ui.add_space(4.0);
            ui.label(format!("Inputs: {}", s.inputs.len()));
            ui.label(format!("Video devices: Media Foundation · Blackmagic driver {}", if media::blackmagic_driver_installed() { "installed" } else { "not found" }));
        }
        _ => {
            ui.label(RichText::new("This section of the original is not available in this version yet.").color(TEXT_DIM));
        }
    }
}

// ---------------------------------------------------------------- dialogs

/// Streaming destinations offered in Streaming Settings (frmStreaming "Destination"); each fills
/// in its usual RTMP address.
const STREAM_DESTS: [(&str, &str); 4] = [
    ("Custom RTMP Server", ""),
    ("YouTube", "rtmp://a.rtmp.youtube.com/live2"),
    ("Facebook", "rtmps://live-api-s.facebook.com:443/rtmp/"),
    ("Twitch", "rtmp://live.twitch.tv/app"),
];
const STREAM_QUALITIES: [&str; 3] = ["720p 3 Mbps", "1080p 6 Mbps", "1080p 9 Mbps"];

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

fn recording_format_picker(ui: &mut egui::Ui, id: &str, format: &mut media::RecordFormat) -> bool {
    let label = |format: media::RecordFormat| format!("{} ({})", format.extension().to_ascii_uppercase(), format.codec());
    let before = *format;
    egui::ComboBox::from_id_salt(id).selected_text(label(*format)).show_ui(ui, |ui| {
        for choice in media::RecordFormat::ALL {
            ui.selectable_value(format, choice, label(choice));
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
        egui::Window::new("Recording Setup").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            let tab_id = ui.id().with("recording-slot");
            let mut secondary = ui.data(|d| d.get_temp::<bool>(tab_id)).unwrap_or(false);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut secondary, false, "1");
                ui.selectable_value(&mut secondary, true, "2");
                if secondary {
                    changed |= ui.checkbox(&mut s.cfg.rec_second, "Enabled").changed();
                }
            });
            ui.data_mut(|d| d.insert_temp(tab_id, secondary));
            let output_names: Vec<_> = s.cfg.outputs.iter().enumerate()
                .map(|(n, route)| format!("Output {} · {}", n + 1, src_name(s, route.source))).collect();
            let name = safe_name(&s.cfg.rec_name);
            egui::Grid::new("rec-setup").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
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
                ui.label("Codec:");
                changed |= recording_format_picker(ui, "rec-format", codec);
                ui.end_row();
                ui.label("Bit Rate:");
                changed |= ui.add(egui::DragValue::new(mbps).range(1..=100).suffix(" Mbps")).changed();
                ui.end_row();
                ui.label("New File Every:");
                ui.horizontal(|ui| {
                    changed |= new_file_every(ui, "rec-restart", &mut s.cfg.rec_split_min);
                    ui.label("Minutes");
                });
                ui.end_row();
            });
            ui.checkbox(&mut s.cfg.recording_confirm, "Ask before stopping (Display confirmation for Record, External, Stream and MultiCorder buttons)");
            ui.label(RichText::new("Video only. HEVC requires a compatible Windows encoder. Both slots share the folder and split interval.").size(11.0).color(TEXT_DIM));
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
        egui::Window::new("Streaming Settings").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            egui::Grid::new("stream-setup").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Destination");
                let cur = (s.cfg.stream_dest as usize).min(STREAM_DESTS.len() - 1);
                egui::ComboBox::from_id_salt("stream-dest").width(240.0).selected_text(STREAM_DESTS[cur].0).show_ui(ui, |ui| {
                    for (k, (name, url)) in STREAM_DESTS.iter().enumerate() {
                        if ui.selectable_label(k == cur, *name).clicked() {
                            s.cfg.stream_dest = k as u8;
                            if !url.is_empty() {
                                s.cfg.stream_url = url.to_string();
                            }
                        }
                    }
                });
                ui.end_row();
                ui.label("URL");
                ui.add(egui::TextEdit::singleline(&mut s.cfg.stream_url).desired_width(320.0));
                ui.end_row();
                ui.label("Stream Name or Key");
                ui.add(egui::TextEdit::singleline(&mut s.cfg.stream_key).password(true).desired_width(320.0));
                ui.end_row();
                ui.label("Quality");
                let q = (s.cfg.stream_quality as usize).min(STREAM_QUALITIES.len() - 1);
                egui::ComboBox::from_id_salt("stream-q").width(240.0).selected_text(STREAM_QUALITIES[q]).show_ui(ui, |ui| {
                    for (k, name) in STREAM_QUALITIES.iter().enumerate() {
                        if ui.selectable_label(k == q, *name).clicked() {
                            s.cfg.stream_quality = k as u8;
                        }
                    }
                });
                ui.end_row();
            });
            ui.label(RichText::new("Sending the stream is not built yet in this version; these settings are saved for it.").size(11.0).color(TEXT_DIM));
            if ui.button("Save and Close").clicked() {
                s.stream_setup = false;
            }
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
        egui::Window::new("MultiCorder").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label("Each selected camera or Output is recorded to its own file.");
            ui.add_space(4.0);
            let cams: Vec<(u32, String)> = s.inputs.iter().filter(|i| matches!(i.kind, Kind::Device { .. })).map(|i| (i.id, format!("{} {}", s.number_of(i.id), i.title))).collect();
            for channel in 0..OUTPUT_COUNT {
                let source = src_name(s, s.cfg.outputs[channel].source);
                changed |= ui.checkbox(&mut s.cfg.mc_outputs[channel], format!("Output {} · {source}", channel + 1)).changed();
            }
            for (id, name) in cams {
                let mut on = !s.mc_off.contains(&id);
                if ui.checkbox(&mut on, name).changed() {
                    if on {
                        s.mc_off.remove(&id);
                    } else {
                        s.mc_off.insert(id);
                    }
                }
            }
            ui.separator();
            egui::Grid::new("mc-setup").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Folder:");
                ui.horizontal(|ui| {
                    folder_line(ui, &s.cfg.mc_folder);
                    if ui.button("Change Recording Folder").clicked() {
                        s.browse_for = 2;
                        s.browse_req = Some(media::PickKind::Folder);
                    }
                });
                ui.end_row();
                ui.label("Codec:");
                changed |= recording_format_picker(ui, "mc-format", &mut s.cfg.mc_codec);
                ui.end_row();
                ui.label("Video Bit Rate:");
                ui.add(egui::DragValue::new(&mut s.cfg.mc_mbps).range(1..=100).suffix(" Mbps"));
                ui.end_row();
                ui.label("New File Every:");
                ui.horizontal(|ui| {
                    new_file_every(ui, "mc-restart", &mut s.cfg.mc_split_min);
                    ui.label("Minutes");
                });
                ui.end_row();
            });
            ui.horizontal(|ui| {
                let running = s.multicorder.is_some();
                if ui.button(if running { "Stop" } else { "Start" }).clicked() {
                    s.multicorder = if running { None } else { Some(t) };
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
        NewStep::Idle => {}
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
                    s.new_step = if s.save_preset() { NewStep::Format } else { NewStep::Idle };
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

fn dialogs(ctx: &egui::Context, s: &mut Switcher, graphics_adapter: &str, encoder: &str, program_path: &str) {
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
        egui::Window::new("Input Select").open(&mut open).collapsible(false).resizable(false).default_width(460.0).show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    // The original's input list (frmInput.tMenu), then this app's extras.
                    ui.set_width(170.0);
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
                        ui.selectable_value(&mut s.add_dialog.kind, k, label);
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    ui.set_width(290.0);
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
                            ui.label(RichText::new("Press OK to capture the main screen.").size(11.0).color(TEXT_DIM));
                        }
                        AddKind::TestVideo => {
                            ui.horizontal(|ui| {
                                ui.label("Length (s)");
                                ui.add(egui::DragValue::new(&mut s.add_dialog.length).range(1.0..=3600.0));
                            });
                        }
                        AddKind::Camera | AddKind::Blackmagic => {
                            let list = s.dialog_devices();
                            if s.add_dialog.kind == AddKind::Blackmagic {
                                let ok = media::blackmagic_driver_installed();
                                ui.label(
                                    RichText::new(if ok { "Blackmagic Desktop Video driver: installed" } else { "Blackmagic Desktop Video driver: not found (needed for DeckLink / Intensity / UltraStudio)" })
                                        .size(11.0)
                                        .color(if ok { Color32::from_rgb(0x7c, 0xe0, 0x8a) } else { Color32::from_rgb(0xff, 0xc8, 0x60) }),
                                );
                            }
                            ui.horizontal(|ui| {
                                ui.label("Device");
                                let sel = list.get(s.add_dialog.device).map(|d| d.name.clone()).unwrap_or_else(|| "— none found —".into());
                                egui::ComboBox::from_id_salt("device").width(200.0).selected_text(sel).show_ui(ui, |ui| {
                                    for (k, d) in list.iter().enumerate() {
                                        let label = if d.blackmagic { format!("{}  (Blackmagic)", d.name) } else { d.name.clone() };
                                        ui.selectable_value(&mut s.add_dialog.device, k, label);
                                    }
                                });
                                if ui.button("Refresh").clicked() {
                                    s.refresh_devices = true;
                                }
                            });
                            if s.add_dialog.devices_loaded {
                                ui.label(RichText::new(format!("{} device(s) found on this computer", list.len())).size(11.0).color(TEXT_DIM));
                            }
                        }
                        AddKind::VideoFile | AddKind::ImageFile => {
                            ui.horizontal(|ui| {
                                ui.label("File");
                                ui.add(egui::TextEdit::singleline(&mut s.add_dialog.path).desired_width(190.0));
                                if ui.button("Browse…").clicked() {
                                    s.browse_req = Some(if s.add_dialog.kind == AddKind::VideoFile { media::PickKind::Video } else { media::PickKind::Image });
                                }
                            });
                        }
                        _ => {}
                    }
                    if !s.dialog_error.is_empty() {
                        ui.label(RichText::new(&s.dialog_error).size(11.0).color(Color32::from_rgb(0xff, 0x9a, 0x9a)));
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(s.add_dialog.kind.available(), egui::Button::new("  OK  ")).clicked() {
                            done = true;
                        }
                        if ui.button("Cancel").clicked() {
                            s.add_dialog.open = false;
                            s.dialog_error.clear();
                        }
                    });
                });
            });
        });
        if done {
            match s.add_from_dialog() {
                Ok(()) => {
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
        // Other inputs, for the Layers section (number, id, title).
        let others: Vec<(usize, u32, String)> =
            s.inputs.iter().enumerate().filter(|(_, i)| i.id != id).map(|(n, i)| (n + 1, i.id, i.title.clone())).collect();
        let number = s.number_of(id);
        let mut page = s.cfg.input_page.min(INPUT_PAGES.len() - 1);
        let clip = s.position_clipboard;
        let mut copy_pos = None;
        if let Some(inp) = s.input_mut(id) {
            egui::Window::new(format!("Input Settings — {} {}", number, inp.title))
                .id(egui::Id::new("input-settings"))
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(640.0)
                .show(ctx, |ui| {
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(140.0);
                            for (k, name) in INPUT_PAGES.iter().enumerate() {
                                let built = INPUT_PAGES_BUILT.contains(&k);
                                let label = if built { RichText::new(*name) } else { RichText::new(*name).color(TEXT_DIM.gamma_multiply(0.7)).italics() };
                                ui.selectable_value(&mut page, k, label);
                            }
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            ui.set_width(460.0);
                            ui.set_min_height(360.0);
                            ui.label(RichText::new(INPUT_PAGES[page]).strong().size(15.0).color(Color32::WHITE));
                            ui.add_space(6.0);
                            match page {
                                0 => input_general(ui, inp),
                                2 => input_colour_key(ui, inp),
                                5 => {
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
                                6 => layers_editor(ui, inp, &others),
                                _ => {
                                    ui.label(RichText::new("This section of the original is not available in this version yet.").color(TEXT_DIM));
                                }
                            }
                        });
                    });
                });
        } else {
            open = false;
        }
        s.cfg.input_page = page;
        if let Some(p) = copy_pos {
            s.position_clipboard = Some(p);
        }
        if !open {
            s.settings_input = None;
        }
    }

    if s.settings_open {
        let mut open = true;
        let mut close = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(720.0)
            .show(ctx, |ui| {
                ui.horizontal_top(|ui| {
                    // The original's sections (frmSettings.tMenu.*).
                    ui.vertical(|ui| {
                        ui.set_width(160.0);
                        for (k, name) in SETTINGS_PAGES.iter().enumerate() {
                            let built = SETTINGS_BUILT.contains(&k);
                            let label = if built { RichText::new(*name) } else { RichText::new(*name).color(TEXT_DIM.gamma_multiply(0.7)).italics() };
                            ui.selectable_value(&mut s.cfg.page, k, label);
                        }
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.set_width(520.0);
                        ui.set_min_height(380.0);
                        egui::ScrollArea::vertical().id_salt("settings-page").max_height(620.0).show(ui, |ui| {
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

fn tiny(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let b = egui::Button::new(RichText::new(text).size(11.0).color(if active { Color32::BLACK } else { TEXT }))
        .fill(if active { Color32::from_rgb(0x50, 0x9a, 0xe0) } else { BTN_DARK })
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(22.0, 22.0));
    ui.add(b)
}

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
        s.input_mut(a).unwrap().layers[0] = Layer { on: true, input: Some(b), place: Position::default() };
        s.remove_input(b);
        assert_eq!(s.input(a).unwrap().layers[0], Layer::default());
    }

    #[test]
    fn overlay_reassignment_waits_for_the_next_activation() {
        let mut s = Switcher::demo();
        let (a, b) = (s.inputs[0].id, s.inputs[1].id);
        s.assign_overlay(1, a, false);
        s.toggle_overlay_channel(1);
        s.assign_overlay(1, b, false);
        assert_eq!(s.overlays[1].input, Some(a));
        assert!(s.overlays[1].on);
        s.toggle_overlay_channel(1);
        s.toggle_overlay_channel(1);
        assert_eq!(s.overlays[1].input, Some(b));
    }
}
