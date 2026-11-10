//! Native reader and compositor for GT Designer packages.
//!
//! vMix opens the composition in mm::l (0x06002acf) and renders to a shared
//! Direct3D surface in mm::Render (0x06002ad1). LiveMix reads the package directly
//! and draws it through its own compositor.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke, Vec2};
use roxmltree::Node;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use zip::ZipArchive;

const XML_LIMIT: usize = 4 * 1024 * 1024;
const IMAGE_LIMIT: usize = 32 * 1024 * 1024;
const TOTAL_IMAGE_LIMIT: usize = 128 * 1024 * 1024;

type Result<T> = std::result::Result<T, String>;
type Xml<'a, 'input> = Node<'a, 'input>;

#[derive(Debug)]
pub struct Document {
    size: Vec2,
    objects: Vec<Object>,
    assets: HashMap<String, Asset>,
    entrance: Storyboard,
    exit: Storyboard,
    pub fields: BTreeMap<String, String>,
    pub unsupported: BTreeSet<String>,
}

struct Asset(Mutex<AssetData>);

enum AssetData {
    Pixels(egui::ColorImage),
    Texture(egui::TextureHandle),
}

impl std::fmt::Debug for Asset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GT bitmap")
    }
}

impl Asset {
    fn texture(&self, ctx: &egui::Context, name: &str) -> egui::TextureId {
        let mut data = self.0.lock().unwrap();
        if let AssetData::Pixels(pixels) = &mut *data {
            let pixels = std::mem::replace(pixels, egui::ColorImage::new([1, 1], Color32::TRANSPARENT));
            *data = AssetData::Texture(ctx.load_texture(name, pixels, egui::TextureOptions::LINEAR));
        }
        match &*data {
            AssetData::Texture(texture) => texture.id(),
            AssetData::Pixels(_) => unreachable!(),
        }
    }
}

#[derive(Debug)]
struct Object {
    name: String,
    location: Vec2,
    size: Vec2,
    opacity: f32,
    visible: bool,
    kind: ObjectKind,
}

#[derive(Debug)]
enum ObjectKind {
    Layer { size: Vec2, objects: Vec<Object> },
    Shape { kind: ShapeKind, fill: Brush, stroke: Color32, thickness: f32 },
    Image(String),
    Text(Text),
}

#[derive(Debug, Clone, Copy)]
enum ShapeKind { Rectangle, Ellipse, Triangle, RightTriangle }

#[derive(Debug)]
enum Brush {
    Solid(Color32),
    Bitmap(String),
    Linear { start: Vec2, end: Vec2, stops: Vec<(f32, Color32)> },
}

#[derive(Debug)]
struct Text {
    value: String,
    family: String,
    size: f32,
    weight: u16,
    italic: bool,
    horizontal: f32,
    vertical: f32,
    wrap: bool,
    shrink: bool,
    color: Color32,
    stroke: Color32,
    thickness: f32,
}

#[derive(Debug, Default)]
struct Storyboard {
    animations: HashMap<String, Vec<Animation>>,
    seconds: f32,
}

#[derive(Clone, Copy)]
pub enum Cue { Rest, In(f32), Out(f32) }

#[derive(Debug)]
struct Animation {
    effect: Motion,
    delay: f32,
    duration: f32,
    direction: Vec2,
    ease: Ease,
}

#[derive(Clone, Copy, Debug)]
enum Motion { Fade, Fly, Reveal, Expand, Zoom, ZoomFade }

#[derive(Clone, Copy, Debug)]
enum Ease { Linear, In, Out, InOut }

fn child<'a, 'input>(node: Xml<'a, 'input>, name: &str) -> Option<Xml<'a, 'input>> {
    node.children().find(|n| n.has_tag_name(name))
}

fn number(value: Option<&str>, fallback: f32) -> f32 {
    value.and_then(|s| s.parse::<f32>().ok()).filter(|n| n.is_finite()).unwrap_or(fallback)
}

fn pair(value: Option<&str>, fallback: Vec2) -> Vec2 {
    let Some(value) = value else { return fallback };
    let mut parts = value.split(',');
    Vec2::new(number(parts.next(), fallback.x), number(parts.next(), fallback.y))
}

fn color(value: Option<&str>, fallback: Color32) -> Color32 {
    let Some(hex) = value.and_then(|s| s.strip_prefix('#')) else { return fallback };
    let Ok(argb) = u32::from_str_radix(hex, 16) else { return fallback };
    let alpha = match hex.len() { 6 => 255, 8 => (argb >> 24) as u8, _ => return fallback };
    Color32::from_rgba_unmultiplied((argb >> 16) as u8, (argb >> 8) as u8, argb as u8, alpha)
}

fn read_entry(archive: &mut ZipArchive<File>, name: &str, limit: usize) -> Result<Vec<u8>> {
    let entry = archive.by_name(name).map_err(|e| format!("{name}: {e}"))?;
    if entry.size() > limit as u64 {
        return Err(format!("GT package entry is too large: {name}"));
    }
    let mut data = Vec::new();
    entry.take(limit as u64 + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.len() > limit {
        return Err(format!("GT package entry is too large: {name}"));
    }
    Ok(data)
}

// GT Designer writes UTF-16 XML in its ZIP container. Parse its text as UTF-8 in memory.
fn xml_text(bytes: Vec<u8>) -> Result<String> {
    let (data, big_endian) = match bytes.as_slice() {
        [0xff, 0xfe, tail @ ..] => (tail, false),
        [0xfe, 0xff, tail @ ..] => (tail, true),
        [b'<', 0, ..] => (bytes.as_slice(), false),
        [0, b'<', ..] => (bytes.as_slice(), true),
        [0xef, 0xbb, 0xbf, tail @ ..] => return String::from_utf8(tail.to_vec()).map_err(|e| e.to_string()),
        _ => return String::from_utf8(bytes).map_err(|e| e.to_string()),
    };
    if data.len() % 2 != 0 { return Err("Invalid UTF-16 GT document".into()); }
    let words: Vec<_> = data.chunks_exact(2).map(|b| {
        if big_endian { u16::from_be_bytes([b[0], b[1]]) } else { u16::from_le_bytes([b[0], b[1]]) }
    }).collect();
    String::from_utf16(&words).map_err(|e| e.to_string())
}

fn composition_size(root: Xml<'_, '_>, fallback: Vec2) -> Vec2 {
    Vec2::new(number(root.attribute("Width"), fallback.x), number(root.attribute("Height"), fallback.y))
}

impl Document {
    pub fn load(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| format!("Could not open GT title: {e}"))?;
        let mut archive = ZipArchive::new(file).map_err(|e| format!("Invalid GT package: {e}"))?;
        let document = xml_text(read_entry(&mut archive, "document.xml", XML_LIMIT)?)?;
        let xml = roxmltree::Document::parse(&document).map_err(|e| format!("Invalid GT document: {e}"))?;
        let root = xml.root_element();
        if !root.has_tag_name("Composition") {
            return Err("The GT document has no root Composition".into());
        }
        let size = composition_size(root, Vec2::new(1920.0, 1080.0));
        if size.x <= 0.0 || size.y <= 0.0 || size.x > 16384.0 || size.y > 16384.0 {
            return Err("Invalid GT composition dimensions".into());
        }
        let mut result = Self {
            size, objects: Vec::new(), assets: HashMap::new(),
            entrance: Storyboard::default(), exit: Storyboard::default(),
            fields: BTreeMap::new(), unsupported: BTreeSet::new(),
        };
        result.objects = result.parse_objects(root, size, 0)?;
        if result.objects.is_empty() {
            return Err("The GT document has no supported objects".into());
        }
        for storyboard in root.children().filter(|n| n.has_tag_name("Storyboard")) {
            let target = match storyboard.attribute("Type").unwrap_or("TransitionIn") {
                "TransitionIn" => &mut result.entrance,
                "TransitionOut" => &mut result.exit,
                name => {
                    result.unsupported.insert(format!("Storyboard {name}"));
                    continue;
                }
            };
            parse_storyboard(storyboard, target, &mut result.unsupported);
        }
        let resources = if archive.index_for_name("resources.xml").is_some() {
            xml_text(read_entry(&mut archive, "resources.xml", XML_LIMIT)?)?
        } else {
            "<resources />".into()
        };
        let resources = roxmltree::Document::parse(&resources).map_err(|e| format!("Invalid GT resources: {e}"))?;
        let resource_names: HashMap<_, _> = resources.descendants().filter(|n| n.has_tag_name("resource"))
            .filter_map(|n| Some((n.attribute("filename")?, child(n, "source")?.attribute("guid")?))).collect();
        let mut bytes_used = 0;
        for bitmap in root.descendants().filter(|n| n.has_tag_name("Bitmap")) {
            let Some(source) = bitmap.attribute("Source") else { continue };
            if result.assets.contains_key(source) { continue; }
            let entry = resource_names.get(source).copied().unwrap_or(source);
            if archive.index_for_name(entry).is_none() {
                result.unsupported.insert("Missing or external bitmap".into());
                continue;
            }
            let bytes = read_entry(&mut archive, entry, IMAGE_LIMIT)?;
            let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some((TOTAL_IMAGE_LIMIT - bytes_used) as u64);
            reader.limits(limits);
            let rgba = match reader.decode() {
                Ok(image) => image.into_rgba8(),
                Err(_) => {
                    result.unsupported.insert("Unsupported or oversized bitmap".into());
                    continue;
                }
            };
            bytes_used += rgba.as_raw().len();
            if bytes_used > TOTAL_IMAGE_LIMIT {
                return Err("GT title exceeds the bitmap memory limit".into());
            }
            let pixels = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
            result.assets.insert(source.into(), Asset(Mutex::new(AssetData::Pixels(pixels))));
        }
        Ok(result)
    }

    fn parse_objects(&mut self, root: Xml<'_, '_>, size: Vec2, depth: usize) -> Result<Vec<Object>> {
        if depth > 24 { return Err("GT composition nesting is too deep".into()); }
        let mut objects = Vec::new();
        for node in root.children().filter(Node::is_element) {
            let tag = node.tag_name().name();
            let name = node.attribute("Name").unwrap_or_default();
            let dimensions = pair(node.attribute("Dimensions"), size);
            let property = |prop: &str| child(node, &format!("{tag}.{prop}"));
            let brush = |prop| property(prop).and_then(|n| child(n, "Brush"));
            let fill = color(brush("Fill").and_then(|b| b.attribute("Color")), Color32::WHITE);
            let stroke = color(brush("Stroke").and_then(|b| b.attribute("Color")), Color32::TRANSPARENT);
            let thickness = number(node.attribute("StrokeThickness"), 0.0).max(0.0);
            for prop in ["Effects", "Mask", "Transform", "Crop", "Geometry"] {
                if property(prop).is_some() { self.unsupported.insert(prop.into()); }
            }
            let kind = match tag {
                "Layer" => {
                    let Some(comp) = property("Composition").and_then(|n| child(n, "Composition")) else { continue };
                    let size = composition_size(comp, dimensions);
                    ObjectKind::Layer { size, objects: self.parse_objects(comp, size, depth + 1)? }
                }
                "Rectangle" | "Ellipse" | "Triangle" | "RightTriangle" => {
                    let kind = match tag {
                        "Ellipse" => ShapeKind::Ellipse,
                        "Triangle" => ShapeKind::Triangle,
                        "RightTriangle" => ShapeKind::RightTriangle,
                        _ => ShapeKind::Rectangle,
                    };
                    ObjectKind::Shape { kind, fill: parse_brush(brush("Fill"), fill), stroke, thickness }
                }
                "Image" => {
                    let Some(source) = property("Bitmap").and_then(|n| child(n, "Bitmap")).and_then(|n| n.attribute("Source")) else { continue };
                    ObjectKind::Image(source.into())
                }
                "TextBlock" => {
                    let value = node.attribute("Text").unwrap_or_default().to_owned();
                    if !name.is_empty() && !node.attribute("DataFlags").is_some_and(|s| s.contains("Hidden")) {
                        self.fields.insert(name.into(), value.clone());
                    }
                    ObjectKind::Text(Text {
                        value, family: node.attribute("FontFamily").unwrap_or("Arial").to_owned(),
                        size: number(node.attribute("FontSize"), 48.0).clamp(1.0, 1024.0),
                        weight: match node.attribute("FontWeight").unwrap_or_default() {
                            "Black" => 900, "Heavy" | "ExtraBold" => 800, "Bold" => 700,
                            "SemiBold" | "DemiBold" => 600, "Medium" => 500, "Light" => 300, _ => 400,
                        },
                        italic: matches!(node.attribute("FontStyle"), Some("Italic" | "Oblique")),
                        horizontal: match node.attribute("TextAlign") { Some("Center") => 0.5, Some("Right") => 1.0, _ => 0.0 },
                        vertical: match node.attribute("VerticalAlign") { Some("Center") => 0.5, Some("Bottom") => 1.0, _ => 0.0 },
                        wrap: node.attribute("TextWordWrapping") != Some("NoWrap"),
                        shrink: node.attribute("AutoSize").is_some_and(|v| v.contains("Shrink")),
                        color: fill, stroke, thickness,
                    })
                }
                "Storyboard" => continue,
                name if name.contains('.') => continue,
                name => {
                    self.unsupported.insert(name.into());
                    continue;
                }
            };
            objects.push(Object {
                name: name.into(), location: pair(node.attribute("Location"), Vec2::ZERO),
                size: dimensions, opacity: number(node.attribute("Opacity"), 1.0).clamp(0.0, 1.0),
                visible: node.attribute("Visible") != Some("False"),
                kind,
            });
        }
        Ok(objects)
    }

    pub fn seconds(&self, exiting: bool) -> Option<f32> {
        let duration = if exiting { self.exit.seconds } else { self.entrance.seconds };
        (duration > 0.0).then_some(duration)
    }

    pub fn draw(&self, p: &Painter, rect: Rect, opacity: f32, texts: &BTreeMap<String, String>, cue: Cue) {
        let scale = (rect.width() / self.size.x).min(rect.height() / self.size.y);
        if scale <= 0.0 { return; }
        let bounds = Rect::from_center_size(rect.center(), self.size * scale);
        let view = View { bounds, opacity };
        let (p, view) = self.animate(p.with_clip_rect(rect), view, "", cue);
        self.draw_objects(&p, &self.objects, view, self.size, texts, cue);
    }

    fn draw_objects(&self, p: &Painter, objects: &[Object], parent: View, size: Vec2, texts: &BTreeMap<String, String>, cue: Cue) {
        if parent.opacity <= 0.0 || size.x <= 0.0 || size.y <= 0.0 { return; }
        let scale = parent.bounds.size() / size;
        for obj in objects {
            if !obj.visible || obj.opacity <= 0.0 || obj.size.x <= 0.0 || obj.size.y <= 0.0 { continue; }
            let bounds = Rect::from_min_size(parent.bounds.min + obj.location * scale, obj.size * scale);
            let view = View { bounds, opacity: parent.opacity * obj.opacity };
            let (p, view) = self.animate(p.clone(), view, &obj.name, cue);
            if view.opacity <= 0.0 || !p.clip_rect().intersects(view.bounds) { continue; }
            match &obj.kind {
                ObjectKind::Layer { size, objects } => {
                    self.draw_objects(&p.with_clip_rect(view.bounds), objects, view, *size, texts, cue);
                }
                ObjectKind::Image(source) => {
                    if let Some(asset) = self.assets.get(source) {
                        p.image(asset.texture(p.ctx(), source), view.bounds, unit_rect(), Color32::WHITE.gamma_multiply(view.opacity));
                    }
                }
                ObjectKind::Shape { kind, fill, stroke, thickness } => {
                    self.draw_shape(&p, view, *kind, fill, Stroke::new(thickness * scale.y, stroke.gamma_multiply(view.opacity)));
                }
                ObjectKind::Text(text) => {
                    let value = texts.get(&obj.name).unwrap_or(&text.value);
                    draw_text(&p, view, text, value, view.bounds.height() / obj.size.y);
                }
            }
        }
    }

    fn animate(&self, mut p: Painter, mut view: View, name: &str, cue: Cue) -> (Painter, View) {
        let (storyboard, time, exiting) = match cue {
            Cue::Rest => return (p, view),
            Cue::In(t) => (&self.entrance, t * self.entrance.seconds, false),
            Cue::Out(t) => (&self.exit, t * self.exit.seconds, true),
        };
        if let Some(animations) = storyboard.animations.get(name) {
            for animation in animations {
                let progress = ((time - animation.delay) / animation.duration).clamp(0.0, 1.0);
                let progress = animation.ease.apply(progress);
                let shown = if exiting { 1.0 - progress } else { progress };
                match animation.effect {
                    Motion::Fade => view.opacity *= shown,
                    Motion::Fly => view.bounds = view.bounds.translate(animation.direction * view.bounds.size() * (1.0 - shown)),
                    Motion::Reveal => p = p.with_clip_rect(reveal(view.bounds, animation.direction, shown)),
                    Motion::Expand | Motion::Zoom | Motion::ZoomFade => {
                        let scale = if matches!(animation.effect, Motion::Expand) && animation.direction.x != 0.0 {
                            Vec2::new(shown, 1.0)
                        } else if matches!(animation.effect, Motion::Expand) {
                            Vec2::new(1.0, shown)
                        } else {
                            Vec2::splat(shown)
                        };
                        view.bounds = Rect::from_center_size(view.bounds.center(), view.bounds.size() * scale);
                        if matches!(animation.effect, Motion::ZoomFade) { view.opacity *= shown; }
                    }
                }
            }
        }
        (p, view)
    }

    fn draw_shape(&self, p: &Painter, view: View, kind: ShapeKind, fill: &Brush, stroke: Stroke) {
        let r = view.bounds;
        let polygon: Vec<Pos2> = match kind {
            ShapeKind::Rectangle => vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
            ShapeKind::Ellipse => (0..48).map(|i| {
                let a = i as f32 * std::f32::consts::TAU / 48.0;
                r.center() + Vec2::new(a.cos(), a.sin()) * r.size() * 0.5
            }).collect(),
            ShapeKind::Triangle => vec![r.center_top(), r.right_bottom(), r.left_bottom()],
            ShapeKind::RightTriangle => vec![r.left_top(), r.right_bottom(), r.left_bottom()],
        };
        match fill {
            Brush::Solid(color) => {
                p.add(egui::Shape::convex_polygon(polygon, color.gamma_multiply(view.opacity), stroke));
                return;
            }
            Brush::Bitmap(source) => {
                if let Some(asset) = self.assets.get(source) {
                    let mut mesh = egui::Mesh::with_texture(asset.texture(p.ctx(), source));
                    append_polygon(&mut mesh, &polygon, |pos| {
                        let uv = (pos - r.min) / r.size();
                        (Pos2::new(uv.x, uv.y), Color32::WHITE.gamma_multiply(view.opacity))
                    });
                    p.add(egui::Shape::mesh(mesh));
                }
            }
            Brush::Linear { start, end, stops } => {
                let axis = (*end - *start) * r.size();
                let origin = r.min + *start * r.size();
                let length = axis.length_sq();
                if length <= f32::EPSILON { return; }
                let project = |point: Pos2| (point - origin).dot(axis) / length;
                let mut mesh = egui::Mesh::default();
                for i in 0..=stops.len() {
                    let mut points = polygon.clone();
                    if i > 0 { points = clip_polygon(&points, |pt| project(pt) - stops[i - 1].0); }
                    if i < stops.len() { points = clip_polygon(&points, |pt| stops[i].0 - project(pt)); }
                    append_polygon(&mut mesh, &points, |pos| {
                        let value = gradient_color(stops, project(pos)).gamma_multiply(view.opacity);
                        (egui::epaint::WHITE_UV, value)
                    });
                }
                p.add(egui::Shape::mesh(mesh));
            }
        }
        if !stroke.is_empty() {
            p.add(egui::Shape::closed_line(polygon, stroke));
        }
    }
}

#[derive(Clone, Copy)]
struct View { bounds: Rect, opacity: f32 }

fn unit_rect() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0))
}

fn parse_brush(node: Option<Xml<'_, '_>>, fallback: Color32) -> Brush {
    let Some(node) = node else { return Brush::Solid(fallback) };
    if let Some(source) = child(node, "Brush.Bitmap").and_then(|n| child(n, "Bitmap")).and_then(|n| n.attribute("Source")) {
        return Brush::Bitmap(source.into());
    }
    if let Some(stops) = child(node, "Brush.Stops") {
        let mut stops: Vec<_> = stops.children().filter(|n| n.has_tag_name("GradientStop")).map(|n| {
            (number(n.attribute("Position"), 0.0).clamp(0.0, 1.0), color(n.attribute("Color"), fallback))
        }).collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        if !stops.is_empty() {
            return Brush::Linear {
                start: pair(node.attribute("StartPoint"), Vec2::ZERO),
                end: pair(node.attribute("EndPoint"), Vec2::Y),
                stops,
            };
        }
    }
    Brush::Solid(fallback)
}

fn gradient_color(stops: &[(f32, Color32)], t: f32) -> Color32 {
    for pair in stops.windows(2) {
        let [(a, ca), (b, cb)] = [pair[0], pair[1]];
        if t <= a { return ca; }
        if t < b {
            let x = (t - a) / (b - a);
            let a = ca.to_array();
            let b = cb.to_array();
            let [r, g, b, a] = std::array::from_fn(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * x).round() as u8);
            return Color32::from_rgba_premultiplied(r, g, b, a);
        }
    }
    stops.last().unwrap().1
}

fn append_polygon(mesh: &mut egui::Mesh, polygon: &[Pos2], vertex: impl Fn(Pos2) -> (Pos2, Color32)) {
    if polygon.len() < 3 { return; }
    let offset = mesh.vertices.len() as u32;
    mesh.vertices.extend(polygon.iter().map(|&pos| {
        let (uv, color) = vertex(pos);
        egui::epaint::Vertex { pos, uv, color }
    }));
    for i in 1..polygon.len() as u32 - 1 {
        mesh.add_triangle(offset, offset + i, offset + i + 1);
    }
}

fn clip_polygon(points: &[Pos2], distance: impl Fn(Pos2) -> f32) -> Vec<Pos2> {
    let Some(&last) = points.last() else { return Vec::new() };
    let mut previous = last;
    let mut before = distance(previous);
    let mut clipped = Vec::with_capacity(points.len() + 2);
    for &point in points {
        let after = distance(point);
        if (before >= 0.0) != (after >= 0.0) {
            clipped.push(previous.lerp(point, before / (before - after)));
        }
        if after >= 0.0 { clipped.push(point); }
        previous = point;
        before = after;
    }
    clipped
}

impl Ease {
    fn apply(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::In => t.powi(3),
            Self::Out => 1.0 - (1.0 - t).powi(3),
            Self::InOut if t < 0.5 => 4.0 * t.powi(3),
            Self::InOut => 1.0 - (-2.0 * t + 2.0).powi(3) * 0.5,
        }
    }
}

fn parse_storyboard(xml: Xml<'_, '_>, board: &mut Storyboard, unsupported: &mut BTreeSet<String>) {
    let Some(list) = child(xml, "Storyboard.Animations") else { return };
    for animation in list.children().filter(Node::is_element) {
        let effect = match animation.tag_name().name() {
            "Fade" => Motion::Fade, "Fly" => Motion::Fly, "Reveal" => Motion::Reveal,
            "Expand" => Motion::Expand, "Zoom" => Motion::Zoom, "ZoomFade" => Motion::ZoomFade,
            "None" => continue,
            name => {
                unsupported.insert(format!("Animation {name}"));
                continue;
            }
        };
        let delay = number(animation.attribute("Delay"), 0.0).clamp(0.0, 60.0);
        let duration = number(animation.attribute("Duration"), 1.0).clamp(0.001, 60.0);
        let direction = match animation.attribute("Direction") {
            Some("Right") => Vec2::X, Some("Top") => -Vec2::Y, Some("Bottom") => Vec2::Y, _ => -Vec2::X,
        };
        let ease = match animation.attribute("Interpolation") {
            Some("CubicEasingIn") => Ease::In, Some("CubicEasingOut") => Ease::Out,
            Some("CubicEasingInOut") => Ease::InOut, _ => Ease::Linear,
        };
        board.seconds = board.seconds.max(delay + duration);
        board.animations.entry(animation.attribute("Object").unwrap_or_default().into()).or_default()
            .push(Animation { effect, delay, duration, direction, ease });
    }
}

fn reveal(bounds: Rect, direction: Vec2, shown: f32) -> Rect {
    let mut rect = bounds;
    if direction.x > 0.0 { rect.max.x = rect.min.x + rect.width() * shown; }
    else if direction.x < 0.0 { rect.min.x = rect.max.x - rect.width() * shown; }
    else if direction.y > 0.0 { rect.max.y = rect.min.y + rect.height() * shown; }
    else { rect.min.y = rect.max.y - rect.height() * shown; }
    rect
}

fn title_font(p: &Painter, text: &Text) -> egui::FontFamily {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    static DATABASE: OnceLock<fontdb::Database> = OnceLock::new();
    let name = format!("gt:{}:{}:{}", text.family, text.weight, text.italic);
    let family = egui::FontFamily::Name(name.clone().into());
    if p.fonts(|fonts| fonts.families().contains(&family)) {
        return family;
    }
    let key = egui::Id::new(&name);
    let first = p.ctx().data_mut(|data| {
        if data.get_temp::<bool>(key).is_some() { return false; }
        data.insert_temp(key, true);
        true
    });
    if first {
        let db = DATABASE.get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            db
        });
        let families = [fontdb::Family::Name(&text.family), fontdb::Family::Name("Arial")];
        if let Some(face) = db.query(&fontdb::Query {
            families: &families, weight: fontdb::Weight(text.weight),
            style: if text.italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
            ..Default::default()
        }) {
            if let Some(data) = db.with_face_data(face, |bytes, index| {
                let mut data = egui::FontData::from_owned(bytes.to_vec());
                data.index = index;
                data
            }) {
                p.ctx().add_font(FontInsert::new(&name, data,
                    vec![InsertFontFamily { family, priority: FontPriority::Highest }]));
            }
        }
    }
    egui::FontFamily::Proportional
}

fn draw_text(p: &Painter, view: View, text: &Text, value: &str, scale: f32) {
    if value.is_empty() || scale <= 0.0 { return; }
    let rect = view.bounds;
    let p = p.with_clip_rect(rect);
    let family = title_font(&p, text);
    let wrap = if text.wrap { rect.width() } else { f32::INFINITY };
    let layout = |size| p.layout(value.to_owned(), egui::FontId::new(size, family.clone()), Color32::PLACEHOLDER, wrap);
    let mut size = (text.size * scale).max(1.0);
    let mut galley = layout(size);
    if text.shrink {
        for _ in 0..4 {
            let ratio = (rect.width() / galley.size().x).min(rect.height() / galley.size().y);
            if ratio >= 1.0 || size <= 1.0 { break; }
            size = (size * ratio).max(1.0);
            galley = layout(size);
        }
    }
    let pos = rect.min + (rect.size() - galley.size()) * Vec2::new(text.horizontal, text.vertical);
    if text.thickness > 0.0 && text.stroke != Color32::TRANSPARENT {
        let stroke = text.stroke.gamma_multiply(view.opacity);
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::TAU / 8.0;
            p.galley(pos + Vec2::new(a.cos(), a.sin()) * text.thickness * scale, galley.clone(), stroke);
        }
    }
    p.galley(pos, galley, text.color.gamma_multiply(view.opacity));
}
