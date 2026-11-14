//! Input tiles, the Add Input helpers and the input settings window.
use super::*;

pub(crate) fn update_network_thumbnails(ctx: &egui::Context, dialog: &mut AddDialog, enabled: bool,
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

pub(crate) fn network_thumbnail_grid(ui: &mut egui::Ui, dialog: &mut AddDialog) {
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
pub(crate) fn input_preview_window(ctx: &egui::Context, s: &mut Switcher, t: f64) {
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
            let live = match input.kind { Kind::Replay { channel, .. } => s.replay_live_source(channel), _ => None };
            if let Some(camera) = live { draw_replay_camera(&painter, out, s, t, camera); }
            else { draw_source(&painter, out, input, 1.0, t, &s.inputs); }
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

pub(crate) fn inputs_panel(ctx: &egui::Context, s: &mut Switcher, t: f64) {
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
                let mut placed = Vec::new();
                let (mut x, mut y) = (TILE_MARGIN, 0.0_f32);
                for (index, cell) in cells.iter().enumerate() {
                    let w = match cell {
                        Some((id, _)) if s.input(*id).map(|i| i.collapsed).unwrap_or(false) => TILE_HEADER,
                        _ => tile_w,
                    };
                    let strip = cell.and_then(|(id, _)| s.input(id)).is_some_and(|input| {
                        !input.collapsed && matches!(input.kind, Kind::Replay { channel: 0, .. })
                    });
                    let span = w + if strip { REPLAY_STRIP_WIDTH + GAP } else { 0.0 };
                    let next_is_replay_b = cells.get(index + 1).and_then(|next| *next)
                        .and_then(|(id, _)| s.input(id)).is_some_and(|input| {
                            !input.collapsed && matches!(input.kind, Kind::Replay { channel: 1, .. })
                        });
                    let pair = span + GAP + tile_w;
                    let row_width = if strip && next_is_replay_b && pair <= avail { pair } else { span };
                    if x > TILE_MARGIN && x + row_width > TILE_MARGIN + avail + 0.5 {
                        x = TILE_MARGIN;
                        y += tile_h + GAP;
                    }
                    placed.push((*cell, x, y, w, strip));
                    x += span + GAP;
                }
                let total_h = y + tile_h;
                let (area, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), total_h.max(1.0)), Sense::hover());
                let mut rects: Vec<(u32, egui::Rect)> = Vec::new();
                for (cell, x, y, w, strip) in placed {
                    let r = egui::Rect::from_min_size(area.min + Vec2::new(x, y), Vec2::new(w, tile_h));
                    match cell {
                        Some((id, number)) => {
                            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)).id_salt(("tile", id)));
                            tile(&mut child, s, id, number, t);
                            if strip {
                                let controls = egui::Rect::from_min_size(r.right_top() + Vec2::new(GAP, 0.0), Vec2::new(REPLAY_STRIP_WIDTH, tile_h));
                                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(controls).layout(egui::Layout::top_down(egui::Align::Center)).id_salt(("replay-strip", id)));
                                replay_strip(&mut child, s, t);
                            }
                            rects.push((id, r));
                        }
                        None => {
                            let r = egui::Rect::from_min_size(r.min, Vec2::new(tile_w, tile_h));
                            let resp = ui.interact(r, egui::Id::new("add-input-tile"), Sense::click());
                            let p = ui.painter();
                            p.rect_stroke(r.shrink(2.0), 4.0, Stroke::new(1.0_f32, EDGE), egui::StrokeKind::Inside);
                            p.text(r.center() - Vec2::new(0.0, 8.0), egui::Align2::CENTER_CENTER, "+", egui::FontId::proportional(30.0), TEXT_DIM);
                            p.text(r.center() + Vec2::new(0.0, 18.0), egui::Align2::CENTER_CENTER, tr(ui, "Add Input"), egui::FontId::proportional(12.0), TEXT_DIM);
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
pub(crate) const TILE_HEADER: f32 = 26.0;
pub(crate) const TILE_ROW: f32 = 20.0;
pub(crate) const BW_CLOSE: f32 = 42.0;
pub(crate) const BW_CUT: f32 = 33.0;
pub(crate) const BW_LOOP: f32 = 38.0;
pub(crate) const BW_AUDIO: f32 = 65.0;
pub(crate) const BW_ICON: f32 = 24.0;

/// (width, height) of one input tile from the InputSize setting.
pub(crate) fn tile_size(s: &Switcher) -> (f32, f32) {
    (s.cfg.input_w, TILE_HEADER + s.cfg.input_h + TILE_ROW * 3.0)
}

/// A flat tile button of a fixed width (the original's button widths).
pub(crate) fn tile_btn(ui: &mut egui::Ui, text: &str, w: f32, active: bool, enabled: bool, on_col: Color32) -> egui::Response {
    let text = tr(ui, text);
    let col = if !enabled { TEXT_DIM.gamma_multiply(0.6) } else if active { Color32::BLACK } else { TEXT };
    let b = egui::Button::new(RichText::new(text).size(11.0).color(col))
        .fill(if active { on_col } else { BTN_DARK })
        .stroke(Stroke::NONE)
        .corner_radius(0)
        .min_size(Vec2::new(w, TILE_ROW));
    ui.add_enabled_ui(enabled, |ui| ui.add_sized([w, TILE_ROW], b)).inner
}

pub(crate) fn tile(ui: &mut egui::Ui, s: &mut Switcher, id: u32, number: usize, t: f64) {
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
            if ui.button(tr(ui, "None")).clicked() {
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
            let live = match inp.kind { Kind::Replay { channel, .. } => s.replay_live_source(channel), _ => None };
            if let Some(camera) = live {
                draw_replay_camera(&painter, r, s, t, camera);
            } else if s.cfg.no_input_effects {
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
        resp.on_hover_text(tr(ui, "Click: send to Preview · Double-click: Input Settings"));

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
                if let Some(input) = s.input_mut(id).filter(|input| matches!(input.kind, Kind::Presentation { .. })) {
                    slide_buttons(ui, input);
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

// ---------------------------------------------------------------- input settings window

/// The original's Input Settings sections, in its order (frmInputSettings.tMenu.*), plus the
/// Audio is mixed in the separate Audio Mixer. In Input Settings it is
/// reachable from General > Audio Settings, so it lives here as its own section.
pub(crate) const INPUT_PAGES: [&str; 13] = [
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
pub(crate) const INPUT_PAGES_BUILT: [usize; 13] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];

/// cboAspectRatio entries: value 0 keeps the source's own shape.
pub(crate) const ASPECTS: [(f32, &str); 9] = [
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

pub(crate) fn aspect_name(v: f32) -> &'static str {
    ASPECTS.iter().find(|(a, _)| (a - v).abs() < 0.001).map_or("Custom", |(_, n)| n)
}

pub(crate) fn input_ptz(ui: &mut egui::Ui, input: &mut Input) {
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
pub(crate) fn camera_mode_rows(ui: &mut egui::Ui, inp: &mut Input) {
    let modes = inp.cam_modes.clone();
    let cur = inp.cam_current.clone();
    ui.label(tr(ui, "Resolution"));
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
        ui.label(tr(ui, "Frame Rate"));
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

pub(crate) fn slide_buttons(ui: &mut egui::Ui, input: &mut Input) {
    let current = input.seek_ms.map_or(input.pos.max(0.0) as u64, |ms| ms / 1000);
    let count = input.duration.max(0.0) as u64;
    ui.horizontal(|ui| {
        if ui.add_enabled(current > 0, egui::Button::new("‹").small()).on_hover_text("Previous slide").clicked() {
            input.seek_ms = Some((current - 1) * 1000);
        }
        ui.label(format!("{} / {count}", if count == 0 { 0 } else { current + 1 }));
        if ui.add_enabled(current + 1 < count, egui::Button::new("›").small()).on_hover_text("Next slide").clicked() {
            input.seek_ms = Some((current + 1) * 1000);
        }
    });
}

pub(crate) fn input_general(ui: &mut egui::Ui, inp: &mut Input, others: &[(usize, u32, String)], page: &mut usize,
    project: ([u32; 2], f32)) {
    egui::Grid::new("in-general").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        ui.label(tr(ui, "Name"));
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
            Kind::Presentation { .. } => "PowerPoint",
            Kind::Web { .. } => "Web Browser",
            Kind::VirtualSet { .. } => "Virtual Set",
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
        if matches!(&inp.kind, Kind::VideoFile { .. } | Kind::VideoList { .. }) {
            ui.label(tr(ui, "Decoder"));
            ui.label(if inp.decoder.is_empty() { "—" } else { &inp.decoder });
            ui.end_row();
        }
        let file = match &inp.kind {
            Kind::VideoFile { path } | Kind::AudioFile { path } | Kind::ImageFile { path } | Kind::GtTitle { path, .. }
            | Kind::Replay { path, .. } | Kind::Presentation { path } => Some(path.clone()),
            Kind::Photos { folder } | Kind::ImageSequence { folder } => Some(folder.clone()),
            _ => None,
        };
        if let Some(f) = file {
            ui.label(tr(ui, "File"));
            ui.label(RichText::new(f).size(11.0).color(TEXT_DIM));
            ui.end_row();
        }
        if let Kind::Web { url, transparent } = &mut inp.kind {
            ui.label("URL");
            let id = egui::Id::new(("web-url", inp.id));
            let mut draft = ui.data(|data| data.get_temp::<String>(id)).unwrap_or_else(|| url.clone());
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut draft);
                if ui.add_enabled(web_input::valid_url(draft.trim()), egui::Button::new("Open")).clicked() {
                    *url = draft.trim().into();
                    inp.restart_req = true;
                }
            });
            ui.data_mut(|data| data.insert_temp(id, draft));
            ui.end_row();
            ui.label("Background");
            setting_toggle(ui, transparent, "Transparent");
            ui.end_row();
            if !inp.status.is_empty() {
                ui.label("Status");
                ui.label(&inp.status);
                ui.end_row();
            }
        }
        if matches!(inp.kind, Kind::Presentation { .. }) {
            ui.label("Slides");
            slide_buttons(ui, inp);
            ui.end_row();
        }
        if let Kind::VirtualSet { camera, previous, changed_at } = &mut inp.kind {
            ui.label("Camera");
            ui.horizontal(|ui| {
                for (index, label) in ["Wide", "Left", "Right"].into_iter().enumerate() {
                    if ui.selectable_label(usize::from(*camera) == index, label).clicked() && usize::from(*camera) != index {
                        *previous = *camera;
                        *camera = index as u8;
                        *changed_at = ui.input(|state| state.time);
                    }
                }
            });
            ui.end_row();
            ui.label("Layers");
            if ui.button("Assign backdrop, presenters and foreground…").clicked() { *page = 7; }
            ui.end_row();
        }
        let decklink = match &inp.kind {
            Kind::Device { link, .. } => decklink::device_index(link),
            _ => None,
        };
        if let Some(device) = decklink {
            ui.label(tr(ui, "Connector"));
            decklink_connector(ui, "decklink-capture-port", &mut inp.decklink_port);
            ui.end_row();
            ui.label(tr(ui, "Video format"));
            decklink_mode_picker(ui, "input-decklink-mode", device, false, &mut inp.decklink_mode);
            ui.end_row();
            ui.label("Detected mode");
            ui.label(inp.cam_current.as_ref().map(|mode| mode.label()).unwrap_or_else(|| inp.status.clone()));
            ui.end_row();
        } else if let Kind::Device { link, .. } = &inp.kind {
            if let Some(name) = link.strip_prefix("ndi:").or_else(|| link.strip_prefix("omt:")) {
                ui.label(tr(ui, "Source"));
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
            ui.label(tr(ui, "Colour"));
            ui.color_edit_button_srgba(c);
            ui.end_row();
        }
        // frmInput tVideoDelay: source, buffer seconds and resolution, with the buffer state.
        if let Kind::VideoDelay { source, seconds, resolution } = &mut inp.kind {
            ui.label(tr(ui, "Source"));
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
            ui.label(tr(ui, "Seconds"));
            ui.add(egui::DragValue::new(seconds).range(1.0..=120.0).suffix(" s").speed(0.1));
            ui.end_row();
            ui.label(tr(ui, "Resolution"));
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
                ui.collapsing(tr(ui, "This template uses unsupported GT features"), |ui| {
                    for feature in &document.unsupported {
                        ui.label(feature);
                    }
                });
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
        ui.label(RichText::new(tr(ui, "Frame Rate")).size(11.0));
        ui.label(RichText::new(source_rate).size(11.0).color(TEXT_DIM));
        ui.end_row();
        ui.label(RichText::new(tr(ui, "Resolution")).size(11.0));
        ui.label(RichText::new(source_res).size(11.0).color(TEXT_DIM));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        // cmdAudioSettings: opens the Audio section of this window.
        if ui.add_enabled(inp.kind.has_audio(), egui::Button::new(tr(ui, "Audio Settings"))).clicked() {
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
            ui.selectable_value(&mut inp.category, None, tr(ui, "None"));
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
pub(crate) fn input_telestrator(ui: &mut egui::Ui, inp: &mut Input, all: &[Input], t: f64, output: Option<u32>, web: bool) {
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
        ui.label(tr(ui, "Colour"));
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

pub(crate) fn input_colour_key(ui: &mut egui::Ui, inp: &mut Input) {
    if inp.kind.supports_picture_fx() {
        ui.strong(tr(ui, "Luma Key"));
        setting_toggle(ui, &mut inp.colour_fx.luma.enabled, "Enable Luma Key");
        ui.add_enabled_ui(inp.colour_fx.luma.enabled, |ui| {
            ui.add(egui::Slider::new(&mut inp.colour_fx.luma.threshold, 0..=255).text(tr(ui, "Threshold")));
            ui.add(egui::Slider::new(&mut inp.colour_fx.luma.softness, 0..=255).text(tr(ui, "Softness")));
            setting_toggle(ui, &mut inp.colour_fx.luma.invert, "Remove bright pixels");
        });
        ui.separator();
    }
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
        if ui.small_button(tr(ui, "Reset")).on_hover_text("Restore the default colour key settings").clicked() {
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
pub(crate) fn colour_ramp(ui: &mut egui::Ui, adj: ColourAdjust) {
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
pub(crate) fn input_colour_adjust(ui: &mut egui::Ui, inp: &mut Input) {
    let adj = &mut inp.colour_adjust;
    ui.add(egui::Slider::new(&mut adj.brightness, -1.0..=1.0).text("Brightness"));
    ui.add(egui::Slider::new(&mut adj.contrast, -1.0..=1.0).text("Contrast"));
    ui.add(egui::Slider::new(&mut adj.saturation, -1.0..=1.0).text("Saturation"));
    ui.add(egui::Slider::new(&mut adj.alpha, -1.0..=1.0).text("Alpha"));
    ui.horizontal(|ui| {
        if ui.button(tr(ui, "Reset")).clicked() {
            *adj = ColourAdjust::default();
        }
        ui.label(RichText::new("Applies to capture, video, image and list inputs.").size(11.0).color(TEXT_DIM));
    });
    ui.add_space(6.0);
    colour_ramp(ui, *adj);
}

/// frmInputSettings Colour Correction: per-channel gains, black/white stretch and the
/// 0-255 / 16-235 level presets.
pub(crate) fn lut_settings(ui: &mut egui::Ui, inp: &mut Input, browse: &mut bool) {
    ui.strong("3D LUT");
    let path_id = egui::Id::new(("lut-path", inp.id));
    let error_id = path_id.with("error");
    let mut path = ui.data(|data| data.get_temp::<String>(path_id)).unwrap_or_else(|| inp.colour_fx.lut_path.clone());
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut path).hint_text(tr(ui, "Path to .cube or PNG LUT")).desired_width(235.0));
        if ui.button("Browse…").clicked() { *browse = true; }
        if ui.add_enabled(!path.trim().is_empty(), egui::Button::new("Load")).clicked() {
            let result = inp.colour_fx.load(std::path::Path::new(path.trim().trim_matches('"')));
            ui.data_mut(|data| data.insert_temp(error_id, result.err().unwrap_or_default()));
        }
        if ui.add_enabled(!inp.colour_fx.lut_path.is_empty(), egui::Button::new("Clear")).clicked() {
            inp.colour_fx.lut = None;
            inp.colour_fx.lut_path.clear();
            path.clear();
            ui.data_mut(|data| data.insert_temp(error_id, String::new()));
        }
    });
    ui.data_mut(|data| data.insert_temp(path_id, path));
    if let Some(error) = ui.data(|data| data.get_temp::<String>(error_id)).filter(|error| !error.is_empty()) {
        ui.colored_label(Color32::LIGHT_RED, error);
    }
    if let Some(lut) = &inp.colour_fx.lut {
        let name = std::path::Path::new(&inp.colour_fx.lut_path).file_name().unwrap_or_default().to_string_lossy();
        ui.label(format!("{name} · {}³", lut.size));
    } else if !inp.colour_fx.lut_path.is_empty() {
        ui.label(tr(ui, "The saved LUT file is unavailable."));
    }
    ui.separator();
}

pub(crate) fn input_colour_correction(ui: &mut egui::Ui, inp: &mut Input, browse: &mut bool) {
    if inp.kind.supports_picture_fx() { lut_settings(ui, inp, browse); }
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
        if ui.button(tr(ui, "Reset")).clicked() {
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
pub(crate) fn input_effects(ui: &mut egui::Ui, inp: &mut Input, page: &mut usize) {
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
    ui.vertical(|ui| {
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
        0 => { ui.label(RichText::new("Mirrors the picture horizontally.").size(11.0).color(TEXT_DIM)); }
        1 => { ui.label(RichText::new("Sharpens the picture of capture inputs.").size(11.0).color(TEXT_DIM)); }
        2 => { ui.label(RichText::new("Blends the fields of interlaced sources.").size(11.0).color(TEXT_DIM)); }
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
pub(crate) fn eq_graph(ui: &mut egui::Ui, eq: &mut [f32; 10]) {
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
pub(crate) fn dynamics_graph(ui: &mut egui::Ui, fx: &mut crate::audio::Effects) {
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

pub(crate) fn input_audio_general(ui: &mut egui::Ui, inp: &mut Input) {
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
                ui.selectable_value(&mut fx.channel, 0, tr(ui, "Stereo"));
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

pub(crate) fn input_audio(ui: &mut egui::Ui, input: &mut Input, page_height: f32) {
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
            if ui.button(tr(ui, "Reset")).clicked() {
                let defaults = audio::Effects::default();
                input.audio_effects.comp_threshold = defaults.comp_threshold;
                input.audio_effects.comp_ratio = defaults.comp_ratio;
            }
        }
        3 => {
            ui.add(egui::Slider::new(&mut input.audio_effects.gate_threshold, -90.0..=0.0).text("Threshold").suffix(" dBFS"));
            if ui.button(tr(ui, "Reset")).clicked() {
                input.audio_effects.gate_threshold = audio::Effects::default().gate_threshold;
            }
        }
        _ => input_audio_general(ui, input),
    });
}

pub(crate) fn input_audio_eq(ui: &mut egui::Ui, inp: &mut Input) {
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
pub(crate) fn input_tally(ui: &mut egui::Ui, inp: &mut Input, number: usize) {
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
            ui.label(tr(ui, "Colour"));
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
pub(crate) fn input_advanced(ui: &mut egui::Ui, inp: &mut Input, number: usize) {
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
pub(crate) fn position_editor(ui: &mut egui::Ui, salt: &str, p: &mut Position) {
    egui::Grid::new(("posgrid", salt)).num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
        let row = |ui: &mut egui::Ui, name: &str, v: &mut f32, range: std::ops::RangeInclusive<f32>, reset: f32| {
            ui.label(name);
            ui.add(egui::Slider::new(v, range).step_by(0.01).fixed_decimals(2));
            if ui.small_button(tr(ui, "Reset")).clicked() {
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
            if ui.small_button(tr(ui, "Reset")).clicked() {
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
pub(crate) fn layers_editor(ui: &mut egui::Ui, inp: &mut Input, others: &[(usize, u32, String)], all: &[Input], t: f64) {
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
        ui.selectable_value(&mut mode, 0, tr(ui, "Position"));
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
                ui.label(tr(ui, "Colour"));
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

pub(crate) fn file_decoder(settings: &Settings, path: &str) -> (media::Source, String) {
    let extension = std::path::Path::new(path).extension().and_then(|extension| extension.to_str());
    let ffmpeg = cfg!(target_os = "macos") || settings.ffmpeg_all_files
        || extension.is_some_and(|extension| settings.ffmpeg_extensions.split([',', ';', ' ']).any(|item| {
            extension.eq_ignore_ascii_case(item.trim().trim_start_matches(['*', '.']))
        }));
    #[cfg(windows)]
    if !ffmpeg && settings.directshow.enabled {
        let config = serde_json::to_string(&settings.directshow).unwrap_or_default();
        let deinterlace = settings.decoder_deinterlace;
        return (media::Source::DirectShowFile(path.into(), settings.directshow.clone(), deinterlace, settings.ffmpeg_hardware),
            format!("directshow:{deinterlace}:{}:{config}:{path}", settings.ffmpeg_hardware));
    }
    if ffmpeg {
        let deinterlace = settings.decoder_deinterlace;
        (media::Source::FfmpegFile(path.to_owned(), deinterlace, settings.ffmpeg_hardware),
            format!("ffmpeg:{deinterlace}:{}:{path}", settings.ffmpeg_hardware))
    } else {
        #[cfg(windows)]
        {
            let deinterlace = settings.decoder_deinterlace;
            (media::Source::AutoFile(path.to_owned(), deinterlace, settings.ffmpeg_hardware),
                format!("auto:{deinterlace}:{}:{path}", settings.ffmpeg_hardware))
        }
        #[cfg(not(windows))]
        (media::Source::File(path.to_owned()), path.to_owned())
    }
}
