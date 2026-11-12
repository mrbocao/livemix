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
    let [width, height] = dimensions(kind);
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
                let last = if kind == SCOPE_VECTOR { point } else { previous.replace(point).unwrap_or(point) };
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

pub const fn dimensions(kind: u8) -> [usize; 2] {
    // hc/a::a(bool), RVA 0x877fc in the reference build.
    if kind == SCOPE_VECTOR { [512, 512] } else { [1200, 256] }
}

pub(crate) struct Gpu {
    framebuffer: eframe::glow::Framebuffer,
    counts: eframe::glow::Texture,
    trace: eframe::glow::Program,
    tone: eframe::glow::Program,
}

use eframe::glow::{self, HasContext};

const TRACE_VERTEX: &str = r#"
#version 330 core
uniform sampler2D picture;
uniform ivec2 source_size;
uniform vec2 scope_size;
uniform int mode;
uniform float zoom;
out vec3 trace;
void main() {
    bool vector = mode == 15;
    bool rgb = mode == 5 || mode == 10;
    int columns = vector ? source_size.x : (source_size.x - 1) * 2;
    int x = gl_VertexID % columns;
    if (!vector) x = x / 2 + x % 2;
    int y = gl_VertexID / columns;
    int channel = y / source_size.y;
    vec3 sample_rgb = texelFetch(picture, ivec2(x, y % source_size.y), 0).rgb;
    float luma = dot(sample_rgb, vec3(0.2126, 0.7152, 0.0722));
    vec2 p;
    if (vector) {
        vec2 chroma = vec2(128.0 / 255.0) + (sample_rgb.br - luma) * (224.0 / 255.0) / vec2(1.8556, 1.5748);
        p = 0.5 + (chroma - 0.5) * vec2(zoom, -zoom);
    } else {
        float horizontal = float(x) / float(source_size.x - 1);
        if (mode == 10) horizontal = (float(channel) + horizontal * 0.95 + 0.025) / 3.0;
        p = vec2(horizontal, 1.0 - (rgb ? sample_rgb[channel] : luma));
    }
    gl_Position = vec4(2.0 * (p * (scope_size - 1.0) + 0.5) / scope_size - 1.0, 0.0, 1.0);
    trace = rgb ? vec3(channel == 0 ? 1.0 : 0.0, channel == 1 ? 1.0 : 0.0, channel == 2 ? 1.0 : 0.0) : vec3(1.0, 0.0, 0.0);
}
"#;

const TRACE_FRAGMENT: &str = r#"
#version 330 core
in vec3 trace;
out vec4 count;
void main() { count = vec4(trace, 0.0); }
"#;

const TONE_FRAGMENT: &str = r#"
#version 330 core
uniform sampler2D picture;
uniform int colored;
out vec4 result;
void main() {
    vec3 counts = texelFetch(picture, ivec2(gl_FragCoord.xy), 0).rgb;
    vec3 value = 1.0 - exp(-0.028 * counts);
    if (colored == 0) {
        float alpha = value.r;
        float gray = alpha <= 0.0031308 ? 12.92 * alpha : 1.055 * pow(alpha, 1.0 / 2.4) - 0.055;
        result = vec4(vec3(gray), alpha);
    } else {
        vec3 rgb = min(vec3(1.0), value + (value.gbr + value.brg) * 0.25);
        result = vec4(rgb, max(rgb.r, max(rgb.g, rgb.b)));
    }
}
"#;

impl Gpu {
    pub(crate) unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        let framebuffer = gl.create_framebuffer()?;
        let counts = match gl.create_texture() {
            Ok(counts) => counts,
            Err(error) => { gl.delete_framebuffer(framebuffer); return Err(error); }
        };
        let trace = crate::program::link(gl, TRACE_VERTEX, TRACE_FRAGMENT);
        let result = trace.and_then(|trace| {
            match crate::program::link(gl, crate::program::PACK_VERTEX, TONE_FRAGMENT) {
                Ok(tone) => Ok(Self { framebuffer, counts, trace, tone }),
                Err(error) => { gl.delete_program(trace); Err(error) }
            }
        });
        if result.is_err() {
            gl.delete_texture(counts);
            gl.delete_framebuffer(framebuffer);
            return result;
        }
        let texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
        gl.bind_texture(glow::TEXTURE_2D, Some(counts));
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA32F as i32, 1200, 512, 0,
            glow::RGBA, glow::FLOAT, glow::PixelUnpackData::Slice(None));
        gl.bind_texture(glow::TEXTURE_2D, texture);
        result
    }

    pub(crate) unsafe fn draw(&self, painter: &eframe::egui_glow::Painter, source: glow::Texture,
        [w, h]: [u32; 2], zoom: bool, scopes: &[(u8, egui::TextureHandle)]) -> Result<(), String>
    {
        let gl = painter.gl();
        let state = [glow::SCISSOR_TEST, glow::BLEND, glow::FRAMEBUFFER_SRGB].map(|flag| (flag, gl.is_enabled(flag)));
        let blend = [glow::BLEND_SRC_RGB, glow::BLEND_DST_RGB, glow::BLEND_SRC_ALPHA, glow::BLEND_DST_ALPHA,
            glow::BLEND_EQUATION_RGB, glow::BLEND_EQUATION_ALPHA].map(|name| gl.get_parameter_i32(name) as u32);
        let previous_fbo = gl.get_parameter_framebuffer(glow::DRAW_FRAMEBUFFER_BINDING);
        let previous_texture = gl.get_parameter_texture(glow::TEXTURE_BINDING_2D);
        let previous_program = gl.get_parameter_program(glow::CURRENT_PROGRAM);
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        gl.disable(glow::SCISSOR_TEST);
        gl.disable(glow::FRAMEBUFFER_SRGB);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(self.framebuffer));
        let result = (|| {
            for (kind, handle) in scopes {
                let [width, height] = dimensions(*kind).map(|value| value as i32);
                let output = painter.texture(handle.id()).ok_or("Scope texture unavailable")?;
                gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(self.counts), 0);
                if gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                    return Err("Scope accumulation framebuffer unavailable".into());
                }
                gl.viewport(0, 0, width, height);
                gl.clear_buffer_f32_slice(glow::COLOR, 0, &[0.0; 4]);
                gl.enable(glow::BLEND);
                gl.blend_equation(glow::FUNC_ADD);
                gl.blend_func(glow::ONE, glow::ONE);
                gl.use_program(Some(self.trace));
                gl.bind_texture(glow::TEXTURE_2D, Some(source));
                let uniform = |name| gl.get_uniform_location(self.trace, name);
                gl.uniform_1_i32(uniform("picture").as_ref(), 0);
                gl.uniform_2_i32(uniform("source_size").as_ref(), w as i32, h as i32);
                gl.uniform_2_f32(uniform("scope_size").as_ref(), width as f32, height as f32);
                gl.uniform_1_i32(uniform("mode").as_ref(), i32::from(*kind));
                gl.uniform_1_f32(uniform("zoom").as_ref(), if zoom { 2.0 } else { 1.0 });
                let rgb = matches!(*kind, SCOPE_RGB | SCOPE_PARADE);
                let (primitive, vertices) = if *kind == SCOPE_VECTOR {
                    (glow::POINTS, w * h)
                } else {
                    (glow::LINES, 2 * (w - 1) * h * if rgb { 3 } else { 1 })
                };
                gl.draw_arrays(primitive, 0, vertices as i32);
                gl.disable(glow::BLEND);
                gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(output), 0);
                if gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                    return Err("Scope display framebuffer unavailable".into());
                }
                gl.use_program(Some(self.tone));
                gl.bind_texture(glow::TEXTURE_2D, Some(self.counts));
                gl.uniform_1_i32(gl.get_uniform_location(self.tone, "picture").as_ref(), 0);
                gl.uniform_1_i32(gl.get_uniform_location(self.tone, "colored").as_ref(), i32::from(rgb));
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
                if gl.get_error() != glow::NO_ERROR { return Err("OpenGL scope rendering failed".into()); }
            }
            Ok(())
        })();
        gl.framebuffer_texture_2d(glow::DRAW_FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, None, 0);
        gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, previous_fbo);
        gl.bind_texture(glow::TEXTURE_2D, previous_texture);
        gl.use_program(previous_program);
        gl.viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
        gl.blend_func_separate(blend[0], blend[1], blend[2], blend[3]);
        gl.blend_equation_separate(blend[4], blend[5]);
        for (flag, enabled) in state {
            if enabled { gl.enable(flag); } else { gl.disable(flag); }
        }
        result
    }

    pub(crate) unsafe fn destroy(self, gl: &glow::Context) {
        gl.delete_framebuffer(self.framebuffer);
        gl.delete_texture(self.counts);
        gl.delete_program(self.trace);
        gl.delete_program(self.tone);
    }
}
