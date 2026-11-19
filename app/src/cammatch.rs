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
    /// Share of pixels in 32 display-luma bins, dark to bright.
    pub hist: [f32; 32],
}

impl Stats {
    fn blend(self, new: Stats, k: f32) -> Stats {
        let mix = |a: f32, b: f32| a + (b - a) * k;
        let mut hist = [0.0; 32];
        for (bin, out) in hist.iter_mut().enumerate() { *out = mix(self.hist[bin], new.hist[bin]); }
        Stats {
            hist,
            lin: [mix(self.lin[0], new.lin[0]), mix(self.lin[1], new.lin[1]), mix(self.lin[2], new.lin[2])],
            luma: mix(self.luma, new.luma),
            p5: mix(self.p5, new.p5),
            p50: mix(self.p50, new.p50),
            p95: mix(self.p95, new.p95),
            sat: mix(self.sat, new.sat),
        }
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

/// A rectangle of the picture as fractions: left, top, right, bottom.
pub type Area = [f32; 4];
/// The whole picture.
#[cfg(test)]
pub const FULL: Area = [0.0, 0.0, 1.0, 1.0];
/// The centre half of the picture, used until the operator drags a rectangle.
pub const DEFAULT_AREA: Area = [0.25, 0.25, 0.75, 0.75];

fn parse_area(text: &str) -> Option<Area> {
    let v: Vec<f32> = text.split(',').filter_map(|n| n.trim().parse().ok()).collect();
    let area: Area = v.try_into().ok()?;
    (area.iter().all(|n| (0.0..=1.0).contains(n)) && area[2] - area[0] >= 0.02 && area[3] - area[1] >= 0.02).then_some(area)
}

/// Measures a rectangle of an RGBA picture, using at most ~16k samples.
/// Clipped (≥250) and crushed (≤5) pixels are left out so they do not bias the averages.
pub fn measure(rgba: &[u8], w: usize, h: usize, area: Area) -> Option<Stats> {
    if w == 0 || h == 0 || rgba.len() < w * h * 4 { return None; }
    let x0 = ((w as f32 * area[0].clamp(0.0, 1.0)) as usize).min(w - 1);
    let y0 = ((h as f32 * area[1].clamp(0.0, 1.0)) as usize).min(h - 1);
    let x1 = ((w as f32 * area[2].clamp(0.0, 1.0)).ceil() as usize).clamp(x0 + 1, w);
    let y1 = ((h as f32 * area[3].clamp(0.0, 1.0)).ceil() as usize).clamp(y0 + 1, h);
    let (rw, rh) = (x1 - x0, y1 - y0);
    let step = ((rw * rh) as f32 / 16_384.0).sqrt().ceil().max(1.0) as usize;
    let table = decode_table();
    let mut sum = [0.0f64; 3]; // linear light
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
        hist: std::array::from_fn(|bin| histogram[bin * 8..bin * 8 + 8].iter().sum::<u32>() as f32 / count as f32),
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
    /// Plain-language headline, e.g. "The target is darker".
    pub summary: &'static str,
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
        out.push(Suggestion { setting: "Exposure", summary: "Brightness matches", value: format!("{ev:+.2} EV"), note: "", matched: true });
    } else {
        let base = if tgt_cam.exposure_known() { Some((tgt_cam, "")) }
            else if ref_cam.exposure_known() { Some((ref_cam, "Based on the reference camera's settings")) }
            else { None };
        match base {
            Some((cam, note)) => {
                let plan = exposure_plan(ev, cam, fps);
                for (index, (text, _)) in plan.into_iter().enumerate() {
                    out.push(Suggestion { setting: if index == 0 { "Exposure" } else { "Exposure (then)" },
                        summary: if index > 0 { "" } else if ev > 0.0 { "The target is darker" } else { "The target is brighter" },
                        value: text, note, matched: false });
                }
            }
            None => out.push(Suggestion { setting: "Exposure",
                summary: if ev > 0.0 { "The target is darker" } else { "The target is brighter" },
                value: format!("{} {:.1} EV ({:+.0}%)", if ev > 0.0 { "Open up" } else { "Close down" }, ev.abs(), (2f32.powf(ev) - 1.0) * 100.0),
                note: "Enter the camera settings for exact values", matched: false }),
        }
    }
    let mired = wb_mired(reference, target);
    if mired.abs() < 6.0 {
        out.push(Suggestion { setting: "White balance", summary: "Colour temperature matches", value: format!("{mired:+.0} mired"), note: "", matched: true });
    } else {
        let value = if tgt_cam.kelvin > 0.0 {
            let now = 1.0e6 / tgt_cam.kelvin;
            let new = (1.0e6 / (now - mired).clamp(60.0, 500.0)).round();
            format!("{:.0} K → {:.0} K", tgt_cam.kelvin, new)
        } else {
            let to = 1.0e6 / (1.0e6 / 5600.0 - mired).clamp(60.0, 500.0);
            format!("{} {:.0} mired (≈ {:.0} K around 5600 K)", if mired > 0.0 { "Raise by" } else { "Lower by" }, mired.abs(), to)
        };
        out.push(Suggestion { setting: "White balance", summary: if mired > 0.0 { "The target looks bluer (cooler)" } else { "The target looks more orange (warmer)" }, value, note: if mired > 0.0 { "Target is bluer: warm it up" } else { "Target is warmer: cool it down" }, matched: false });
    }
    let green = tint(target) - tint(reference);
    if green.abs() < 0.02 {
        out.push(Suggestion { setting: "Tint", summary: "No green or magenta difference", value: "OK".into(), note: "", matched: true });
    } else {
        out.push(Suggestion { setting: "Tint", summary: if green > 0.0 { "The target looks greener" } else { "The target looks more magenta" }, value: format!("{:.0}% {}", green.abs() * 100.0, if green > 0.0 { "greener: add magenta" } else { "more magenta: add green" }), note: "", matched: false });
    }
    let (spread_r, spread_t) = ((reference.p95 - reference.p5).max(0.02), (target.p95 - target.p5).max(0.02));
    let contrast = spread_t / spread_r;
    if (contrast - 1.0).abs() < 0.08 {
        out.push(Suggestion { setting: "Contrast", summary: "Contrast matches", value: format!("{:+.0}%", (contrast - 1.0) * 100.0), note: "", matched: true });
    } else {
        out.push(Suggestion { setting: "Contrast", summary: if contrast > 1.0 { "The target has more contrast" } else { "The target looks flatter (less contrast)" }, value: format!("{} {:.0}%", if contrast > 1.0 { "Lower by" } else { "Raise by" }, (contrast - 1.0).abs() * 100.0),
            note: "Picture profile, gamma or detail settings", matched: false });
    }
    let saturation = target.sat / reference.sat.max(0.01);
    if (saturation - 1.0).abs() < 0.1 {
        out.push(Suggestion { setting: "Saturation", summary: "Colour intensity matches", value: format!("{:+.0}%", (saturation - 1.0) * 100.0), note: "", matched: true });
    } else {
        out.push(Suggestion { setting: "Saturation", summary: if saturation > 1.0 { "The target colours are stronger" } else { "The target colours are paler" }, value: format!("{} {:.0}%", if saturation > 1.0 { "Lower by" } else { "Raise by" }, (saturation - 1.0).abs() * 100.0),
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
    /// Part of each picture that is measured (drag on a preview to change it).
    pub ref_area: Area,
    pub tgt_area: Area,
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
    pub hardware_request: Option<(u32, f32, f32)>,
}

impl Default for State {
    fn default() -> Self {
        Self { open: false, reference: None, target: None, ref_area: DEFAULT_AREA, tgt_area: DEFAULT_AREA, ref_cam: Camera::default(), tgt_cam: Camera::default(),
            ref_raw: Meter::default(), tgt_raw: Meter::default(), ref_final: Meter::default(), tgt_final: Meter::default(),
            auto: false, steps: 0, undo: None, status: String::new(), hardware_request: None }
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
        let join = |a: Area| format!("{},{},{},{}", a[0], a[1], a[2], a[3]);
        Some(format!("cammatch\t{}\t{}\t{};{}\t{cams}\n", number(self.reference), number(self.target),
            join(self.ref_area), join(self.tgt_area)))
    }

    /// `areas` is "left,top,right,bottom;left,top,right,bottom". Older presets stored a single
    /// centred fraction, which is read as a centred rectangle for both cameras.
    pub fn load_fields(&mut self, reference: Option<u32>, target: Option<u32>, areas: &str, cams: &str) {
        self.reference = reference;
        self.target = target;
        let centred = |f: f32| { let m = (1.0 - f.clamp(0.1, 1.0)) / 2.0; [m, m, 1.0 - m, 1.0 - m] };
        (self.ref_area, self.tgt_area) = match areas.split_once(';') {
            Some((a, b)) => (parse_area(a).unwrap_or(DEFAULT_AREA), parse_area(b).unwrap_or(DEFAULT_AREA)),
            None => { let a = areas.parse::<f32>().map_or(DEFAULT_AREA, centred); (a, a) }
        };
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

const REF_COLOUR: Color32 = Color32::from_rgb(110, 170, 255);
const TARGET_COLOUR: Color32 = Color32::from_rgb(255, 170, 80);
const GOOD: Color32 = Color32::from_rgb(110, 190, 120);
const WARN: Color32 = Color32::from_rgb(235, 190, 90);
const CHART_BG: Color32 = Color32::from_rgb(24, 26, 30);
const CHART_W: f32 = 224.0;
const CHART_H: f32 = 70.0;

/// 0–100 similarity score from the colour difference of the two means.
pub fn similarity(de: f32) -> f32 { (100.0 - de * 4.0).clamp(0.0, 100.0) }

fn verdict(score: f32) -> (&'static str, Color32) {
    if score >= 90.0 { ("Almost identical", GOOD) }
    else if score >= 75.0 { ("Very close", GOOD) }
    else if score >= 50.0 { ("Visible difference", WARN) }
    else { ("Very different", Color32::from_rgb(230, 100, 90)) }
}

/// Live picture of an input. Drag on it to choose the measured area; double-click resets it.
/// Returns true when the area changed.
fn preview(ui: &mut egui::Ui, title: &str, colour: Color32, texture: Option<(egui::TextureId, [usize; 2])>, area: &mut Area) -> bool {
    let size = Vec2::new(CHART_W, CHART_W * 9.0 / 16.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::BLACK);
    let mut shown = rect;
    if let Some((id, [w, h])) = texture {
        let scale = (rect.width() / w.max(1) as f32).min(rect.height() / h.max(1) as f32);
        shown = egui::Rect::from_center_size(rect.center(), Vec2::new(w as f32 * scale, h as f32 * scale));
        painter.image(id, shown, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    } else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, tr(ui, "No picture yet"), egui::FontId::proportional(13.0), Color32::GRAY);
    }
    let mut changed = false;
    if shown.width() > 1.0 && shown.height() > 1.0 {
        let to_fraction = |p: egui::Pos2| [((p.x - shown.left()) / shown.width()).clamp(0.0, 1.0),
            ((p.y - shown.top()) / shown.height()).clamp(0.0, 1.0)];
        if response.dragged() {
            if let (Some(from), Some(to)) = (ui.input(|i| i.pointer.press_origin()), response.interact_pointer_pos()) {
                let (a, b) = (to_fraction(from), to_fraction(to));
                let dragged = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
                if dragged[2] - dragged[0] >= 0.05 && dragged[3] - dragged[1] >= 0.05 {
                    *area = dragged;
                    changed = true;
                }
            }
        }
        if response.double_clicked() { *area = DEFAULT_AREA; changed = true; }
    }
    let marked = egui::Rect::from_min_max(
        egui::pos2(shown.left() + shown.width() * area[0], shown.top() + shown.height() * area[1]),
        egui::pos2(shown.left() + shown.width() * area[2], shown.top() + shown.height() * area[3]));
    let shade = Color32::from_black_alpha(130);
    for outside in [
        egui::Rect::from_min_max(shown.min, egui::pos2(shown.right(), marked.top())),
        egui::Rect::from_min_max(egui::pos2(shown.left(), marked.bottom()), shown.max),
        egui::Rect::from_min_max(egui::pos2(shown.left(), marked.top()), egui::pos2(marked.left(), marked.bottom())),
        egui::Rect::from_min_max(egui::pos2(marked.right(), marked.top()), egui::pos2(shown.right(), marked.bottom())),
    ] { painter.rect_filled(outside, 0.0, shade); }
    painter.rect_stroke(marked, 0.0, egui::Stroke::new(1.5f32, colour), egui::StrokeKind::Middle);
    let label = painter.layout_no_wrap(title.to_owned(), egui::FontId::proportional(12.0), Color32::BLACK);
    let chip = egui::Rect::from_min_size(rect.min + Vec2::new(6.0, 6.0), label.size() + Vec2::new(10.0, 4.0));
    painter.rect_filled(chip, 3.0, colour);
    painter.galley(chip.min + Vec2::new(5.0, 2.0), label, Color32::BLACK);
    response.on_hover_text(tr(ui, "Drag on the picture to choose the area to measure. Double-click to reset."));
    changed
}

/// A titled dark chart area; returns its painter and plot rectangle.
fn chart_area(ui: &mut egui::Ui, title: &str, hint: &str) -> (egui::Painter, egui::Rect) {
    ui.label(RichText::new(title).strong());
    let (rect, response) = ui.allocate_exact_size(Vec2::new(CHART_W, CHART_H), Sense::hover());
    response.on_hover_text(hint);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, CHART_BG);
    (painter, rect.shrink2(Vec2::new(8.0, 8.0)))
}

fn legend(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.colored_label(REF_COLOUR, format!("■ {}", tr(ui, "Reference")));
        ui.colored_label(TARGET_COLOUR, format!("■ {}", tr(ui, "Target")));
    });
}

/// Brightness distribution of both pictures: bars for the target, outline for the reference.
fn brightness_chart(ui: &mut egui::Ui, r: &Stats, t: &Stats) {
    let (p, plot) = chart_area(ui, tr(ui, "Brightness"), tr(ui, "Left is dark, right is bright. The shapes should overlap."));
    let top = r.hist.iter().chain(t.hist.iter()).fold(0.01f32, |a, b| a.max(*b));
    let bin = plot.width() / 32.0;
    let height = |v: f32| plot.height() * (v / top).sqrt().min(1.0);
    for (i, v) in t.hist.iter().enumerate() {
        let x = plot.left() + i as f32 * bin;
        p.rect_filled(egui::Rect::from_min_max(egui::pos2(x + 0.5, plot.bottom() - height(*v)), egui::pos2(x + bin - 0.5, plot.bottom())),
            0.0, TARGET_COLOUR.gamma_multiply(0.7));
    }
    let points: Vec<_> = r.hist.iter().enumerate()
        .map(|(i, v)| egui::pos2(plot.left() + (i as f32 + 0.5) * bin, plot.bottom() - height(*v))).collect();
    p.add(egui::Shape::line(points, egui::Stroke::new(2.0f32, REF_COLOUR)));
}

/// Average red, green and blue of both pictures side by side.
fn colour_chart(ui: &mut egui::Ui, r: &Stats, t: &Stats) {
    let (p, plot) = chart_area(ui, tr(ui, "Colours"), tr(ui, "Each pair of bars should have the same height."));
    let group = plot.width() / 3.0;
    let base = plot.bottom() - 14.0;
    let full = base - plot.top();
    for (c, (name, colour)) in [("R", Color32::from_rgb(235, 90, 80)), ("G", Color32::from_rgb(110, 200, 110)), ("B", Color32::from_rgb(100, 140, 240))].into_iter().enumerate() {
        let x = plot.left() + c as f32 * group;
        for (k, (stats, tint)) in [(r, REF_COLOUR), (t, TARGET_COLOUR)].into_iter().enumerate() {
            let h = full * encode(stats.lin[c]);
            let bx = x + group * 0.18 + k as f32 * group * 0.32;
            p.rect_filled(egui::Rect::from_min_max(egui::pos2(bx, base - h), egui::pos2(bx + group * 0.28, base)), 2.0, tint);
        }
        p.text(egui::pos2(x + group * 0.5, plot.bottom() - 6.0), egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(12.0), colour);
    }
}

/// Cool-to-warm scale with a marker per camera.
fn temperature_chart(ui: &mut egui::Ui, r: &Stats, t: &Stats) {
    let (p, plot) = chart_area(ui, tr(ui, "Colour temperature"), tr(ui, "Both markers should sit in the same place."));
    let bar = egui::Rect::from_center_size(plot.center(), Vec2::new(plot.width(), 18.0));
    let (cool, warm) = (Color32::from_rgb(110, 160, 255), Color32::from_rgb(255, 160, 70));
    let steps = 32;
    let mut mesh = egui::epaint::Mesh::default();
    for i in 0..steps {
        let (a, b) = (i as f32 / steps as f32, (i + 1) as f32 / steps as f32);
        let colour = |k: f32| {
            let mid = Color32::from_gray(235);
            if k < 0.5 { lerp_colour(cool, mid, k * 2.0) } else { lerp_colour(mid, warm, (k - 0.5) * 2.0) }
        };
        let (x0, x1) = (bar.left() + bar.width() * a, bar.left() + bar.width() * b);
        let i0 = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(x0, bar.top()), colour(a));
        mesh.colored_vertex(egui::pos2(x1, bar.top()), colour(b));
        mesh.colored_vertex(egui::pos2(x1, bar.bottom()), colour(b));
        mesh.colored_vertex(egui::pos2(x0, bar.bottom()), colour(a));
        mesh.add_triangle(i0, i0 + 1, i0 + 2);
        mesh.add_triangle(i0, i0 + 2, i0 + 3);
    }
    p.add(egui::Shape::mesh(mesh));
    let position = |s: &Stats| 0.5 - (s.lin[2].max(1e-4) / s.lin[0].max(1e-4)).ln() / 3.0;
    for (stats, colour, above) in [(r, REF_COLOUR, true), (t, TARGET_COLOUR, false)] {
        let x = bar.left() + bar.width() * position(stats).clamp(0.0, 1.0);
        let (tip, far) = if above { (bar.top(), bar.top() - 12.0) } else { (bar.bottom(), bar.bottom() + 12.0) };
        p.add(egui::Shape::convex_polygon(vec![egui::pos2(x, tip), egui::pos2(x - 6.0, far), egui::pos2(x + 6.0, far)], colour, egui::Stroke::new(1.0f32, Color32::BLACK)));
    }
    p.text(egui::pos2(bar.left(), plot.bottom()), egui::Align2::LEFT_BOTTOM, tr(ui, "Cooler"), egui::FontId::proportional(11.0), cool);
    p.text(egui::pos2(bar.right(), plot.bottom()), egui::Align2::RIGHT_BOTTOM, tr(ui, "Warmer"), egui::FontId::proportional(11.0), warm);
}

/// Range from the darkest to the brightest areas of each picture.
fn contrast_chart(ui: &mut egui::Ui, r: &Stats, t: &Stats) {
    let (p, plot) = chart_area(ui, tr(ui, "Contrast range"), tr(ui, "Longer bars have more contrast. The ends should line up."));
    for (row, (stats, colour)) in [(r, REF_COLOUR), (t, TARGET_COLOUR)].into_iter().enumerate() {
        let y = plot.top() + plot.height() * (0.25 + 0.5 * row as f32);
        let x = |v: f32| plot.left() + plot.width() * v.clamp(0.0, 1.0);
        p.line_segment([egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)], egui::Stroke::new(1.0f32, Color32::from_gray(60)));
        p.rect_filled(egui::Rect::from_min_max(egui::pos2(x(stats.p5), y - 7.0), egui::pos2(x(stats.p95), y + 7.0)), 3.0, colour.gamma_multiply(0.8));
        p.line_segment([egui::pos2(x(stats.p50), y - 9.0), egui::pos2(x(stats.p50), y + 9.0)], egui::Stroke::new(2.0f32, Color32::WHITE));
    }
}

fn camera_fields_row(ui: &mut egui::Ui, state: &mut State) {
    ui.columns(2, |cols| {
        cols[0].label(RichText::new(tr(&cols[0], "Reference")).color(REF_COLOUR).strong());
        camera_fields(&mut cols[0], "cam-match-ref-fields", &mut state.ref_cam);
        cols[1].label(RichText::new(tr(&cols[1], "Target")).color(TARGET_COLOUR).strong());
        camera_fields(&mut cols[1], "cam-match-target-fields", &mut state.tgt_cam);
    });
}

fn lerp_colour(a: Color32, b: Color32, k: f32) -> Color32 {
    let mix = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * k).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

/// The Camera Match window (opened from the top bar).
pub fn window(ctx: &egui::Context, s: &mut Switcher) {
    if !s.cam_match.open { return; }
    let choices: Vec<(u32, String)> = s.inputs.iter().filter(|i| i.kind.streamed()).map(|i| (i.id, i.title.clone())).collect();
    let fps = output_fps();
    let textures = {
        let tex = |id: Option<u32>| id.and_then(|id| s.input(id)).and_then(|input| input.tex);
        [tex(s.cam_match.reference), tex(s.cam_match.target)]
    };
    let mut open = true;
    let mut state = std::mem::take(&mut s.cam_match);
    if state.reference.is_none() && state.target.is_none() && choices.len() >= 2 {
        state.reference = Some(choices[0].0);
        state.target = Some(choices[1].0);
    }
    let mut start = false;
    let mut stop = false;
    let mut undo = false;
    egui::Window::new(locale::text(ctx, "Camera Match")).id(egui::Id::new("Camera Match")).open(&mut open)
        .default_width(700.0).show(ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            if choices.len() < 2 {
                ui.label(tr(ui, "Add at least two camera inputs to compare them."));
                return;
            }
            ui.label(RichText::new(tr(ui, "1. Pick two cameras   2. Aim both at the same scene   3. Follow the tips or press Match automatically")).weak());
            ui.add_space(4.0);
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.colored_label(REF_COLOUR, tr(ui, "Reference"));
                changed |= input_combo(ui, "cam-match-ref", &choices, &mut state.reference);
                if ui.button("⇄").on_hover_text(tr(ui, "Swap reference and target")).clicked() {
                    std::mem::swap(&mut state.reference, &mut state.target);
                    std::mem::swap(&mut state.ref_cam, &mut state.tgt_cam);
                    changed = true;
                }
                ui.colored_label(TARGET_COLOUR, tr(ui, "Target"));
                changed |= input_combo(ui, "cam-match-target", &choices, &mut state.target);
            });
            if changed { state.reset_measurements(); state.auto = false; state.undo = None; state.status.clear(); }
            if state.reference.is_some() && state.reference == state.target {
                ui.label(tr(ui, "Choose two different inputs."));
                return;
            }
            ui.add_space(4.0);
            let stats = state.ref_raw.stats.zip(state.tgt_raw.stats);
            let ready = state.ref_raw.fresh >= SETTLE_PICTURES && state.tgt_raw.fresh >= SETTLE_PICTURES;
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    if preview(ui, tr(ui, "Reference"), REF_COLOUR, textures[0], &mut state.ref_area) {
                        state.ref_raw.reset();
                        state.ref_final.reset();
                    }
                    ui.add_space(4.0);
                    if preview(ui, tr(ui, "Target"), TARGET_COLOUR, textures[1], &mut state.tgt_area) {
                        state.tgt_raw.reset();
                        state.tgt_final.reset();
                    }
                });
                ui.add_space(6.0);
                ui.vertical(|ui| {
                    let Some((r, t)) = stats else {
                        ui.label(tr(ui, "Waiting for pictures from both inputs…"));
                        return;
                    };
                    let score = similarity(delta_e(&r, &t));
                    let (words, colour) = verdict(score);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{score:.0}%")).size(30.0).strong().color(colour));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(if ready { tr(ui, words) } else { tr(ui, "Measuring…") }).size(15.0).color(colour));
                            ui.add(egui::ProgressBar::new(score / 100.0).desired_width(200.0).fill(colour));
                        });
                        ui.add_space(10.0);
                        legend(ui);
                    });
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| { brightness_chart(ui, &r, &t); ui.add_space(2.0); temperature_chart(ui, &r, &t); });
                        ui.vertical(|ui| { colour_chart(ui, &r, &t); ui.add_space(2.0); contrast_chart(ui, &r, &t); });
                    });
                });
            });
            let Some((r, t)) = stats else { return };
            ui.separator();
            ui.columns(2, |cols| {
                let ui = &mut cols[0];
                ui.label(RichText::new(tr(ui, "What to change on the target camera")).strong());
                for item in suggest(&r, &t, &state.ref_cam, &state.tgt_cam, fps) {
                    let colour = if item.matched { GOOD } else { WARN };
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(if item.matched { "✔" } else { "⚠" }).color(colour));
                        ui.vertical(|ui| {
                            if !item.summary.is_empty() { ui.label(RichText::new(tr(ui, item.summary)).color(colour).strong()); }
                            if !item.matched || item.setting != "Exposure" {
                                ui.label(format!("{}: {}", tr(ui, item.setting), item.value));
                            }
                            if !item.note.is_empty() { ui.label(RichText::new(tr(ui, item.note)).weak().small()); }
                        });
                    });
                }
                let ui = &mut cols[1];
                ui.label(RichText::new(tr(ui, "Match automatically")).strong());
                ui.label(RichText::new(tr(ui, "Corrects the target picture inside LiveMix. The camera itself is not changed, so you can undo it at any time.")).weak().small());
                ui.horizontal(|ui| {
                    if state.auto {
                        if ui.button(tr(ui, "Stop")).clicked() { stop = true; }
                        ui.add(egui::ProgressBar::new(state.steps as f32 / MAX_STEPS as f32).desired_width(110.0));
                    } else if ui.add_sized([150.0, 28.0], egui::Button::new(tr(ui, "Match automatically"))).clicked() {
                        start = true;
                    }
                    if ui.add_enabled(state.undo.is_some(), egui::Button::new(tr(ui, "Undo"))).clicked() { undo = true; }
                });
                if let Some(camera) = state.target.and_then(|id| s.input(id))
                    .and_then(|input| input.sony_camera.as_deref()).filter(|camera| !camera.is_empty()) {
                    crate::sony_remote::match_controls(ui, camera, delta_ev(&r, &t), wb_mired(&r, &t), fps, !state.auto);
                }
                #[cfg(windows)]
                if let Some(target) = state.target.filter(|id| s.input(*id).is_some_and(|input| {
                    matches!(&input.kind, crate::Kind::Device { link, .. } if link.starts_with(r"\\?\usb#"))
                })) {
                    if ui.add_enabled(!state.auto, egui::Button::new("Apply exposure and white balance to USB camera")).clicked() {
                        state.hardware_request = Some((target, delta_ev(&r, &t).clamp(-3.0, 3.0), wb_mired(&r, &t).clamp(-100.0, 100.0)));
                        state.status = "Sending exposure and white balance to the capture driver…".into();
                    }
                }
                if let Some(status) = state.target.and_then(|id| s.input(id)).map(|input| &input.status)
                    .filter(|status| status.starts_with("USB camera:")) {
                    ui.label(status);
                } else if !state.status.is_empty() {
                    ui.label(&state.status);
                }
                ui.add_space(4.0);
                egui::CollapsingHeader::new(tr(ui, "Camera settings and measuring (optional)")).show(ui, |ui| {
                    ui.label(RichText::new(tr(ui, "Enter the settings you know to get exact values instead of general advice.")).weak().small());
                    camera_fields_row(ui, &mut state);
                    ui.horizontal(|ui| {
                        if ui.button(tr(ui, "Reset areas")).clicked() {
                            state.ref_area = DEFAULT_AREA;
                            state.tgt_area = DEFAULT_AREA;
                            state.reset_measurements();
                        }
                        if ui.button(tr(ui, "Restart measuring")).clicked() { state.reset_measurements(); }
                    });
                });
            });
        });
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

    fn stats(rgb: [u8; 3]) -> Stats { measure(&flat(64, 64, rgb), 64, 64, FULL).unwrap() }

    #[test]
    fn measures_flat_pictures() {
        let grey = stats([128, 128, 128]);
        assert!((grey.lin[0] - 0.2158).abs() < 0.002, "{:?}", grey.lin);
        assert!((grey.p50 - 128.0 / 255.0).abs() < 0.01);
        assert!(grey.sat < 0.001);
        assert!(measure(&flat(64, 64, [255, 255, 255]), 64, 64, FULL).is_none(), "clipped pictures give no measurement");
        assert!(measure(&[0; 4], 64, 64, FULL).is_none());
    }

    #[test]
    fn measures_only_the_chosen_rectangle() {
        // Left half dark grey, right half light grey.
        let (w, h) = (64usize, 64usize);
        let picture: Vec<u8> = (0..w * h).flat_map(|i| { let v = if i % w < w / 2 { 60 } else { 180 }; [v, v, v, 255] }).collect();
        let left = measure(&picture, w, h, [0.0, 0.0, 0.5, 1.0]).unwrap();
        let right = measure(&picture, w, h, [0.5, 0.0, 1.0, 1.0]).unwrap();
        assert!(right.luma > left.luma * 3.0, "{} vs {}", left.luma, right.luma);
        assert!(measure(&picture, w, h, [0.9, 0.9, 0.9, 0.9]).is_none(), "a degenerate rectangle gives too few samples");
        assert_eq!(parse_area("0.1,0.2,0.6,0.9"), Some([0.1, 0.2, 0.6, 0.9]));
        assert_eq!(parse_area("0.5,0.5,0.5,0.5"), None);
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
        let goal = measure(&flat(64, 64, reference), 64, 64, FULL).unwrap();
        let mut adjust = ColourAdjust::default();
        let mut last = f32::MAX;
        let mut done = false;
        for _ in 0..30 {
            let mut picture = source.clone();
            adjust.apply(&mut picture);
            let now = measure(&picture, 64, 64, FULL).unwrap();
            let (next, de, finished) = correction_step(adjust, &goal, &now);
            last = de;
            adjust = next;
            if finished { done = true; break; }
        }
        assert!(done && last < 1.0, "did not converge: ΔE {last}");
    }

    #[test]
    fn state_round_trips_through_its_preset_line() {
        let mut state = State { reference: Some(3), target: Some(5), ref_area: [0.1, 0.2, 0.6, 0.9], tgt_area: [0.3, 0.3, 0.8, 0.8], ..State::default() };
        state.tgt_cam = Camera { kelvin: 5600.0, iso: 800.0, shutter: 50.0, aperture: 2.8 };
        let line = state.save_line(|id| id.map_or(0, |id| id as usize)).unwrap();
        let f: Vec<&str> = line.trim_end().split('\t').collect();
        let mut loaded = State::default();
        loaded.load_fields(Some(3), Some(5), f[3], f[4]);
        assert_eq!(loaded.tgt_cam, state.tgt_cam);
        assert_eq!((loaded.ref_area, loaded.tgt_area), (state.ref_area, state.tgt_area));
        assert!(State::default().save_line(|_| 0).is_none());
    }
}
