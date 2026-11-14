//! Application start-up, the media update loop, frame completion and UI requests.
use super::*;

fn thumbnail_image(frame: &media::Frame, [max_w, max_h]: [usize; 2]) -> egui::ColorImage {
    let ratio = (max_w as f64 / frame.w as f64).min(max_h as f64 / frame.h as f64).min(1.0);
    let w = ((frame.w as f64 * ratio).round() as usize).max(1);
    let h = ((frame.h as f64 * ratio).round() as usize).max(1);
    let pixels = (0..h).flat_map(|y| {
        (0..w).map(move |x| {
            let offset = ((2 * y + 1) * frame.h / (2 * h) * frame.w
                + (2 * x + 1) * frame.w / (2 * w)) * 4;
            let p = &frame.rgba[offset..offset + 4];
            Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3])
        })
    }).collect();
    egui::ColorImage { size: [w, h], pixels }
}

fn update_video_texture(texture: &mut egui::TextureHandle, image: egui::ColorImage) {
    // egui_glow uses glTexSubImage2D for partial updates and glTexImage2D for full ones.
    let options = egui::TextureOptions::LINEAR;
    if texture.size() == image.size {
        texture.set_partial([0, 0], image, options);
    } else {
        texture.set(image, options);
    }
}

#[test]
fn tile_thumbnail_preserves_aspect_and_alpha() {
    let rgba = [255, 0, 0, 128].repeat(8 * 4);
    let frame = media::Frame { w: 8, h: 4, rgba };
    let thumbnail = thumbnail_image(&frame, [4, 4]);
    assert_eq!(thumbnail.size, [4, 2]);
    assert!(thumbnail.pixels.iter().all(|pixel| *pixel == Color32::from_rgba_unmultiplied(255, 0, 0, 128)));
}

impl App {
    pub(crate) fn refresh_external_devices(ctx: egui::Context) {
        std::thread::spawn(move || {
            decklink::refresh_devices();
            ctx.data_mut(|data| {
                let generation = egui::Id::new("decklink-device-generation");
                let next = data.get_temp::<u64>(generation).unwrap_or(0).wrapping_add(1);
                data.insert_temp(generation, next);
            });
            ctx.request_repaint();
        });
    }
    pub(crate) fn new(screenshot: Option<String>) -> Self {
        let mut s = Switcher::demo();
        // Settings window options saved last time (defaults are the original's config defaults).
        s.load_settings();
        if screenshot.is_some() {
            if let Ok(language) = std::env::var("LIVEMIX_LANGUAGE") {
                s.cfg.language = locale::Language::from_code(&language);
            }
        }
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
            "settings" => {
                s.settings_open = true;
                if let Some(page) = std::env::var("LIVEMIX_SETTINGS_PAGE").ok().and_then(|v| v.parse().ok()) { s.cfg.page = page; }
            }
            "decoders" => {
                s.settings_open = true;
                s.cfg.page = 4;
            }
            "recording" => s.rec_setup = true,
            "shortcuts" => {
                s.settings_open = true;
                s.cfg.page = 11;
            }
            "replay-strip" => { s.ensure_replay_inputs(); }
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
                s.cfg.wave_type = screenshot.as_ref().and_then(|_| std::env::var("LIVEMIX_SCOPE_MODE").ok())
                    .and_then(|mode| mode.parse().ok()).unwrap_or(60);
            }
            "studio" if screenshot.is_some() => {
                s.external[0].open = false;
                s.cfg.safe_type = 0;
                s.cfg.wave_type = 0;
                for overlay in &mut s.overlays { overlay.on = false; overlay.level = 0.0; }
                s.add_dialog.kind = AddKind::VirtualSet;
                if s.add_from_dialog().is_ok() {
                    let presenters = [s.inputs[1].id, s.inputs[2].id];
                    if let Some(set) = s.inputs.last_mut() {
                        for (layer, presenter) in set.layers[1..=2].iter_mut().zip(presenters) {
                            layer.input = Some(presenter);
                            layer.on = true;
                        }
                        s.preview = Some(set.id);
                    }
                }
                if let Ok(path) = std::env::var("LIVEMIX_TEST_PRESENTATION") {
                    let input = Input::new(0, "Presentation", Kind::Presentation { path });
                    s.output = Some(s.add(input));
                }
            }
            "gpu-input" if screenshot.is_some() => {
                if let Ok(path) = std::env::var("LIVEMIX_TEST_VIDEO") {
                    s.cfg.ffmpeg_all_files = false;
                    s.cfg.ffmpeg_extensions.clear();
                    s.cfg.directshow.enabled = false;
                    s.cfg.video_adapter = std::env::var("LIVEMIX_TEST_ADAPTER").unwrap_or_default();
                    s.external[0].open = false;
                    let mut input = Input::new(0, "GPU effects fixture", Kind::VideoFile { path });
                    input.key = Key { on: true, colour: [0, 255, 0], tolerance: 45 };
                    input.colour_adjust.green = 0.75;
                    if let Ok(path) = std::env::var("LIVEMIX_TEST_LUT") {
                        if let Err(error) = input.colour_fx.load(std::path::Path::new(&path)) { eprintln!("{error}"); }
                        input.colour_fx.luma.enabled = true;
                        input.colour_fx.luma.threshold = 24;
                    }
                    s.preview = Some(s.add(input));
                }
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
            "replay-advanced" if screenshot.is_some() => {
                s.ensure_replay_inputs();
                s.replay_open = true;
                s.replay_setup.show_live_sources = true;
                s.replay_setup.advanced = true;
                s.replay_angles[0] = s.inputs[1].id + 1;
                s.replay_angles[1] = s.inputs[2].id + 1;
                s.replay_cameras = [0, 1];
                // Synthetic highlights, tags and a playlist for the controller screenshot.
                for (index, tag) in ["Goal", "Foul", "Penalty", "Reacts", "Half Time", "Injury"].into_iter().enumerate() {
                    s.replay_ui.tags[index] = tag.into();
                }
                for (id, name) in ["Goal 11", "Foul 07", "Goal reacts"].into_iter().enumerate() {
                    let start = 60_000 + id as u64 * 95_000;
                    let mut event = replay::Event { id: id as u64 + 1, start, end: start + 8_000 + id as u64 * 2_000,
                        description: name.into(), speed: 100, ..Default::default() };
                    event.angles[..2].fill(true);
                    event.selected[0] = id != 1;
                    event.favourite[1] = id == 1;
                    s.replay_ui.playlists[0].push(replay::Clip { id: 10 + id as u64, angle: id % 2, comment: name.into(),
                        start: event.start, end: event.end, speed: if id == 2 { 50 } else { 100 }, mix_ms: 400 });
                    s.replay_events.insert(0, event);
                }
                s.replay_event = Some(3);
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
            last_autosave: LastAutosave::new(&s, screenshot.is_none()),
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
            decklink_settings_visible: false,
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
            multi_audio: Default::default(),
            group_audio: Vec::new(),
            window_audio: Default::default(),
            scopes: Default::default(),
            next_wave: 0.0,
            scope_frame: Default::default(),
            scope_key: (None, 0, false),
            scope_jobs,
            scope_images,
            controls: Default::default(),
        }
    }

    /// Starts/stops camera and file readers and turns their frames into pictures.
    pub(crate) fn update_media(&mut self, ctx: &egui::Context, video_due: bool) {
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
                    || self.s.srt_active[*channel] || self.s.decklink_active[*channel]
            }).map(|(_, route)| route.source))
            .chain(self.s.cfg.streams.iter().zip(&self.s.stream_active).filter(|(_, active)| **active)
                .map(|(stream, _)| stream_route(&self.s, stream).source))
            .collect();
        #[cfg(windows)]
        let pending_gpu = &mut self.pending_gpu;
        let App { s, media: feeds, delays, multi, multi_started, multi_audio, .. } = self;
        let ids: std::collections::HashSet<u32> = s.inputs.iter().map(|i| i.id).collect();
        let mut audio_history = std::collections::HashMap::<u32, f32>::new();
        for input in &s.inputs {
            if let Kind::VideoDelay { source: Some(source), seconds, .. } = input.kind {
                audio_history.entry(source).and_modify(|current| *current = current.max(seconds)).or_insert(seconds);
            }
        }
        #[cfg(windows)]
        let delay_sources: std::collections::HashSet<u32> = s.inputs.iter().filter_map(|input| match input.kind {
            Kind::VideoDelay { source, .. } => source,
            _ => None,
        }).collect();
        feeds.retain(|id, _| ids.contains(id));
        let multicorder = s.multicorder.is_some();
        let mc_split = s.cfg.mc_split_min as f64 * 60.0;
        if !multicorder {
            multi.clear();
            multi_started.clear();
            multi_audio.clear();
        } else {
            // Inputs ticked off in the MultiCorder window stop recording; "New File Every"
            // closes a file once it is old enough (a new one starts with the next frame).
            multi.retain(|id, _| ids.contains(id) && !s.mc_off.contains(id) && !(mc_split > 0.0 && now - multi_started.get(id).copied().unwrap_or(now) >= mc_split));
            multi_audio.retain(|id, _| multi.contains_key(id));
        }
        let (mc_dir, mc_mbps, mc_fps, mc_hw, mc_format, mc_audio) = (
            s.cfg.mc_folder.clone(), s.cfg.mc_mbps, s.cfg.frame_rate(), s.cfg.hw_encoder, s.cfg.mc_codec, s.cfg.record_audio[2],
        );
        // Inputs seen big right now (Preview, Output, transitions, overlays and their layers).
        let skip = u64::from(s.cfg.preview_skip.max(1));
        let ui_frame = s.ui_frame;
        let thumbnail_size = [s.cfg.input_w, s.cfg.input_h]
            .map(|points| (points * ctx.pixels_per_point()).ceil().max(1.0) as usize);
        let mut live: std::collections::HashSet<u32> = [s.preview, s.output].into_iter().flatten().collect();
        live.extend(s.input_preview);
        if s.replay_arm || s.replay_open {
            live.extend(s.replay_angles.into_iter().filter_map(|source| source.checked_sub(1))
                .filter(|id| s.input(*id).is_some()));
        }
        if let Some(tr) = &s.trans {
            live.insert(tr.to);
            live.extend(tr.from);
            live.extend(tr.stinger.as_ref().map(|stinger| stinger.input));
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
        // The renderer draws layers and their children (depth two).
        let mut layer_parents: Vec<u32> = live.iter().copied().collect();
        for _ in 0..2 {
            let children: Vec<u32> = layer_parents.iter().filter_map(|id| s.input(*id))
                .flat_map(|input| input.layers.iter().filter(|layer| layer.on).filter_map(|layer| layer.input))
                .filter(|id| live.insert(*id)).collect();
            layer_parents = children;
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
                            media::Source::Device(link.clone(), inp.cam_mode.clone(), inp.cam_audio.clone()),
                            format!("{link}|{}|{}|{}", inp.cam_mode.as_ref().map(|m| m.code()).unwrap_or_default(), s.cfg.low_latency_capture,
                                inp.cam_audio.as_deref().unwrap_or_default()),
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
                        Kind::Web { url, transparent } => (
                            media::Source::Web(url.clone(), *transparent), format!("web:{transparent}:{url}"), false, false),
                        Kind::Presentation { path } => (media::Source::Presentation(path.clone()), format!("slides:{path}"), true, false),
                        Kind::ImageSequence { folder } => (media::Source::Sequence(folder.clone(), output_fps()), folder.clone(), true, false),
                        Kind::Photos { folder } => (media::Source::Photos(folder.clone(), PHOTO_SECS), folder.clone(), true, false),
                        _ => continue,
                    };
                    let is_list = matches!(inp.kind, Kind::VideoList { .. });
                    let reload_page = matches!(inp.kind, Kind::Web { .. }) && std::mem::take(&mut inp.restart_req);
                    if feeds.get(&inp.id).is_some_and(|f| f.key != key || reload_page) {
                        // keep the last picture while the next list item starts
                        let (old_tex, thumbnail_source) = feeds.remove(&inp.id).map(|mut feed| {
                            #[cfg(windows)]
                            if feed.gpu_size.is_some() { return (None, None); }
                            (feed.tex.take(), feed.thumbnail_source.take())
                        }).unwrap_or_default();
                        feeds.insert(inp.id, Feed { shared: Some(media::start_for_gl(src.clone(), is_file && !inp.playing)),
                            tex: old_tex, thumbnail_source, seen: 0, recorded: 0, key: key.clone(), prev_tex: None, fade_start: 0.0,
                            colour_adjust: Default::default(),
                            colour_fx: Default::default(),
                            #[cfg(windows)]
                            gpu_size: None,
                        });
                    }
                    let feed = feeds.entry(inp.id).or_insert_with(|| Feed { shared: Some(media::start_for_gl(src, is_file && !inp.playing)),
                        tex: None, thumbnail_source: None, seen: 0, recorded: 0, key, prev_tex: None, fade_start: 0.0,
                        colour_adjust: Default::default(),
                        colour_fx: Default::default(),
                        #[cfg(windows)]
                        gpu_size: None,
                    });
                    let Some(sh) = feed.shared.clone() else { continue };
                    if let Ok(mut buffer) = sh.audio.try_lock() {
                        buffer.retain_for_delay(audio_history.get(&inp.id).copied().unwrap_or(0.0));
                    }
                    #[cfg(windows)]
                    let delay_changed = {
                        let required = delay_sources.contains(&inp.id) || inp.colour_fx.needs_cpu();
                        sh.cpu_frames_required.swap(required, std::sync::atomic::Ordering::AcqRel) != required
                    };
                    #[cfg(not(windows))]
                    let delay_changed = false;
                    let k = inp.key;
                    let key = if k.on { (1u64 << 40) | ((k.tolerance as u64) << 24) | ((k.colour[0] as u64) << 16) | ((k.colour[1] as u64) << 8) | k.colour[2] as u64 } else { 0 };
                    let key_changed = sh.key.swap(key, Relaxed) != key;
                    if matches!(inp.kind, Kind::Web { .. }) {
                        sh.browser_fps.store(if live.contains(&inp.id) { 15 } else { 2 }, Relaxed);
                    }
                    if feed.colour_adjust != inp.colour_adjust || feed.colour_fx != inp.colour_fx || key_changed || delay_changed {
                        feed.colour_adjust = inp.colour_adjust;
                        feed.colour_fx = inp.colour_fx.clone();
                        if is_file && inp.kind.supports_picture_fx() && !inp.playing && inp.seek_ms.is_none() {
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
                    inp.status = sh.status.lock().map(|x| x.clone()).unwrap_or_default();
                    if let Ok(decoder) = sh.decoder.try_lock() {
                        if inp.decoder != *decoder { inp.decoder.clone_from(&decoder); }
                    }
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
                            let nominal = (f64::from(audio::RATE) / f64::from(mc_fps)).round() as usize;
                            let (end, count) = multi_audio.entry(inp.id).or_default().window(at, nominal);
                            let packet = audio::mix([(&*sh, false, 1.0)], end, count);
                            rec.push_audio(rgba, w, h, at, packet);
                        }
                    }
                    // Preview Skip counts project pictures, even during rapid UI events.
                    let grade_only = !live.contains(&inp.id) && !recording_this;
                    let skip_now = feed.tex.is_some() && skip > 1 && grade_only && ui_frame % skip != 0;
                    if video_due && seq != feed.seen && !skip_now {
                        feed.seen = seq;
                        #[cfg(windows)]
                        if let Some(picture) = sh.gpu_frame.lock().unwrap().take()
                        {
                            feed.thumbnail_source = None;
                            let resize = feed.gpu_size != Some(picture.size);
                            if feed.gpu_size.is_none() {
                                feed.tex = Some(ctx.load_texture(format!("gpu-input-{}", inp.id),
                                    egui::ColorImage::new([1, 1], Color32::BLACK), egui::TextureOptions::LINEAR));
                            }
                            feed.gpu_size = Some(picture.size);
                            if let Some(texture) = &feed.tex {
                                pending_gpu.push(GpuUpload { picture, texture: texture.clone(), resize, source: sh.clone(),
                                    effects: dx_interop::Effects { key, colour: inp.colour_adjust, extra: inp.colour_fx.clone() } });
                            }
                        }
                        let fr = sh.frame.lock().ok().and_then(|mut f| f.take());
                        if let Some(mut fr) = fr {
                            inp.colour_fx.luma.apply(&mut fr.rgba);
                            inp.colour_adjust.apply(&mut fr.rgba);
                            inp.colour_fx.apply_lut(&mut fr.rgba);
                            inp.colour_fx.apply_spatial(&mut fr.rgba, fr.w, fr.h);
                            // Video Delay inputs that copy this one take the new picture.
                            for delay in delays.values_mut().filter(|delay| delay.source == inp.id) {
                                delay.capture(&fr.rgba, fr.w, fr.h, now);
                            }
                            #[cfg(windows)]
                            if feed.gpu_size.take().is_some() { feed.tex = None; }
                            let photos = matches!(inp.kind, Kind::Photos { .. });
                            let img = if grade_only && !photos && (fr.w > thumbnail_size[0] || fr.h > thumbnail_size[1]) {
                                let image = thumbnail_image(&fr, thumbnail_size);
                                feed.thumbnail_source = Some(fr);
                                image
                            } else {
                                feed.thumbnail_source = None;
                                egui::ColorImage::from_rgba_unmultiplied([fr.w, fr.h], &fr.rgba)
                            };
                            if photos && feed.tex.is_some() {
                                // Keep the old picture and fade the new one in over it.
                                feed.prev_tex = feed.tex.take();
                                feed.fade_start = now;
                                feed.tex = Some(ctx.load_texture(format!("input-{}-{}", inp.id, seq), img, egui::TextureOptions::LINEAR));
                            } else {
                                match &mut feed.tex {
                                    Some(t) => update_video_texture(t, img),
                                    None => feed.tex = Some(ctx.load_texture(format!("input-{}", inp.id), img, egui::TextureOptions::LINEAR)),
                                }
                            }
                        }
                    }
                    // If a new picture arrived, it already replaced the retained frame.
                    // Otherwise promote the retained picture, including for paused clips.
                    if !grade_only {
                        if let (Some(frame), Some(tex)) = (feed.thumbnail_source.take(), &mut feed.tex) {
                            update_video_texture(tex,
                                egui::ColorImage::from_rgba_unmultiplied([frame.w, frame.h], &frame.rgba));
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
                    if feeds.get(&inp.id).is_none_or(|feed| feed.colour_adjust != inp.colour_adjust || feed.colour_fx != inp.colour_fx) {
                        let mut f = Feed { shared: None, tex: None, thumbnail_source: None, seen: 0, recorded: 0, key: String::new(), prev_tex: None, fade_start: 0.0,
                            colour_adjust: inp.colour_adjust,
                            colour_fx: inp.colour_fx.clone(),
                            #[cfg(windows)]
                            gpu_size: None,
                        };
                        match image::open(path) {
                            Ok(im) => {
                                let mut im = im.to_rgba8();
                                inp.colour_fx.luma.apply(im.as_mut());
                                inp.colour_adjust.apply(im.as_mut());
                                inp.colour_fx.apply_lut(im.as_mut());
                                let (width, height) = im.dimensions();
                                inp.colour_fx.apply_spatial(im.as_mut(), width as usize, height as usize);
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
            let source = source.unwrap_or(0);
            if feed.size != size || feed.source != source {
                feed.size = size;
                feed.frames.clear();
                feed.last = -1.0;
                feed.tex = None;
            }
            feed.seconds = seconds;
            feed.source = source;
            match feed.ready(now) {
                Some(time) => {
                    if time != feed.last {
                        feed.last = time;
                        let img = {
                            let frame = &feed.frames[0];
                            egui::ColorImage::from_rgba_unmultiplied([frame.2, frame.3], &frame.1)
                        };
                        match &mut feed.tex {
                            Some(texture) => update_video_texture(texture, img),
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

    /// Capture the Program when due and record the elapsed time at the end of drawing.
    pub(crate) fn finish_frame(&mut self, ctx: &egui::Context, now: f64, started: Instant) {
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
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, destinations)| *existing == route && *audio == audio_source && !destinations.key_fill) {
                    destinations.recorders.push(Arc::clone(&recording.recorder));
                } else {
                    groups.push((route, audio_source, program::Destinations { recorders: vec![Arc::clone(&recording.recorder)], ..Default::default() }));
                }
            }
            for (channel, (config, sink)) in self.decklink_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))) {
                let route = self.s.cfg.outputs[channel];
                if config.key_fill {
                    groups.push((route, route.source, program::Destinations {
                        key_fill: true, decklinks: vec![(channel, sink.clone(), Default::default())], ..Default::default()
                    }));
                    continue;
                }
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, destinations)| *existing == route && *audio == route.source && !destinations.key_fill) {
                    destinations.decklinks.push((channel, sink.clone(), Default::default()));
                } else {
                    groups.push((route, route.source, program::Destinations { decklinks: vec![(channel, sink.clone(), Default::default())], ..Default::default() }));
                }
            }
            for (channel, (_, sink)) in self.ndi_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))).filter(|(_, (_, sink))| sink.subscribed()) {
                let route = self.s.cfg.outputs[channel];
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, destinations)| *existing == route && *audio == route.source && !destinations.key_fill) {
                    destinations.ndi.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { ndi: vec![sink.clone()], ..Default::default() }));
                }
            }
            for (channel, (_, sink)) in self.omt_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))).filter(|(_, (_, sink))| sink.subscribed()) {
                let route = self.s.cfg.outputs[channel];
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, destinations)| *existing == route && *audio == route.source && !destinations.key_fill) {
                    destinations.omt.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { omt: vec![sink.clone()], ..Default::default() }));
                }
            }
            for (channel, (_, sink)) in self.srt_outputs.iter().enumerate().filter_map(|(channel, sink)| sink.as_ref().map(|sink| (channel, sink))) {
                let route = self.s.cfg.outputs[channel];
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, destinations)| *existing == route && *audio == route.source && !destinations.key_fill) {
                    destinations.streams.push(sink.clone());
                } else {
                    groups.push((route, route.source, program::Destinations { streams: vec![sink.clone()], ..Default::default() }));
                }
            }
            for (index, (_, _, sink)) in self.stream_outputs.iter().enumerate().filter_map(|(index, sink)| sink.as_ref().map(|sink| (index, sink))) {
                let destination = &self.s.cfg.streams[index];
                let route = stream_route(&self.s, destination);
                let audio_source = if destination.source_audio { route.source } else { SRC_OUTPUT };
                if let Some((_, _, destinations)) = groups.iter_mut().find(|(existing, audio, destinations)| *existing == route && *audio == audio_source && !destinations.key_fill) {
                    destinations.streams.push(sink.clone());
                } else {
                    groups.push((route, audio_source, program::Destinations { streams: vec![sink.clone()], ..Default::default() }));
                }
            }
            let captured_at = Instant::now();
            let nominal = (f64::from(audio::RATE) / self.record_format.1).round() as usize;
            // Groups that ended lose their cursor, so a new output starts a new run.
            self.group_audio.retain(|(key, _)| groups.iter().any(|(route, audio, _)| (*route, *audio) == *key));
            for (route, audio_source, mut recorders) in groups {
                let cursor = match self.group_audio.iter().position(|(key, _)| *key == (route, audio_source)) {
                    Some(index) => &mut self.group_audio[index].1,
                    None => {
                        self.group_audio.push(((route, audio_source), audio::AudioCursor::default()));
                        &mut self.group_audio.last_mut().unwrap().1
                    }
                };
                let (end, count) = cursor.window(captured_at, nominal);
                if !recorders.recorders.is_empty() || !recorders.ndi.is_empty() || !recorders.omt.is_empty()
                    || !recorders.streams.is_empty()
                {
                    recorders.audio = self.mix_audio(audio_source, end, count);
                }
                for (channel, _, packet) in &mut recorders.decklinks {
                    if recorders.key_fill { continue; }
                    let config = self.s.cfg.decklink_outputs[*channel];
                    *packet = self.mix_external_audio(config.audio_bus, config.audio_delay_ms, end, count);
                }
                let painter = egui::Painter::new(ctx.clone(), layer, rect);
                if recorders.key_fill {
                    draw_overlay_channels(&painter, rect, &self.s, now, false);
                } else {
                    draw_src(&painter, rect, &self.s, now, route.source, route.overlays);
                }
                let shapes = ctx.graphics_mut(|graphics| {
                    std::mem::take(graphics.entry(layer)).all_entries().cloned().collect()
                });
                frames.push((size, ctx.tessellate(shapes, 1.0), captured_at, recorders));
            }
        }
        if scopes_available && self.scope_key.1 != 0 && now >= self.next_wave {
            self.next_wave = now + 0.1;
            let size = [640, 360];
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
                scope: Some((self.scope_key, Arc::clone(&self.scope_frame))),
                scope_textures: self.scopes.iter().map(|(&kind, texture)| (kind, texture.clone())).collect(),
                ..Default::default()
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
                            let result = bridge.as_mut().map_err(|error| error.clone()).and_then(|bridge| {
                                let texture = painter.texture(upload.texture.id()).ok_or("OpenGL input texture unavailable")?;
                                unsafe { bridge.import(painter.gl(), &upload.picture, texture, upload.resize, &upload.effects) }
                            });
                            if let Err(error) = result {
                                eprintln!("D3D11 input fallback: {error}");
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
    pub(crate) fn handle_screenshots(&mut self, ctx: &egui::Context, now: f64) {
        let slides_loading = self.screenshot.is_some() && now < 15.0 && self.s.inputs.iter().any(|input| {
            matches!(input.kind, Kind::Presentation { .. }) && input.tex.is_none()
        });
        let check_due = self.screenshot.is_some() && self.frame >= 120 && !slides_loading;
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
                let nominal = (f64::from(audio::RATE) / self.record_format.1).round() as usize;
                let (end, count) = self.window_audio.window(at, nominal);
                let audio = self.mix_audio(SRC_OUTPUT, end, count);
                media::Recorder::push_group_audio(&recorders, px, w, h, at, false, audio);
            }
        }
        if check_due {
            if let Some(path) = self.screenshot.clone() {
                if let Some(target) = &self.program.lock().unwrap().scope_target {
                    eprintln!("Scopes: {}", target.scope_backend());
                }
                if let Some(input) = self.s.preview.and_then(|id| self.s.input(id)).filter(|input| !input.decoder.is_empty()) {
                    eprintln!("Preview decoder: {}", input.decoder);
                }
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
    pub(crate) fn handle_requests(&mut self, ctx: &egui::Context) {
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
                        (media::PickKind::Lut(id), _) => {
                            if let Some(input) = self.s.input_mut(id) {
                                let result = input.colour_fx.load(std::path::Path::new(&p));
                                let field = egui::Id::new(("lut-path", id));
                                ctx.data_mut(|data| {
                                    data.insert_temp(field, p);
                                    data.insert_temp(field.with("error"), result.err().unwrap_or_default());
                                });
                            }
                        }
                        (media::PickKind::Script(target), _) => match model::read_script(std::path::Path::new(&p)) {
                            Ok(text) => match target {
                                None => self.s.add_dialog.script = text,
                                Some(id) => {
                                    if let Some(input) = self.s.input_mut(id) {
                                        if let Kind::Teleprompter(cfg) = &mut input.kind {
                                            cfg.text = text;
                                            input.pos = 0.0;
                                        }
                                    }
                                }
                            },
                            Err(error) => self.s.notice(&error),
                        },
                        (media::PickKind::SettingsOpen, _) => {
                            let result = self.s.import_settings_from(std::path::Path::new(&p));
                            self.s.notice(&result.unwrap_or_else(|error| error));
                        }
                        (media::PickKind::SettingsSave, _) => {
                            let result = self.s.export_settings_to(std::path::Path::new(&p));
                            self.s.notice(&result.map_or_else(|error| error, |_| "Settings exported without stream credentials".into()));
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
