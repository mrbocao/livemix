//! Shared widgets and the application dialogs.
use super::*;

pub(crate) fn setting_toggle(ui: &mut egui::Ui, value: &mut bool, label: impl Into<egui::WidgetText>) -> egui::Response {
    let label = label.into();
    let translation = tr(ui, label.text());
    let label = if translation == label.text() { label } else { translation.to_owned().into() };
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

/// Setup windows opened by the gear buttons: Recording Setup, Streaming Settings, MultiCorder
/// and PlayList.
pub(crate) fn setup_windows(ctx: &egui::Context, s: &mut Switcher) {
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
        egui::Window::new(locale::text(ctx, "Recording Setup")).id(egui::Id::new("Recording Setup")).open(&mut open).collapsible(false).resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).default_width(590.0).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(8.0);
            let tab_id = ui.id().with("recording-slot");
            let mut secondary = ui.data(|d| d.get_temp::<bool>(tab_id)).unwrap_or(false);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut secondary, false, tr(ui, "Recording 1"));
                ui.selectable_value(&mut secondary, true, tr(ui, "Recording 2"));
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
                ui.label(tr(ui, "Filename:"));
                ui.horizontal(|ui| {
                    folder_line(ui, &s.cfg.rec_folder);
                    if ui.button(tr(ui, "Browse")).clicked() {
                        s.browse_for = 1;
                        s.browse_req = Some(media::PickKind::Folder);
                    }
                    if !s.cfg.rec_folder.is_empty() && ui.small_button(tr(ui, "Default")).clicked() {
                        s.cfg.rec_folder.clear();
                    }
                });
                ui.end_row();
                ui.label(tr(ui, "Name:"));
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
                ui.label(tr(ui, "Source:"));
                let before = *source;
                egui::ComboBox::from_id_salt("rec-source").width(240.0)
                    .selected_text(&output_names[*source]).show_ui(ui, |ui| {
                        for (index, name) in output_names.iter().enumerate() {
                            ui.selectable_value(source, index, name);
                        }
                    });
                changed |= *source != before;
                ui.end_row();
                ui.label(tr(ui, "Video codec"));
                changed |= recording_profile(ui, "rec-profile", codec);
                ui.end_row();
                ui.label(tr(ui, "Video bitrate"));
                changed |= ui.add_enabled(codec.ffmpeg_profile().bitrate,
                    egui::DragValue::new(mbps).range(1..=100).suffix(" Mbps")).changed();
                ui.end_row();
                ui.label(tr(ui, "Audio track"));
                changed |= setting_toggle(ui, &mut s.cfg.record_audio[usize::from(secondary)],
                    format!("{} · {}", codec.audio_codec(), tr(ui, "Stereo"))).changed();
                ui.end_row();
                ui.label(tr(ui, "New File Every:"));
                ui.horizontal(|ui| {
                    changed |= new_file_every(ui, "rec-restart", &mut s.cfg.rec_split_min);
                    ui.label(tr(ui, "Minutes"));
                });
                ui.end_row();
            });
            setting_toggle(ui, &mut s.cfg.recording_confirm, "Ask before stopping (Display confirmation for Record, External, Stream and MultiCorder buttons)");
            ui.label(RichText::new(tr(ui, "Both slots share the folder and split interval.")).size(11.0).color(TEXT_DIM));
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
        egui::Window::new(locale::text(ctx, "Streaming Settings")).id(egui::Id::new("Streaming Settings")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
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
                ui.label(tr(ui, "Destination"));
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
                ui.label(tr(ui, "Stream Name or Key"));
                ui.add(egui::TextEdit::singleline(&mut destination.key).password(true).desired_width(320.0));
                ui.end_row();
                ui.label(tr(ui, "Resolution"));
                let q = usize::from(destination.quality).min(stream::QUALITIES.len() - 1);
                egui::ComboBox::from_id_salt("stream-q").width(240.0).selected_text(stream::QUALITIES[q].0).show_ui(ui, |ui| {
                    for (k, (label, [w, h], mbps)) in stream::QUALITIES.iter().enumerate() {
                        ui.selectable_value(&mut destination.quality, k as u8, format!("{label} · {w}×{h} · {mbps} Mbps"));
                    }
                });
                ui.end_row();
                ui.label(tr(ui, "Video bitrate"));
                ui.horizontal(|ui| {
                    let mut bitrate = destination.bitrate.unwrap_or(stream::QUALITIES[q].2);
                    if ui.add(egui::DragValue::new(&mut bitrate).range(1..=100).suffix(" Mbps")).changed() {
                        destination.bitrate = Some(bitrate);
                    }
                    if ui.small_button(tr(ui, "Preset")).clicked() { destination.bitrate = None; }
                });
                ui.end_row();
                ui.label(tr(ui, "Video codec"));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut destination.hevc, false, "H.264");
                    ui.selectable_value(&mut destination.hevc, true, "H.265 (HEVC)");
                });
                ui.end_row();
                ui.label(tr(ui, "Source"));
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
                ui.label(tr(ui, "Audio source"));
                egui::ComboBox::from_id_salt("stream-audio-source")
                    .selected_text(tr(ui, if destination.source_audio { "Selected video source" } else { "Master mix" }))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut destination.source_audio, false, tr(ui, "Master mix"));
                        ui.selectable_value(&mut destination.source_audio, true, tr(ui, "Selected video source"));
                    });
                ui.end_row();
                ui.label(tr(ui, "Encoder"));
                setting_toggle(ui, &mut destination.hardware, stream::HARDWARE_ENCODER_LABEL);
                ui.end_row();
            }));
            ui.label(RichText::new("FFmpeg · RTMP/RTMPS · stereo AAC").size(11.0).color(TEXT_DIM));
            ui.label(&s.stream_status[index]);
            stream_log(ui, &s.stream_logs[index]);
            ui.horizontal(|ui| {
                if ui.add_enabled(*active || !destination.url.trim().is_empty(),
                    egui::Button::new(tr(ui, if *active { "Stop" } else { "Start" }))).clicked() {
                    *active = !*active;
                }
                if ui.button(tr(ui, "Save and Close")).clicked() { s.stream_setup = false; }
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
                        ui.label(tr(ui, "Video codec"));
                        changed |= recording_profile(ui, "mc-profile", &mut s.cfg.mc_codec);
                        ui.end_row();
                        ui.label(tr(ui, "Video bitrate"));
                        changed |= ui.add_enabled(s.cfg.mc_codec.ffmpeg_profile().bitrate,
                            egui::DragValue::new(&mut s.cfg.mc_mbps).range(1..=100).suffix(" Mbps")).changed();
                        ui.end_row();
                        ui.label(tr(ui, "Audio track"));
                        changed |= setting_toggle(ui, &mut s.cfg.record_audio[2],
                            format!("{} · {}", s.cfg.mc_codec.audio_codec(), tr(ui, "Stereo"))).changed();
                        ui.end_row();
                        ui.label(tr(ui, "New File Every:"));
                        ui.horizontal(|ui| {
                            changed |= new_file_every(ui, "mc-restart", &mut s.cfg.mc_split_min);
                            ui.label(tr(ui, "Minutes"));
                        });
                        ui.end_row();
                    });
                });
                if ui.add_sized([86.0, 78.0], egui::Button::new(tr(ui, if running { "Stop" } else { "Start" })).fill(BTN)).clicked() {
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
                    ui.strong(tr(ui, "Input"));
                    ui.strong(tr(ui, "Folder"));
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
                if ui.button(tr(ui, "Change Recording Folder")).clicked() {
                    s.browse_for = 2;
                    s.browse_req = Some(media::PickKind::Folder);
                }
                if ui.button(tr(ui, "Close")).clicked() {
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
        egui::Window::new(locale::text(ctx, "PlayList")).id(egui::Id::new("PlayList")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
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
                ui.label(tr(ui, "Duration:"));
                ui.add(egui::DragValue::new(&mut s.playlist_secs).range(1.0..=600.0).suffix(" s"));
                ui.checkbox(&mut s.cfg.playlist_loop, "Loop");
            });
            ui.horizontal(|ui| {
                let running = s.playlist.is_some();
                if ui.button(if running { "Stop" } else { "Start" }).clicked() {
                    s.playlist = if running { None } else { Some(t - s.playlist_secs as f64) };
                }
                if ui.button(tr(ui, "Close")).clicked() {
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
pub(crate) fn new_preset_windows(ctx: &egui::Context, s: &mut Switcher, last_autosave: &mut LastAutosave) {
    match s.new_step {
        NewStep::Idle | NewStep::WaitingForSave => {}
        NewStep::SaveChanges => {
            let mut next = None;
            egui::Window::new("LiveMix").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.add_space(4.0);
                // sr::c passes the preset's name ("Untitled" when never saved) to the question.
                ui.label(RichText::new(s.preset_name.clone()).strong());
                ui.label(tr(ui, "Do you wish to save changes to the current preset?"));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("  Save  ").clicked() {
                        next = Some(1);
                    }
                    if ui.button("Discard changes").clicked() {
                        next = Some(2);
                    }
                    if ui.button(tr(ui, "Cancel")).clicked() {
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
            egui::Window::new(locale::text(ctx, "New Preset")).id(egui::Id::new("New Preset")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(tr(ui, "Create a new blank preset with the following video format"));
                ui.add_space(8.0);
                let (size, rate, il) = s.new_format;
                egui::Grid::new("new-preset").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label(tr(ui, "Resolution"));
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
                    ui.label(tr(ui, "Frame Rate"));
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
                    if ui.button(tr(ui, "Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
            if ok {
                let (size, rate, il) = s.new_format;
                s.new_step = NewStep::Idle;
                last_autosave.flush(s);
                s.new_preset_with(size, rate, il);
            } else if cancel {
                s.new_step = NewStep::Idle;
            }
        }
    }
}

pub(crate) fn dialogs(ctx: &egui::Context, s: &mut Switcher, graphics_adapter: &str, encoder: &str, program_path: &str, mixer: &mut MixerControls, controls: &mut controls::Controls, last_autosave: &mut LastAutosave) {
    let t = ctx.input(|input| input.time);
    replay_windows(ctx, s, t);
    if let Some(report) = &s.import_report {
        let mut open = true;
        let mut close = false;
        egui::Window::new(locale::text(ctx, "Project Import Report")).id(egui::Id::new("Project Import Report")).open(&mut open).default_width(560.0).show(ctx, |ui| {
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
            close = ui.button(tr(ui, "Done")).clicked();
        });
        if !open || close { s.import_report = None; }
    }
    srt_output_dialog(ctx, s);
    new_preset_windows(ctx, s, last_autosave);
    setup_windows(ctx, s);
    // RecordingConfirm (config default True): "Stop Recording?" Yes / No (ConfirmRecording @0x250d10).
    if let Some(kind) = s.confirm_stop {
        egui::Window::new(locale::text(ctx, "Confirm")).id(egui::Id::new("Confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("Stop {kind}?")).size(15.0).color(Color32::WHITE));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add_sized([80.0, 26.0], egui::Button::new(tr(ui, "Yes"))).clicked() {
                        if kind == "Recording" {
                            s.rec = None;
                        } else {
                            s.multicorder = None;
                        }
                        s.confirm_stop = None;
                    }
                    if ui.add_sized([80.0, 26.0], egui::Button::new(tr(ui, "No"))).clicked() {
                        s.confirm_stop = None;
                    }
                });
            });
    }
    if s.add_dialog.open {
        let mut open = true;
        let mut done = false;
        egui::Window::new(locale::text(ctx, "Input Select")).id(egui::Id::new("Input Select")).open(&mut open).collapsible(false).resizable(false)
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
                                ui.label(RichText::new(tr(ui, "Extras")).size(10.0).color(TEXT_DIM));
                            }
                            let label = if k.available() {
                                RichText::new(tr(ui, k.label()))
                            } else {
                                RichText::new(tr(ui, k.label())).color(TEXT_DIM.gamma_multiply(0.7)).italics()
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
                    ui.label(RichText::new(tr(ui, s.add_dialog.kind.label())).strong());
                    ui.label(RichText::new(s.add_dialog.kind.help()).color(TEXT_DIM).size(11.0));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(tr(ui, "Name"));
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
                                    ui.label(tr(ui, "Colour"));
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
                                ui.label(tr(ui, "Folder"));
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
                                if ui.button(tr(ui, "Refresh")).clicked() { d.ndi_scan = true; }
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
                                ui.label(tr(ui, "Length (s)"));
                                ui.add(egui::DragValue::new(&mut s.add_dialog.length).range(1.0..=3600.0));
                            });
                        }
                        AddKind::VideoDelay => {
                            // frmInput tVideoDelay: Source, buffer Seconds, Resolution.
                            let choices: Vec<(u32, usize, String)> = s.inputs.iter().enumerate()
                                .map(|(n, i)| (i.id, n + 1, i.title.clone())).collect();
                            let d = &mut s.add_dialog;
                            ui.horizontal(|ui| {
                                ui.label(tr(ui, "Source"));
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
                                ui.label(tr(ui, "Seconds"));
                                ui.add(egui::Slider::new(&mut d.delay_seconds, 1.0..=120.0).suffix(" s"));
                            });
                            let [w, h] = DELAY_RESOLUTIONS[d.delay_resolution.min(DELAY_RESOLUTIONS.len() - 1)];
                            ui.horizontal(|ui| {
                                ui.label(tr(ui, "Resolution"));
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
                        AddKind::Teleprompter => {
                            ui.horizontal(|ui| {
                                ui.label("Script");
                                if ui.button("Load text file…").clicked() {
                                    s.browse_req = Some(media::PickKind::Script(None));
                                }
                            });
                            ui.add(egui::TextEdit::multiline(&mut s.add_dialog.script).desired_rows(8)
                                .desired_width(f32::INFINITY).hint_text("Paste the script here (it can be edited later in Input Settings)"));
                            ui.label(RichText::new("The text scrolls while the input plays. Speed, size and mirroring are in Input Settings → Teleprompter.")
                                .size(11.0).color(TEXT_DIM));
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
                        AddKind::Camera | AddKind::AudioInput => {
                            let list = s.dialog_devices();
                            if s.add_dialog.kind == AddKind::Camera && list.iter().any(|device| device.blackmagic) {
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
                                ui.label(tr(ui, "Device"));
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
                                if ui.button(tr(ui, "Refresh")).clicked() {
                                    s.refresh_devices = true;
                                }
                            });
                            if let Some(device) = list.get(s.add_dialog.device).and_then(|d| decklink::device_index(&d.link)) {
                                ui.horizontal(|ui| {
                                    ui.label(tr(ui, "Connector"));
                                    decklink_connector(ui, "add-decklink-connector", &mut s.add_dialog.decklink_port);
                                });
                                ui.horizontal(|ui| {
                                    ui.label(tr(ui, "Video format"));
                                    decklink_mode_picker(ui, "add-decklink-mode", device, false, &mut s.add_dialog.decklink_mode);
                                });
                            }
                            if s.add_dialog.kind == AddKind::Camera {
                                let link = list.get(s.add_dialog.device).map(|d| d.link.clone())
                                    .filter(|link| decklink::device_index(link).is_none());
                                camera_mode_section(ui, &mut s.add_dialog, link);
                            }
                            if s.add_dialog.devices_loaded {
                                ui.label(RichText::new(format!("{} device(s) found on this computer", list.len())).size(11.0).color(TEXT_DIM));
                            }
                        }
                        AddKind::WebBrowser => {
                            ui.label(tr(ui, "Page URL"));
                            ui.add(egui::TextEdit::singleline(&mut s.add_dialog.path).hint_text("https://").desired_width(320.0));
                        }
                        AddKind::VideoFile | AddKind::ImageFile | AddKind::AudioFile | AddKind::PowerPoint => {
                            ui.horizontal(|ui| {
                                ui.label(tr(ui, "File"));
                                ui.add(egui::TextEdit::singleline(&mut s.add_dialog.path).desired_width(190.0));
                                if ui.button("Browse…").clicked() {
                                    s.browse_req = Some(match s.add_dialog.kind {
                                        AddKind::VideoFile => media::PickKind::Video,
                                        AddKind::AudioFile => media::PickKind::Audio,
                                        AddKind::PowerPoint => media::PickKind::Presentation,
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
                if ui.button(tr(ui, "Cancel")).clicked() {
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
        let page_height = (ctx.screen_rect().height() - 150.0).clamp(160.0, 620.0);
        let clip = s.position_clipboard;
        let output = s.output;
        let project = { let (w, h) = s.cfg.out_dims(); ([w, h], s.cfg.frame_rate()) };
        let web = s.remote.is_some();
        let mut copy_pos = None;
        let mut browse_lut = false;
        let mut browse_script = false;
        if let Some(inp) = s.input_mut(id) {
            egui::Window::new(format!("{} — {} {}", locale::text(ctx, "Input Settings"), number, inp.title))
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
                                ("Source", &[0, 12, 13]), ("Picture", &[1, 2, 3, 5, 6, 7]),
                                ("Sound", &[4]), ("Automation", &[8, 9, 10, 11]),
                            ], &mut page, page_height);
                        });
                        ui.separator();
                        ui.vertical(|ui| {
                            let width = ui.available_width().clamp(340.0, 900.0);
                            ui.set_width(width);
                            ui.label(RichText::new(tr(ui, INPUT_PAGES[page])).strong().size(15.0).color(Color32::WHITE));
                            ui.add_space(6.0);
                            let height = page_height;
                            egui::ScrollArea::vertical().id_salt(("input-properties", id, page))
                                .auto_shrink([false, false]).max_height(height).show(ui, |ui| match page {
                                0 => input_general(ui, inp, &others, &mut page, project),
                                1 => input_colour_adjust(ui, inp),
                                2 => input_colour_key(ui, inp),
                                3 => input_colour_correction(ui, inp, &mut browse_lut),
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
                                    ui.label(tr(ui, "Run when this input goes to Program"));
                                    ui.add(egui::TextEdit::multiline(&mut inp.on_program).code_editor().desired_rows(6));
                                    ui.label("Commands: Preview 2, Cut, Fade 500, Wait 1000.");
                                }
                                9 => input_tally(ui, inp, number),
                                10 => input_ptz(ui, inp),
                                11 => input_advanced(ui, inp, number),
                                12 => input_telestrator(ui, inp, &all, t, output, web),
                                13 => input_teleprompter(ui, inp, &mut browse_script),
                                _ => {
                                    ui.label(RichText::new("This section of the original is not available in this version yet.").color(TEXT_DIM));
                                }
                            });
                        });
                    });
                    ui.separator();
                    ui.allocate_ui_with_layout(Vec2::new(ui.available_width(), 24.0),
                        egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(tr(ui, "Close")).clicked() {
                            close = true;
                        }
                    });
                });
        } else {
            open = false;
        }
        s.cfg.input_page = page;
        if browse_lut { s.browse_req = Some(media::PickKind::Lut(id)); }
        if browse_script { s.browse_req = Some(media::PickKind::Script(Some(id))); }
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
        let page_height = (screen.height() - 150.0).clamp(160.0, 620.0);
        let page_width = (screen.width() - 250.0).clamp(340.0, 700.0);
        egui::Window::new(locale::text(ctx, "Settings")).id(egui::Id::new("Settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([page_width + 210.0, page_height + 58.0])
            .default_pos(screen.center() - Vec2::new((page_width + 210.0) / 2.0, (page_height + 110.0) / 2.0))
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
                            settings_page(ui, s, graphics_adapter, encoder, program_path, controls);
                        });
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr(ui, "Import")).clicked() { s.browse_req = Some(media::PickKind::SettingsOpen); }
                    if ui.button(tr(ui, "Export")).clicked() { s.browse_req = Some(media::PickKind::SettingsSave); }
                    ui.menu_button(tr(ui, "Default"), |ui| {
                        ui.label(tr(ui, "Restore the original LiveMix settings?"));
                        if ui.button(tr(ui, "Restore defaults")).clicked() {
                            let result = s.reset_settings();
                            s.notice(&result.map_or_else(|error| error, |_| "Default settings restored".into()));
                            ui.close_menu();
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("  OK  ").clicked() { close = true; }
                    });
                });
            });
        if !open || close {
            s.settings_open = false;
            s.save_settings();
        }
    }

    if s.overlay_dialog {
        let mut open = true;
        egui::Window::new(locale::text(ctx, "Overlay Settings")).id(egui::Id::new("Overlay Settings")).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
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
/// Add Input, Camera: the capture options the selected camera reports (Resolution, Frame Rate,
/// Video Format). They are read from the device on a worker thread when it is selected.
fn camera_mode_section(ui: &mut egui::Ui, d: &mut AddDialog, link: Option<String>) {
    if let Some((read_for, modes)) = d.cam_modes_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
        d.cam_modes_rx = None;
        if read_for == d.cam_modes_link {
            d.cam_modes = modes;
        }
    }
    let Some(link) = link else {
        d.cam_modes_link.clear();
        d.cam_modes.clear();
        d.cam_mode = None;
        d.cam_modes_rx = None;
        return;
    };
    if d.cam_modes_link != link {
        d.cam_modes_link.clone_from(&link);
        d.cam_modes.clear();
        d.cam_mode = None;
        // Capture cards expose their audio as a separate input that usually carries the card's
        // name; pick it when one matches, else leave audio off.
        let camera_name = d.devices.iter().find(|x| x.link == link).map(|x| x.name.to_lowercase()).unwrap_or_default();
        d.cam_audio = d.devices.iter().filter(|x| x.link.starts_with("audio:")).find(|x| {
            let audio = x.name.to_lowercase();
            !camera_name.is_empty() && (audio.contains(&camera_name) || camera_name.contains(&audio))
        }).map(|x| x.link["audio:".len()..].to_owned());
        let (tx, rx) = std::sync::mpsc::channel();
        d.cam_modes_rx = Some(rx);
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            let modes = media::camera_modes(&link);
            let _ = tx.send((link, modes));
            ctx.request_repaint();
        });
    }
    let reading = d.cam_modes_rx.is_some();
    egui::Grid::new("add-camera-modes").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
        let text = if reading { "Reading the camera's formats…" } else { "The camera did not report its formats (Auto)." };
        sources::camera_mode_fields(ui, u32::MAX, &d.cam_modes, None, &mut d.cam_mode, text);
        let audio: Vec<media::DeviceInfo> = d.devices.iter().filter(|x| x.link.starts_with("audio:"))
            .map(|x| media::DeviceInfo { name: x.name.clone(), link: x.link.clone(), blackmagic: false }).collect();
        sources::camera_audio_row(ui, u32::MAX, &audio, &mut d.cam_audio);
    });
}

pub(crate) fn gear_btn(ui: &mut egui::Ui, open: bool, tip: &str) -> egui::Response {
    let tip = tr(ui, tip);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(38.0, 30.0), Sense::click());
    let p = ui.painter();
    let fill = if open { HOVER } else if resp.hovered() { BTN } else { BTN_DARK };
    p.rect_filled(r, 0.0, fill);
    gear_icon(p, r.center(), 8.0, Color32::WHITE, fill);
    resp.on_hover_text(tip)
}

/// A cog wheel: eight teeth round a ring with a hole.
pub(crate) fn gear_icon(p: &egui::Painter, c: egui::Pos2, radius: f32, col: Color32, bg: Color32) {
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

pub(crate) fn big_btn(ui: &mut egui::Ui, text: &str, active: bool, active_col: Option<Color32>) -> egui::Response {
    let text = tr(ui, text);
    let fill = if active { active_col.unwrap_or(HOVER) } else { BTN_DARK };
    let b = egui::Button::new(RichText::new(text).strong().size(13.0).color(Color32::WHITE)).fill(fill).stroke(Stroke::NONE).min_size(Vec2::new(116.0, 30.0));
    ui.add(b)
}
