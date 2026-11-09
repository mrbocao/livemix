// Drawing of inputs, transitions, overlays and meters.

use crate::model::{Effect, Input, Kind, Position};
use eframe::egui::{self, pos2, Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Vec2};

// Colours sampled from the screenshot of the running original (reference/picture-1.png).
// (The blue values in the layout code — 26,60,117 / 40,81,155 — are design-time defaults that
// the original's theme replaces at run time.)
/// Top bar, bottom bar and status line #0A0F14.
pub const WINDOW: Color32 = Color32::from_rgb(0x0a, 0x0f, 0x14);
/// Main background #1E2328.
pub const PANEL: Color32 = Color32::from_rgb(0x1e, 0x23, 0x28);
pub const BG: Color32 = PANEL;
/// Button fill #404B56 (transition buttons); top-bar buttons #293038.
pub const BTN: Color32 = Color32::from_rgb(0x40, 0x4b, 0x56);
pub const BTN_DARK: Color32 = Color32::from_rgb(0x29, 0x30, 0x38);
pub const BTN_HOVER: Color32 = Color32::from_rgb(0x55, 0x63, 0x71);
/// Original's MouseOverBackColor 39,90,176 (used for "busy/active" blue).
pub const HOVER: Color32 = Color32::from_rgb(39, 90, 176);
/// Transition/Cut/FTB/overlay button hover 45,216,255 (used for "on" overlay buttons).
pub const CYAN: Color32 = Color32::from_rgb(45, 216, 255);
/// Active red 208,24,24.
pub const ACTIVE_RED: Color32 = Color32::from_rgb(208, 24, 24);
/// Divider #293038.
pub const EDGE: Color32 = Color32::from_rgb(0x29, 0x30, 0x38);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xea);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9a, 0xa2, 0xaa);
/// config PreviewColour = DarkOrange
pub const PREVIEW: Color32 = Color32::from_rgb(0xff, 0x8c, 0x00);
/// config ActiveColour = DarkGreen
pub const OUTPUT: Color32 = Color32::from_rgb(0x00, 0x64, 0x00);
pub const OUTPUT_BRIGHT: Color32 = Color32::from_rgb(0x00, 0x64, 0x00);

pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

fn vgrad(p: &Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.right_bottom(), bottom);
    m.colored_vertex(r.left_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

fn hgrad(p: &Painter, r: Rect, left: Color32, right: Color32) {
    let mut m = Mesh::default();
    m.colored_vertex(r.left_top(), left);
    m.colored_vertex(r.right_top(), right);
    m.colored_vertex(r.right_bottom(), right);
    m.colored_vertex(r.left_bottom(), left);
    m.add_triangle(0, 1, 2);
    m.add_triangle(0, 2, 3);
    p.add(Shape::mesh(m));
}

fn hsv(h: f32, s: f32, v: f32) -> Color32 {
    egui::ecolor::Hsva::new(h.rem_euclid(1.0), s, v, 1.0).into()
}

pub fn checker(p: &Painter, r: Rect) {
    p.rect_filled(r, 0.0, Color32::from_rgb(0x30, 0x30, 0x30));
    let s = 8.0;
    let mut y = r.top();
    let mut row = 0;
    while y < r.bottom() {
        let mut x = r.left() + if row % 2 == 0 { 0.0 } else { s };
        while x < r.right() {
            let c = Rect::from_min_max(pos2(x, y), pos2((x + s).min(r.right()), (y + s).min(r.bottom())));
            p.rect_filled(c, 0.0, Color32::from_rgb(0x3c, 0x3c, 0x3c));
            x += s * 2.0;
        }
        y += s;
        row += 1;
    }
}

/// Rectangle an input takes inside `r` for a position (MatrixPosition @0x2bc778: scale about the
/// centre by zoom, then move by pan × half the size; positive Pan Y moves up).
pub fn place_rect(r: Rect, pos: &Position) -> Rect {
    let c = r.center() + Vec2::new(pos.pan_x * r.width() / 2.0, -pos.pan_y * r.height() / 2.0);
    Rect::from_center_size(c, r.size() * pos.zoom.max(0.0))
}

/// The visible part of a placed picture after cropping.
fn crop_rect(sr: Rect, pos: &Position) -> Rect {
    let [x1, y1, x2, y2] = pos.crop;
    Rect::from_min_max(
        sr.min + Vec2::new(x1.min(x2) * sr.width(), y1.min(y2) * sr.height()),
        sr.min + Vec2::new(x2.max(x1) * sr.width(), y2.max(y1) * sr.height()),
    )
}

/// Draws one input in `r` with its Position, crop and Layers, at opacity `a`.
/// `all` is the input list (layers refer to other inputs).
pub fn draw_source(p: &Painter, r: Rect, inp: &Input, a: f32, t: f64, all: &[Input]) {
    draw_layered(p, r, inp, a, t, all, 0);
}

/// Input box picture with "Disable Effects on Input Previews": the input's own picture only,
/// without its Position, crop or Layers.
pub fn draw_plain(p: &Painter, r: Rect, inp: &Input, t: f64) {
    draw_raw(p, r, inp, 1.0, t);
}

fn draw_layered(p: &Painter, r: Rect, inp: &Input, a: f32, t: f64, all: &[Input], depth: u32) {
    let sr = place_rect(r, &inp.place);
    let clip = crop_rect(sr, &inp.place).intersect(p.clip_rect());
    if clip.width() > 0.0 && clip.height() > 0.0 {
        draw_raw(&p.with_clip_rect(clip), sr, inp, a, t);
    }
    // Layers 1–10 on top, in order (an input can't be its own layer; two levels deep at most).
    if depth >= 2 {
        return;
    }
    for l in inp.layers.iter().filter(|l| l.on) {
        let Some(li) = l.input.filter(|id| *id != inp.id).and_then(|id| all.iter().find(|i| i.id == id)) else { continue };
        let lr = place_rect(r, &l.place);
        let lc = crop_rect(lr, &l.place).intersect(p.clip_rect());
        if lc.width() > 0.0 && lc.height() > 0.0 {
            draw_layered(&p.with_clip_rect(lc), lr, li, a, t, all, depth + 1);
        }
    }
}

/// Draws the input's own picture filling `r` with opacity `a`.
fn draw_raw(p: &Painter, r: Rect, inp: &Input, a: f32, t: f64) {
    if a <= 0.0 {
        return;
    }
    let h = r.height();
    let w = r.width();
    match &inp.kind {
        Kind::Colour(c) => {
            p.rect_filled(r, 0.0, fade(*c, a));
        }
        Kind::Blank => {}
        Kind::Bars => {
            let cols = [
                Color32::from_rgb(191, 191, 191),
                Color32::from_rgb(191, 191, 0),
                Color32::from_rgb(0, 191, 191),
                Color32::from_rgb(0, 191, 0),
                Color32::from_rgb(191, 0, 191),
                Color32::from_rgb(191, 0, 0),
                Color32::from_rgb(0, 0, 191),
            ];
            let bw = w / 7.0;
            for (k, c) in cols.iter().enumerate() {
                let br = Rect::from_min_size(pos2(r.left() + bw * k as f32, r.top()), Vec2::new(bw + 0.5, h * 0.67));
                p.rect_filled(br, 0.0, fade(*c, a));
            }
            let mid = [cols[6], Color32::from_rgb(19, 19, 19), cols[4], Color32::from_rgb(19, 19, 19), cols[2], Color32::from_rgb(19, 19, 19), cols[0]];
            for (k, c) in mid.iter().enumerate() {
                let br = Rect::from_min_size(pos2(r.left() + bw * k as f32, r.top() + h * 0.67), Vec2::new(bw + 0.5, h * 0.08));
                p.rect_filled(br, 0.0, fade(*c, a));
            }
            let low = Rect::from_min_max(pos2(r.left(), r.top() + h * 0.75), r.right_bottom());
            p.rect_filled(low, 0.0, fade(Color32::from_rgb(16, 16, 16), a));
            let wr = Rect::from_min_size(pos2(r.left() + bw * 1.25, low.top()), Vec2::new(bw * 1.25, low.height()));
            p.rect_filled(wr, 0.0, fade(Color32::from_rgb(235, 235, 235), a));
        }
        Kind::Title { text, sub } => {
            let bar = Rect::from_min_max(pos2(r.left() + w * 0.06, r.top() + h * 0.72), pos2(r.left() + w * 0.62, r.top() + h * 0.86));
            hgrad(p, bar, fade(Color32::from_rgb(0x12, 0x36, 0x7a), a), fade(Color32::from_rgb(0x1c, 0x5c, 0xc0), a * 0.9));
            let accent = Rect::from_min_size(bar.left_top(), Vec2::new(w * 0.008, bar.height()));
            p.rect_filled(accent, 0.0, fade(Color32::from_rgb(0xff, 0xb4, 0x20), a));
            let sbar = Rect::from_min_max(pos2(bar.left(), bar.bottom()), pos2(bar.left() + w * 0.46, bar.bottom() + h * 0.06));
            p.rect_filled(sbar, 0.0, fade(Color32::from_rgba_unmultiplied(10, 10, 14, 220), a));
            p.text(pos2(bar.left() + w * 0.025, bar.center().y), Align2::LEFT_CENTER, text, FontId::proportional((h * 0.075).max(6.0)), fade(Color32::WHITE, a));
            p.text(pos2(sbar.left() + w * 0.025, sbar.center().y), Align2::LEFT_CENTER, sub, FontId::proportional((h * 0.038).max(5.0)), fade(Color32::from_rgb(0xdd, 0xdd, 0xdd), a));
        }
        Kind::Clock => {
            vgrad(p, r, fade(Color32::from_rgb(0x08, 0x08, 0x10), a), fade(Color32::from_rgb(0x16, 0x16, 0x26), a));
            p.text(r.center() - Vec2::new(0.0, h * 0.06), Align2::CENTER_CENTER, clock(t), FontId::monospace((h * 0.2).max(8.0)), fade(Color32::from_rgb(0xff, 0xd0, 0x40), a));
            p.text(r.center() + Vec2::new(0.0, h * 0.14), Align2::CENTER_CENTER, "SHOW TIME", FontId::proportional((h * 0.06).max(5.0)), fade(TEXT_DIM, a));
        }
        Kind::Camera(seed) => {
            let sd = *seed as f32;
            let tint = (sd * 0.27) % 1.0;
            // studio backdrop
            vgrad(p, r, fade(hsv(0.6 + tint * 0.3, 0.45, 0.42), a), fade(hsv(0.62 + tint * 0.3, 0.55, 0.16), a));
            let floor = Rect::from_min_max(pos2(r.left(), r.top() + h * 0.72), r.right_bottom());
            vgrad(p, floor, fade(Color32::from_rgb(0x2a, 0x22, 0x1c), a), fade(Color32::from_rgb(0x12, 0x0e, 0x0c), a));
            // light spots
            for k in 0..3 {
                let cx = r.left() + w * (0.2 + 0.3 * k as f32);
                p.circle_filled(pos2(cx, r.top() + h * 0.12), h * 0.05, fade(Color32::from_rgba_unmultiplied(255, 250, 220, 40), a));
            }
            // desk
            let desk = Rect::from_min_max(pos2(r.left() + w * 0.18, r.top() + h * 0.66), pos2(r.left() + w * 0.82, r.top() + h * 0.8));
            p.rect_filled(desk, h * 0.02, fade(Color32::from_rgb(0x3a, 0x3e, 0x48), a));
            // person(s), slight sway
            let people = if seed % 2 == 0 { 2 } else { 1 };
            for k in 0..people {
                let sway = ((t * 0.9 + sd as f64 + k as f64 * 2.0).sin() as f32) * w * 0.01;
                let cx = if people == 1 { r.center().x } else { r.left() + w * (0.36 + 0.28 * k as f32) } + sway;
                let shirt = hsv(0.05 + 0.4 * ((sd + k as f32) * 0.37 % 1.0), 0.5, 0.55);
                let body = Rect::from_center_size(pos2(cx, r.top() + h * 0.6), Vec2::new(w * 0.16, h * 0.26));
                p.rect_filled(body, h * 0.06, fade(shirt, a));
                p.circle_filled(pos2(cx, r.top() + h * 0.38), h * 0.085, fade(Color32::from_rgb(0xd8, 0xa8, 0x84), a));
                p.circle_filled(pos2(cx, r.top() + h * 0.34), h * 0.07, fade(Color32::from_rgb(0x3a, 0x26, 0x1a), a));
            }
            p.text(r.left_top() + Vec2::new(w * 0.03, h * 0.04), Align2::LEFT_TOP, format!("CAM {seed}"), FontId::proportional((h * 0.06).max(5.0)), fade(Color32::from_white_alpha(160), a));
        }
        Kind::Video { hue } => {
            let ph = inp.pos * 0.05;
            hgrad(p, r, fade(hsv(*hue + ph, 0.7, 0.55), a), fade(hsv(*hue + 0.15 + ph, 0.8, 0.25), a));
            // moving shapes driven by the play position
            for k in 0..5 {
                let kf = k as f32;
                let x = r.left() + w * ((inp.pos * 0.07 + kf * 0.23).rem_euclid(1.2) - 0.1);
                let y = r.top() + h * (0.3 + 0.4 * ((inp.pos * 0.6 + kf * 1.3).sin() * 0.5 + 0.5));
                p.circle_filled(pos2(x, y), h * (0.05 + 0.03 * kf), fade(Color32::from_rgba_unmultiplied(255, 255, 255, 40), a));
            }
            p.text(r.center(), Align2::CENTER_CENTER, &inp.title, FontId::proportional((h * 0.11).max(6.0)), fade(Color32::from_white_alpha(230), a));
        }
        Kind::Device { .. } | Kind::VideoFile { .. } | Kind::ImageFile { .. } | Kind::VideoList { .. } | Kind::Desktop | Kind::ImageSequence { .. } | Kind::Photos { .. } => {
            let keyed = inp.key.on || matches!(inp.kind, Kind::ImageFile { .. } | Kind::ImageSequence { .. });
            match inp.tex {
                Some((tex, [tw, th])) if tw > 0 && th > 0 => {
                    // Letterbox a picture into the frame.
                    let fit = |tw: usize, th: usize| {
                        let ar = tw as f32 / th.max(1) as f32;
                        let mut fw = w;
                        let mut fh = w / ar;
                        if fh > h {
                            fh = h;
                            fw = h * ar;
                        }
                        Rect::from_center_size(r.center(), Vec2::new(fw, fh))
                    };
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    if !keyed {
                        p.rect_filled(r, 0.0, fade(Color32::BLACK, a));
                    }
                    let a = a.clamp(0.0, 1.0);
                    match inp.prev_tex {
                        // Photos: old picture underneath, new one fading in (500 ms Fade).
                        Some((ptex, [pw, ph], k)) if k < 1.0 => {
                            p.image(ptex, fit(pw, ph), uv, Color32::WHITE.gamma_multiply(a));
                            p.image(tex, fit(tw, th), uv, Color32::WHITE.gamma_multiply(a * k));
                        }
                        _ => {
                            p.image(tex, fit(tw, th), uv, Color32::WHITE.gamma_multiply(a));
                        }
                    }
                }
                _ => {
                    p.rect_filled(r, 0.0, fade(Color32::from_rgb(0x0c, 0x14, 0x26), a));
                    let msg = if inp.status.is_empty() { "Starting…".to_string() } else { inp.status.clone() };
                    p.text(r.center() - Vec2::new(0.0, h * 0.06), Align2::CENTER_CENTER, &inp.title, FontId::proportional((h * 0.08).max(7.0)), fade(Color32::WHITE, a));
                    p.text(r.center() + Vec2::new(0.0, h * 0.06), Align2::CENTER_CENTER, msg, FontId::proportional((h * 0.05).max(6.0)), fade(TEXT_DIM, a));
                }
            }
        }
        Kind::Image(seed) => {
            let sd = *seed as f32;
            vgrad(p, r, fade(hsv(0.55 + sd * 0.05, 0.35, 0.85), a), fade(hsv(0.08 + sd * 0.02, 0.45, 0.95), a));
            p.circle_filled(pos2(r.left() + w * 0.75, r.top() + h * 0.32), h * 0.1, fade(Color32::from_rgb(0xff, 0xf0, 0xb0), a));
            let mount = |pts: Vec<Pos2>, c: Color32| {
                p.add(Shape::convex_polygon(pts, fade(c, a), egui::Stroke::NONE));
            };
            mount(vec![pos2(r.left(), r.bottom()), pos2(r.left() + w * 0.3, r.top() + h * 0.45), pos2(r.left() + w * 0.62, r.bottom())], Color32::from_rgb(0x4a, 0x5a, 0x7a));
            mount(vec![pos2(r.left() + w * 0.35, r.bottom()), pos2(r.left() + w * 0.7, r.top() + h * 0.5), pos2(r.right(), r.bottom())], Color32::from_rgb(0x33, 0x40, 0x5c));
            let band = Rect::from_min_max(pos2(r.left(), r.top() + h * 0.08), pos2(r.right(), r.top() + h * 0.22));
            p.rect_filled(band, 0.0, fade(Color32::from_rgba_unmultiplied(0, 0, 0, 90), a));
            p.text(band.center(), Align2::CENTER_CENTER, "We'll be right back", FontId::proportional((h * 0.07).max(5.0)), fade(Color32::WHITE, a));
        }
    }
}

fn scaled(r: Rect, k: f32) -> Rect {
    Rect::from_center_size(r.center(), r.size() * k.max(0.0))
}

fn draw_in(p: &Painter, clip: Rect, r: Rect, inp: Option<&Input>, a: f32, t: f64, all: &[Input]) {
    if let Some(i) = inp {
        let c = clip.intersect(p.clip_rect());
        if c.width() <= 0.0 || c.height() <= 0.0 {
            return;
        }
        draw_source(&p.with_clip_rect(c), r, i, a, t, all);
    }
}

/// Output picture during a transition from `a` to `b` at progress `k` (0..1).
pub fn draw_transition(p: &Painter, r: Rect, a: Option<&Input>, b: Option<&Input>, e: Effect, k: f32, t: f64, all: &[Input]) {
    let k = k.clamp(0.0, 1.0);
    let w = r.width();
    let h = r.height();
    let ease = k * k * (3.0 - 2.0 * k);
    match e {
        Effect::Fade | Effect::Merge => {
            draw_in(p, r, r, a, 1.0, t, all);
            draw_in(p, r, r, b, k, t, all);
        }
        Effect::Cut => draw_in(p, r, r, b, 1.0, t, all),
        Effect::Zoom => {
            draw_in(p, r, r, a, 1.0, t, all);
            draw_in(p, r, scaled(r, ease), b, k.sqrt(), t, all);
        }
        Effect::CrossZoom => {
            draw_in(p, r, scaled(r, 1.0 + ease), a, 1.0 - k, t, all);
            draw_in(p, r, scaled(r, 2.0 - ease), b, k, t, all);
        }
        Effect::Wipe => {
            draw_in(p, r, r, a, 1.0, t, all);
            draw_in(p, Rect::from_min_max(r.min, pos2(r.left() + w * ease, r.bottom())), r, b, 1.0, t, all);
        }
        Effect::WipeReverse => {
            draw_in(p, r, r, a, 1.0, t, all);
            draw_in(p, Rect::from_min_max(pos2(r.right() - w * ease, r.top()), r.max), r, b, 1.0, t, all);
        }
        Effect::VerticalWipe => {
            draw_in(p, r, r, a, 1.0, t, all);
            draw_in(p, Rect::from_min_max(r.min, pos2(r.right(), r.top() + h * ease)), r, b, 1.0, t, all);
        }
        Effect::VerticalWipeReverse => {
            draw_in(p, r, r, a, 1.0, t, all);
            draw_in(p, Rect::from_min_max(pos2(r.left(), r.bottom() - h * ease), r.max), r, b, 1.0, t, all);
        }
        Effect::Slide => {
            draw_in(p, r, r.translate(Vec2::new(-w * ease, 0.0)), a, 1.0, t, all);
            draw_in(p, r, r.translate(Vec2::new(w * (1.0 - ease), 0.0)), b, 1.0, t, all);
        }
        Effect::SlideReverse => {
            draw_in(p, r, r.translate(Vec2::new(w * ease, 0.0)), a, 1.0, t, all);
            draw_in(p, r, r.translate(Vec2::new(-w * (1.0 - ease), 0.0)), b, 1.0, t, all);
        }
        Effect::VerticalSlide => {
            draw_in(p, r, r.translate(Vec2::new(0.0, -h * ease)), a, 1.0, t, all);
            draw_in(p, r, r.translate(Vec2::new(0.0, h * (1.0 - ease))), b, 1.0, t, all);
        }
        Effect::VerticalSlideReverse => {
            draw_in(p, r, r.translate(Vec2::new(0.0, h * ease)), a, 1.0, t, all);
            draw_in(p, r, r.translate(Vec2::new(0.0, -h * (1.0 - ease))), b, 1.0, t, all);
        }
        Effect::Fly | Effect::FlyRotate => {
            draw_in(p, r, r, a, 1.0, t, all);
            let start = Rect::from_center_size(pos2(r.right() - w * 0.1, r.top() + h * 0.1), r.size() * 0.05);
            let cur = Rect::from_min_max(start.min.lerp(r.min, ease), start.max.lerp(r.max, ease));
            draw_in(p, r, cur, b, k.sqrt(), t, all);
        }
        Effect::Cube | Effect::CubeZoom => {
            let z = if e == Effect::CubeZoom { 1.0 - 0.2 * (k * std::f32::consts::PI).sin() } else { 1.0 };
            let rr = scaled(r, z);
            let split = rr.left() + rr.width() * (1.0 - ease);
            let ar = Rect::from_min_max(rr.min, pos2(split, rr.bottom()));
            let br = Rect::from_min_max(pos2(split, rr.top()), rr.max);
            draw_in(p, ar, ar, a, 1.0 - 0.4 * ease, t, all);
            draw_in(p, br, br, b, 0.6 + 0.4 * ease, t, all);
        }
        Effect::BarnDoor => {
            draw_in(p, r, r, b, 1.0, t, all);
            let half = w / 2.0;
            let left = Rect::from_min_max(r.min, pos2(r.left() + half, r.bottom())).translate(Vec2::new(-half * ease, 0.0));
            let right = Rect::from_min_max(pos2(r.left() + half, r.top()), r.max).translate(Vec2::new(half * ease, 0.0));
            draw_in(p, left.intersect(r), r.translate(Vec2::new(-half * ease, 0.0)), a, 1.0, t, all);
            draw_in(p, right.intersect(r), r.translate(Vec2::new(half * ease, 0.0)), a, 1.0, t, all);
        }
        Effect::RollerDoor => {
            draw_in(p, r, r, a, 1.0, t, all);
            draw_in(p, r, r.translate(Vec2::new(0.0, -h * (1.0 - ease))), b, 1.0, t, all);
        }
    }
}

pub fn draw_overlay(p: &Painter, r: Rect, inp: &Input, e: Effect, level: f32, t: f64, all: &[Input]) {
    let k = level.clamp(0.0, 1.0);
    match e {
        Effect::Zoom => draw_in(p, r, scaled(r, k), Some(inp), k, t, all),
        Effect::Wipe => draw_in(p, Rect::from_min_max(r.min, pos2(r.left() + r.width() * k, r.bottom())), r, Some(inp), 1.0, t, all),
        Effect::Slide => draw_in(p, r, r.translate(Vec2::new(-r.width() * (1.0 - k), 0.0)), Some(inp), 1.0, t, all),
        Effect::VerticalSlide => draw_in(p, r, r.translate(Vec2::new(0.0, r.height() * (1.0 - k))), Some(inp), 1.0, t, all),
        _ => draw_in(p, r, r, Some(inp), k, t, all),
    }
}

pub fn meter(p: &Painter, r: Rect, level: f32) {
    p.rect_filled(r, 1.0, Color32::from_rgb(0x10, 0x10, 0x10));
    let segs = 24;
    let gap = 1.0;
    let sh = (r.height() - gap * (segs as f32 - 1.0)) / segs as f32;
    let lit = (level.clamp(0.0, 1.0) * segs as f32).round() as i32;
    for k in 0..segs {
        let y = r.bottom() - (k as f32 + 1.0) * sh - k as f32 * gap;
        let seg = Rect::from_min_size(pos2(r.left() + 2.0, y), Vec2::new(r.width() - 4.0, sh));
        let frac = k as f32 / segs as f32;
        let col = if frac > 0.85 {
            Color32::from_rgb(0xe0, 0x30, 0x30)
        } else if frac > 0.65 {
            Color32::from_rgb(0xe8, 0xc8, 0x30)
        } else {
            Color32::from_rgb(0x30, 0xc8, 0x40)
        };
        p.rect_filled(seg, 0.0, if (k as i32) < lit { col } else { col.gamma_multiply(0.12) });
    }
}
