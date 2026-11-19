use crate::{clock, egui, tr, Switcher};
use serde::{Deserialize, Serialize};

const MODES: [&str; 6] = ["Off", "Elapsed", "Countdown", "Recording", "Streaming", "Program remaining"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    mode: usize,
    seconds: u32,
    message: String,
}

impl Default for Config {
    fn default() -> Self { Self { mode: 1, seconds: 300, message: String::new() } }
}

#[derive(Clone, Copy)]
struct State {
    mode: usize,
    start: Option<f64>,
    elapsed: f64,
}

pub fn defaults() -> [Config; 2] { [Config::default(), Config { mode: 0, ..Config::default() }] }

pub fn rows(clocks: &[Config; 2]) -> usize { clocks.iter().filter(|clock| clock.mode != 0).count() }

fn editor(ui: &mut egui::Ui, cfg: &mut Config) {
    egui::ComboBox::from_id_salt("mode").selected_text(tr(ui, MODES[cfg.mode.min(5)])).show_ui(ui, |ui| {
        for (index, label) in MODES.iter().enumerate() { ui.selectable_value(&mut cfg.mode, index, tr(ui, label)); }
    });
    if cfg.mode == 2 { ui.add(egui::DragValue::new(&mut cfg.seconds).range(1..=359_999).suffix(" s")); }
    ui.add(egui::TextEdit::singleline(&mut cfg.message).char_limit(64).hint_text(tr(ui, "Message")));
}

pub fn settings(ui: &mut egui::Ui, clocks: &mut [Config; 2]) {
    for (index, cfg) in clocks.iter_mut().enumerate() {
        ui.push_id(("clock-settings", index), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!("{} {}", tr(ui, "Production Clock"), index + 1));
                editor(ui, cfg);
            });
        });
    }
}

pub fn show(ui: &mut egui::Ui, s: &mut Switcher, width: f32) {
    let now = ui.input(|input| input.time);
    let remaining = s.output.and_then(|id| s.input(id)).filter(|input| input.duration > 0.0)
        .map(|input| f64::from((input.duration - input.pos).max(0.0)));
    let mut save = false;
    for (index, cfg) in s.cfg.production_clocks.iter_mut().enumerate() {
        let id = ui.make_persistent_id(("production-clock", index));
        let mode = cfg.mode.min(5);
        let active = match mode { 3 => s.rec.is_some(), 4 => s.stream_active.contains(&true), _ => false };
        let mut state = ui.data(|data| data.get_temp::<State>(id)).filter(|state| state.mode == mode)
            .unwrap_or(State { mode, start: (mode == 1).then_some(now), elapsed: 0.0 });
        if matches!(mode, 3 | 4) {
            if active { state.start.get_or_insert(now); }
            else if let Some(start) = state.start.take() { state.elapsed = now - start; }
        }
        let elapsed = if matches!(mode, 3 | 4) { state.start.map_or(state.elapsed, |start| now - start) }
            else { state.elapsed + state.start.map_or(0.0, |start| now - start) };
        let value = match mode {
            2 => clock((f64::from(cfg.seconds) - elapsed).max(0.0).ceil()),
            5 => remaining.map_or_else(|| "—".into(), clock),
            _ => clock(elapsed),
        };
        if mode != 0 && s.cfg.production_clock {
            let label = if cfg.message.is_empty() { value } else { format!("{}  {value}", cfg.message) };
            let response = ui.add_sized([width, 22.0], egui::Button::new(egui::RichText::new(label).monospace()).truncate());
            if response.clicked() && matches!(mode, 1 | 2) {
                if state.start.take().is_some() { state.elapsed = elapsed; }
                else { state.start = Some(now); }
            }
            response.context_menu(|ui| {
                ui.push_id(index, |ui| {
                    let before = (cfg.mode, cfg.seconds, cfg.message.clone());
                    editor(ui, cfg);
                    save |= before != (cfg.mode, cfg.seconds, cfg.message.clone());
                });
                if matches!(mode, 1 | 2) && ui.button(tr(ui, "Reset Production Clock")).clicked() {
                    state.elapsed = 0.0;
                    state.start = state.start.map(|_| now);
                    ui.close_menu();
                }
            });
        }
        ui.data_mut(|data| data.insert_temp(id, state));
    }
    if save { s.save_settings(); }
}
