//! Audio mixing, metering, monitoring and the mixer panel.
use super::*;

impl App {
    pub(crate) fn audio_input(&self, input: &Input) -> Option<(&Arc<media::Shared>, bool, f32, f32)> {
        let (original, delay) = match input.kind {
            Kind::VideoDelay { source: Some(source), seconds, .. } => (self.s.input(source)?, seconds),
            _ => (input, 0.0),
        };
        let source = self.media.get(&original.id)?.shared.as_ref()?;
        let file = matches!(original.kind, Kind::VideoFile { .. } | Kind::AudioFile { .. } | Kind::VideoList { .. } | Kind::Replay { .. });
        let gain = if input.audio_on { input.volume / 100.0 } else { 0.0 };
        Some((source, file, gain, delay))
    }

    /// Peak level of every audio input, kept for the mixer strips and the Input Settings meter.
    pub(crate) fn update_levels(&mut self) {
        let at = Instant::now();
        let levels: Vec<(u32, [f32; 2])> = self.s.inputs.iter().filter(|input| input.kind.has_audio())
            .map(|input| (input.id, self.audio_input(input).map_or([0.0; 2], |(source, file, gain, delay)| {
                audio::delayed_peaks((source, file, gain, delay), at)
            })))
            .collect();
        for (id, level) in levels {
            if let Some(input) = self.s.input_mut(id) {
                input.level = level;
            }
        }
    }

    pub(crate) fn mix_audio(&self, route: u32, at: Instant, frames: usize) -> audio::Packet {
        let master = matches!(route, SRC_OUTPUT | SRC_MV1 | SRC_MV2);
        let selected = match route {
            SRC_PREVIEW => self.s.preview,
            _ => route.checked_sub(1),
        };
        let gain = if master { self.master_gain() } else { 1.0 };
        audio::mix_delayed(self.s.inputs.iter().filter(|input| {
            if master { input.bus_m } else { Some(input.id) == selected }
        }).filter_map(|input| {
            let (source, file, level, delay) = self.audio_input(input)?;
            let auto = if master { self.s.automatic_audio_gain(input) } else { 1.0 };
            Some((source.as_ref(), file, level * gain * auto, delay))
        }), at, frames)
    }

    pub(crate) fn master_gain(&self) -> f32 {
        let fade = if self.s.ftb_audio { 1.0 - self.s.ftb_level / FULL } else { 1.0 };
        if self.s.master_on { self.s.master_volume / 100.0 * fade } else { 0.0 }
    }

    pub(crate) fn mix_external_audio(&self, bus: u8, delay_ms: u32, at: Instant, frames: usize) -> audio::Packet {
        let bus_gain = if bus == 0 { self.master_gain() } else { 1.0 };
        audio::mix_delayed(self.s.inputs.iter().filter(|input| match bus {
            1 => input.bus_a,
            2 => input.bus_b,
            _ => input.bus_m,
        }).filter_map(|input| {
            let (source, file, gain, delay) = self.audio_input(input)?;
            Some((source.as_ref(), file, gain * bus_gain * self.s.automatic_audio_gain(input),
                delay + delay_ms as f32 / 1000.0))
        }), at, frames)
    }

    pub(crate) fn update_audio_monitor(&mut self) {
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
            let (source, file, volume, delay) = self.audio_input(input)?;
            let gain = if solo {
                if input.solo_pfl { 1.0 } else { volume }
            } else {
                volume * bus_gain * self.s.automatic_audio_gain(input)
            };
            Some((source.clone(), file, gain * headphones, delay))
        }).collect::<Vec<_>>().into();
        *monitor.sources.lock().unwrap() = sources;
    }
}

// ---------------------------------------------------------------- mixer

#[derive(Clone, Copy)]
pub(crate) enum MixerSettings {
    Master,
    Input(u32),
}

pub(crate) struct MixerControls {
    pub(crate) bus: u8,
    pub(crate) headphones: f32,
    pub(crate) statistics: bool,
    pub(crate) settings: Option<MixerSettings>,
    pub(crate) last_error: Option<String>,
}

impl Default for MixerControls {
    fn default() -> Self {
        Self { bus: 0, headphones: 100.0, statistics: false, settings: None, last_error: None }
    }
}

impl App {
    pub(crate) fn audio_mixer_windows(&mut self, ctx: &egui::Context) {
        if self.mixer.statistics {
            egui::Window::new(locale::text(ctx, "Audio Engine")).id(egui::Id::new("Audio Engine"))
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
                                ui.strong(tr(ui, label));
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
                    egui::Window::new(format!("{} — {}", locale::text(ctx, "Audio Settings"), input.title))
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

pub(crate) fn picture_audio_meter(painter: &egui::Painter, picture: egui::Rect, levels: [f32; 2]) {
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

pub(crate) fn meter_fraction(level: f32) -> f32 {
    ((20.0 * level.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

pub(crate) fn headphone_knob(ui: &mut egui::Ui, volume: &mut f32) {
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

pub(crate) fn mixer_panel(ctx: &egui::Context, s: &mut Switcher, master: [f32; 2], monitor: &mut bool, controls: &mut MixerControls) {
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

pub(crate) fn mixer_section(ui: &mut egui::Ui, name: &str, height: f32) -> bool {
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

pub(crate) struct MixerInputControls<'a> {
    pub(crate) buses: [&'a mut bool; 3],
    pub(crate) solo: &'a mut bool,
    pub(crate) solo_pfl: &'a mut bool,
    pub(crate) auto: &'a mut bool,
}

pub(crate) fn mixer_strip(ui: &mut egui::Ui, name: &str, volume: &mut f32, on: &mut bool, levels: [f32; 2],
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
