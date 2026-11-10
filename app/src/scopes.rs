//! Waveform, RGB parade and vectorscope sampling and graticules.

use crate::model::{SCOPE_PARADE, SCOPE_RGB, SCOPE_VECTOR};
use crate::program::{ScopeFrame, ScopeKey};
use eframe::egui::{self, pos2, Color32, Painter, Rect, Stroke, Vec2};
use std::sync::{mpsc, Arc, Mutex};

pub type Mailbox = Arc<Mutex<Option<(ScopeKey, Vec<(u8, egui::ColorImage)>)>>>;

pub fn worker() -> (mpsc::SyncSender<ScopeFrame>, Mailbox) {
    let (tx, rx) = mpsc::sync_channel::<ScopeFrame>(1);
    let output: Mailbox = Default::default();
    let images = Arc::clone(&output);
    std::thread::spawn(move || {
        while let Ok(frame) = rx.recv() {
            let [w, h] = frame.size.map(|d| d as usize);
            let drawn = crate::model::scope_layout(frame.key.1).1.into_iter()
                .map(|(kind, _)| (kind, picture(&frame.pixels, w, h, kind, frame.key.2)))
                .collect();
            *images.lock().unwrap() = Some((frame.key, drawn));
        }
    });
    (tx, output)
}

fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn vector(r: f32, g: f32, b: f32, zoom: f32) -> [f32; 2] {
    let y = luma(r, g, b);
    let center = 128.0 / 255.0;
    let range = 224.0 / 255.0;
    let u = center + (b - y) * range / 1.8556;
    let v = center + (r - y) * range / 1.5748;
    [0.5 + (u - 0.5) * zoom, 0.5 - (v - 0.5) * zoom]
}

fn picture(pixels: &[u8], w: usize, h: usize, kind: u8, zoomed: bool) -> egui::ColorImage {
    let (width, height) = if kind == SCOPE_VECTOR { (384, 384) } else { (576, 324) };
    let mut counts = vec![[0u16; 3]; width * height];
    let channels = if matches!(kind, SCOPE_RGB | SCOPE_PARADE) { 3 } else { 1 };
    let zoom = if zoomed { 2.0 } else { 1.0 };
    let project = |x: usize, px: &[u8], channel: usize| {
        let [r, g, b] = [px[2], px[1], px[0]].map(|v| v as f32 / 255.0);
        let x = x as f32 / (w.saturating_sub(1)).max(1) as f32;
        if kind == SCOPE_VECTOR { return vector(r, g, b, zoom); }
        let y = if channels == 3 { [r, g, b][channel] } else { luma(r, g, b) };
        let x = if kind == SCOPE_PARADE { (channel as f32 + x * 0.95 + 0.025) / 3.0 } else { x };
        [x, 1.0 - y]
    };
    for row in pixels.chunks_exact(w * 4).take(h) {
        for channel in 0..channels {
            let mut previous = None;
            for (x, px) in row.chunks_exact(4).enumerate() {
                let [x, y] = project(x, px, channel);
                let point = [x * (width - 1) as f32, y * (height - 1) as f32];
                let last = previous.replace(point).unwrap_or(point);
                let dx = point[0] - last[0];
                let dy = point[1] - last[1];
                let steps = dx.abs().max(dy.abs()).ceil() as usize;
                for i in 0..=steps {
                    let t = i as f32 / steps.max(1) as f32;
                    let x = (last[0] + dx * t).round() as i32;
                    let y = (last[1] + dy * t).round() as i32;
                    if x >= 0 && y >= 0 && x < width as i32 && y < height as i32 {
                        let n = &mut counts[y as usize * width + x as usize][channel];
                        *n = n.saturating_add(1);
                    }
                }
            }
        }
    }
    let mut image = egui::ColorImage::new([width, height], Color32::TRANSPARENT);
    let gain = |n: u16| (1.0 - (-0.028 * n as f32).exp()) * 255.0;
    for (dst, [r, g, b]) in image.pixels.iter_mut().zip(counts) {
        if r == 0 && g == 0 && b == 0 { continue; }
        if channels == 1 {
            *dst = Color32::from_white_alpha(gain(r) as u8);
        } else {
            let [r, g, b] = [gain(r), gain(g), gain(b)];
            let channels = [r + (g + b) * 0.25, g + (r + b) * 0.25, b + (r + g) * 0.25];
            let [r, g, b] = channels.map(|v| v.min(255.0) as u8);
            *dst = Color32::from_rgba_premultiplied(r, g, b, r.max(g).max(b));
        }
    }
    image
}

pub fn draw(painter: &Painter, bounds: Rect, texture: Option<egui::TextureId>, kind: u8, over: bool) {
    let p = painter.with_clip_rect(bounds);
    p.rect_filled(bounds, 0.0, if over { Color32::from_black_alpha(220) } else { Color32::BLACK });
    let is_vector = kind == SCOPE_VECTOR;
    let size = if is_vector { Vec2::splat(bounds.width().min(bounds.height()) * 0.95) } else { bounds.size() * 0.98 };
    let plot = Rect::from_center_size(bounds.center(), size);
    if is_vector {
        let axis = Stroke::new(1.0_f32, Color32::from_gray(102));
        p.line_segment([plot.center_top(), plot.center_bottom()], axis);
        p.line_segment([plot.left_center(), plot.right_center()], axis);
        for [r, g, b] in [[1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0]]
        {
            let color = Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8);
            for amplitude in [0.75, 1.0] {
                let [u, v] = vector(r * amplitude, g * amplitude, b * amplitude, 1.0);
                let center = plot.min + Vec2::new(u * plot.width(), v * plot.height());
                p.rect_stroke(Rect::from_center_size(center, Vec2::splat((plot.width() / 48.0).clamp(3.0, 8.0))),
                    0.0, Stroke::new(1.0_f32, color), egui::StrokeKind::Inside);
            }
        }
    } else {
        let grid = Stroke::new(1.0_f32, Color32::from_rgb(179, 148, 38));
        for step in 0..=8 {
            let y = plot.top() + step as f32 * plot.height() / 8.0;
            p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], grid);
        }
    }
    if let Some(texture) = texture {
        p.image(texture, plot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectorscope_uses_legal_range_709_targets() {
        let [u, v] = vector(1.0, 0.0, 0.0, 1.0);
        assert!((u - 0.4013).abs() < 0.001);
        assert!((v - 0.0588).abs() < 0.001);
        let [u2, v2] = vector(1.0, 0.0, 0.0, 2.0);
        assert!((u2 - (2.0 * u - 0.5)).abs() < 0.001);
        assert!((v2 - (2.0 * v - 0.5)).abs() < 0.001);
    }
}
