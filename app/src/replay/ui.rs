//! Replay controller windows and the compact Replay strip.
use crate::*;

pub(crate) const REPLAY_STRIP_WIDTH: f32 = 58.0;

pub(crate) fn replay_strip(ui: &mut egui::Ui, s: &mut Switcher, t: f64) {
    ui.spacing_mut().item_spacing = Vec2::new(2.0, 4.0);
    ui.set_width(REPLAY_STRIP_WIDTH);
    let writable = !s.replay_session.is_empty() && !s.replay_read_only;
    let record = egui::Button::new("REC").selected(s.replay_arm).min_size(Vec2::new(48.0, 20.0));
    if ui.add_enabled(writable, record).on_hover_text(tr(ui, "Record Replay")).clicked() {
        let command = if s.replay_arm { "ReplayStopRecording" } else { "ReplayStartRecording" };
        if let Err(error) = s.perform_action(command, t) { s.notice(&error); }
    }
    for seconds in [5, 10] {
        let button = egui::Button::new(format!("−{seconds}s")).min_size(Vec2::new(48.0, 20.0));
        if ui.add_enabled(s.replay_arm, button).on_hover_text(tr(ui, "Mark last seconds")).clicked() {
            s.replay_command = Some(replay::Action::MarkLast(seconds * 1000));
        }
    }
    let play = egui::Button::new("▶").min_size(Vec2::new(48.0, 20.0));
    if ui.add_enabled(!s.replay_events.is_empty(), play).on_hover_text(tr(ui, "Play last event to Program")).clicked() {
        s.replay_channel = 0;
        if let Err(error) = s.perform_action("ReplayPlayLast", t) { s.notice(&error); }
    }
    ui.add(egui::DragValue::new(&mut s.replay_speed[0]).range(0..=100).suffix("%"))
        .on_hover_text(tr(ui, "Replay A speed"));
    if ui.button(tr(ui, "Setup")).clicked() { s.replay_setup_open = true; }
}

pub(crate) fn draw_replay_camera(painter: &egui::Painter, rect: egui::Rect, s: &Switcher, t: f64, source: u32) {
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    draw_src(painter, rect, s, t, source, false);
}

pub(crate) fn replay_windows(ctx: &egui::Context, s: &mut Switcher, t: f64) {
    if s.replay_setup_open {
        let choices: Vec<_> = std::iter::once((MV_AUTO, "Off".into()))
            .chain(std::iter::once((SRC_OUTPUT, "Program".into())))
            .chain(s.inputs.iter().filter(|input| !matches!(input.kind, Kind::Replay { .. }))
                .map(|input| (input.id + 1, input.title.clone()))).collect();
        egui::Window::new(locale::text(ctx, "Instant Replay Setup")).id(egui::Id::new("Instant Replay Setup")).open(&mut s.replay_setup_open).default_width(570.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(330.0).show(ui, |ui| {
                ui.add_enabled_ui(!s.replay_arm, |ui| {
                    let setup = &mut s.replay_setup;
                    ui.horizontal(|ui| {
                        ui.label(tr(ui, "Session Folder"));
                        ui.add(egui::TextEdit::singleline(&mut setup.folder).desired_width(280.0));
                        if ui.button("Browse…").clicked() {
                            s.browse_for = 3;
                            s.browse_req = Some(media::PickKind::Folder);
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button(tr(ui, "New Session")).clicked() { s.replay_load = Some(true); }
                        if ui.button(tr(ui, "Open Session")).clicked() { s.replay_load = Some(false); }
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
                        ui.label(tr(ui, "Playback Audio Source"));
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
                        ui.label(tr(ui, "Program transition"));
                        ui.add(egui::DragValue::new(&mut setup.transition_ms).range(0..=5000).suffix(" ms"));
                        ui.checkbox(&mut setup.match_speed, "Match Replay Speed");
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(tr(ui, "Recording Buffer"));
                        ui.add(egui::DragValue::new(&mut setup.buffer_seconds).range(10..=3600).suffix(" s"));
                        ui.checkbox(&mut setup.delete_old, "Delete old unmarked video");
                    });
                    let cameras = s.replay_angles.iter().filter(|&&source| source != MV_AUTO).count();
                    let mb = cameras as u64 * u64::from(setup.mbps) * setup.buffer_seconds / 8;
                    ui.label(format!("Estimated rolling video storage: {mb} MB"));
                });
                ui.separator();
                ui.strong(tr(ui, "Replay preview options"));
                let setup = &mut s.replay_setup;
                setting_toggle(ui, &mut setup.show_live_sources, "Show live sources in Replay A and B in Live mode");
                setting_toggle(ui, &mut setup.play_to_program, "Play selected event to Program");
            });
            ui.label(&s.replay_status);
        });
    }

    super::controller::replay_controller(ctx, s, t);
}
