//! The Replay controller: A/B players, camera grid, event lists, tags, playlists and transport.
use crate::*;
use crate::replay::{timecode, Action, Clip, QueueItem, EVENT_LISTS, PLAYLISTS, TAGS_PER_PAGE};

const EVENTS_TAB: Color32 = HOVER;
const PLAYLIST_TAB: Color32 = Color32::from_rgb(0xc8, 0x6a, 0x00);
const MONITOR_ON: Color32 = Color32::from_rgb(0x1f, 0x8a, 0x3b);
const ROW: f32 = 24.0;
const TAG_COLUMNS: usize = TAGS_PER_PAGE / 2;

pub(crate) fn replay_controller(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    if !s.replay_open { return; }
    let mut open = true;
    let title = locale::text(ctx, "Instant Replay");
    if s.replay_ui.undocked {
        let viewport = egui::ViewportId::from_hash_of("livemix-replay-controller");
        let builder = egui::ViewportBuilder::default().with_title("LiveMix · Instant Replay Controller")
            .with_inner_size([1280.0, 720.0]).with_min_inner_size([960.0, 540.0]);
        ctx.show_viewport_immediate(viewport, builder, |ctx, class| {
            if class == egui::ViewportClass::Embedded {
                egui::Window::new(title).id(egui::Id::new("Instant Replay")).open(&mut open)
                    .default_width(1100.0).show(ctx, |ui| controller(ui, s, t, false));
            } else {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(6)))
                    .show(ctx, |ui| controller(ui, s, t, true));
                // Closing the separate window docks the controller again.
                if ctx.input(|input| input.viewport().close_requested()) { s.replay_ui.undocked = false; }
            }
        });
    } else {
        egui::Window::new(title).id(egui::Id::new("Instant Replay")).open(&mut open)
            .default_width(1100.0).show(ctx, |ui| controller(ui, s, t, false));
    }
    s.replay_open = open;
}

fn controller(ui: &mut egui::Ui, s: &mut Switcher, t: f64, fill: bool) {
    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
    let mut cameras: Vec<usize> = (0..REPLAY_ANGLES).filter(|&angle| s.replay_angles[angle] != MV_AUTO).collect();
    if cameras.is_empty() { cameras = (0..4).collect(); }
    let inputs: [Option<u32>; 2] = std::array::from_fn(|channel| s.inputs.iter()
        .find(|input| matches!(input.kind, Kind::Replay { channel: c, .. } if c == channel)).map(|input| input.id));
    let on_air = (0..2).find(|&channel| inputs[channel].is_some() && s.output == inputs[channel]);
    // Preview Mode keeps cue and jog on the player that is off air.
    if s.replay_ui.preview_mode { s.replay_channel = 1 - on_air.unwrap_or(0); }
    if s.replay_ui.monitors { monitors_row(ui, s, t, &cameras, inputs); }
    tabs_row(ui, s);
    let height = if fill { (ui.available_height() - 215.0).max(120.0) } else { 220.0 };
    let width = ui.available_width();
    let left = (width * 0.56).floor();
    ui.horizontal_top(|ui| {
        let layout = egui::Layout::top_down(egui::Align::Min);
        ui.allocate_ui_with_layout(Vec2::new(left, height), layout, |ui| events_list(ui, s, &cameras, height));
        ui.allocate_ui_with_layout(Vec2::new(width - left - 4.0, height), layout, |ui| playlist(ui, s, height));
    });
    tags_bar(ui, s);
    toolbar(ui, s, t, &cameras, on_air);
    ui.label(RichText::new(&s.replay_status).color(TEXT_DIM));
}

fn strip(painter: &egui::Painter, rect: egui::Rect, text: &str, fill: Color32) {
    let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 16.0), rect.max);
    painter.rect_filled(bar, 0.0, fill);
    painter.text(bar.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(11.0), TEXT);
}

fn monitors_row(ui: &mut egui::Ui, s: &mut Switcher, t: f64, cameras: &[usize], inputs: [Option<u32>; 2]) {
    let gap = 4.0;
    let columns: usize = if cameras.len() > 4 { 4 } else { 2 };
    let width = ui.available_width();
    let monitor = ((width - gap * 2.0) / (2.0 + columns as f32 / 2.0)).clamp(100.0, 640.0);
    let height = monitor * 9.0 / 16.0;
    let (row, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(row);
    let shade = Color32::from_black_alpha(170);
    let camera = tr(ui, "Camera");
    for channel in 0..2 {
        let rect = egui::Rect::from_min_size(row.min + Vec2::new(channel as f32 * (monitor + gap), 0.0), Vec2::new(monitor, height));
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        let live = s.replay_live_source(channel);
        let picture = painter.with_clip_rect(rect);
        if let Some(source) = live {
            draw_replay_camera(&picture, rect, s, t, source);
        } else if let Some(input) = inputs[channel].and_then(|id| s.input(id)) {
            draw_source(&picture, rect, input, 1.0, t, &s.inputs);
        }
        let tally = if inputs[channel].is_none() { shade }
            else if s.output == inputs[channel] { ACTIVE_RED }
            else if s.preview == inputs[channel] { PREVIEW }
            else { shade };
        let state = if live.is_some() { "LIVE" } else { "REPLAY" };
        let text = format!("{} · {camera} {} · {state}", ["A", "B"][channel], s.replay_cameras[channel] + 1);
        strip(&painter, rect, &text, tally);
        if s.replay_channel == channel || s.replay_channel == 2 {
            painter.rect_stroke(rect, 0.0, Stroke::new(2.0_f32, Color32::WHITE), egui::StrokeKind::Inside);
        }
        let response = ui.interact(rect, ui.id().with(("replay-monitor", channel)), Sense::click());
        if response.clicked() && !s.replay_ui.preview_mode { s.replay_channel = channel; }
    }
    let cell = Vec2::new((monitor - gap) / 2.0, (height - gap) / 2.0);
    let origin = row.min + Vec2::new(2.0 * (monitor + gap), 0.0);
    for (slot, &angle) in cameras.iter().take(columns * 2).enumerate() {
        let offset = Vec2::new((slot % columns) as f32 * (cell.x + gap), (slot / columns) as f32 * (cell.y + gap));
        let rect = egui::Rect::from_min_size(origin + offset, cell);
        painter.rect_filled(rect, 0.0, Color32::BLACK);
        if let Some(source) = s.replay_camera_source(angle) {
            draw_replay_camera(&painter.with_clip_rect(rect), rect, s, t, source);
        }
        let mut text = format!("{camera} {}", angle + 1);
        for (channel, name) in ["A", "B"].into_iter().enumerate() {
            if s.replay_cameras[channel] == angle {
                text.push_str(" · ");
                text.push_str(name);
            }
        }
        strip(&painter, rect, &text, shade);
        let response = ui.interact(rect, ui.id().with(("replay-camera", angle)), Sense::click());
        if response.clicked() {
            for channel in 0..2 {
                if s.replay_channel == channel || s.replay_channel == 2 { s.replay_cameras[channel] = angle; }
            }
        }
        if response.secondary_clicked() { s.replay_cameras[1] = angle; }
        response.on_hover_text(tr(ui, "Click assigns this camera to the selected player. Right-click assigns it to B."));
    }
}

fn tab(ui: &mut egui::Ui, text: String, width: f32, active: bool, colour: Color32) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).size(10.5).color(TEXT))
        .fill(if active { colour } else { BTN_DARK });
    ui.add_sized([width, 20.0], button)
}

fn tabs_row(ui: &mut egui::Ui, s: &mut Switcher) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let dock = tr(ui, if s.replay_ui.undocked { "Dock" } else { "Undock" });
        if ui.add_sized([52.0, 20.0], egui::Button::new(RichText::new(dock).size(10.5)))
            .on_hover_text(tr(ui, "Move the Replay controller to its own window or back")).clicked()
        {
            s.replay_ui.undocked = !s.replay_ui.undocked;
        }
        if gear_btn(ui, s.replay_setup_open, "Instant Replay Setup").clicked() { s.replay_setup_open = true; }
        let tabs = (EVENT_LISTS + PLAYLISTS + 2) as f32;
        let width = (ui.available_width() / tabs - 2.0).max(44.0);
        for index in 0..EVENT_LISTS {
            let text = format!("{} {}", tr(ui, "EVENTS"), index + 1);
            if tab(ui, text, width, s.replay_ui.list == index, EVENTS_TAB).clicked() { s.replay_ui.list = index; }
        }
        for index in 0..PLAYLISTS {
            let text = format!("{} {}", tr(ui, "PLAYLIST"), index + 1);
            if tab(ui, text, width, s.replay_ui.playlist == index, PLAYLIST_TAB).clicked() {
                s.replay_ui.playlist = index;
                s.replay_ui.clip = None;
            }
        }
        let text = tr(ui, "MONITOR").to_owned();
        if tab(ui, text, width, s.replay_ui.monitors, MONITOR_ON).clicked() { s.replay_ui.monitors = !s.replay_ui.monitors; }
        let text = tr(ui, "LIVE").to_owned();
        if tab(ui, text, width, s.replay_live, ACTIVE_RED).clicked() { s.replay_live = !s.replay_live; }
    });
}

fn title_bar(ui: &mut egui::Ui, text: String, fill: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(11.0), TEXT);
}

fn header(ui: &mut egui::Ui, text: &str) {
    let text = tr(ui, text);
    ui.label(RichText::new(text).color(TEXT_DIM).size(11.0));
}

fn parse_timecode(text: &str) -> Option<f64> {
    let mut seconds = 0.0;
    for part in text.trim().split(':') {
        seconds = seconds * 60.0 + part.trim().parse::<f64>().ok().filter(|value| *value >= 0.0)?;
    }
    Some(seconds * 1000.0)
}

/// A time that drags left or right in coarse steps and moves one frame per wheel notch.
fn time_drag(ui: &mut egui::Ui, value: &mut u64, range: std::ops::RangeInclusive<u64>, frame: f64, enabled: bool) -> bool {
    let before = *value;
    let response = ui.add_enabled(enabled, egui::DragValue::new(value).speed(100.0).range(range.clone())
        .custom_formatter(|value, _| timecode(value.max(0.0).round() as u64))
        .custom_parser(parse_timecode));
    if enabled && response.hovered() {
        let wheel = ui.input(|input| input.raw_scroll_delta.y);
        if wheel != 0.0 {
            let step = frame.max(1.0).round() as u64;
            let moved = if wheel > 0.0 { value.saturating_add(step) } else { value.saturating_sub(step) };
            *value = moved.clamp(*range.start(), *range.end());
            ui.input_mut(|input| {
                input.raw_scroll_delta = Vec2::ZERO;
                input.smooth_scroll_delta = Vec2::ZERO;
            });
        }
    }
    *value != before
}

fn events_list(ui: &mut egui::Ui, s: &mut Switcher, cameras: &[usize], height: f32) {
    let list = s.replay_ui.list;
    let on_air = s.replay_ui.on_air.is_some_and(|id| {
        s.replay_events.iter().any(|event| event.id == id && usize::from(event.list) == list)
    });
    let title = format!("{} {}", tr(ui, "EVENT LIST"), list + 1);
    title_bar(ui, title, if on_air { ACTIVE_RED } else { EVENTS_TAB });
    let frame = s.replay_ui.frame_ms;
    let editable = !s.replay_read_only;
    let channel = s.replay_channel.min(1);
    let camera = tr(ui, "Camera");
    let mut command = None;
    let mut dirty = false;
    egui::ScrollArea::both().id_salt("replay-event-list").max_height(height - 22.0).auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("replay-events").striped(true).spacing(Vec2::new(8.0, 2.0)).show(ui, |ui| {
            for title in ["In Point", "Out Point", "Duration"] { header(ui, title); }
            for &angle in cameras {
                ui.label("");
                ui.label(RichText::new(format!("{camera} {}", angle + 1)).color(TEXT_DIM).size(11.0));
            }
            ui.end_row();
            for index in 0..s.replay_events.len() {
                let event = &mut s.replay_events[index];
                if usize::from(event.list) != list { continue; }
                let id = event.id;
                let (start, end) = (event.start, event.end);
                dirty |= time_drag(ui, &mut event.start, 0..=end.saturating_sub(1), frame, editable);
                dirty |= time_drag(ui, &mut event.end, start + 1..=u64::MAX / 4, frame, editable);
                ui.label(timecode(event.end.saturating_sub(event.start)));
                for &angle in cameras {
                    let available = event.angles[angle];
                    let (mark, response) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::click());
                    let fill = if event.selected[angle] { ACTIVE_RED }
                        else if event.favourite[angle] { CYAN }
                        else if available { BTN } else { BTN_DARK };
                    ui.painter().rect_filled(mark, 2.0, fill);
                    if response.clicked() && available && editable {
                        event.selected[angle] = !event.selected[angle];
                        dirty = true;
                    }
                    if available && editable {
                        response.context_menu(|ui| {
                            let label = tr(ui, "Favourite");
                            if ui.checkbox(&mut event.favourite[angle], label).changed() {
                                dirty = true;
                                ui.close_menu();
                            }
                        });
                    }
                    let current = s.replay_event == Some(id) && s.replay_ui.angle == angle;
                    let text = if !event.comments[angle].is_empty() { event.comments[angle].clone() }
                        else if available { event.description.clone() }
                        else { String::new() };
                    let cell = ui.add_enabled(available, egui::SelectableLabel::new(current, text));
                    if cell.clicked() {
                        s.replay_event = Some(id);
                        s.replay_ui.angle = angle;
                        s.replay_ui.shift = None;
                    }
                    // A double-click loads the highlight into the selected player, cued at its In Point.
                    if cell.double_clicked() {
                        s.replay_cameras[channel] = angle;
                        s.replay_speed[channel] = event.speed;
                        command = Some(Action::Cue(channel, id));
                    }
                    cell.context_menu(|ui| {
                        for (label, program) in [("Play", false), ("Play to Program", true)] {
                            if ui.button(tr(ui, label)).clicked() {
                                s.replay_event = Some(id);
                                s.replay_ui.angle = angle;
                                s.replay_cameras[channel] = angle;
                                command = Some(Action::Play { event: id, program });
                                ui.close_menu();
                            }
                        }
                        if ui.button(tr(ui, "Export")).clicked() {
                            command = Some(Action::Export { event: id, angle });
                            ui.close_menu();
                        }
                        if editable {
                            ui.menu_button(tr(ui, "Move to Events list"), |ui| {
                                for target in 0..EVENT_LISTS {
                                    if ui.button((target + 1).to_string()).clicked() {
                                        event.list = target as u8;
                                        dirty = true;
                                        ui.close_menu();
                                    }
                                }
                            });
                            if ui.button(tr(ui, "Delete")).clicked() {
                                command = Some(Action::DeleteEvent(id));
                                ui.close_menu();
                            }
                        }
                    });
                }
                ui.end_row();
            }
        });
    });
    if dirty { s.replay_ui.dirty = true; }
    if command.is_some() { s.replay_command = command; }
}

fn playlist(ui: &mut egui::Ui, s: &mut Switcher, height: f32) {
    let index = s.replay_ui.playlist;
    let on_air = s.replay_ui.on_air.is_some_and(|id| s.replay_ui.playlists[index].iter().any(|clip| clip.id == id));
    let title = format!("{} {}", tr(ui, "PLAY LIST"), index + 1);
    title_bar(ui, title, if on_air { ACTIVE_RED } else { PLAYLIST_TAB });
    let frame = s.replay_ui.frame_ms;
    let channel = s.replay_channel.min(1);
    let program = s.replay_setup.play_to_program;
    let selected = s.replay_ui.clip;
    let mut command = None;
    let mut dirty = false;
    let mut pick = None;
    egui::ScrollArea::both().id_salt("replay-playlist").max_height(height - 22.0).auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("replay-playlist-rows").striped(true).spacing(Vec2::new(8.0, 2.0)).show(ui, |ui| {
            for title in ["Angle", "Event", "In Point", "Out Point", "Duration", "Speed", "Mix"] { header(ui, title); }
            ui.end_row();
            for (row, clip) in s.replay_ui.playlists[index].iter_mut().enumerate() {
                let label = ui.selectable_label(selected == Some(row), format!("Cam {}", clip.angle + 1));
                if label.clicked() { pick = Some(row); }
                if label.double_clicked() && clip.id != 0 {
                    s.replay_cameras[channel] = clip.angle;
                    s.replay_speed[channel] = clip.speed;
                    command = Some(Action::Play { event: clip.id, program });
                }
                ui.label(&clip.comment);
                let (start, end) = (clip.start, clip.end);
                dirty |= time_drag(ui, &mut clip.start, 0..=end.saturating_sub(1), frame, true);
                dirty |= time_drag(ui, &mut clip.end, start + 1..=u64::MAX / 4, frame, true);
                ui.label(timecode(clip.end.saturating_sub(clip.start)));
                dirty |= ui.add(egui::DragValue::new(&mut clip.speed).range(1..=100).suffix("%")).changed();
                dirty |= ui.add(egui::DragValue::new(&mut clip.mix_ms).range(0..=5000).speed(10.0).suffix(" ms")).changed();
                ui.end_row();
            }
        });
    });
    if pick.is_some() { s.replay_ui.clip = pick; }
    if dirty { s.replay_ui.dirty = true; }
    if command.is_some() { s.replay_command = command; }
}

fn tags_bar(ui: &mut egui::Ui, s: &mut Switcher) {
    let angle = s.replay_ui.angle;
    let page = s.replay_ui.tag_page;
    let mut toggles: Vec<String> = Vec::new();
    let mut clear = false;
    let mut edit_comment = false;
    let side = 58.0;
    let width = ((ui.available_width() - 2.0 * side - 8.0) / TAG_COLUMNS as f32 - 2.0).max(30.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(2.0);
        let text = format!("{}\n{}/2", tr(ui, "TAGS"), page + 1);
        if ui.add_sized([side, 46.0], egui::Button::new(RichText::new(text).size(10.5))).clicked() {
            s.replay_ui.tag_page = 1 - page;
            s.replay_ui.tag_edit = None;
        }
        ui.vertical(|ui| {
            for row in 0..2 {
                ui.horizontal(|ui| {
                    for column in 0..TAG_COLUMNS {
                        let index = page * TAGS_PER_PAGE + row * TAG_COLUMNS + column;
                        if s.replay_ui.tag_edit.as_ref().is_some_and(|(editing, _)| *editing == index) {
                            let (_, text) = s.replay_ui.tag_edit.as_mut().unwrap();
                            let edit = ui.add_sized([width, 22.0], egui::TextEdit::singleline(text));
                            let done = edit.lost_focus();
                            if !done && !edit.has_focus() { edit.request_focus(); }
                            if done {
                                let (_, text) = s.replay_ui.tag_edit.take().unwrap();
                                s.replay_ui.tags[index] = text.trim().to_owned();
                                s.replay_ui.dirty = true;
                            }
                            continue;
                        }
                        let tag = s.replay_ui.tags[index].clone();
                        let button = ui.add_sized([width, 22.0], egui::Button::new(RichText::new(&tag).size(11.0))
                            .wrap_mode(egui::TextWrapMode::Truncate));
                        if button.double_clicked() || (button.clicked() && tag.is_empty()) {
                            // The first click of a double-click toggled the tag; toggle it back.
                            if !tag.is_empty() { toggles.push(tag.clone()); }
                            s.replay_ui.tag_edit = Some((index, tag));
                        } else if button.clicked() {
                            toggles.push(tag);
                        }
                    }
                });
            }
        });
        ui.vertical(|ui| {
            clear = ui.add_sized([side, 22.0], egui::Button::new(RichText::new(tr(ui, "Clear")).size(11.0))).clicked();
            edit_comment = ui.add_sized([side, 22.0], egui::Button::new(RichText::new(tr(ui, "Edit")).size(11.0))).clicked();
        });
    });
    let event = s.replay_event.and_then(|id| s.replay_events.iter_mut().find(|event| event.id == id))
        .filter(|_| !s.replay_read_only);
    let Some(event) = event else {
        s.replay_ui.comment_edit = None;
        return;
    };
    for tag in toggles {
        let mut parts: Vec<&str> = event.comments[angle].split(", ").filter(|part| !part.is_empty()).collect();
        match parts.iter().position(|part| *part == tag) {
            Some(position) => { parts.remove(position); }
            None => parts.push(&tag),
        }
        event.comments[angle] = parts.join(", ");
        s.replay_ui.dirty = true;
    }
    if clear {
        event.comments[angle].clear();
        s.replay_ui.dirty = true;
    }
    if edit_comment { s.replay_ui.comment_edit = Some(event.comments[angle].clone()); }
    if let Some(text) = &mut s.replay_ui.comment_edit {
        let mut done = false;
        ui.horizontal(|ui| {
            ui.label(tr(ui, "Comment"));
            let edit = ui.add(egui::TextEdit::singleline(text).desired_width(ui.available_width() - 8.0));
            done = edit.lost_focus();
            if !done && !edit.has_focus() { edit.request_focus(); }
        });
        if done {
            event.comments[angle] = s.replay_ui.comment_edit.take().unwrap_or_default().trim().to_owned();
            s.replay_ui.dirty = true;
        }
    }
}

fn tool(ui: &mut egui::Ui, text: &str, width: f32, active: Option<Color32>) -> egui::Response {
    let text = tr(ui, text);
    let mut button = egui::Button::new(RichText::new(text).size(12.0).color(TEXT));
    if let Some(colour) = active { button = button.fill(colour); }
    ui.add_sized([width, ROW], button)
}

fn tool_if(ui: &mut egui::Ui, enabled: bool, text: &str, width: f32, active: Option<Color32>) -> bool {
    ui.add_enabled_ui(enabled, |ui| tool(ui, text, width, active)).inner.clicked()
}

fn group(ui: &mut egui::Ui, text: &str) {
    let text = tr(ui, text);
    ui.label(RichText::new(text).size(10.0).color(TEXT_DIM));
}

/// One player's row: Preview, camera selection and playlist playback.
fn player_row(ui: &mut egui::Ui, s: &mut Switcher, channel: usize, cameras: &[usize], role: &str, command: &mut Option<Action>) {
    let name = RichText::new(["A", "B"][channel]).strong();
    let select = ui.add_sized([22.0, ROW], egui::SelectableLabel::new(s.replay_channel == channel || s.replay_channel == 2, name));
    if select.clicked() && !s.replay_ui.preview_mode { s.replay_channel = channel; }
    if tool(ui, "P", 26.0, None).on_hover_text(tr(ui, "Send this player to Preview")).clicked() {
        let id = s.ensure_replay_inputs()[channel];
        s.preview = Some(id);
    }
    for &angle in cameras {
        let active = (s.replay_cameras[channel] == angle).then_some(EVENTS_TAB);
        if tool(ui, &(angle + 1).to_string(), 26.0, active).clicked() {
            s.replay_cameras[channel] = angle;
            // In Preview Mode the off-air player cues the selected highlight on the new angle.
            if s.replay_ui.preview_mode && s.replay_channel == channel {
                if let Some(event) = s.replay_event { *command = Some(Action::Cue(channel, event)); }
            }
        }
    }
    if tool(ui, "PL", 30.0, None).on_hover_text(tr(ui, "Play the current playlist on this player")).clicked() {
        let items = s.replay_ui.playlists[s.replay_ui.playlist].iter().filter(|clip| clip.id != 0)
            .map(|clip| QueueItem { id: clip.id, angle: clip.angle, speed: clip.speed, mix_ms: clip.mix_ms }).collect();
        *command = Some(Action::PlayQueue { items, channel, program: s.replay_setup.play_to_program,
            looped: s.replay_ui.looped });
    }
    if !role.is_empty() {
        let colour = if role == "PGM" { ACTIVE_RED } else { PREVIEW };
        ui.label(RichText::new(role).size(10.0).strong().color(colour));
    }
}

fn toolbar(ui: &mut egui::Ui, s: &mut Switcher, t: f64, cameras: &[usize], on_air: Option<usize>) {
    let channel = s.replay_channel.min(1);
    let preview_mode = s.replay_ui.preview_mode;
    let program_channel = on_air.unwrap_or(0);
    // In Preview Mode the speed control stays with the player on air.
    let speed_channel = if preview_mode { program_channel } else { channel };
    let angle = s.replay_ui.angle;
    let event = s.replay_event.and_then(|id| s.replay_events.iter().position(|event| event.id == id));
    let frame = s.replay_ui.frame_ms.max(1.0).round() as u64;
    let writable = !s.replay_session.is_empty() && !s.replay_read_only;
    let can_mark = !s.replay_read_only && (s.replay_arm || (!s.replay_live && !s.replay_session.is_empty()));
    let program = s.replay_setup.play_to_program;
    let position = s.replay_ui.position[channel];
    let role = |target: usize| if !preview_mode { "" } else if target == program_channel { "PGM" } else { "PVW" };
    let mut command: Option<Action> = None;

    ui.horizontal(|ui| {
        for out in [false, true] {
            group(ui, if out { "OUT" } else { "IN" });
            let armed = s.replay_ui.shift == Some(out);
            let marking = s.replay_marking == out;
            if tool_if(ui, (can_mark && marking) || armed, "Mark", 48.0, armed.then_some(EVENTS_TAB)) {
                match (armed, event, position) {
                    // Shift In / Shift Out: the cued frame replaces the highlight's point.
                    (true, Some(index), Some(position)) if !s.replay_read_only => {
                        let event = &mut s.replay_events[index];
                        if out && position >= event.start { event.end = position + frame; }
                        if !out && position < event.end { event.start = position; }
                        s.replay_ui.dirty = true;
                        s.replay_ui.shift = None;
                    }
                    _ => command = Some(if out { Action::MarkOut } else { Action::MarkIn }),
                }
            }
            let tip = tr(ui, "Go to this point of the selected highlight, jog to a new frame, then press Mark");
            let shift = ui.add_enabled_ui(event.is_some() && !s.replay_read_only, |ui| tool(ui, "Shift", 48.0, None)).inner;
            if shift.on_hover_text(tip).clicked() {
                let event = &s.replay_events[event.unwrap()];
                if event.angles[angle] { s.replay_cameras[channel] = angle; }
                command = Some(Action::Seek(if out { event.end.saturating_sub(frame) } else { event.start }));
                s.replay_ui.shift = Some(out);
            }
        }
        if s.replay_marking && tool(ui, "Cancel Mark", 84.0, None).clicked() { command = Some(Action::CancelMark); }
        ui.separator();
        player_row(ui, s, 0, cameras, role(0), &mut command);
        ui.separator();
        let shown = position.or(s.replay_ui.clock).map_or_else(|| "--:--:--.---".to_owned(), timecode);
        ui.label(RichText::new(shown).monospace().size(18.0).color(TEXT));
        ui.separator();
        group(ui, "PLAYLIST");
        let source = event.map(|index| &s.replay_events[index]).filter(|event| event.angles[angle]).map(|event| Clip {
            id: 0, angle, start: event.start, end: event.end, speed: event.speed, mix_ms: s.replay_setup.transition_ms,
            comment: if event.comments[angle].is_empty() { event.description.clone() } else { event.comments[angle].clone() },
        });
        let selected = s.replay_ui.clip;
        let list = &mut s.replay_ui.playlists[s.replay_ui.playlist];
        let selected = selected.filter(|&row| row < list.len());
        let mut changed = false;
        if tool_if(ui, source.is_some(), "Add", 44.0, None) {
            list.extend(source.clone());
            changed = true;
        }
        if tool_if(ui, source.is_some(), "Insert", 50.0, None) {
            list.insert(selected.unwrap_or(list.len()), source.unwrap());
            changed = true;
        }
        if tool_if(ui, selected.is_some(), "Delete", 50.0, None) {
            list.remove(selected.unwrap());
            s.replay_ui.clip = None;
            changed = true;
        }
        if tool_if(ui, selected.is_some_and(|row| row > 0), "Up", 34.0, None) {
            let row = selected.unwrap();
            list.swap(row, row - 1);
            s.replay_ui.clip = Some(row - 1);
            changed = true;
        }
        if tool_if(ui, selected.is_some_and(|row| row + 1 < list.len()), "Down", 44.0, None) {
            let row = selected.unwrap();
            list.swap(row, row + 1);
            s.replay_ui.clip = Some(row + 1);
            changed = true;
        }
        if changed { s.replay_ui.dirty = true; }
        if tool(ui, "Loop", 44.0, s.replay_ui.looped.then_some(PLAYLIST_TAB)).clicked() { s.replay_ui.looped = !s.replay_ui.looped; }
        ui.separator();
        if tool(ui, "⏭ Live", 56.0, None).on_hover_text(tr(ui, "Jump to the most recent recording")).clicked() {
            command = Some(Action::Live);
        }
        if tool_if(ui, writable, "REC", 48.0, s.replay_arm.then_some(ACTIVE_RED)) { s.replay_arm = !s.replay_arm; }
    });

    ui.horizontal(|ui| {
        for seconds in [5_u64, 10, 15, 20] {
            if tool_if(ui, can_mark, &format!("-{seconds}"), 36.0, None) { command = Some(Action::MarkLast(seconds * 1000)); }
        }
        ui.separator();
        if tool(ui, "Preview Mode", 96.0, preview_mode.then_some(PLAYLIST_TAB))
            .on_hover_text(tr(ui, "Cue and jog the off-air player while the other plays, then Take")).clicked()
        {
            s.replay_ui.preview_mode = !preview_mode;
        }
        ui.separator();
        player_row(ui, s, 1, cameras, role(1), &mut command);
        if tool(ui, "Swap", 44.0, None).on_hover_text(tr(ui, "Swap the cameras of A and B")).clicked() { s.replay_cameras.swap(0, 1); }
        ui.separator();
        ui.label(RichText::new(format!("{}%", s.replay_speed[speed_channel])).monospace().size(18.0).color(TEXT));
        ui.separator();
        for speed in [25_u8, 33, 50, 75, 100] {
            let active = (s.replay_speed[speed_channel] == speed).then_some(MONITOR_ON);
            if tool(ui, &format!("{speed}%"), 44.0, active).clicked() { s.replay_speed[speed_channel] = speed; }
        }
        ui.separator();
        if tool_if(ui, event.is_some(), "Export Clips", 88.0, None) {
            command = Some(Action::Export { event: s.replay_events[event.unwrap()].id, angle });
        }
    });

    ui.horizontal(|ui| {
        let active = s.replay_ui.active[channel];
        if tool(ui, if active { "⏸" } else { "▶" }, 40.0, None).on_hover_text(tr(ui, "Play / Pause")).clicked() {
            command = Some(if active || s.replay_ui.paused[channel] { Action::Pause(channel) }
                else if s.replay_ui.cued[channel] { Action::TakeCue(channel, program) }
                else if let Some(index) = event { Action::Play { event: s.replay_events[index].id, program } }
                else { Action::Pause(channel) });
        }
        if tool(ui, "Rev", 44.0, None).on_hover_text(tr(ui, "Reverse")).clicked() { command = Some(Action::Reverse); }
        if tool_if(ui, !s.replay_events.is_empty(), "Last", 44.0, None) {
            if let Err(error) = s.perform_action("ReplayPlayLast", t) { s.notice(&error); }
        }
        if preview_mode {
            if tool_if(ui, s.replay_ui.cued[channel], "Take", 104.0, Some(ACTIVE_RED)) {
                command = Some(Action::TakeCue(channel, true));
            }
        } else if tool(ui, "Play Selected", 104.0, s.replay_ui.queue.then_some(EVENTS_TAB)).clicked() {
            let list = s.replay_ui.list;
            let mut events: Vec<_> = s.replay_events.iter().filter(|event| usize::from(event.list) == list).collect();
            events.sort_by_key(|event| (event.start, event.id));
            let mix_ms = s.replay_setup.transition_ms;
            let items = events.into_iter().flat_map(|event| (0..REPLAY_ANGLES)
                .filter(|&angle| event.selected[angle] && event.angles[angle])
                .map(|angle| QueueItem { id: event.id, angle, speed: event.speed, mix_ms })).collect();
            command = Some(Action::PlayQueue { items, channel, program, looped: false });
        }
        if tool_if(ui, event.is_some(), "⏮", 34.0, None) { command = Some(Action::Cue(channel, s.replay_events[event.unwrap()].id)); }
        if tool_if(ui, event.is_some(), "⏭", 34.0, None) { command = Some(Action::CueOut(channel, s.replay_events[event.unwrap()].id)); }
        if tool(ui, "Stop", 44.0, None).clicked() { command = Some(Action::Stop(channel)); }
        for step in [-1_i8, 1] {
            if tool(ui, &format!("{step:+}"), 30.0, None).on_hover_text(tr(ui, "Jog by one frame")).clicked() {
                command = Some(if preview_mode { Action::Jog(channel, step) } else { Action::Step(step) });
            }
        }
        ui.separator();
        ui.spacing_mut().slider_width = (ui.available_width() - 150.0).max(120.0);
        ui.add(egui::Slider::new(&mut s.replay_speed[speed_channel], 0..=100).suffix("%").text(tr(ui, "Replay speed")))
            .on_hover_text(tr(ui, "0% pauses this channel. Other values apply when playback starts."));
    });

    if let Some((start, end, position)) = s.replay_timeline.filter(|(start, end, _)| start < end) {
        let id = ui.id().with(("replay-timeline", s.replay_channel));
        let mut draft = ui.data(|data| data.get_temp::<u64>(id)).unwrap_or(position).clamp(start, end);
        ui.spacing_mut().slider_width = (ui.available_width() - 230.0).max(120.0);
        let response = ui.add(egui::Slider::new(&mut draft, start..=end).text(tr(ui, "Recorded position"))
            .custom_formatter(|value, _| timecode(value.max(0.0).round() as u64)));
        if response.dragged() {
            ui.data_mut(|data| data.insert_temp(id, draft));
        } else {
            if response.drag_stopped() || response.changed() { command = Some(Action::Seek(draft)); }
            ui.data_mut(|data| data.remove::<u64>(id));
        }
    }
    if command.is_some() { s.replay_command = command; }
}
