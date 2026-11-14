//! Camera Match: compares a reference camera with a target camera, tells the operator which
//! camera settings to change on the target (exposure, white balance, tint, contrast, saturation)
//! and can close the remaining gap with the target input's Colour Correction.
//!
//! Pictures are measured on small subsampled samples taken from the frames the input loop is
//! already processing, so measuring never takes frames away from the display or recorders.
use super::*;
use std::sync::OnceLock;

/// Average picture statistics of one input over its region of interest.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Mean linear-light red, green and blue (0..1).
    pub lin: [f32; 3],
    /// Linear-light luma of the mean colour.
    pub luma: f32,
    /// 5th, 50th and 95th percentile of display-referred luma (0..1).
    pub p5: f32,
    pub p50: f32,
    pub p95: f32,
    /// Mean HSV saturation of display-referred pixels (0..1).
    pub sat: f32,
}

impl Stats {
    fn blend(self, new: Stats, k: f32) -> Stats {
        let mix = |a: f32, b: f32| a + (b - a) * k;
        Stats {
            lin: [mix(self.lin[0], new.lin[0]), mix(self.lin[1], new.lin[1]), mix(self.lin[2], new.lin[2])],
            luma: mix(self.luma, new.luma),
            p5: mix(self.p5, new.p5),
            p50: mix(self.p50, new.p50),
            p95: mix(self.p95, new.p95),
            sat: mix(self.sat, new.sat),
        }
    }

    /// Mean colour as display bytes, for swatches.
    pub fn display_rgb(&self) -> [u8; 3] {
        [0, 1, 2].map(|c| (encode(self.lin[c]) * 255.0).round().clamp(0.0, 255.0) as u8)
    }
}

fn decode_table() -> &'static [f32; 256] {
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0.0; 256];
        for (value, out) in table.iter_mut().enumerate() {
            let v = value as f32 / 255.0;
            *out = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        }
        table
    })
}

fn encode(linear: f32) -> f32 {
    let v = linear.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Measures the central `roi` fraction of an RGBA picture, using at most ~16k samples.
/// Clipped (≥250) and crushed (≤5) pixels are left out so they do not bias the averages.
pub fn measure(rgba: &[u8], w: usize, h: usize, roi: f32) -> Option<Stats> {
    if w == 0 || h == 0 || rgba.len() < w * h * 4 { return None; }
    let roi = roi.clamp(0.1, 1.0);
    let (rw, rh) = (((w as f32 * roi) as usize).clamp(1, w), ((h as f32 * roi) as usize).clamp(1, h));
    let (x0, y0) = ((w - rw) / 2, (h - rh) / 2);
    let step = ((rw * rh) as f32 / 16_384.0).sqrt().ceil().max(1.0) as usize;
    let table = decode_table();
    let mut sum = [0.0f64; 3];
    let mut sat = 0.0f64;
    let mut histogram = [0u32; 256];
    let mut count = 0u32;
    for y in (y0..y0 + rh).step_by(step) {
        for x in (x0..x0 + rw).step_by(step) {
            let at = (y * w + x) * 4;
            let p = &rgba[at..at + 3];
            let (max, min) = (p[0].max(p[1]).max(p[2]), p[0].min(p[1]).min(p[2]));
            if max >= 250 || max <= 5 { continue; }
            for c in 0..3 { sum[c] += f64::from(table[p[c] as usize]); }
            sat += f64::from(max - min) / f64::from(max);
            let luma = f32::from(p[0]) * 0.2126 + f32::from(p[1]) * 0.7152 + f32::from(p[2]) * 0.0722;
            histogram[luma.round().clamp(0.0, 255.0) as usize] += 1;
            count += 1;
        }
    }
    if count < 16 { return None; }
    let n = f64::from(count);
    let lin = [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32];
    let percentile = |fraction: f32| {
        let target = (fraction * count as f32).ceil() as u32;
        let mut seen = 0;
        for (value, bin) in histogram.iter().enumerate() {
            seen += bin;
            if seen >= target { return value as f32 / 255.0; }
        }
        1.0
    };
    Some(Stats {
        lin,
        luma: lin[0] * 0.2126 + lin[1] * 0.7152 + lin[2] * 0.0722,
        p5: percentile(0.05),
        p50: percentile(0.5),
        p95: percentile(0.95),
        sat: (sat / n) as f32,
    })
}

fn lab(lin: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = lin;
    let xyz = [
        (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.9505,
        0.2126 * r + 0.7152 * g + 0.0722 * b,
        (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.089,
    ];
    let f = |t: f32| if t > 0.008_856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 };
    let [fx, fy, fz] = xyz.map(f);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIE76 colour difference between the mean colours of two pictures (≈1 is barely visible).
pub fn delta_e(a: &Stats, b: &Stats) -> f32 {
    let (a, b) = (lab(a.lin), lab(b.lin));
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// What the operator knows about a camera's own settings. Zero means "not entered".
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Camera {
    /// White balance in kelvin.
    pub kelvin: f32,
    pub iso: f32,
    /// Shutter denominator: 50 means 1/50 s.
    pub shutter: f32,
    /// Aperture f-number.
    pub aperture: f32,
}

impl Camera {
    fn exposure_known(&self) -> bool { self.iso > 0.0 && self.shutter > 0.0 && self.aperture > 0.0 }
}

/// One line of advice for the target camera.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// Untranslated label, e.g. "Exposure".
    pub setting: &'static str,
    pub value: String,
    /// Untranslated note, empty when none.
    pub note: &'static str,
    pub matched: bool,
}

/// Stops of light the target needs to look as bright as the reference (positive: open up).
pub fn delta_ev(reference: &Stats, target: &Stats) -> f32 {
    (reference.luma.max(1e-4) / target.luma.max(1e-4)).log2()
}

/// Mired change that makes the target as warm as the reference. Positive means the target
/// picture is bluer, so its white balance should be raised. About 0.0082 ln(B/R) per mired
/// along the daylight-to-tungsten locus.
pub fn wb_mired(reference: &Stats, target: &Stats) -> f32 {
    let balance = |s: &Stats| (s.lin[2].max(1e-4) / s.lin[0].max(1e-4)).ln();
    (balance(target) - balance(reference)) / 0.0082
}

/// Green/magenta imbalance (ln G over the R/B geometric mean); positive means greener.
fn tint(s: &Stats) -> f32 {
    (s.lin[1].max(1e-4) / (s.lin[0].max(1e-4) * s.lin[2].max(1e-4)).sqrt()).ln()
}

fn third(stops: f32) -> f32 { (stops * 3.0).round() / 3.0 }

/// Splits a brightness gap over shutter, iris and ISO (in that order), in thirds of a stop.
fn exposure_plan(need: f32, cam: &Camera, fps: f32) -> Vec<(String, f32)> {
    let mut left = need;
    let mut out = Vec::new();
    let limit_slow = 1.0 / (2.0 * fps.max(1.0));
    let t = 1.0 / cam.shutter;
    let wanted = third((t * 2f32.powf(left)).clamp(1.0 / 8000.0, limit_slow.max(t)).log2() - t.log2());
    if wanted.abs() > 0.1 {
        let new = t * 2f32.powf(wanted);
        out.push((format!("Shutter 1/{:.0} → 1/{:.0}", cam.shutter, 1.0 / new), wanted));
        left -= wanted;
    }
    let n = cam.aperture;
    let stops = third(2.0 * (n / (n / 2f32.powf(left / 2.0)).clamp(1.4f32.min(n), 22.0f32.max(n))).log2());
    if stops.abs() > 0.1 {
        let new = n / 2f32.powf(stops / 2.0);
        out.push((format!("Iris f/{n:.1} → f/{new:.1}"), stops));
        left -= stops;
    }
    let iso = cam.iso;
    let stops = third((iso * 2f32.powf(left)).clamp(100.0f32.min(iso), 12_800.0f32.max(iso)).log2() - iso.log2());
    if stops.abs() > 0.1 {
        out.push((format!("ISO {iso:.0} → {:.0}", iso * 2f32.powf(stops)), stops));
        left -= stops;
    }
    if left.abs() > 0.25 {
        out.push((format!("{left:+.1} EV left: use an ND filter or change the light"), left));
    }
    out
}

/// Advice for bringing `target` (as the camera sees it) to the look of `reference`.
pub fn suggest(reference: &Stats, target: &Stats, ref_cam: &Camera, tgt_cam: &Camera, fps: f32) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let ev = delta_ev(reference, target);
    if ev.abs() < 0.15 {
        out.push(Suggestion { setting: "Exposure", value: format!("{ev:+.2} EV"), note: "", matched: true });
    } else {
        let base = if tgt_cam.exposure_known() { Some((tgt_cam, "")) }
            else if ref_cam.exposure_known() { Some((ref_cam, "Based on the reference camera's settings")) }
            else { None };
        match base {
            Some((cam, note)) => {
                let plan = exposure_plan(ev, cam, fps);
                for (index, (text, _)) in plan.into_iter().enumerate() {
                    out.push(Suggestion { setting: if index == 0 { "Exposure" } else { "Exposure (then)" }, value: text, note, matched: false });
                }
            }
            None => out.push(Suggestion { setting: "Exposure",
                value: format!("{} {:.1} EV ({:+.0}%)", if ev > 0.0 { "Open up" } else { "Close down" }, ev.abs(), (2f32.powf(ev) - 1.0) * 100.0),
                note: "Enter the camera settings for exact values", matched: false }),
        }
    }
    let mired = wb_mired(reference, target);
    if mired.abs() < 6.0 {
        out.push(Suggestion { setting: "White balance", value: format!("{mired:+.0} mired"), note: "", matched: true });
    } else {
        let value = if tgt_cam.kelvin > 0.0 {
            let now = 1.0e6 / tgt_cam.kelvin;
            let new = (1.0e6 / (now - mired).clamp(60.0, 500.0)).round();
            format!("{:.0} K → {:.0} K", tgt_cam.kelvin, new)
        } else {
            let to = 1.0e6 / (1.0e6 / 5600.0 - mired).clamp(60.0, 500.0);
            format!("{} {:.0} mired (≈ {:.0} K around 5600 K)", if mired > 0.0 { "Raise by" } else { "Lower by" }, mired.abs(), to)
        };
        out.push(Suggestion { setting: "White balance", value, note: if mired > 0.0 { "Target is bluer: warm it up" } else { "Target is warmer: cool it down" }, matched: false });
    }
    let green = tint(target) - tint(reference);
    if green.abs() < 0.02 {
        out.push(Suggestion { setting: "Tint", value: "OK".into(), note: "", matched: true });
    } else {
        out.push(Suggestion { setting: "Tint", value: format!("{:.0}% {}", green.abs() * 100.0, if green > 0.0 { "greener: add magenta" } else { "more magenta: add green" }), note: "", matched: false });
    }
    let (spread_r, spread_t) = ((reference.p95 - reference.p5).max(0.02), (target.p95 - target.p5).max(0.02));
    let contrast = spread_t / spread_r;
    if (contrast - 1.0).abs() < 0.08 {
        out.push(Suggestion { setting: "Contrast", value: format!("{:+.0}%", (contrast - 1.0) * 100.0), note: "", matched: true });
    } else {
        out.push(Suggestion { setting: "Contrast", value: format!("{} {:.0}%", if contrast > 1.0 { "Lower by" } else { "Raise by" }, (contrast - 1.0).abs() * 100.0),
            note: "Picture profile, gamma or detail settings", matched: false });
    }
    let saturation = target.sat / reference.sat.max(0.01);
    if (saturation - 1.0).abs() < 0.1 {
        out.push(Suggestion { setting: "Saturation", value: format!("{:+.0}%", (saturation - 1.0) * 100.0), note: "", matched: true });
    } else {
        out.push(Suggestion { setting: "Saturation", value: format!("{} {:.0}%", if saturation > 1.0 { "Lower by" } else { "Raise by" }, (saturation - 1.0).abs() * 100.0),
            note: "Picture profile or colour matrix", matched: false });
    }
    out
}

/// One software correction step: nudges `adj` so the target's processed picture approaches the
/// reference's. Returns the new adjustment, the colour difference before the step and whether
/// the pictures already match.
pub fn correction_step(adj: ColourAdjust, reference: &Stats, target: &Stats) -> (ColourAdjust, f32, bool) {
    let de = delta_e(reference, target);
    let mut next = adj;
    let channels = [&mut next.red, &mut next.green, &mut next.blue];
    for (c, value) in channels.into_iter().enumerate() {
        let error = (reference.lin[c].max(1e-4) / target.lin[c].max(1e-4)).ln();
        // Linear light scales with the display value to the 2.2-ish power.
        let gain = (1.0 + *value) * (0.6 * error / 2.2).exp();
        *value = (gain - 1.0).clamp(-0.9, 1.0);
    }
    let sat_ratio = reference.sat / target.sat.max(0.01);
    let spread_ratio = (reference.p95 - reference.p5).max(0.02) / (target.p95 - target.p5).max(0.02);
    let shape_ok = (sat_ratio - 1.0).abs() < 0.05 && (spread_ratio - 1.0).abs() < 0.05;
    if de < 3.0 {
        next.saturation = ((1.0 + next.saturation) * sat_ratio.sqrt() - 1.0).clamp(-1.0, 1.0);
        next.contrast = ((1.0 + next.contrast) * spread_ratio.powf(0.4) - 1.0).clamp(-0.9, 1.0);
    }
    (next, de, de < 1.0 && shape_ok)
}

/// A running average of the pictures measured since the last reset.
#[derive(Clone, Copy, Debug, Default)]
pub struct Meter {
    pub stats: Option<Stats>,
    /// Pictures measured since the last reset.
    pub fresh: u32,
}

impl Meter {
    fn push(&mut self, stats: Stats) {
        self.stats = Some(self.stats.map_or(stats, |old| old.blend(stats, 0.25)));
        self.fresh += 1;
    }
    fn reset(&mut self) { *self = Meter::default(); }
}

/// Window state of the Camera Match assistant.
#[derive(Clone, Debug)]
pub struct State {
    pub open: bool,
    pub reference: Option<u32>,
    pub target: Option<u32>,
    /// Central fraction of the picture that is measured.
    pub roi: f32,
    pub ref_cam: Camera,
    pub tgt_cam: Camera,
    /// What each camera delivers before the input's own Colour Correction.
    pub ref_raw: Meter,
    pub tgt_raw: Meter,
    /// What the Program receives after the input's Colour Correction.
    pub ref_final: Meter,
    pub tgt_final: Meter,
    /// Software matching in progress.
    pub auto: bool,
    pub steps: u32,
    pub undo: Option<ColourAdjust>,
    pub status: String,
}

impl Default for State {
    fn default() -> Self {
        Self { open: false, reference: None, target: None, roi: 0.5, ref_cam: Camera::default(), tgt_cam: Camera::default(),
            ref_raw: Meter::default(), tgt_raw: Meter::default(), ref_final: Meter::default(), tgt_final: Meter::default(),
            auto: false, steps: 0, undo: None, status: String::new() }
    }
}

const MAX_STEPS: u32 = 14;
const SETTLE_PICTURES: u32 = 4;

impl State {
    /// The inputs whose pictures should be measured right now.
    pub fn watching(&self) -> [Option<u32>; 2] {
        if self.open { [self.reference, self.target] } else { [None, None] }
    }

    pub fn reset_measurements(&mut self) {
        for meter in [&mut self.ref_raw, &mut self.tgt_raw, &mut self.ref_final, &mut self.tgt_final] { meter.reset(); }
    }

    /// Records one picture of input `id`: `raw` as delivered, `processed` after Colour Correction.
    pub fn feed(&mut self, id: u32, raw: Option<Stats>, processed: Option<Stats>) {
        if Some(id) == self.reference {
            if let Some(s) = raw { self.ref_raw.push(s); }
            if let Some(s) = processed { self.ref_final.push(s); }
        }
        if Some(id) == self.target && Some(id) != self.reference {
            if let Some(s) = raw { self.tgt_raw.push(s); }
            if let Some(s) = processed { self.tgt_final.push(s); }
        }
    }

    /// Persisted form: input numbers (1-based, 0 = none), ROI and the entered camera data.
    pub fn save_line(&self, number: impl Fn(Option<u32>) -> usize) -> Option<String> {
        if self.reference.is_none() && self.target.is_none() { return None; }
        let cams = serde_json::to_string(&(self.ref_cam, self.tgt_cam)).ok()?;
        Some(format!("cammatch\t{}\t{}\t{}\t{cams}\n", number(self.reference), number(self.target), self.roi))
    }

    pub fn load_fields(&mut self, reference: Option<u32>, target: Option<u32>, roi: &str, cams: &str) {
        self.reference = reference;
        self.target = target;
        self.roi = roi.parse::<f32>().unwrap_or(0.5).clamp(0.1, 1.0);
        if let Ok((r, t)) = serde_json::from_str::<(Camera, Camera)>(cams) {
            self.ref_cam = r;
            self.tgt_cam = t;
        }
        self.reset_measurements();
    }
}

/// Runs once per frame: drops inputs that no longer exist and advances software matching.
pub fn drive(s: &mut Switcher) {
    let state = &mut s.cam_match;
    let (reference, target) = (state.reference, state.target);
    let missing = |id: Option<u32>| id.is_some_and(|id| !s.inputs.iter().any(|input| input.id == id));
    if missing(reference) || missing(target) {
        let state = &mut s.cam_match;
        if missing(reference) { state.reference = None; }
        if missing(target) { state.target = None; }
        state.auto = false;
        state.reset_measurements();
        return;
    }
    let state = &mut s.cam_match;
    if !state.auto { return; }
    let (Some(target), Some(reference)) = (state.target, state.reference) else { state.auto = false; return };
    if !state.open || target == reference { state.auto = false; return; }
    let (Some(goal), Some(now)) = (state.ref_final.stats, state.tgt_final.stats) else { return };
    if state.tgt_final.fresh < SETTLE_PICTURES || state.ref_final.fresh < SETTLE_PICTURES { return; }
    let Some(input) = s.inputs.iter_mut().find(|input| input.id == target) else { return };
    let (next, de, done) = correction_step(input.colour_adjust, &goal, &now);
    let state = &mut s.cam_match;
    if done {
        state.auto = false;
        state.status = format!("Matched (ΔE {de:.1}) after {} steps", state.steps);
    } else if state.steps >= MAX_STEPS {
        state.auto = false;
        state.status = format!("Stopped after {MAX_STEPS} steps (ΔE {de:.1}); the cameras may need adjusting first");
    } else {
        input.colour_adjust = next;
        state.steps += 1;
        state.tgt_final.reset();
        state.status = format!("Matching… step {} (ΔE {de:.1})", state.steps);
    }
}

fn input_combo(ui: &mut egui::Ui, id: &str, choices: &[(u32, String)], value: &mut Option<u32>) -> bool {
    let before = *value;
    let name = value.and_then(|v| choices.iter().find(|(i, _)| *i == v)).map_or_else(|| "—".to_owned(), |(_, n)| n.clone());
    egui::ComboBox::from_id_salt(id).selected_text(name).width(170.0).show_ui(ui, |ui| {
        ui.selectable_value(value, None, "—");
        for (input, title) in choices { ui.selectable_value(value, Some(*input), title); }
    });
    before != *value
}

fn camera_fields(ui: &mut egui::Ui, id: &str, cam: &mut Camera) {
    egui::Grid::new(id).num_columns(2).spacing([10.0, 3.0]).show(ui, |ui| {
        ui.label(tr(ui, "White balance (K)"));
        ui.add(egui::DragValue::new(&mut cam.kelvin).range(0.0..=15000.0).speed(50.0).custom_formatter(|v, _| if v <= 0.0 { "—".into() } else { format!("{v:.0}") }));
        ui.end_row();
        ui.label("ISO");
        ui.add(egui::DragValue::new(&mut cam.iso).range(0.0..=102_400.0).speed(10.0).custom_formatter(|v, _| if v <= 0.0 { "—".into() } else { format!("{v:.0}") }));
        ui.end_row();
        ui.label(tr(ui, "Shutter 1/x"));
        ui.add(egui::DragValue::new(&mut cam.shutter).range(0.0..=32_000.0).speed(5.0).custom_formatter(|v, _| if v <= 0.0 { "—".into() } else { format!("1/{v:.0}") }));
        ui.end_row();
        ui.label(tr(ui, "Iris f/"));
        ui.add(egui::DragValue::new(&mut cam.aperture).range(0.0..=64.0).speed(0.1).custom_formatter(|v, _| if v <= 0.0 { "—".into() } else { format!("f/{v:.1}") }));
        ui.end_row();
    });
}

fn measurement(ui: &mut egui::Ui, title: &str, meter: &Meter) {
    ui.vertical(|ui| {
        ui.label(RichText::new(title).strong());
        match meter.stats {
            Some(stats) => {
                let [r, g, b] = stats.display_rgb();
                let (rect, _) = ui.allocate_exact_size(Vec2::new(150.0, 26.0), Sense::hover());
                ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(r, g, b));
                ui.label(format!("RGB {r} {g} {b}"));
                ui.label(format!("{}: {:.0}%  ({:.0}–{:.0}%)", tr(ui, "Luma"), stats.p50 * 100.0, stats.p5 * 100.0, stats.p95 * 100.0));
                ui.label(format!("{}: {:.0}%", tr(ui, "Saturation"), stats.sat * 100.0));
            }
            None => { ui.label(tr(ui, "No picture yet")); }
        }
    });
}

/// The Camera Match window (opened from the top bar).
pub fn window(ctx: &egui::Context, s: &mut Switcher) {
    if !s.cam_match.open { return; }
    let choices: Vec<(u32, String)> = s.inputs.iter().filter(|i| i.kind.streamed()).map(|i| (i.id, i.title.clone())).collect();
    let fps = output_fps();
    let mut open = true;
    let mut state = std::mem::take(&mut s.cam_match);
    let mut start = false;
    let mut stop = false;
    let mut undo = false;
    egui::Window::new(locale::text(ctx, "Camera Match")).id(egui::Id::new("Camera Match")).open(&mut open)
        .default_width(560.0).show(ctx, |ui| {
        ui.label(tr(ui, "Pick a reference camera and the camera to match. Enter the camera settings you know for better advice."));
        ui.add_space(4.0);
        let mut changed = false;
        ui.columns(2, |cols| {
            cols[0].label(RichText::new(tr(&cols[0], "Reference")).strong());
            changed |= input_combo(&mut cols[0], "cam-match-ref", &choices, &mut state.reference);
            camera_fields(&mut cols[0], "cam-match-ref-fields", &mut state.ref_cam);
            cols[1].label(RichText::new(tr(&cols[1], "Target")).strong());
            changed |= input_combo(&mut cols[1], "cam-match-target", &choices, &mut state.target);
            camera_fields(&mut cols[1], "cam-match-target-fields", &mut state.tgt_cam);
        });
        if changed { state.reset_measurements(); state.auto = false; state.undo = None; state.status.clear(); }
        ui.horizontal(|ui| {
            ui.label(tr(ui, "Measured area"));
            ui.add(egui::Slider::new(&mut state.roi, 0.1..=1.0).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            if ui.button(tr(ui, "Restart measuring")).clicked() { state.reset_measurements(); }
        });
        ui.separator();
        if state.reference.is_some() && state.reference == state.target {
            ui.label(tr(ui, "Choose two different inputs."));
            return;
        }
        ui.horizontal_top(|ui| {
            measurement(ui, &tr(ui, "Reference camera"), &state.ref_raw);
            ui.add_space(24.0);
            measurement(ui, &tr(ui, "Target camera"), &state.tgt_raw);
        });
        let ready = state.ref_raw.fresh >= SETTLE_PICTURES && state.tgt_raw.fresh >= SETTLE_PICTURES;
        if let (Some(r), Some(t)) = (state.ref_raw.stats, state.tgt_raw.stats) {
            ui.add_space(4.0);
            ui.label(format!("{}: {:.1}", tr(ui, "Colour difference (ΔE)"), delta_e(&r, &t)));
            if !ready { ui.label(RichText::new(tr(ui, "Measuring…")).weak()); }
            ui.add_space(4.0);
            ui.label(RichText::new(tr(ui, "Change on the target camera")).strong());
            egui::Grid::new("cam-match-advice").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                for item in suggest(&r, &t, &state.ref_cam, &state.tgt_cam, fps) {
                    let colour = if item.matched { Color32::from_rgb(110, 190, 120) } else { Color32::from_rgb(235, 190, 90) };
                    ui.label(tr(ui, item.setting));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&item.value).color(colour));
                        if !item.note.is_empty() { ui.label(RichText::new(tr(ui, item.note)).weak().small()); }
                    });
                    ui.end_row();
                }
            });
        } else if state.reference.is_some() && state.target.is_some() {
            ui.label(tr(ui, "Waiting for pictures from both inputs…"));
        }
        ui.separator();
        ui.label(RichText::new(tr(ui, "Match in software")).strong());
        ui.label(RichText::new(tr(ui, "Adjusts the target input's Colour Correction. The cameras themselves are not changed.")).weak().small());
        ui.horizontal(|ui| {
            let can = state.reference.is_some() && state.target.is_some() && state.reference != state.target;
            if state.auto {
                if ui.button(tr(ui, "Stop")).clicked() { stop = true; }
            } else if ui.add_enabled(can, egui::Button::new(tr(ui, "Match target"))).clicked() {
                start = true;
            }
            if ui.add_enabled(state.undo.is_some(), egui::Button::new(tr(ui, "Undo"))).clicked() { undo = true; }
        });
        if !state.status.is_empty() { ui.label(&state.status); }
    });
    if start {
        if let Some(input) = state.target.and_then(|id| s.input_mut(id)) {
            state.undo = Some(input.colour_adjust);
        }
        state.auto = true;
        state.steps = 0;
        state.tgt_final.reset();
        state.status = "Matching…".to_owned();
    }
    if stop { state.auto = false; state.status = "Stopped".to_owned(); }
    if undo {
        if let (Some(previous), Some(input)) = (state.undo.take(), state.target.and_then(|id| s.input_mut(id))) {
            input.colour_adjust = previous;
            state.auto = false;
            state.tgt_final.reset();
            state.status = "Restored the previous Colour Correction".to_owned();
        }
    }
    state.open = open;
    if !open { state.auto = false; state.reset_measurements(); }
    s.cam_match = state;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    fn stats(rgb: [u8; 3]) -> Stats { measure(&flat(64, 64, rgb), 64, 64, 1.0).unwrap() }

    #[test]
    fn measures_flat_pictures() {
        let grey = stats([128, 128, 128]);
        assert!((grey.lin[0] - 0.2158).abs() < 0.002, "{:?}", grey.lin);
        assert!((grey.p50 - 128.0 / 255.0).abs() < 0.01);
        assert!(grey.sat < 0.001);
        assert!(measure(&flat(64, 64, [255, 255, 255]), 64, 64, 1.0).is_none(), "clipped pictures give no measurement");
        assert!(measure(&[0; 4], 64, 64, 1.0).is_none());
    }

    #[test]
    fn exposure_gap_is_in_stops() {
        let bright = stats([180, 180, 180]);
        let dark = stats([90, 90, 90]);
        let ev = delta_ev(&bright, &dark);
        assert!(ev > 1.5 && ev < 2.5, "{ev}");
        assert!(delta_ev(&dark, &dark).abs() < 1e-4);
    }

    #[test]
    fn warm_and_cool_pictures_give_signed_mired() {
        let neutral = stats([120, 120, 120]);
        let blue = stats([100, 120, 150]);
        assert!(wb_mired(&neutral, &blue) > 10.0, "a bluer target must be warmed up");
        assert!(wb_mired(&blue, &neutral) < -10.0);
        assert!(wb_mired(&neutral, &neutral).abs() < 0.5);
    }

    #[test]
    fn exposure_plan_uses_shutter_then_iris_then_iso() {
        let cam = Camera { iso: 400.0, shutter: 100.0, aperture: 4.0, kelvin: 0.0 };
        let plan = exposure_plan(1.0, &cam, 25.0);
        assert!(plan[0].0.starts_with("Shutter 1/100 → 1/50"), "{plan:?}");
        let plan = exposure_plan(3.0, &Camera { shutter: 50.0, ..cam }, 25.0);
        assert!(plan.iter().any(|(text, _)| text.starts_with("Iris")), "{plan:?}");
        let total: f32 = plan.iter().map(|(_, stops)| stops).sum();
        assert!((total - 3.0).abs() < 0.2, "{plan:?}");
        let plan = exposure_plan(-1.0, &cam, 25.0);
        assert!(plan[0].0.starts_with("Shutter 1/100 → 1/200"), "{plan:?}");
    }

    #[test]
    fn suggestions_report_matches_and_gaps() {
        let reference = stats([140, 130, 120]);
        let all_ok = suggest(&reference, &reference, &Camera::default(), &Camera::default(), 25.0);
        assert!(all_ok.iter().all(|item| item.matched), "{all_ok:?}");
        let target = stats([90, 100, 130]);
        let advice = suggest(&reference, &target, &Camera::default(), &Camera { kelvin: 4000.0, ..Camera::default() }, 25.0);
        let wb = advice.iter().find(|item| item.setting == "White balance").unwrap();
        assert!(!wb.matched && wb.value.contains(" K → "), "{wb:?}");
        let new: f32 = wb.value.split("→ ").nth(1).unwrap().trim_end_matches(" K").parse().unwrap();
        assert!(new > 4000.0, "a bluer target needs a higher kelvin setting: {wb:?}");
    }

    #[test]
    fn closed_loop_converges_on_a_shifted_picture() {
        let reference = [140u8, 120, 100];
        let shifted = [105u8, 118, 128];
        let source = flat(64, 64, shifted);
        let goal = measure(&flat(64, 64, reference), 64, 64, 1.0).unwrap();
        let mut adjust = ColourAdjust::default();
        let mut last = f32::MAX;
        let mut done = false;
        for _ in 0..30 {
            let mut picture = source.clone();
            adjust.apply(&mut picture);
            let now = measure(&picture, 64, 64, 1.0).unwrap();
            let (next, de, finished) = correction_step(adjust, &goal, &now);
            last = de;
            adjust = next;
            if finished { done = true; break; }
        }
        assert!(done && last < 1.0, "did not converge: ΔE {last}");
    }

    #[test]
    fn state_round_trips_through_its_preset_line() {
        let mut state = State { reference: Some(3), target: Some(5), roi: 0.4, ..State::default() };
        state.tgt_cam = Camera { kelvin: 5600.0, iso: 800.0, shutter: 50.0, aperture: 2.8 };
        let line = state.save_line(|id| id.map_or(0, |id| id as usize)).unwrap();
        let f: Vec<&str> = line.trim_end().split('\t').collect();
        let mut loaded = State::default();
        loaded.load_fields(Some(3), Some(5), f[3], f[4]);
        assert_eq!(loaded.tgt_cam, state.tgt_cam);
        assert!((loaded.roi - 0.4).abs() < 1e-6);
        assert!(State::default().save_line(|_| 0).is_none());
    }
}
