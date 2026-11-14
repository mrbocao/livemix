//! The Settings window and its output, recording and device pages.
use super::*;

// ---------------------------------------------------------------- settings window

/// The original's Settings sections, in its order (language file frmSettings.tMenu.*).
pub(crate) const SETTINGS_PAGES: [&str; 17] = [
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
pub(crate) const SETTINGS_BUILT: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16];

pub(crate) fn settings_navigation(ui: &mut egui::Ui, pages: &[&str], built: &[usize], groups: &[(&str, &[usize])],
    selected: &mut usize, max_height: f32)
{
    egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        for (group, entries) in groups {
            ui.label(RichText::new(tr(ui, group)).strong().color(TEXT_DIM));
            for &index in *entries {
                let label = if built.contains(&index) { RichText::new(tr(ui, pages[index])) }
                    else { RichText::new(tr(ui, pages[index])).color(TEXT_DIM.gamma_multiply(0.7)).italics() };
                ui.selectable_value(selected, index, label);
            }
            ui.add_space(5.0);
        }
    });
}

/// Settings table for the same routes exposed by the External cog.
pub(crate) fn output_routes(ui: &mut egui::Ui, s: &mut Switcher) {
    let choices = src_choices(s, true);
    egui::Grid::new("output-routes").spacing([9.0, 6.0]).show(ui, |ui| {
        for label in ["Output", "Source", "Overlays", "NDI", "OMT", "SRT", "Local window"] {
            ui.strong(tr(ui, label));
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

pub(crate) fn omt_quality(ui: &mut egui::Ui, quality: &mut u8) -> bool {
    let values = [(0, "Default"), (1, "Low"), (50, "Medium"), (100, "High")];
    let before = *quality;
    let label = values.iter().find(|(value, _)| *value == before).map_or("Default", |(_, name)| *name);
    egui::ComboBox::from_id_salt("omt-quality").selected_text(format!("OMT Quality · {label}")).show_ui(ui, |ui| {
        for (value, name) in values { ui.selectable_value(quality, value, name); }
    });
    *quality != before
}

pub(crate) fn srt_connection_rows(ui: &mut egui::Ui, config: &mut srt::Connection) {
    ui.label(tr(ui, "Mode"));
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
    ui.label(tr(ui, "Latency"));
    ui.add(egui::DragValue::new(&mut config.latency_ms).range(20..=8000).suffix(" ms"));
    ui.end_row();
    ui.label(tr(ui, "Passphrase"));
    ui.add(egui::TextEdit::singleline(&mut config.passphrase).password(true));
    ui.end_row();
    if !config.passphrase.is_empty() {
        ui.label(tr(ui, "Key length"));
        egui::ComboBox::from_id_salt("srt-key-bytes").selected_text(format!("{} bit", u16::from(config.key_bytes) * 8))
            .show_ui(ui, |ui| for bytes in [16, 24, 32] {
                ui.selectable_value(&mut config.key_bytes, bytes, format!("{} bit", u16::from(bytes) * 8));
            });
        ui.end_row();
    }
    ui.label(tr(ui, "Stream ID"));
    ui.add_enabled(!config.listen, egui::TextEdit::singleline(&mut config.stream_id));
    ui.end_row();
}

pub(crate) fn stream_log(ui: &mut egui::Ui, log: &str) {
    ui.collapsing(tr(ui, "Streaming log"), |ui| {
        if log.is_empty() {
            ui.label(tr(ui, "Start this stream to see encoder and audio diagnostics."));
        } else {
            if ui.button(tr(ui, "Copy log")).clicked() { ui.ctx().copy_text(log.to_owned()); }
            egui::ScrollArea::vertical().max_height(150.0).stick_to_bottom(true)
                .show(ui, |ui| { ui.monospace(log); });
        }
    });
}

pub(crate) fn srt_output_dialog(ctx: &egui::Context, s: &mut Switcher) {
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
                    ui.label(tr(ui, "Video codec"));
                    egui::ComboBox::from_id_salt("srt-codec").selected_text(if cfg.hevc { "HEVC" } else { "H.264" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut cfg.hevc, false, "H.264");
                            ui.selectable_value(&mut cfg.hevc, true, "HEVC");
                        });
                    ui.end_row();
                    ui.label(tr(ui, "Video bitrate"));
                    ui.add(egui::DragValue::new(&mut cfg.video_kbps).range(100..=100_000).suffix(" kbps"));
                    ui.end_row();
                    ui.label("AAC bitrate");
                    ui.add(egui::DragValue::new(&mut cfg.audio_kbps).range(32..=384).suffix(" kbps"));
                    ui.end_row();
                    ui.label(tr(ui, "Keyframe interval"));
                    ui.add(egui::DragValue::new(&mut cfg.gop_seconds).range(1..=10).suffix(" s"));
                    ui.end_row();
                    ui.label(tr(ui, "Encoder"));
                    ui.checkbox(&mut cfg.hardware, stream::HARDWARE_ENCODER_LABEL);
                    ui.end_row();
                });
            });
            ui.label(RichText::new("Progressive video with stereo AAC.").size(11.0).color(TEXT_DIM));
            ui.label(&s.srt_status[channel]);
            stream_log(ui, &s.srt_logs[channel]);
            ui.horizontal(|ui| {
                if ui.button(if *active { "Stop SRT" } else { "Start SRT" }).clicked() {
                    *active = !*active;
                    s.srt_status[channel].clear();
                }
                close = ui.button(tr(ui, "Save and Close")).clicked();
            });
        });
    if !open || close {
        s.srt_setup = None;
        s.save_settings();
    }
}

pub(crate) fn external_menu(ui: &mut egui::Ui, s: &mut Switcher) {
    let button = egui::Button::new("").fill(BTN_DARK).min_size(Vec2::new(38.0, 30.0));
    let menu = egui::menu::menu_custom_button(ui, button, |ui| external_menu_items(ui, s));
    gear_icon(ui.painter(), menu.response.rect.center(), 8.0, Color32::WHITE, BTN_DARK);
    menu.response.on_hover_text("Configure Output 1–4, NDI, OMT and SRT")
        .context_menu(|ui| external_menu_items(ui, s));
}

pub(crate) fn external_menu_items(ui: &mut egui::Ui, s: &mut Switcher) {
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

pub(crate) fn decklink_connector(ui: &mut egui::Ui, id: impl std::hash::Hash, port: &mut u32) {
    egui::ComboBox::from_id_salt(id).selected_text(decklink::connection_name(*port)).show_ui(ui, |ui| {
        for (label, value) in decklink::CONNECTIONS {
            ui.selectable_value(port, value, label);
        }
    });
}

pub(crate) fn decklink_mode_picker(ui: &mut egui::Ui, id: &str, device: usize, playback: bool, mode: &mut u32) {
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

pub(crate) fn decklink_output_settings(ui: &mut egui::Ui, s: &mut Switcher) {
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
        ui.selectable_value(&mut route.device, None, tr(ui, "None"));
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
                ui.label(tr(ui, "Connector"));
                decklink_connector(ui, "decklink-output-port", &mut route.port);
            }
            ui.label(tr(ui, "Mode"));
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
        ui.label(tr(ui, "No DeckLink playback device detected."));
        if cfg!(target_os = "macos") {
            ui.label("Install Desktop Video and use a DeckLink-enabled FFmpeg build, then refresh.");
        }
    }
}

/// MultiView 1 / 2 (frmSettings cmdMultiView1/2, lblMultiViewLayout "MultiView Layout",
/// cmdMVInputs "Customise Layout"): layout, what each box shows, and a live picture of it.
pub(crate) fn multiview_settings(ui: &mut egui::Ui, s: &mut Switcher) {
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
        ui.label(tr(ui, "MultiView Layout:"));
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

pub(crate) fn edit_tally_colour(ui: &mut egui::Ui, colour: &mut Color32) {
    let mut rgb = [colour.r(), colour.g(), colour.b()];
    if ui.color_edit_button_srgb(&mut rgb).changed() {
        *colour = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    }
}

/// Display: project format, tally colours and theme.
fn display_project(ui: &mut egui::Ui, s: &mut Switcher) {
    egui::Grid::new("set-display").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        // Project format (frmSettings lblOutputFormat / lblOutputSize / lblSystem).
        ui.label(tr(ui, "Output Size:"));
        let (w, h) = s.cfg.out_dims();
        egui::ComboBox::from_id_salt("out-size").selected_text(format!("{w}x{h}")).show_ui(ui, |ui| {
            for (k, (w, h)) in OUTPUT_SIZES.iter().enumerate() {
                ui.selectable_value(&mut s.cfg.out_size, k as u8, format!("{w}x{h}"));
            }
        });
        ui.end_row();
        ui.label(tr(ui, "Master Frame Rate:"));
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
        ui.label(tr(ui, "Output Format:"));
        ui.label(RichText::new(s.cfg.format_label()).color(Color32::from_rgb(0x3c, 0xe0, 0x50)))
            .on_hover_text(format!("Recordings use {w}x{h} at {:.3} frames/s", s.cfg.frame_rate()));
        ui.end_row();
        ui.label(tr(ui, "Input Size:"));
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
        ui.label(tr(ui, "Preview Colour:"));
        ui.horizontal(|ui| {
            edit_tally_colour(ui, &mut s.cfg.preview_colour);
            if ui.small_button(tr(ui, "Reset")).clicked() { s.cfg.preview_colour = PREVIEW; }
        });
        ui.end_row();
        ui.label(tr(ui, "Output Colour:"));
        ui.horizontal(|ui| {
            edit_tally_colour(ui, &mut s.cfg.output_colour);
            if ui.small_button(tr(ui, "Reset")).clicked() { s.cfg.output_colour = OUTPUT; }
        });
        ui.end_row();
        ui.label(tr(ui, "Theme:"));
        egui::ComboBox::from_id_salt("interface-theme")
            .selected_text(tr(ui, s.cfg.theme.label()))
            .show_ui(ui, |ui| {
                for theme in InterfaceTheme::ALL { ui.selectable_value(&mut s.cfg.theme, theme, tr(ui, theme.label())); }
            });
        ui.end_row();
    });
}

/// Display: source and monitor of the two Fullscreen outputs.
fn display_fullscreen(ui: &mut egui::Ui, s: &mut Switcher) {
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
                ui.label(tr(ui, "Source"));
                let label = src_name(s, s.cfg.fs_source[slot]);
                egui::ComboBox::from_id_salt("source").width(235.0).selected_text(label).show_ui(ui, |ui| {
                    for (value, name) in &choices {
                        ui.selectable_value(&mut s.cfg.fs_source[slot], *value, name);
                    }
                });
                ui.end_row();
                ui.label(tr(ui, "Monitor"));
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
    setting_toggle(ui, &mut s.cfg.input_numbers, "Show Input Numbers");
    setting_toggle(ui, &mut s.cfg.production_clock, "Show Production Clock");
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

pub(crate) fn settings_page(ui: &mut egui::Ui, s: &mut Switcher, graphics_adapter: &str, encoder: &str, program_path: &str, controls: &mut controls::Controls) {
    let page = s.cfg.page.min(SETTINGS_PAGES.len() - 1);
    ui.label(RichText::new(tr(ui, SETTINGS_PAGES[page])).strong().size(15.0).color(Color32::WHITE));
    ui.add_space(6.0);
    match page {
        0 => {
            // Display
            if ui.available_width() >= 600.0 {
                ui.columns(2, |columns| {
                    display_project(&mut columns[0], s);
                    display_fullscreen(&mut columns[1], s);
                });
            } else {
                display_project(ui, s);
                ui.add_space(6.0);
                display_fullscreen(ui, s);
            }
            ui.separator();
            multiview_settings(ui, s);
        }
        2 => {
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Interface language"));
                let mut language = s.cfg.language;
                egui::ComboBox::from_id_salt("interface-language").selected_text(language.label()).show_ui(ui, |ui| {
                    for choice in locale::Language::all() {
                        ui.selectable_value(&mut language, choice, choice.label());
                    }
                });
                if language != s.cfg.language {
                    s.cfg.language = language;
                    locale::set(ui.ctx(), language);
                    s.save_settings();
                }
            });
            ui.separator();
            // Options
            egui::Grid::new("set-options").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                ui.label(tr(ui, "QuickPlay Transition:"));
                egui::ComboBox::from_id_salt("qp").selected_text(s.quickplay_effect.name()).show_ui(ui, |ui| {
                    for e in ALL_EFFECTS {
                        ui.selectable_value(&mut s.quickplay_effect, e, e.name());
                    }
                });
                ui.end_row();
                ui.label(tr(ui, "Duration:"));
                ui.add(egui::DragValue::new(&mut s.quickplay_ms).range(1..=9999).speed(10.0).suffix(" Milliseconds"));
                ui.end_row();
                ui.label(tr(ui, "Fade To Black Duration:"));
                ui.add(egui::DragValue::new(&mut s.ftb_ms).range(1..=9999).speed(10.0).suffix(" Milliseconds"));
                ui.end_row();
                ui.label(tr(ui, "PlayList time per Input:"));
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
                ui.label(tr(ui, "Preview Skip:"));
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("perf-skip").selected_text(if s.cfg.preview_skip <= 1 { "None".to_string() } else { format!("Every {} frames", s.cfg.preview_skip) }).show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.cfg.preview_skip, 1, tr(ui, "None"));
                        for n in [2u8, 3, 4] {
                            ui.selectable_value(&mut s.cfg.preview_skip, n, format!("Every {n} frames"));
                        }
                    });
                    ui.label(RichText::new("input boxes redraw less often").size(11.0).color(TEXT_DIM));
                });
                ui.end_row();
                ui.label(tr(ui, "Camera Auto mode up to:"));
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
            ui.strong(tr(ui, "File decoding"));
            setting_toggle(ui, &mut s.cfg.ffmpeg_hardware, "Use GPU decoding for high-resolution FFmpeg files");
            if cfg!(windows) {
                setting_toggle(ui, &mut s.cfg.ffmpeg_all_files, "Use FFmpeg for all video files");
                ui.add_enabled_ui(!s.cfg.ffmpeg_all_files, |ui| {
                    ui.label(tr(ui, "Use FFmpeg for these extensions"));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.cfg.ffmpeg_extensions)
                            .hint_text("*.mov, *.qt, *.mxf, *.mp4").desired_width(230.0));
                        if ui.button(tr(ui, "Common formats")).clicked() {
                            s.cfg.ffmpeg_extensions = "*.mov,*.qt,*.mxf,*.mp4".into();
                        }
                    });
                    if !s.cfg.directshow.enabled {
                        ui.label(tr(ui, "Other video files try Windows Media Foundation first, then FFmpeg if playback cannot start."));
                    }
                });
            } else {
                ui.label(tr(ui, "Video files use FFmpeg on macOS."));
            }
            ui.separator();
            ui.label(tr(ui, "FFmpeg deinterlacing"));
            let methods = ["Off", "BWDIF", "YADIF"];
            egui::ComboBox::from_id_salt("decoder-deinterlace")
                .selected_text(tr(ui, methods[usize::from(s.cfg.decoder_deinterlace.min(2))]))
                .show_ui(ui, |ui| {
                    for (index, method) in methods.into_iter().enumerate() {
                        ui.selectable_value(&mut s.cfg.decoder_deinterlace, index as u8, tr(ui, method));
                    }
                });
            ui.label(RichText::new(tr(ui, "Applies to interlaced video files and playlists. FFmpeg and FFprobe are required.")).color(TEXT_DIM));
            #[cfg(windows)]
            directshow_settings(ui, &mut s.cfg.directshow);
        }
        5 => {
            ui.label(tr(ui, "Default recording folder:"));
            ui.label(RichText::new(media::storage_folder().display().to_string()).color(TEXT_DIM));
            if ui.button(tr(ui, "Recording Setup…")).clicked() {
                s.rec_setup = true;
            }
            if ui.button(tr(ui, "MultiCorder Setup…")).clicked() {
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
            egui::Grid::new("set-ext").num_columns(2).spacing([16.0, 8.0]).show(ui, |ui| {
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
            if ui.button("Configure Output 1–4…").clicked() {
                s.cfg.page = 1;
            }
        }
        7 => {
            // Audio
            setting_toggle(ui, &mut s.cfg.master_meter, "Show Master Audio Meter in Main Window");
            setting_toggle(ui, &mut s.cfg.input_meter, "Show Audio Meter next to Inputs");
            setting_toggle(ui, &mut s.cfg.default_auto_mix, "Automatically mix new inputs");
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Default Bus for New Inputs"));
                for (index, label) in ["Master", "A", "B"].into_iter().enumerate() {
                    ui.selectable_value(&mut s.cfg.default_audio_bus, index, label);
                }
            });
            ui.horizontal(|ui| {
                ui.label(tr(ui, "Default Input Audio Delay"));
                ui.add(egui::DragValue::new(&mut s.cfg.default_audio_delay_ms).range(0..=1000).suffix(" ms"));
            });
        }
        8 => {
            ui.label(tr(ui, "Headphones"));
            let label = if s.cfg.headphones_device.is_empty() { "System default" } else { &s.cfg.headphones_device };
            egui::ComboBox::from_id_salt("headphones-device").selected_text(label).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.cfg.headphones_device, String::new(), "System default");
                for name in audio::output_devices() {
                    ui.selectable_value(&mut s.cfg.headphones_device, name.clone(), name);
                }
            });
            ui.label(tr(ui, "The selection applies the next time headphone monitoring starts."));
        }
        11 => controls.shortcuts(ui, s),
        12 => controls.activators(ui, s),
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
            ui.label(tr(ui, "A live video production switcher."));
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
pub(crate) const STREAM_DESTS: [(&str, &str); 4] = [
    ("Custom RTMP Server", ""),
    ("YouTube RTMPS", "rtmps://a.rtmps.youtube.com/live2"),
    ("Facebook", "rtmps://live-api-s.facebook.com:443/rtmp/"),
    ("Twitch", "rtmp://live.twitch.tv/app"),
];
#[cfg(windows)]
pub(crate) fn directshow_settings(ui: &mut egui::Ui, config: &mut settings_xml::DirectShow) {
    ui.separator();
    ui.strong("DirectShow");
    setting_toggle(ui, &mut config.enabled, "Use DirectShow for files outside the FFmpeg list");
    ui.label("Reopen the input after changing these settings. Audio continues through FFmpeg.");
    let filters = ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_insert_with(egui::Id::new("directshow-filters"), directshow::filter_names).clone()
    });
    egui::Grid::new("directshow-decoders").spacing([16.0, 8.0]).show(ui, |ui| {
        for (label, value) in [("MPEG-2 Video", &mut config.mpeg_video), ("H.264 Video", &mut config.h264_video)] {
            ui.label(label);
            let selected = if value.is_empty() { "Auto" } else { value.as_str() };
            egui::ComboBox::from_id_salt(label).selected_text(selected).show_ui(ui, |ui| {
                ui.selectable_value(value, String::new(), "Auto");
                for name in &filters {
                    if !config.blocked.iter().any(|blocked| name.eq_ignore_ascii_case(blocked)) {
                        ui.selectable_value(value, name.clone(), name);
                    }
                }
            });
            ui.end_row();
        }
    });
    ui.collapsing("Blocked Filter List", |ui| {
        let mut names = filters;
        names.extend(config.blocked.iter().cloned());
        names.sort_unstable();
        names.dedup();
        egui::ScrollArea::vertical().id_salt("directshow-filter-list").max_height(200.0).show(ui, |ui| {
            for name in names {
                let mut blocked = config.blocked.iter().any(|value| value.eq_ignore_ascii_case(&name));
                if setting_toggle(ui, &mut blocked, &name).changed() {
                    if blocked { config.blocked.push(name); }
                    else { config.blocked.retain(|value| !value.eq_ignore_ascii_case(&name)); }
                }
            }
        });
    });
}

pub(crate) fn stream_route(s: &Switcher, destination: &stream::Destination) -> OutputRoute {
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
pub(crate) fn new_file_every(ui: &mut egui::Ui, id: &str, v: &mut u32) -> bool {
    let none = tr(ui, "None");
    let name = |m: u32| if m == 0 { none.to_owned() } else { m.to_string() };
    let before = *v;
    egui::ComboBox::from_id_salt(id).width(80.0).selected_text(name(*v)).show_ui(ui, |ui| {
        for m in media::NEW_FILE_EVERY {
            ui.selectable_value(v, m, name(m));
        }
    });
    *v != before
}

pub(crate) fn recording_groups() -> &'static [(&'static str, &'static [media::RecordFormat])] {
    use media::RecordFormat::*;
    const WMV: &[media::RecordFormat] = if cfg!(windows) { &[Wmv9, Wmv] } else { &[Wmv] };
    &[
        ("AVI", &[Mjpeg, MagicYuv]), ("MPEG-2", &[Mpeg2Ts, Mpeg2Ps]),
        ("WMV", WMV), ("WMV Streaming", &[]), ("MP4", &[H264, Hevc]),
        ("FFmpeg", &[ProRes, DnxHr, DnxHrMxf]), ("VMX AVI", &[]), ("MKV", &[H264Mkv, HevcMkv]),
    ]
}

pub(crate) fn recording_format_tabs(ui: &mut egui::Ui, format: &mut media::RecordFormat) -> bool {
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

pub(crate) fn recording_profile(ui: &mut egui::Ui, id: &str, format: &mut media::RecordFormat) -> bool {
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
