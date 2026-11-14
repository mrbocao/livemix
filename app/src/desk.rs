//! The switching desk: top and bottom bars, monitors, transitions, MultiView and output windows.
use super::*;

// ---------------------------------------------------------------- top bar

pub(crate) fn top_bar(ctx: &egui::Context, s: &mut Switcher) {
    // Top bar of the original: Preset | New Open Save Save As Last ... Fullscreen ... Pause Inputs, Settings, ?
    egui::TopBottomPanel::top("top")
        .exact_height(30.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(ctx.style().visuals.window_fill()).inner_margin(egui::Margin::symmetric(4, 3)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.add_sized([80.0, 24.0], egui::Label::new(RichText::new(tr(ui, "Preset")).strong().color(Color32::WHITE)));
                if bar_btn(ui, "New", s.new_step != NewStep::Idle).on_hover_text(tr(ui, "Start a new blank preset with a video format you choose")).clicked() {
                    s.press_new();
                }
                if bar_btn(ui, "Open", false).on_hover_text(tr(ui, "Choose a project preset")).clicked() {
                    if s.outputs_busy() {
                        s.notice(STOP_OUTPUTS_MSG);
                    } else {
                        s.browse_req = Some(media::PickKind::PresetOpen);
                    }
                }
                if bar_btn(ui, "Save", false).on_hover_text(tr(ui, "Save changes to currently open Preset file (Ctrl+S)")).clicked() {
                    s.save_preset();
                }
                if bar_btn(ui, "Save As", false).on_hover_text(tr(ui, "Save to a new Preset file")).clicked() {
                    s.browse_req = Some(media::PickKind::PresetSave);
                }
                if bar_btn(ui, "Last", false).on_hover_text(tr(ui, "Restore the last automatic project copy")).clicked() {
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
                    if bar_btn(ui, "Pause Inputs", false).on_hover_text(tr(ui, "Pause all currently playing video inputs")).clicked() {
                        s.pause_video_inputs();
                    }
                });
            });
        });
}

/// Status line at the very bottom (format, FPS, render time, recording state, messages).
pub(crate) fn status_bar(ctx: &egui::Context, s: &Switcher, fps: Option<f64>, stats: &performance::Status) {
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
pub(crate) fn bar_btn(ui: &mut egui::Ui, text: &str, active: bool) -> egui::Response {
    let text = tr(ui, text);
    let b = egui::Button::new(RichText::new(text).strong().size(13.0).color(Color32::WHITE))
        .fill(if active { HOVER } else { BTN_DARK })
        .stroke(Stroke::NONE)
        .min_size(Vec2::new(if text.len() <= 2 { 24.0 } else { 90.0 }, 24.0));
    ui.add(b)
}

// ---------------------------------------------------------------- monitors

pub(crate) fn monitors(ctx: &egui::Context, s: &mut Switcher, t: f64) {
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
pub(crate) fn draw_safe_areas(p: &egui::Painter, r: egui::Rect, kind: u16) {
    let line = Stroke::new(1.0_f32, Color32::from_white_alpha(170));
    for g in safe_area_guides(kind, r.width(), r.height()) {
        p.line_segment([r.min + Vec2::new(g[0], g[1]), r.min + Vec2::new(g[2], g[3])], line);
    }
}

/// One scope in its place: black behind it (or the picture showing through when it lies over
/// the whole Preview), lines at 0/25/50/75/100 % for the waveforms (parade split in thirds), a
/// circle and cross for the vectorscope.
pub(crate) fn monitor(ui: &mut egui::Ui, s: &mut Switcher, t: f64, is_output: bool, w: f32, h: f32) {
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
        let label = ui.painter().text(left, egui::Align2::LEFT_CENTER, tr(ui, heading), egui::FontId::proportional(14.0), Color32::WHITE);
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

pub(crate) fn video_transport(ui: &mut egui::Ui, inp: &mut Input, width: f32, picture: egui::Rect) {
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
        let time = if matches!(inp.kind, Kind::Teleprompter(_)) {
            format!("Line {:.0} / {:.0}", inp.pos.floor() + 1.0, inp.duration)
        } else {
            format!("{}  /  {}  /  {}", clock(inp.pos as f64), clock(inp.duration as f64),
                clock((inp.duration - inp.pos).max(0.0) as f64))
        };
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
pub(crate) fn center_column(ui: &mut egui::Ui, s: &mut Switcher, w: f32, h: f32) {
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
            .add_sized([bw, bh], egui::Button::new(RichText::new(tr(ui, "Quick Play")).strong()).fill(if busy { HOVER } else { BTN }))
            .on_hover_text("Transition from Preview to Output using the Quick Play effect")
            .clicked()
        {
            let (e, d) = (s.quickplay_effect, s.quickplay_ms);
            s.start_transition(e, d);
        }
        if ui.add_sized([bw, bh], egui::Button::new(RichText::new(tr(ui, "Cut")).strong())).on_hover_text("Cut (key 0)").clicked() {
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
pub(crate) fn tbar(ui: &mut egui::Ui, s: &mut Switcher, w: f32, h: f32) {
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

pub(crate) fn draw_output(painter: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64) {
    draw_output_opts(painter, rect, s, t, true);
}

pub(crate) fn draw_overlay_channels(painter: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, preview: bool) {
    for ch in s.overlays.iter().filter(|c| preview || !c.on_preview) {
        if let Some(inp) = ch.input.and_then(|i| s.input(i)) {
            if ch.level > 0.0 {
                draw_overlay(painter, rect, inp, ch, t, &s.inputs);
            }
        }
    }
}

pub(crate) fn draw_output_opts(painter: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, overlays: bool) {
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
pub(crate) const EXT_SIZES: [(&str, [f32; 2]); 3] = [("960 x 540", [960.0, 540.0]), ("1280 x 720", [1280.0, 720.0]), ("1920 x 1080", [1920.0, 1080.0])];

/// Name of an output source code (Settings → Outputs "Source" column).
pub(crate) fn src_name(s: &Switcher, v: u32) -> String {
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
pub(crate) fn src_choices(s: &Switcher, multiview: bool) -> Vec<(u32, String)> {
    let mut v: Vec<u32> = vec![SRC_OUTPUT, SRC_PREVIEW];
    if multiview {
        v.extend([SRC_MV1, SRC_MV2]);
    }
    v.extend(s.inputs.iter().map(|i| i.id + 1));
    v.into_iter().map(|c| (c, src_name(s, c))).collect()
}

/// Draws one output source: Output, Preview (with overlays as the Preview monitor shows them),
/// a MultiView or one input.
pub(crate) fn draw_src(p: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, src: u32, overlays: bool) {
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
pub(crate) const MV_IDLE: Color32 = Color32::from_rgb(26, 60, 117);

/// A MultiView: several sources in one picture, each with a name strip underneath whose colour
/// is the tally — `qu::o` (@0x194c4c) pre-draws each strip in three states through `qu::p`
/// (@0x194cc8): the PreviewColour setting, the ActiveColour setting, or MV_IDLE
/// (colours handed over by ro/a::.ctor @0x1af984 from ozzzzzs::d / e / f).
pub(crate) fn draw_multiview(p: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, mv: usize) {
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
        let live = src.checked_sub(1).and_then(|id| s.input(id)).and_then(|input| match input.kind {
            Kind::Replay { channel, .. } => s.replay_live_source(channel),
            _ => None,
        });
        if let Some(camera) = live { draw_replay_camera(&bp, r, s, t, camera); }
        else { draw_src(&bp, r, s, t, src, src == SRC_OUTPUT || src == SRC_PREVIEW); }
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
pub(crate) fn screens() -> Vec<[i32; 4]> {
    let mut v = media::monitors();
    if v.is_empty() {
        v.push([0, 0, 1920, 1080]);
    }
    v
}

/// The Fullscreen display (top bar "Fullscreen"): a borderless window covering the screen chosen
/// in Settings → Display, showing the Fullscreen source. Esc or a double-click closes it.
pub(crate) fn fullscreen_display(index: usize, count: usize) -> usize {
    if index < count { index } else { 0 }
}

pub(crate) fn toggle_fullscreen(s: &mut Switcher, slot: usize) {
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

pub(crate) fn fullscreen_window(ctx: &egui::Context, s: &mut Switcher, t: f64, slot: usize) {
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
pub(crate) fn fit_16_9(r: egui::Rect) -> egui::Rect {
    let mut w = r.width();
    let mut h = w * 9.0 / 16.0;
    if h > r.height() {
        h = r.height();
        w = h * 16.0 / 9.0;
    }
    egui::Rect::from_center_size(r.center(), Vec2::new(w, h))
}

pub(crate) fn output_window(ctx: &egui::Context, s: &mut Switcher, t: f64, channel: usize) {
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

// ---------------------------------------------------------------- bottom bar

pub(crate) fn bottom_bar(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    egui::TopBottomPanel::bottom("bottom")
        .exact_height(40.0)
        .show_separator_line(false)
        .frame(egui::Frame::new().fill(ctx.style().visuals.window_fill()).inner_margin(egui::Margin::symmetric(2, 4)))
        .show(ctx, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                if big_btn(ui, "Add Input", false, None).on_hover_text(tr(ui, "Add a new Input such as a Video, Camera or Image")).clicked() {
                    s.add_dialog.open = true;
                }
                ui.add_space(14.0);
                let red = ACTIVE_RED;
                if gear_btn(ui, s.rec_setup, "Configure Recording").clicked() {
                    s.rec_setup = !s.rec_setup;
                }
                if big_btn(ui, &timer_label(locale::text(ctx, "Record"), s.rec, t), s.rec.is_some(), Some(red)).on_hover_text(tr(ui, "Start or stop Recording")).clicked() {
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
                if big_btn(ui, "External", s.external[0].open || s.decklink_active[0], Some(red)).on_hover_text(tr(ui, "Start or stop External Output 1")).clicked() {
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
                    .on_hover_text(if status.is_empty() { tr(ui, "Start or stop all configured streams").into() } else { status })
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
                if big_btn(ui, &timer_label("MultiCorder", s.multicorder, t), s.multicorder.is_some(), Some(red)).on_hover_text(tr(ui, "Start or Stop MultiCorder")).clicked() {
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
                if big_btn(ui, "PlayList", s.playlist.is_some(), Some(HOVER)).on_hover_text(tr(ui, "Start or Stop a PlayList of Inputs")).clicked() {
                    s.playlist = if s.playlist.is_some() { None } else { Some(t) };
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    if big_btn(ui, "Snapshot", false, None).on_hover_text(tr(ui, "Take image snapshot of Output (saved in Pictures\\LiveMix)")).clicked() {
                        s.snapshot_req = true;
                    }
                    if big_btn(ui, "Replay", s.replay_arm, None).clicked() {
                        s.replay_open = true;
                        s.replay_setup_open = s.replay_session.is_empty();
                    }
                    if big_btn(ui, "Audio Mixer", s.show_mixer, Some(Color32::from_rgb(0x00, 0x64, 0x00))).on_hover_text(tr(ui, "Show or hide audio mixer pane")).clicked() {
                        s.show_mixer = !s.show_mixer;
                    }
                    if big_btn(ui, "Overlay", s.overlay_dialog, None).on_hover_text(tr(ui, "Configure overlay settings")).clicked() {
                        s.overlay_dialog = !s.overlay_dialog;
                    }
                });
            });
        });
}

pub(crate) fn timer_label(name: &str, v: Option<f64>, t: f64) -> String {
    match v {
        Some(st) => format!("{name}  {}", clock(t - st)),
        None => name.to_string(),
    }
}
