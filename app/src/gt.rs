//! Native reader and compositor for GT Designer packages.
//!
//! The reader draws the package through the LiveMix compositor.

use eframe::egui::{self, Color32, Painter, Pos2, Rect, Stroke, Vec2};
use roxmltree::Node;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
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
    data_in: Storyboard,
    data_out: Storyboard,
    field_boards: HashMap<String, (Storyboard, Storyboard)>,
    rotations: HashMap<String, Spin>,
    /// Continuous brush scrolling by object name: [fill, stroke].
    drifts: HashMap<String, [Option<Drift>; 2]>,
    pub fields: BTreeMap<String, String>,
    pub unsupported: BTreeSet<String>,
    /// Attributes and properties of known objects that the reader does not use. They are not
    /// shown in the editor; `--gt-report` lists them.
    pub ignored: BTreeSet<String>,
    /// Features drawn only approximately (a mask is applied as its object's bounding box).
    pub approximated: BTreeSet<String>,
    /// The area of every named object, in template units, for masks.
    bounds: HashMap<String, Rect>,
    /// Page storyboards (`Page1`, `Page2`…), played on request.
    pages: BTreeMap<usize, Storyboard>,
    /// How many pictures an image has when its resource lists several, by source name.
    frames: HashMap<String, usize>,
    /// Images that cycle through their pictures (ImageSequenceLoop).
    sequences: HashSet<String>,
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
            // GT brushes mirror past their edges by default (GraphicsBrush WrapX/WrapY = Mirror).
            *data = AssetData::Texture(ctx.load_texture(name, pixels, egui::TextureOptions::LINEAR_MIRRORED_REPEAT));
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
    crop: Option<[f32; 4]>,
    skew: Vec2,
    shadow: Option<Shadow>,
    /// Static rotation about the object's centre, radians clockwise (Transform Rotate Z).
    rotation: f32,
    /// Mirrored horizontally and vertically by the FlipX and FlipY effects.
    mirror: [bool; 2],
    /// Rotation about X and Y, radians. It squashes the object by the cosine of each angle, and a
    /// negative cosine mirrors it. Half turns are exact; other angles lack the original's perspective.
    tilt: [f32; 2],
    /// The object whose area this one is limited to.
    mask: Option<String>,
    kind: ObjectKind,
}

#[derive(Debug)]
enum ObjectKind {
    Layer { size: Vec2, objects: Vec<Object> },
    /// `cube_depth` draws the shape as the front face of a box that long, which may spin.
    Shape { kind: ShapeKind, fill: Brush, stroke: Brush, thickness: f32, cube_depth: Option<f32> },
    /// `flip` mirrors the picture horizontally and vertically (the FlipX and FlipY effects).
    Image { source: String, cube_depth: Option<f32>, flip: [bool; 2] },
    Text(Text),
    Ticker(Ticker),
    /// A QR code of the text of the field named like the object.
    QrCode,
}

/// A scrolling list of items inside the object's area, in a loop. The items are text of the
/// field named like the object, one per line. A ticker with a template draws each item with that
/// template, and the parts of a line separated by `|` fill the template's text fields in order.
#[derive(Debug)]
struct Ticker {
    /// Template units per frame at 60 frames per second.
    speed: f32,
    /// Where the items travel, as a unit step: `(-1, 0)` crawls to the left.
    travel: Vec2,
    style: Text,
    template: Option<TickerTemplate>,
}

#[derive(Debug)]
struct TickerTemplate {
    objects: Vec<Object>,
    size: Vec2,
    /// The template's text fields in drawing order, with their default text.
    fields: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy)]
struct Shadow {
    blur: f32,
    offset: Vec2,
    color: Color32,
}

#[derive(Clone, Debug)]
pub struct DataChange {
    pub started: f64,
    pub field: String,
    /// The text the field showed before it changed. It stays visible while the field animates out.
    pub previous: String,
}

/// What a frame needs besides the template: the text of every field, the entrance or exit cue,
/// the data-change cues of the fields that just changed, and rows moved away from their place.
struct Scene<'a> {
    texts: &'a BTreeMap<String, String>,
    cue: Cue<'a>,
    /// Each running data-change cue with the text its field showed before.
    data: &'a [(Cue<'a>, &'a str)],
    /// Vertical offsets, in template units, by row name (`Row1`). Their children move with them.
    /// A NaN offset hides the row.
    offsets: &'a HashMap<String, f32>,
    /// Where the template's area starts on screen and the screen size of a template unit.
    /// Masks need it; items drawn by a ticker have none.
    root: Option<(Pos2, Vec2)>,
}

#[derive(Debug, Clone, Copy)]
struct Drift {
    /// Brush offset change per second, in units of the object size.
    velocity: Vec2,
    delay: f32,
}

impl Drift {
    fn at(drift: Option<Drift>, t: f64) -> Vec2 {
        // Mirrored brushes repeat every 2 units, so the offset is kept within one period.
        drift.map_or(Vec2::ZERO, |d| {
            let elapsed = (t - f64::from(d.delay)).max(0.0);
            let wrap = |v: f32| (f64::from(v) * elapsed).rem_euclid(2.0) as f32;
            Vec2::new(wrap(d.velocity.x), wrap(d.velocity.y))
        })
    }
}

#[derive(Debug)]
struct Spin {
    speed: f32,
    delay: f32,
    axes: [f32; 3],
}

impl Spin {
    /// Angles in radians at `t` seconds: Speed turns per second on each selected axis.
    fn angles(&self, t: f64) -> [f32; 3] {
        let phase = ((t - f64::from(self.delay)).max(0.0) * f64::from(self.speed)).rem_euclid(1.0);
        self.axes.map(|axis| axis * phase as f32 * std::f32::consts::TAU)
    }
}

#[derive(Debug, Clone, Copy)]
enum ShapeKind {
    Rectangle,
    /// A rectangle with elliptical corners of this radius, in template units.
    Rounded(Vec2),
    Ellipse,
    Triangle,
    RightTriangle,
}

#[derive(Debug)]
enum Brush {
    Solid(Color32),
    Bitmap(String),
    Linear { start: Vec2, end: Vec2, stops: Vec<(f32, Color32)>, wrap: Wrap },
}

#[derive(Debug, Clone)]
struct Text {
    value: String,
    family: String,
    size: f32,
    spacing: f32,
    weight: u16,
    italic: bool,
    horizontal: f32,
    vertical: f32,
    wrap: bool,
    shrink: bool,
    case: TextCase,
    stretch: fontdb::Stretch,
    color: Color32,
    stroke: Color32,
    thickness: f32,
}

/// The TextEffect attribute: the text is shown in capitals or lower case whatever its value.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TextCase { Normal, Upper, Lower }

#[derive(Debug, Default)]
struct Storyboard {
    animations: HashMap<String, Vec<Animation>>,
    seconds: f32,
}

#[derive(Clone, Copy)]
pub enum Cue<'a> { Rest, In(f32), Out(f32), DataIn(f32, &'a str), DataOut(f32, &'a str), Page(f32, usize) }

#[derive(Debug)]
struct Animation {
    effect: Motion,
    delay: f32,
    duration: f32,
    direction: Vec2,
    centered: bool,
    reverse: bool,
    ease: Ease,
}

#[derive(Clone, Copy, Debug)]
enum Motion { Fade, Fly, Bounce, Reveal, Expand, Zoom, ZoomFade, Rotate, Hidden, Scroll }

#[derive(Clone, Copy, Debug)]
enum Ease { Linear, In, Out, InOut, BounceIn, BounceOut }

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
        Self::load_with(path, &texture_roots())
    }

    /// Loads a title. Bitmaps missing from the package are looked for next to it and under
    /// `textures` folders (`roots`), so titles that point at the vMix texture library still work.
    fn load_with(path: &Path, roots: &[PathBuf]) -> Result<Self> {
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
            data_in: Storyboard::default(), data_out: Storyboard::default(),
            field_boards: HashMap::new(),
            rotations: HashMap::new(), drifts: HashMap::new(),
            fields: BTreeMap::new(), unsupported: BTreeSet::new(), ignored: BTreeSet::new(),
            approximated: BTreeSet::new(), bounds: HashMap::new(),
            pages: BTreeMap::new(), frames: HashMap::new(), sequences: HashSet::new(),
        };
        result.objects = result.parse_objects(root, size, 0)?;
        let mut bounds = HashMap::new();
        collect_bounds(&result.objects, Vec2::ZERO, Vec2::splat(1.0), &mut bounds);
        result.bounds = bounds;
        result.check_masks();
        if result.objects.is_empty() {
            return Err("The GT document has no supported objects".into());
        }
        for storyboard in root.children().filter(|n| n.has_tag_name("Storyboard")) {
            let kind = storyboard.attribute("Type").unwrap_or("TransitionIn");
            if matches!(kind, "DataChangeIn" | "DataChangeOut") {
                if let Some(field) = storyboard.attribute("DataName").filter(|field| !field.is_empty()) {
                    let field = field.strip_suffix(".Text").unwrap_or(field);
                    let (entrance, exit) = result.field_boards.entry(field.into()).or_default();
                    parse_storyboard(storyboard, if kind == "DataChangeIn" { entrance } else { exit }, &mut result.unsupported);
                    continue;
                }
            }
            let target = match kind {
                "TransitionIn" => &mut result.entrance,
                "TransitionOut" => &mut result.exit,
                "DataChangeIn" => &mut result.data_in,
                "DataChangeOut" => &mut result.data_out,
                "Continuous" => {
                    result.parse_continuous(storyboard);
                    continue;
                }
                name if name.strip_prefix("Page").is_some_and(|number| number.parse::<usize>().is_ok()) => {
                    let page = name["Page".len()..].parse::<usize>().unwrap_or_default();
                    parse_storyboard(storyboard, result.pages.entry(page).or_default(), &mut result.unsupported);
                    result.approximated.insert("Page storyboards (played once, like an entrance, when started)".into());
                    continue;
                }
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
        // A resource with several sources is an image sequence: the first names it, the rest follow.
        let resource_pictures: HashMap<_, Vec<_>> = resources.descendants().filter(|n| n.has_tag_name("resource"))
            .filter_map(|n| Some((n.attribute("filename")?, n.children().filter(|s| s.has_tag_name("source"))
                .filter_map(|s| s.attribute("guid")).collect()))).collect();
        let mut bytes_used = 0;
        for bitmap in root.descendants().filter(|n| n.has_tag_name("Bitmap")) {
            let Some(source) = bitmap.attribute("Source") else { continue };
            if result.assets.contains_key(source) { continue; }
            let entry = resource_names.get(source).copied().unwrap_or(source);
            let bytes = if archive.index_for_name(entry).is_some() {
                read_entry(&mut archive, entry, IMAGE_LIMIT)?
            } else if let Some(bytes) = external_bitmap(source, path.parent(), roots) {
                bytes
            } else if source.replace('\\', "/").rsplit('/').next().is_some_and(|name| name.eq_ignore_ascii_case("Clear Image.png")) {
                // vMix's transparent placeholder image is not stored in the package.
                let clear = egui::ColorImage::from_rgba_unmultiplied([1, 1], &[0, 0, 0, 0]);
                result.assets.insert(source.into(), Asset(Mutex::new(AssetData::Pixels(clear))));
                continue;
            } else {
                result.unsupported.insert("Missing or external bitmap".into());
                continue;
            };
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
        for (name, pictures) in &resource_pictures {
            if pictures.len() < 2 || !result.assets.contains_key(*name) { continue; }
            let mut count = 1;
            for (index, entry) in pictures.iter().enumerate().skip(1) {
                if archive.index_for_name(entry).is_none() { break; }
                let bytes = read_entry(&mut archive, entry, IMAGE_LIMIT)?;
                let Some(pixels) = decode_picture(bytes, &mut bytes_used)? else { break };
                result.assets.insert(format!("{name}#{index}"), Asset(Mutex::new(AssetData::Pixels(pixels))));
                count += 1;
            }
            if count > 1 { result.frames.insert((*name).to_owned(), count); }
        }
        Ok(result)
    }

    fn parse_objects(&mut self, root: Xml<'_, '_>, size: Vec2, depth: usize) -> Result<Vec<Object>> {
        if depth > 24 { return Err("GT composition nesting is too deep".into()); }
        let mut objects = Vec::new();
        for node in root.children().filter(Node::is_element) {
            let tag = node.tag_name().name();
            if matches!(tag, "Layer" | "Rectangle" | "Ellipse" | "Triangle" | "RightTriangle" | "Image" | "TextBlock" | "Ticker" | "QRCode") {
                for attribute in node.attributes().filter(|attribute| !known_attribute(tag, attribute.name(), attribute.value())) {
                    self.ignored.insert(format!("{tag}.{} attribute", attribute.name()));
                }
                let prefix = format!("{tag}.");
                for element in node.children().filter(Node::is_element) {
                    let Some(property) = element.tag_name().name().strip_prefix(&prefix) else { continue };
                    match property {
                        "Fill" | "Stroke" | "Mask" | "Transform" | "Crop" | "Effects" | "Geometry" | "Composition" | "Bitmap" | "Template" => {}
                        // The geometry a title was saved with is the one drawn; a changed text does not move it.
                        "Bounding" => {
                            self.approximated.insert("Bounding (the saved size is used, it does not follow changed text)".into());
                        }
                        // `<TextDecoration />` with nothing in it decorates nothing.
                        "TextDecorations" if element.children().filter(Node::is_element).all(|decoration| decoration.attributes().count() == 0) => {}
                        _ => { self.ignored.insert(format!("{tag}.{property} property")); }
                    }
                }
            }
            let name = node.attribute("Name").unwrap_or_default();
            let dimensions = pair(node.attribute("Dimensions"), size);
            let property = |prop: &str| child(node, &format!("{tag}.{prop}"));
            let brush = |prop| property(prop).and_then(|n| child(n, "Brush"));
            let fill = brush_color(brush("Fill"), Color32::WHITE);
            let stroke = brush_color(brush("Stroke"), Color32::TRANSPARENT);
            let thickness = number(node.attribute("StrokeThickness"), 0.0).max(0.0);
            let mask = match property("Mask") {
                None => None,
                Some(mask) => match child(mask, "Mask").and_then(|mask| mask.attribute("Object")).filter(|name| !name.is_empty()) {
                    Some(name) => Some(name.to_owned()),
                    None => {
                        self.unsupported.insert("Mask".into());
                        None
                    }
                },
            };
            let is_shape = matches!(tag, "Rectangle" | "Ellipse" | "Triangle" | "RightTriangle");
            let mut mirror = [false; 2];
            let mut tilt = [0.0f32; 2];
            // GraphicsEngine GraphicsTransform (ctor 0x0600058c): Rotate = X,Y,Z radians, Scale 1,
            // Pan 0, RotateOrigin (0,0,-1) = the object's own centre. Titles only rotate on Z.
            let mut rotation = 0.0;
            if let Some(transform) = property("Transform").and_then(|p| child(p, "Transform")) {
                let rotate: Vec<f32> = transform.attribute("Rotate").unwrap_or("0,0,0").split(',')
                    .map(|v| number(Some(v.trim()), 0.0)).collect();
                let (x, y) = (rotate.first().copied().unwrap_or(0.0), rotate.get(1).copied().unwrap_or(0.0));
                if x.is_finite() && y.is_finite() && (x != 0.0 || y != 0.0) {
                    tilt = [x, y];
                    // Half turns only mirror the object, which is exact. Other angles squash it.
                    let half_turn = |angle: f32| (angle - (angle / std::f32::consts::PI).round() * std::f32::consts::PI).abs() < 1e-3;
                    if !(half_turn(x) && half_turn(y)) {
                        self.approximated.insert("3D rotation (the object is squashed, without perspective)".into());
                    }
                }
                if transform.attributes().any(|a| !matches!(a.name(), "Rotate")) {
                    self.unsupported.insert("Transform scale, pan or origin".into());
                }
                rotation = rotate.get(2).copied().filter(|a| a.is_finite()).unwrap_or(0.0);
                if tag == "Layer" && rotation != 0.0 {
                    self.unsupported.insert("Transform rotated layer".into());
                    rotation = 0.0;
                }
            }
            let crop = property("Crop").and_then(|p| child(p, "Crop")).and_then(|c| {
                if c.attribute("Feather").is_some_and(|f| f.split(',').any(|s| number(Some(s), 0.0) != 0.0)) {
                    self.unsupported.insert("Crop feather".into());
                }
                let values: Vec<f32> = c.attribute("Range")?.split(',').filter_map(|n| n.parse().ok()).collect();
                <[f32; 4]>::try_from(values).ok().filter(|r| r.iter().all(|n| n.is_finite()))
            });
            let mut shadow = None;
            let mut skew = Vec2::ZERO;
            let mut flip = [false; 2];
            if let Some(effects) = property("Effects") {
                for effect in effects.children().filter(Node::is_element) {
                    match effect.attribute("Type").unwrap_or_default() {
                        "Shadow" if matches!(tag, "Image" | "TextBlock" | "Rectangle" | "Ellipse" | "Triangle" | "RightTriangle")
                            && effect.attribute("Mode").unwrap_or("Shadow") == "Shadow" => {
                            shadow = Some(Shadow {
                                blur: number(effect.attribute("BlurAmount"), 5.0).clamp(0.0, 64.0),
                                offset: pair(effect.attribute("Offset"), Vec2::ZERO),
                                color: color(effect.attribute("Color"), Color32::BLACK),
                            });
                        }
                        "Skew" if is_shape || matches!(tag, "Image" | "TextBlock") => {
                            let angle = pair(effect.attribute("Angle"), Vec2::ZERO);
                            skew = Vec2::new(angle.x.clamp(-80.0, 80.0).to_radians().tan(),
                                angle.y.clamp(-80.0, 80.0).to_radians().tan());
                        }
                        "FlipX" if tag == "Image" => flip[0] = true,
                        "FlipY" if tag == "Image" => flip[1] = true,
                        "FlipX" if is_shape || tag == "TextBlock" => mirror[0] = !mirror[0],
                        "FlipY" if is_shape || tag == "TextBlock" => mirror[1] = !mirror[1],
                        kind => { self.unsupported.insert(format!("Effect {kind}")); }
                    }
                }
            }
            let geometry = property("Geometry").and_then(|p| child(p, "Geometry")).and_then(|n| n.attribute("Type"));
            if let Some(geometry) = geometry {
                if !matches!(tag, "Image" | "Rectangle") || geometry != "Cube" {
                    self.unsupported.insert(format!("Geometry {geometry}"));
                }
            }
            let kind = match tag {
                "Layer" => {
                    let Some(comp) = property("Composition").and_then(|n| child(n, "Composition")) else {
                        self.unsupported.insert("Layer without composition".into());
                        continue;
                    };
                    let size = composition_size(comp, dimensions);
                    ObjectKind::Layer { size, objects: self.parse_objects(comp, size, depth + 1)? }
                }
                "Rectangle" | "Ellipse" | "Triangle" | "RightTriangle" => {
                    let kind = match tag {
                        "Ellipse" => ShapeKind::Ellipse,
                        "Triangle" => ShapeKind::Triangle,
                        "RightTriangle" => ShapeKind::RightTriangle,
                        _ => rounded(node.attribute("Radius"), &mut self.ignored),
                    };
                    let stroke = parse_brush(brush("Stroke"), stroke);
                    if matches!(stroke, Brush::Bitmap(_)) { self.unsupported.insert("Bitmap stroke".into()); }
                    let cube_depth = (geometry == Some("Cube")).then(|| {
                        number(node.attribute("Dimensions").and_then(|value| value.split(',').nth(2)), dimensions.x)
                    });
                    ObjectKind::Shape { kind, fill: parse_brush(brush("Fill"), fill), stroke, thickness, cube_depth }
                }
                "Image" => {
                    let Some(source) = property("Bitmap").and_then(|n| child(n, "Bitmap")).and_then(|n| n.attribute("Source")) else {
                        self.unsupported.insert("Image without bitmap".into());
                        continue;
                    };
                    let cube_depth = (geometry == Some("Cube")).then(|| {
                        number(node.attribute("Dimensions").and_then(|value| value.split(',').nth(2)), dimensions.x)
                    });
                    ObjectKind::Image { source: source.into(), cube_depth, flip }
                }
                "TextBlock" => {
                    let value = node.attribute("Text").unwrap_or_default().to_owned();
                    if !name.is_empty() && !node.attribute("DataFlags").is_some_and(|s| s.contains("Hidden")) {
                        self.fields.insert(name.into(), value.clone());
                    }
                    let case = match node.attribute("TextEffect") {
                        None | Some("None") => TextCase::Normal,
                        Some("Uppercase") => TextCase::Upper,
                        Some("Lowercase") => TextCase::Lower,
                        Some(other) => {
                            self.unsupported.insert(format!("Text effect {other}"));
                            TextCase::Normal
                        }
                    };
                    ObjectKind::Text(Text {
                        value, family: node.attribute("FontFamily").unwrap_or("Arial").to_owned(),
                        size: number(node.attribute("FontSize"), 48.0).clamp(1.0, 1024.0),
                        spacing: number(node.attribute("LetterSpacing"), 0.0).clamp(-128.0, 128.0),
                        weight: match node.attribute("FontWeight").unwrap_or_default() {
                            "Black" => 900, "Heavy" | "ExtraBold" => 800, "Bold" => 700,
                            "SemiBold" | "DemiBold" => 600, "Medium" => 500, "Light" => 300, _ => 400,
                        },
                        italic: matches!(node.attribute("FontStyle"), Some("Italic" | "Oblique")),
                        horizontal: match node.attribute("TextAlign") { Some("Center") => 0.5, Some("Right") => 1.0, _ => 0.0 },
                        vertical: match node.attribute("VerticalAlign") { Some("Center") => 0.5, Some("Bottom") => 1.0, _ => 0.0 },
                        wrap: node.attribute("TextWordWrapping") != Some("NoWrap"),
                        shrink: node.attribute("AutoSize").is_some_and(|v| v.contains("Shrink")),
                        case, stretch: parse_stretch(node.attribute("FontStretch")),
                        color: fill, stroke, thickness,
                    })
                }
                "QRCode" => {
                    if !name.is_empty() && !node.attribute("DataFlags").is_some_and(|flags| flags.contains("Hidden")) {
                        self.fields.insert(name.into(), node.attribute("Text").unwrap_or_default().to_owned());
                    }
                    ObjectKind::QrCode
                }
                "Ticker" => {
                    if let Some(kind) = node.attribute("Type").filter(|kind| *kind != "Add") {
                        self.unsupported.insert(format!("Ticker type {kind}"));
                    }
                    let template = match property("Template") {
                        Some(template) => {
                            // The template's fields belong to its items, not to the title.
                            let saved = self.fields.clone();
                            let objects = self.parse_objects(template, dimensions, depth + 1)?;
                            self.fields = saved;
                            let size = objects.first().map(|first| first.size);
                            size.map(|size| {
                                let mut fields = Vec::new();
                                text_fields(&objects, &mut fields);
                                TickerTemplate { objects, size, fields }
                            })
                        }
                        None => None,
                    };
                    if !name.is_empty() && !node.attribute("DataFlags").is_some_and(|flags| flags.contains("Hidden")) {
                        let sample = template.as_ref().map_or_else(
                            || "Ticker text".to_owned(),
                            |template| template.fields.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>().join("|"));
                        self.fields.insert(name.into(), sample);
                    }
                    ObjectKind::Ticker(Ticker {
                        speed: number(node.attribute("Speed"), 2.0).clamp(0.0, 100.0),
                        travel: match node.attribute("Direction") {
                            Some("Right") => Vec2::X, Some("Top") => -Vec2::Y, Some("Bottom") => Vec2::Y, _ => -Vec2::X,
                        },
                        template,
                        style: Text {
                            value: String::new(), family: node.attribute("FontFamily").unwrap_or("Arial").to_owned(),
                            size: number(node.attribute("FontSize"), 48.0).clamp(1.0, 1024.0), spacing: 0.0,
                            weight: font_weight(node.attribute("FontWeight")),
                            italic: matches!(node.attribute("FontStyle"), Some("Italic" | "Oblique")),
                            horizontal: match node.attribute("TextAlign") { Some("Center") => 0.5, Some("Right") => 1.0, _ => 0.0 },
                            vertical: match node.attribute("VerticalAlign") { Some("Center") => 0.5, Some("Bottom") => 1.0, _ => 0.0 },
                            wrap: node.attribute("TextWordWrapping") != Some("NoWrap"), shrink: false,
                            case: TextCase::Normal, stretch: parse_stretch(node.attribute("FontStretch")),
                            color: fill, stroke, thickness,
                        },
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
                crop, skew, shadow, rotation, mirror, tilt, mask,
                kind,
            });
        }
        Ok(objects)
    }

    /// A mask is applied as the area of the object it names. Say so, and say when it names nothing.
    fn check_masks(&mut self) {
        fn walk(objects: &[Object], bounds: &HashMap<String, Rect>, approximated: &mut BTreeSet<String>, ignored: &mut BTreeSet<String>) {
            for object in objects {
                if let Some(mask) = &object.mask {
                    if bounds.contains_key(mask) {
                        approximated.insert("Mask (limited to the area of the mask object)".into());
                    } else {
                        // The object it names is not in the title, so there is nothing to limit it to.
                        ignored.insert("Mask object missing (nothing to limit the object)".into());
                    }
                }
                match &object.kind {
                    ObjectKind::Layer { objects, .. } => walk(objects, bounds, approximated, ignored),
                    ObjectKind::Ticker(Ticker { template: Some(template), .. }) => walk(&template.objects, bounds, approximated, ignored),
                    _ => {}
                }
            }
        }
        walk(&self.objects, &self.bounds, &mut self.approximated, &mut self.ignored);
    }

    pub fn seconds(&self, exiting: bool) -> Option<f32> {
        let duration = if exiting { self.exit.seconds } else { self.entrance.seconds };
        (duration > 0.0).then_some(duration)
    }

    fn data_board(&self, field: &str, exiting: bool) -> &Storyboard {
        self.field_boards.get(field).map(|(enter, exit)| if exiting { exit } else { enter })
            .filter(|b| b.seconds > 0.0 || !b.animations.is_empty())
            .unwrap_or(if exiting { &self.data_out } else { &self.data_in })
    }

    pub fn data_cue<'a>(&self, age: f32, field: &'a str) -> Option<Cue<'a>> {
        let out = self.data_board(field, true).seconds;
        let enter = self.data_board(field, false).seconds;
        if age < out {
            Some(Cue::DataOut(age / out, field))
        } else if age < out + enter {
            Some(Cue::DataIn((age - out) / enter, field))
        } else {
            None
        }
    }

    fn parse_continuous(&mut self, storyboard: Xml<'_, '_>) {
        let Some(animations) = child(storyboard, "Storyboard.Animations") else { return; };
        for animation in animations.children().filter(Node::is_element) {
            let effect = animation.tag_name().name();
            let target = animation.attribute("Object").unwrap_or_default();
            // An explicit "no continuous animation" (titles named "… Static").
            if effect == "None" { continue; }
            if effect == "ImageSequenceLoop" {
                self.sequences.insert(target.into());
                self.approximated.insert("Image sequence (one picture per second)".into());
                continue;
            }
            if let Some(slot) = match effect { "FillOffset" => Some(0), "StrokeOffset" => Some(1), _ => None } {
                // OffsetTransition::BuildInternal (0x060002c1 @0xfa70): the brush Offset changes by
                // Speed per second (GetSpeed = Speed / FPS per frame) in the chosen direction.
                let speed = number(animation.attribute("Speed"), 1.0);
                let velocity = match animation.attribute("Direction").unwrap_or("Left") {
                    "Top" | "10" => Vec2::new(0.0, speed),
                    "TopLeft" | "20" => Vec2::new(speed, speed),
                    "TopRight" | "30" => Vec2::new(-speed, speed),
                    "Bottom" | "40" => Vec2::new(0.0, -speed),
                    "BottomLeft" | "50" => Vec2::new(speed, -speed),
                    "BottomRight" | "60" => Vec2::new(-speed, -speed),
                    "Right" | "80" => Vec2::new(-speed, 0.0),
                    _ => Vec2::new(speed, 0.0),
                };
                let delay = number(animation.attribute("Delay"), 0.0).max(0.0) / 1000.0;
                self.drifts.entry(target.into()).or_default()[slot] = Some(Drift { velocity, delay });
                continue;
            }
            if effect != "RotateContinuous" {
                self.unsupported.insert(format!("Continuous animation {effect}"));
                continue;
            }
            // GraphicsEngine RotateContinuousTransition::BuildInternal (0x060002c7):
            // a 1000 ms segment changes the selected axes by Speed × 360 degrees.
            let axes = match animation.attribute("Direction").unwrap_or("Left") {
                "Top" | "10" => [-1.0, 0.0, 0.0],
                "Bottom" | "40" => [1.0, 0.0, 0.0],
                "Right" | "80" => [0.0, -1.0, 0.0],
                "TopLeft" | "20" => [-1.0, -1.0, 0.0],
                "TopRight" | "30" => [1.0, -1.0, 0.0],
                "BottomLeft" | "50" => [-1.0, 1.0, 0.0],
                "BottomRight" | "60" => [1.0, 1.0, 0.0],
                "Center" | "5" => [0.0, 0.0, -1.0],
                _ => [0.0, 1.0, 0.0],
            };
            let reverse = if animation.attribute("Reverse") == Some("True") { -1.0 } else { 1.0 };
            self.rotations.insert(target.into(), Spin {
                axes,
                speed: number(animation.attribute("Speed"), 1.0) * reverse,
                delay: number(animation.attribute("Delay"), 0.0).max(0.0) / 1000.0,
            });
        }
    }

    #[cfg(test)]
    pub fn draw(&self, p: &Painter, rect: Rect, opacity: f32, texts: &BTreeMap<String, String>, cue: Cue<'_>, t: f64) {
        self.draw_with(p, rect, opacity, texts, cue, &[], &HashMap::new(), t);
    }

    /// Draws with several fields animating at once (`data`, each cue with the text its field
    /// showed before) and with rows moved by `offsets`.
    pub fn draw_with(&self, p: &Painter, rect: Rect, opacity: f32, texts: &BTreeMap<String, String>, cue: Cue<'_>,
        data: &[(Cue<'_>, &str)], offsets: &HashMap<String, f32>, t: f64)
    {
        let scale = (rect.width() / self.size.x).min(rect.height() / self.size.y);
        if scale <= 0.0 { return; }
        let bounds = Rect::from_center_size(rect.center(), self.size * scale);
        let view = View { bounds, opacity };
        let (p, view) = self.animate(p.with_clip_rect(rect), view, "", cue);
        let scene = Scene { texts, cue, data, offsets, root: Some((bounds.min, Vec2::splat(scale))) };
        self.draw_objects(&p, &self.objects, view, self.size, &scene, false, t);
    }

    /// Distance between the first two rows of a timing tower, in template units.
    pub fn row_pitch(&self) -> Option<f32> {
        fn find<'a>(objects: &'a [Object], name: &str) -> Option<&'a Object> {
            objects.iter().find_map(|object| match &object.kind {
                _ if object.name == name => Some(object),
                ObjectKind::Layer { objects, .. } => find(objects, name),
                _ => None,
            })
        }
        let first = find(&self.objects, "Row1")?;
        Some(find(&self.objects, "Row2").map_or(first.size.y, |second| second.location.y - first.location.y))
    }

    fn draw_objects(&self, p: &Painter, objects: &[Object], parent: View, size: Vec2, scene: &Scene<'_>, shifted: bool, t: f64) {
        if parent.opacity <= 0.0 || size.x <= 0.0 || size.y <= 0.0 { return; }
        let scale = parent.bounds.size() / size;
        for obj in objects {
            if !obj.visible || obj.opacity <= 0.0 || obj.size.x <= 0.0 || obj.size.y <= 0.0 { continue; }
            // A moved row carries its children, so only the row itself or loose `Row1.…` objects shift.
            let offset = if shifted || scene.offsets.is_empty() { None } else {
                scene.offsets.get(obj.name.split('.').next().unwrap_or(&obj.name)).copied()
            };
            if offset.is_some_and(f32::is_nan) { continue; }
            let location = obj.location + Vec2::new(0.0, offset.unwrap_or(0.0));
            let bounds = Rect::from_min_size(parent.bounds.min + location * scale, obj.size * scale);
            let view = View { bounds, opacity: parent.opacity * obj.opacity };
            let (mut p, mut view) = self.animate(p.clone(), view, &obj.name, scene.cue);
            for (cue, _) in scene.data {
                (p, view) = self.animate(p, view, &obj.name, *cue);
            }
            // RotateContinuous on a flat object: a Z spin turns it; X and Y spins are seen face-on,
            // so its height or width shrinks by |cos| as it flips (a cube image draws in 3D below).
            let mut rotation = obj.rotation;
            let is_cube = matches!(obj.kind, ObjectKind::Image { cube_depth: Some(_), .. } | ObjectKind::Shape { cube_depth: Some(_), .. });
            if let Some(spin) = self.rotations.get(&obj.name).filter(|_| !is_cube) {
                let [x, y, z] = spin.angles(t);
                let squash = Vec2::new(y.cos().abs(), x.cos().abs());
                view.bounds = Rect::from_center_size(view.bounds.center(), view.bounds.size() * squash);
                if !matches!(obj.kind, ObjectKind::Layer { .. }) { rotation += z; }
            }
            // A turn about X or Y squashes the object about its centre; past a quarter turn it is also mirrored.
            let (cos_x, cos_y) = (obj.tilt[0].cos(), obj.tilt[1].cos());
            if obj.tilt != [0.0; 2] {
                view.bounds = Rect::from_center_size(view.bounds.center(), view.bounds.size() * Vec2::new(cos_y.abs(), cos_x.abs()));
            }
            let mirror = [obj.mirror[0] ^ (cos_y < 0.0), obj.mirror[1] ^ (cos_x < 0.0)];
            if let Some([x1, y1, x2, y2]) = obj.crop {
                let r = view.bounds;
                p = p.with_clip_rect(Rect::from_min_max(r.min + r.size() * Vec2::new(x1, y1),
                    r.min + r.size() * Vec2::new(x2, y2)));
            }
            if let (Some(mask), Some((origin, unit))) = (&obj.mask, scene.root) {
                if let Some(area) = self.bounds.get(mask) {
                    let screen = Rect::from_min_size(origin + area.min.to_vec2() * unit, area.size() * unit);
                    p = p.with_clip_rect(p.clip_rect().intersect(screen));
                }
            }
            if view.opacity <= 0.0 || !p.clip_rect().intersects(turned_bounds(view.bounds, rotation)) { continue; }
            match &obj.kind {
                ObjectKind::Layer { size, objects } => {
                    self.draw_objects(&p.with_clip_rect(view.bounds), objects, view, *size, scene, shifted || offset.is_some(), t);
                }
                ObjectKind::Image { source, cube_depth, flip } => {
                    // An image sequence shows one picture of its resource at a time, one a second.
                    let frame;
                    let source = match self.frames.get(source).filter(|_| self.sequences.contains(&obj.name)) {
                        Some(count) => {
                            let index = (t.max(0.0) as usize) % count;
                            frame = if index == 0 { source.clone() } else { format!("{source}#{index}") };
                            &frame
                        }
                        None => source,
                    };
                    if let Some(asset) = self.assets.get(source) {
                        let texture = asset.texture(p.ctx(), source);
                        if let Some(depth) = cube_depth {
                            let angles = self.rotations.get(&obj.name).map_or([0.0; 3], |spin| spin.angles(t));
                            draw_cube(&p, view, texture, depth * scale.y, angles, None);
                        } else {
                            let [flip_x, flip_y] = [flip[0] ^ mirror[0], flip[1] ^ mirror[1]]
                                .map(|flipped| if flipped { (1.0, 0.0) } else { (0.0, 1.0) });
                            let uv = Rect::from_min_max(Pos2::new(flip_x.0, flip_y.0), Pos2::new(flip_x.1, flip_y.1));
                            let quad = |bounds: Rect, color: Color32| {
                                let mut mesh = egui::Mesh::with_texture(texture);
                                mesh.add_rect_with_uv(bounds, uv, color);
                                let turn = turn(view.bounds.center(), rotation);
                                let center = view.bounds.center();
                                for vertex in &mut mesh.vertices {
                                    let d = vertex.pos - center;
                                    vertex.pos = turn(vertex.pos + Vec2::new(d.y * obj.skew.x, d.x * obj.skew.y));
                                }
                                egui::Shape::mesh(mesh)
                            };
                            if let Some(shadow) = obj.shadow {
                                shadow_samples(shadow, scale, view.opacity, |offset, color| {
                                    p.add(quad(view.bounds.translate(offset), color));
                                });
                            }
                            p.add(quad(view.bounds, Color32::WHITE.gamma_multiply(view.opacity)));
                        }
                    }
                }
                ObjectKind::Shape { cube_depth: Some(depth), fill, .. } => {
                    let angles = self.rotations.get(&obj.name).map_or([0.0; 3], |spin| spin.angles(t));
                    let solid = |color: Color32| draw_cube(&p, view, egui::TextureId::default(), depth * scale.y, angles, Some(color));
                    match fill {
                        Brush::Bitmap(source) => match self.assets.get(source) {
                            Some(asset) => draw_cube(&p, view, asset.texture(p.ctx(), source), depth * scale.y, angles, None),
                            None => {}
                        },
                        Brush::Solid(color) => solid(*color),
                        Brush::Linear { stops, .. } => solid(stops.first().map_or(Color32::WHITE, |stop| stop.1)),
                    }
                }
                ObjectKind::Shape { kind, fill, stroke, thickness, .. } => {
                    let [fill_drift, stroke_drift] = self.drifts.get(&obj.name).copied().unwrap_or_default();
                    let fill_drift = Drift::at(fill_drift, t) + self.scroll_offset(&obj.name, scene.cue);
                    self.draw_shape(&p, view, *kind, (fill, fill_drift),
                        (stroke, thickness * scale.y, Drift::at(stroke_drift, t)), obj.skew, obj.shadow, scale, rotation, mirror);
                }
                ObjectKind::QrCode => {
                    let text = scene.texts.get(&obj.name).or_else(|| self.fields.get(&obj.name)).map_or("", String::as_str);
                    draw_qr(&p, view, text);
                }
                ObjectKind::Ticker(ticker) => {
                    let items = scene.texts.get(&obj.name).or_else(|| self.fields.get(&obj.name)).map_or("", String::as_str);
                    self.draw_ticker(&p, view, ticker, items, scale, t);
                }
                ObjectKind::Text(text) => {
                    // A field that is animating out still shows the text it had before.
                    let value = scene.data.iter().find_map(|(cue, previous)| {
                        matches!(cue, Cue::DataOut(_, field) if *field == obj.name).then_some(*previous)
                    }).or_else(|| scene.texts.get(&obj.name).map(String::as_str)).unwrap_or(&text.value);
                    let clock = value.starts_with("{0:").then(|| {
                        clock_text(value, crate::media::local_clock_ms())
                    }).flatten();
                    let value = clock.as_deref().unwrap_or(value);
                    draw_text(&p, view, text, value, view.bounds.height() / obj.size.y, obj.shadow, obj.skew, mirror, rotation);
                }
            }
        }
    }

    /// Draws the items of a ticker as a loop that moves along its travel direction. Items queue up
    /// behind the edge they enter from, and each one is drawn only while it overlaps the area.
    fn draw_ticker(&self, p: &Painter, view: View, ticker: &Ticker, items: &str, scale: Vec2, t: f64) {
        let items: Vec<&str> = items.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        let area = view.bounds;
        if items.is_empty() || area.width() <= 0.0 || area.height() <= 0.0 { return; }
        let clip = p.with_clip_rect(area.intersect(p.clip_rect()));
        let direction = ticker.travel;
        let horizontal = direction.x != 0.0;
        let (axis, span) = if horizontal { (scale.x, area.width()) } else { (scale.y, area.height()) };
        // An item's length along the travel axis, in screen pixels, with the space after it.
        let gap = ticker.style.size * 2.0 * axis;
        let lengths: Vec<f32> = items.iter().map(|item| match &ticker.template {
            Some(template) => (if horizontal { template.size.x } else { template.size.y }) * axis,
            None => {
                let wrap = if horizontal { f32::INFINITY } else { area.width() };
                let size = measure_text(&clip, &ticker.style, item, ticker.style.size * scale.y, wrap);
                (if horizontal { size.x } else { size.y }) + gap
            }
        }).collect();
        let period = lengths.iter().sum::<f32>().max(span).max(1.0);
        let phase = (t * f64::from(ticker.speed) * 60.0 * f64::from(axis)).rem_euclid(f64::from(period)) as f32;
        // The travel coordinate grows in the direction of travel; items enter at its lowest end.
        let along = |point: Pos2| direction.x * point.x + direction.y * point.y;
        let start = along(area.min).min(along(area.max));
        let mut offset = 0.0;
        for (item, length) in items.iter().zip(&lengths) {
            for copy in -2..=2 {
                let head = start + phase - offset + copy as f32 * period;
                let tail = head - length;
                if head <= start || tail >= start + span { continue; }
                let rect = if direction.x < 0.0 { Rect::from_min_max(Pos2::new(-head, area.top()), Pos2::new(-tail, area.bottom())) }
                    else if direction.x > 0.0 { Rect::from_min_max(Pos2::new(tail, area.top()), Pos2::new(head, area.bottom())) }
                    else if direction.y < 0.0 { Rect::from_min_max(Pos2::new(area.left(), -head), Pos2::new(area.right(), -tail)) }
                    else { Rect::from_min_max(Pos2::new(area.left(), tail), Pos2::new(area.right(), head)) };
                let item_view = View { bounds: rect, opacity: view.opacity };
                match &ticker.template {
                    Some(template) => {
                        let values = HashMap::new();
                        let parts: Vec<&str> = item.split('|').map(str::trim).collect();
                        let texts: BTreeMap<String, String> = template.fields.iter().enumerate().map(|(index, (name, default))| {
                            (name.clone(), parts.get(index).filter(|part| !part.is_empty()).map_or_else(|| default.clone(), |part| (*part).to_owned()))
                        }).collect();
                        let scene = Scene { texts: &texts, cue: Cue::Rest, data: &[], offsets: &values, root: None };
                        let bounds = Rect::from_min_size(rect.min, template.size * scale);
                        self.draw_objects(&clip, &template.objects, View { bounds, opacity: view.opacity },
                            template.size, &scene, true, t);
                    }
                    None => {
                        let mut style = ticker.style.clone();
                        style.wrap = !horizontal;
                        if horizontal { style.horizontal = 0.0; } else { style.vertical = 0.0; }
                        draw_text(&clip, item_view, &style, item, scale.y, None, Vec2::ZERO, [false; 2], 0.0);
                    }
                }
            }
            offset += length;
        }
    }

    /// The storyboard a cue plays, how far into it (seconds), and whether the title is leaving.
    fn cue_board(&self, cue: Cue<'_>) -> Option<(&Storyboard, f32, bool)> {
        Some(match cue {
            Cue::Rest => return None,
            Cue::In(t) => (&self.entrance, t * self.entrance.seconds, false),
            Cue::Out(t) => (&self.exit, t * self.exit.seconds, true),
            Cue::DataIn(t, field) => {
                let board = self.data_board(field, false);
                (board, t * board.seconds, false)
            }
            Cue::DataOut(t, field) => {
                let board = self.data_board(field, true);
                (board, t * board.seconds, true)
            }
            Cue::Page(t, page) => {
                let board = self.pages.get(&page)?;
                (board, t * board.seconds, false)
            }
        })
    }

    /// How far the Scroll animations of `name` have moved its brush: one object size over the
    /// animation, in the direction it names.
    fn scroll_offset(&self, name: &str, cue: Cue<'_>) -> Vec2 {
        let Some((board, time, exiting)) = self.cue_board(cue) else { return Vec2::ZERO };
        board.animations.get(name).into_iter().flatten().filter(|animation| matches!(animation.effect, Motion::Scroll))
            .fold(Vec2::ZERO, |sum, animation| {
                let progress = animation.ease.apply(((time - animation.delay) / animation.duration).clamp(0.0, 1.0));
                sum - animation.direction * if exiting { 1.0 - progress } else { progress }
            })
    }

    /// The pages this title has (`Page1` is 1).
    pub fn pages(&self) -> Vec<usize> {
        self.pages.keys().copied().collect()
    }

    /// How long a page takes to play.
    pub fn page_seconds(&self, page: usize) -> Option<f32> {
        self.pages.get(&page).map(|board| board.seconds).filter(|seconds| *seconds > 0.0)
    }

    fn animate(&self, mut p: Painter, mut view: View, name: &str, cue: Cue<'_>) -> (Painter, View) {
        let Some((storyboard, time, exiting)) = self.cue_board(cue) else { return (p, view) };
        if let Some(animations) = storyboard.animations.get(name) {
            for animation in animations {
                let raw = ((time - animation.delay) / animation.duration).clamp(0.0, 1.0);
                let progress = animation.ease.apply(raw);
                let shown = if exiting { 1.0 - progress } else { progress };
                match animation.effect {
                    Motion::Fade => view.opacity *= shown,
                    Motion::Fly | Motion::Bounce => {
                        let direction = if animation.reverse { -animation.direction } else { animation.direction };
                        view.bounds = view.bounds.translate(direction * view.bounds.size() * (1.0 - shown));
                        if matches!(animation.effect, Motion::Bounce) {
                            // BounceTransition::BuildInternal (0x060002b5 @0xf7bc): Location.Y also
                            // falls from Y − Height/3 to Y with BounceOut; leaving reverses it with Bounce.
                            let lift = if exiting { Ease::BounceIn.apply(raw) } else { 1.0 - Ease::BounceOut.apply(raw) };
                            view.bounds = view.bounds.translate(Vec2::new(0.0, -view.bounds.height() / 3.0 * lift));
                        }
                    }
                    Motion::Reveal => p = p.with_clip_rect(reveal(view.bounds, animation.direction, animation.centered, shown)),
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
                    Motion::Hidden if time >= animation.delay => view.opacity = 0.0,
                    Motion::Hidden => {},
                    // Scroll moves the brush, not the object (see `scroll_offset`).
                    Motion::Scroll => {},
                    Motion::Rotate => {
                        let axis = (shown * std::f32::consts::FRAC_PI_2).sin();
                        let scale = if animation.direction.y != 0.0 { Vec2::new(1.0, axis) } else { Vec2::new(axis, 1.0) };
                        view.bounds = Rect::from_center_size(view.bounds.center(), view.bounds.size() * scale);
                    }
                }
            }
        }
        (p, view)
    }

    fn draw_shape(&self, p: &Painter, view: View, kind: ShapeKind, (fill, fill_offset): (&Brush, Vec2),
        stroke: (&Brush, f32, Vec2), skew: Vec2, shadow: Option<Shadow>, scale: Vec2, rotation: f32, mirror: [bool; 2])
    {
        let mut shapes = Vec::new();
        self.shape_parts(p.ctx(), &mut shapes, view, kind, fill, fill_offset, (stroke.0, stroke.1, stroke.2, (rotation, mirror)), skew, shadow, scale);
        if mirror != [false; 2] {
            // Mirrored about the centre first, then turned: the same order the gradients undo below.
            let flip = mirrored(view.bounds.center(), mirror);
            for shape in &mut shapes { mirror_shape(shape, &flip, mirror[0] != mirror[1]); }
        }
        if rotation != 0.0 {
            // Fills, gradients and bitmaps are laid out on the upright shape, then every vertex
            // turns about the centre, so a gradient turns with its shape.
            let turn = turn(view.bounds.center(), rotation);
            for shape in &mut shapes { rotate_shape(shape, &turn); }
        }
        p.extend(shapes);
    }

    fn shape_parts(&self, ctx: &egui::Context, out: &mut Vec<egui::Shape>, view: View, kind: ShapeKind, fill: &Brush,
        fill_offset: Vec2, stroke: (&Brush, f32, Vec2, (f32, [bool; 2])), skew: Vec2, shadow: Option<Shadow>, scale: Vec2)
    {
        let r = view.bounds;
        let mut polygon: Vec<Pos2> = match kind {
            ShapeKind::Rectangle => vec![r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()],
            ShapeKind::Rounded(radius) => rounded_polygon(r, radius * scale),
            ShapeKind::Ellipse => (0..48).map(|i| {
                let a = i as f32 * std::f32::consts::TAU / 48.0;
                r.center() + Vec2::new(a.cos(), a.sin()) * r.size() * 0.5
            }).collect(),
            ShapeKind::Triangle => vec![r.center_top(), r.right_bottom(), r.left_bottom()],
            ShapeKind::RightTriangle => vec![r.left_top(), r.right_bottom(), r.left_bottom()],
        };
        for point in &mut polygon {
            let delta = *point - r.center();
            *point += Vec2::new(delta.y * skew.x, delta.x * skew.y);
        }
        if let Some(shadow) = shadow {
            // A shadow follows what is painted: with a transparent fill only the outline casts one.
            let hollow = matches!(fill, Brush::Solid(color) if *color == Color32::TRANSPARENT);
            let outline = stroke.1;
            shadow_samples(shadow, scale, view.opacity, |offset, color| {
                let points: Vec<Pos2> = polygon.iter().map(|&p| p + offset).collect();
                if !hollow {
                    out.push(egui::Shape::convex_polygon(points, color, Stroke::NONE));
                } else if outline > 0.0 && !matches!(stroke.0, Brush::Solid(c) if *c == Color32::TRANSPARENT) {
                    out.push(egui::Shape::closed_line(points, Stroke::new(outline, color)));
                }
            });
        }
        // GraphicsBrush (ctor 0x0600032c): Offset (0,0) and WrapX/WrapY = Mirror by default, so
        // a gradient reflects past its ends and a moving Offset scrolls it back and forth.
        let (stroke_brush, width, stroke_offset, (rotation, flips)) = stroke;
        match fill {
            Brush::Solid(color) => {
                out.push(egui::Shape::convex_polygon(polygon.clone(), color.gamma_multiply(view.opacity), Stroke::NONE));
            }
            Brush::Bitmap(source) => {
                if let Some(asset) = self.assets.get(source) {
                    let mut mesh = egui::Mesh::with_texture(asset.texture(ctx, source));
                    append_polygon(&mut mesh, &polygon, |pos| {
                        let uv = (pos - r.min) / r.size() + fill_offset;
                        (Pos2::new(uv.x, uv.y), Color32::WHITE.gamma_multiply(view.opacity))
                    });
                    out.push(egui::Shape::mesh(mesh));
                }
            }
            Brush::Linear { start, end, stops, wrap } => {
                if let Some(project) = gradient_axis(r, *start, *end, fill_offset) {
                    let (low, high) = polygon.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &pt| {
                        let t = project(pt);
                        (lo.min(t), hi.max(t))
                    });
                    let mut cuts = wrap.cuts(stops, low, high);
                    cuts.retain(|c| *c > low && *c < high);
                    cuts.sort_by(f32::total_cmp);
                    cuts.dedup();
                    let mut mesh = egui::Mesh::default();
                    for i in 0..=cuts.len() {
                        let mut points = polygon.clone();
                        if i > 0 { points = clip_polygon(&points, |pt| project(pt) - cuts[i - 1]); }
                        if i < cuts.len() { points = clip_polygon(&points, |pt| cuts[i] - project(pt)); }
                        append_polygon(&mut mesh, &points, |pos| {
                            let value = gradient_color(stops, wrap.apply(project(pos))).gamma_multiply(view.opacity);
                            (egui::epaint::WHITE_UV, value)
                        });
                    }
                    out.push(egui::Shape::mesh(mesh));
                }
            }
        }
        if width <= 0.0 { return; }
        let color = match stroke_brush {
            Brush::Solid(color) if *color == Color32::TRANSPARENT => return,
            Brush::Solid(color) => egui::epaint::ColorMode::Solid(color.gamma_multiply(view.opacity)),
            Brush::Linear { start, end, stops, wrap } => {
                let Some(project) = gradient_axis(r, *start, *end, stroke_offset) else { return };
                let (stops, opacity, wrap) = (stops.clone(), view.opacity, *wrap);
                // Points arrive already turned; turn them back to read the upright gradient.
                let (back, flip) = (turn(r.center(), -rotation), mirrored(r.center(), flips));
                let upright = move |pos: Pos2| flip(back(pos));
                egui::epaint::ColorMode::UV(std::sync::Arc::new(move |_, pos| {
                    gradient_color(&stops, wrap.apply(project(upright(pos)))).gamma_multiply(opacity)
                }))
            }
            Brush::Bitmap(_) => return,
        };
        out.push(egui::Shape::closed_line(polygon, egui::epaint::PathStroke { width, color, kind: egui::StrokeKind::Middle }));
    }

}

/// Position along a linear gradient for a screen point, with the brush Offset (in object
/// units) applied the way GT samples it: brush coordinates = object coordinates + Offset.
fn gradient_axis(r: Rect, start: Vec2, end: Vec2, offset: Vec2) -> Option<impl Fn(Pos2) -> f32 + Clone + Send + Sync + 'static> {
    let axis = (end - start) * r.size();
    let length = axis.length_sq();
    if length <= f32::EPSILON || !length.is_finite() { return None; }
    let origin = r.min + start * r.size();
    let shift = offset * r.size();
    Some(move |point: Pos2| (point + shift - origin).dot(axis) / length)
}

/// Mirror wrapping (GraphicsWrap.Mirror): 0→1, then back 1→0, repeating.
fn mirror(t: f32) -> f32 {
    let m = t.rem_euclid(2.0);
    if m > 1.0 { 2.0 - m } else { m }
}

/// GraphicsWrap (Clamp, Wrap, Mirror). GraphicsBrush defaults WrapX and WrapY to Mirror
/// (ctor 0x0600032c); a gradient follows WrapX.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Wrap { Clamp, Repeat, Mirror }

impl Wrap {
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some("Clamp" | "0") => Self::Clamp,
            Some("Wrap" | "1") => Self::Repeat,
            _ => Self::Mirror,
        }
    }

    fn apply(self, t: f32) -> f32 {
        match self {
            Self::Clamp => t.clamp(0.0, 1.0),
            Self::Repeat => t.rem_euclid(1.0),
            Self::Mirror => mirror(t),
        }
    }

    /// Every place between `low` and `high` where the wrapped gradient changes slope or jumps:
    /// each stop of each period, and the period edges.
    fn cuts(self, stops: &[(f32, Color32)], low: f32, high: f32) -> Vec<f32> {
        let mut cuts = Vec::new();
        let periods = if self == Self::Clamp { 0..=0 } else {
            (low.floor() as i32)..=(high.floor() as i32).min(low.floor() as i32 + 64)
        };
        for period in periods {
            let base = period as f32;
            cuts.push(base);
            cuts.push(base + 1.0);
            for &(position, _) in stops {
                let reflected = self == Self::Mirror && period.rem_euclid(2) == 1;
                cuts.push(base + if reflected { 1.0 - position } else { position });
            }
        }
        cuts
    }
}

/// Turns points by `angle` radians (clockwise on screen) about `center`.
fn turn(center: Pos2, angle: f32) -> impl Fn(Pos2) -> Pos2 {
    let (sin, cos) = angle.sin_cos();
    move |point| {
        let d = point - center;
        center + Vec2::new(d.x * cos - d.y * sin, d.x * sin + d.y * cos)
    }
}

/// The screen area a rectangle covers once turned about its centre.
fn turned_bounds(rect: Rect, angle: f32) -> Rect {
    if angle == 0.0 { return rect; }
    let turn = turn(rect.center(), angle);
    Rect::from_points(&[rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()].map(turn))
}

fn rotate_shape(shape: &mut egui::Shape, turn: &impl Fn(Pos2) -> Pos2) {
    match shape {
        egui::Shape::Path(path) => for point in &mut path.points { *point = turn(*point); },
        egui::Shape::Mesh(mesh) => for vertex in &mut std::sync::Arc::make_mut(mesh).vertices { vertex.pos = turn(vertex.pos); },
        _ => {}
    }
}

/// Reflects points about `center` in the directions marked in `mirror` (x, y).
fn mirrored(center: Pos2, mirror: [bool; 2]) -> impl Fn(Pos2) -> Pos2 {
    move |point| Pos2::new(
        if mirror[0] { 2.0 * center.x - point.x } else { point.x },
        if mirror[1] { 2.0 * center.y - point.y } else { point.y })
}

/// Mirrors a shape. An odd number of mirrors reverses the winding, so a path's points are reversed
/// back to keep its edges smooth.
fn mirror_shape(shape: &mut egui::Shape, flip: &impl Fn(Pos2) -> Pos2, reverses: bool) {
    match shape {
        egui::Shape::Path(path) => {
            for point in &mut path.points { *point = flip(*point); }
            if reverses { path.points.reverse(); }
        }
        egui::Shape::Mesh(mesh) => for vertex in &mut std::sync::Arc::make_mut(mesh).vertices { vertex.pos = flip(vertex.pos); },
        _ => {}
    }
}

/// Text sheared by `skew` (tangents, as in the Skew effect) about `center`: the glyph triangles
/// of the laid-out text, moved one by one, since a text shape cannot be sheared.
fn skewed_text(p: &Painter, galley: &egui::Galley, pos: Pos2, color: Color32, skew: Vec2, mirror: [bool; 2], center: Pos2,
    turn: &impl Fn(Pos2) -> Pos2) -> egui::Shape
{
    let flip = mirrored(center, mirror);
    let [width, height] = p.fonts(|fonts| fonts.font_image_size());
    let mut mesh = egui::Mesh::default();
    for row in &galley.rows {
        let mut part = row.visuals.mesh.clone();
        for vertex in &mut part.vertices {
            vertex.uv = Pos2::new(vertex.uv.x / width as f32, vertex.uv.y / height as f32);
            if vertex.color == Color32::PLACEHOLDER { vertex.color = color; }
            let point = flip(pos + vertex.pos.to_vec2());
            let d = point - center;
            vertex.pos = turn(point + Vec2::new(d.y * skew.x, d.x * skew.y));
        }
        mesh.append(part);
    }
    egui::Shape::mesh(mesh)
}

/// The area of every named object, in template units: `origin` and `scale` place the objects of
/// the composition being walked, as drawing does for each layer.
fn collect_bounds(objects: &[Object], origin: Vec2, scale: Vec2, out: &mut HashMap<String, Rect>) {
    for object in objects {
        let min = origin + object.location * scale;
        let rect = Rect::from_min_size(Pos2::new(min.x, min.y), object.size * scale);
        if !object.name.is_empty() { out.entry(object.name.clone()).or_insert(rect); }
        if let ObjectKind::Layer { size, objects } = &object.kind {
            if size.x > 0.0 && size.y > 0.0 { collect_bounds(objects, min, object.size * scale / *size, out); }
        }
    }
}

/// A QR code of `text`, black on white with a margin of two modules, in the largest square that
/// fits the area. The code is made once per text.
fn draw_qr(p: &Painter, view: View, text: &str) {
    if text.is_empty() { return; }
    let code = p.ctx().data_mut(|data| data.get_temp_mut_or_insert_with(egui::Id::new(("gt-qr", text)), || {
        std::sync::Arc::new(match qrcode::QrCode::new(text.as_bytes()) {
            Ok(code) => (code.width(), code.to_colors().into_iter().map(|module| module == qrcode::Color::Dark).collect::<Vec<bool>>()),
            Err(_) => (0, Vec::new()),
        })
    }).clone());
    let (width, dark) = (code.0, &code.1);
    if width == 0 { return; }
    let side = view.bounds.width().min(view.bounds.height());
    let square = Rect::from_center_size(view.bounds.center(), Vec2::splat(side));
    p.rect_filled(square, 0.0, Color32::WHITE.gamma_multiply(view.opacity));
    let module = side / (width + 4) as f32;
    let ink = Color32::BLACK.gamma_multiply(view.opacity);
    let mut mesh = egui::Mesh::default();
    for (index, _) in dark.iter().enumerate().filter(|(_, dark)| **dark) {
        let corner = square.min + Vec2::new((index % width + 2) as f32, (index / width + 2) as f32) * module;
        mesh.add_colored_rect(Rect::from_min_size(corner, Vec2::splat(module)), ink);
    }
    p.add(egui::Shape::mesh(mesh));
}

fn font_weight(value: Option<&str>) -> u16 {
    match value.unwrap_or_default() {
        "Black" => 900, "Heavy" | "ExtraBold" => 800, "Bold" => 700,
        "SemiBold" | "DemiBold" => 600, "Medium" => 500, "Light" => 300, _ => 400,
    }
}

/// The text objects of a template, in drawing order, with their default text.
fn text_fields(objects: &[Object], out: &mut Vec<(String, String)>) {
    for object in objects {
        match &object.kind {
            ObjectKind::Text(text) if !object.name.is_empty() => out.push((object.name.clone(), text.value.clone())),
            ObjectKind::Layer { objects, .. } => text_fields(objects, out),
            _ => {}
        }
    }
}

/// The size `value` takes in `text`'s font at `size` pixels, wrapped at `wrap` pixels.
fn measure_text(p: &Painter, text: &Text, value: &str, size: f32, wrap: f32) -> Vec2 {
    let family = title_font(p, text);
    let mut job = egui::text::LayoutJob::simple(value.to_owned(), egui::FontId::new(size.max(1.0), family), Color32::PLACEHOLDER, wrap);
    job.sections[0].format.extra_letter_spacing = text.spacing * size / text.size;
    p.layout_job(job).size()
}

/// A rectangle, rounded when it has a `Radius` of `x,y`. A single positive value rounds both
/// directions. A negative radius is not understood and leaves the corners square.
fn rounded(radius: Option<&str>, ignored: &mut BTreeSet<String>) -> ShapeKind {
    let Some(radius) = radius else { return ShapeKind::Rectangle };
    let radius = pair(Some(radius), Vec2::ZERO);
    if radius.x < 0.0 || radius.y < 0.0 {
        ignored.insert("Rectangle.Radius negative".into());
        return ShapeKind::Rectangle;
    }
    let radius = Vec2::new(if radius.x > 0.0 { radius.x } else { radius.y }, if radius.y > 0.0 { radius.y } else { radius.x });
    if radius == Vec2::ZERO { ShapeKind::Rectangle } else { ShapeKind::Rounded(radius) }
}

/// The outline of a rectangle whose corners are quarter ellipses, clockwise from the top edge.
fn rounded_polygon(r: Rect, radius: Vec2) -> Vec<Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};
    let (rx, ry) = (radius.x.clamp(0.0, r.width() / 2.0), radius.y.clamp(0.0, r.height() / 2.0));
    const STEPS: usize = 8;
    let corners = [
        (Pos2::new(r.right() - rx, r.top() + ry), -FRAC_PI_2),
        (Pos2::new(r.right() - rx, r.bottom() - ry), 0.0),
        (Pos2::new(r.left() + rx, r.bottom() - ry), FRAC_PI_2),
        (Pos2::new(r.left() + rx, r.top() + ry), PI),
    ];
    let mut points = Vec::with_capacity(4 * (STEPS + 1));
    for (center, start) in corners {
        for step in 0..=STEPS {
            let angle = start + step as f32 * FRAC_PI_2 / STEPS as f32;
            points.push(Pos2::new(center.x + rx * angle.cos(), center.y + ry * angle.sin()));
        }
    }
    points
}

fn parse_stretch(value: Option<&str>) -> fontdb::Stretch {
    match value.unwrap_or_default() {
        "UltraCondensed" => fontdb::Stretch::UltraCondensed,
        "ExtraCondensed" => fontdb::Stretch::ExtraCondensed,
        "Condensed" => fontdb::Stretch::Condensed,
        "SemiCondensed" => fontdb::Stretch::SemiCondensed,
        "SemiExpanded" => fontdb::Stretch::SemiExpanded,
        "Expanded" => fontdb::Stretch::Expanded,
        "ExtraExpanded" => fontdb::Stretch::ExtraExpanded,
        "UltraExpanded" => fontdb::Stretch::UltraExpanded,
        _ => fontdb::Stretch::Normal,
    }
}

/// Folders searched for textures a title refers to by an absolute path of another machine:
/// `LIVEMIX_GT_TEXTURES` and the vMix texture library when it is installed.
fn texture_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::env::var_os("LIVEMIX_GT_TEXTURES").map(PathBuf::from).into_iter().collect();
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(base) = std::env::var_os(variable) {
            roots.push(PathBuf::from(base).join("vMix").join("GT").join("textures"));
        }
    }
    roots.retain(|root| root.is_dir());
    roots
}

/// A picture the package does not contain. A relative source is read next to the title. An
/// absolute one is read only through its `textures/…` tail, under the trusted `roots`; an
/// arbitrary path on the disk is never opened. Only picture files, and no `..`.
fn external_bitmap(source: &str, template_dir: Option<&Path>, roots: &[PathBuf]) -> Option<Vec<u8>> {
    const EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "bmp", "gif", "tif", "tiff"];
    let source = source.replace('\\', "/");
    let extension = Path::new(&source).extension()?.to_str()?;
    if !EXTENSIONS.iter().any(|known| extension.eq_ignore_ascii_case(known)) { return None; }
    let plain = |text: &str| -> Option<PathBuf> {
        let path = Path::new(text);
        (!text.is_empty() && path.components().all(|part| matches!(part, Component::Normal(_)))).then(|| path.to_owned())
    };
    let absolute = source.starts_with('/') || source.get(1..2) == Some(":");
    let mut candidates = Vec::new();
    if !absolute {
        if let (Some(folder), Some(relative)) = (template_dir, plain(&source)) { candidates.push(folder.join(relative)); }
    }
    if let Some(at) = source.to_ascii_lowercase().rfind("/textures/") {
        if let Some(relative) = plain(&source[at + "/textures/".len()..]) {
            candidates.extend(roots.iter().map(|root| root.join(&relative)));
        }
    }
    candidates.into_iter().find_map(|path| {
        std::fs::metadata(&path).ok().filter(|meta| meta.is_file() && meta.len() <= IMAGE_LIMIT as u64)?;
        std::fs::read(path).ok()
    })
}

/// Reads a picture, within the bitmap memory budget. `None` when it cannot be decoded.
fn decode_picture(bytes: Vec<u8>, bytes_used: &mut usize) -> Result<Option<egui::ColorImage>> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(TOTAL_IMAGE_LIMIT.saturating_sub(*bytes_used) as u64);
    reader.limits(limits);
    let Ok(picture) = reader.decode() else { return Ok(None) };
    let rgba = picture.into_rgba8();
    *bytes_used += rgba.as_raw().len();
    if *bytes_used > TOTAL_IMAGE_LIMIT { return Err("GT title exceeds the bitmap memory limit".into()); }
    Ok(Some(egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw())))
}

/// Attributes the reader uses, by object type. Anything else on a known object is ignored.
fn known_attribute(tag: &str, attribute: &str, value: &str) -> bool {
    // Locked and DataFlags are editor and data-source marks; Style Square and SizeMode Stretch are the defaults.
    matches!(attribute, "Name" | "Location" | "Dimensions" | "Opacity" | "Visible" | "StrokeThickness" | "Locked" | "DataFlags")
        || tag == "Rectangle" && attribute == "Style" && value == "Square"
        || tag == "Image" && attribute == "SizeMode" && value == "Stretch"
        || tag == "Rectangle" && attribute == "Radius"
        || tag == "TextBlock" && matches!(attribute, "Text" | "FontFamily" | "FontSize" | "LetterSpacing" | "FontWeight"
            | "FontStyle" | "TextAlign" | "VerticalAlign" | "TextWordWrapping" | "AutoSize" | "DataFlags"
            | "TextEffect" | "FontStretch")
        || tag == "QRCode" && matches!(attribute, "Text" | "DataFlags")
        || tag == "Ticker" && matches!(attribute, "Speed" | "Direction" | "Type" | "FontFamily" | "FontSize" | "FontWeight"
            | "FontStyle" | "FontStretch" | "VerticalAlign" | "TextAlign" | "TextWordWrapping" | "DataFlags")
}

/// Collects the `.gtzip` files under `path`. Only the path given is an error when it is missing;
/// folders that cannot be read, or that are too deep, are skipped.
fn gt_files(path: &Path, files: &mut Vec<std::path::PathBuf>, depth: usize) -> Result<()> {
    if path.is_file() {
        files.push(path.to_owned());
    } else if path.is_dir() {
        if depth > 12 { return Ok(()); }
        let Ok(entries) = std::fs::read_dir(path) else { return Ok(()) };
        for entry in entries.flatten() {
            let entry = entry.path();
            if entry.is_dir() || entry.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("gtzip")) {
                gt_files(&entry, files, depth + 1)?;
            }
        }
    } else {
        return Err(format!("{}: not a file or folder", path.display()));
    }
    Ok(())
}

/// Loads one `.gtzip` file, or every one under a folder, and lists what LiveMix does not support:
/// features it reports, and attributes it silently ignores.
pub fn report(path: &Path) -> Result<String> {
    use std::fmt::Write as _;
    let mut files = Vec::new();
    gt_files(path, &mut files, 0)?;
    if files.is_empty() { return Err(format!("No .gtzip files found in {}", path.display())); }
    files.sort();
    let (mut clean, mut warned, mut failed) = (0, 0, 0);
    let mut unsupported: BTreeMap<String, usize> = BTreeMap::new();
    let mut ignored: BTreeMap<String, usize> = BTreeMap::new();
    let mut approximated: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = String::new();
    let _ = writeln!(out, "GT report: {} ({} file{})", path.display(), files.len(), if files.len() == 1 { "" } else { "s" });
    for file in &files {
        let name = file.strip_prefix(path).ok().filter(|name| !name.as_os_str().is_empty())
            .map_or_else(|| file.file_name().unwrap_or_default().to_string_lossy().into_owned(), |name| name.display().to_string());
        let document = match Document::load(file) {
            Ok(document) => document,
            Err(error) => {
                failed += 1;
                let _ = writeln!(out, "FAIL {name}: {error}");
                continue;
            }
        };
        let seconds = |exiting| document.seconds(exiting).map_or_else(|| "-".to_owned(), |seconds| format!("{seconds:.2}s"));
        let status = if document.unsupported.is_empty() { clean += 1; "OK  " } else { warned += 1; "WARN" };
        let _ = writeln!(out, "{status} {name}  {}x{}  {} fields  in {} out {}",
            document.size.x, document.size.y, document.fields.len(), seconds(false), seconds(true));
        for (label, set, tally) in [("unsupported", &document.unsupported, &mut unsupported), ("ignored", &document.ignored, &mut ignored),
            ("approximate", &document.approximated, &mut approximated)]
        {
            if set.is_empty() { continue; }
            let _ = writeln!(out, "       {label}: {}", set.iter().cloned().collect::<Vec<_>>().join(", "));
            for feature in set { *tally.entry(feature.clone()).or_default() += 1; }
        }
    }
    let _ = writeln!(out, "\n{clean} OK, {warned} with unsupported features, {failed} failed");
    for (title, tally) in [("Unsupported features by number of files", &unsupported), ("Ignored attributes by number of files", &ignored),
        ("Approximated features by number of files", &approximated)]
    {
        if tally.is_empty() { continue; }
        let mut rows: Vec<_> = tally.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let _ = writeln!(out, "\n{title}:");
        for (feature, files) in rows { let _ = writeln!(out, "  {files:>4}  {feature}"); }
    }
    Ok(out)
}

fn clock_text(value: &str, now_ms: u32) -> Option<String> {
    let pattern = value.strip_prefix("{0:")?.strip_suffix('}')?;
    let (ms, pattern, countdown) = match pattern.split_once('|') {
        Some((target, pattern)) => (clock_target(target)?.saturating_sub(now_ms), pattern, true),
        None => (now_ms, pattern, false),
    };
    let hours = ms / 3_600_000;
    let minutes = ms / 60_000;
    let seconds = (ms / 1000) % 60;
    let twelve_hour = (hours + 11) % 12 + 1;
    let has_hours = pattern.contains(['H', 'h']);
    let minutes = if countdown && !has_hours { minutes } else { minutes % 60 };
    let mut result = String::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        let mut count = 1;
        while chars.next_if_eq(&c).is_some() { count += 1; }
        let number = match c {
            'H' => hours,
            'h' => if countdown { hours } else { twelve_hour },
            'm' => minutes,
            's' => seconds,
            'f' if count <= 3 => (ms % 1000) / 10_u32.pow(3 - count as u32),
            't' if !countdown && count <= 2 => {
                let suffix = if hours < 12 { "AM" } else { "PM" };
                result.push_str(&suffix[..count]);
                continue;
            }
            ':' | '.' | ' ' => {
                result.extend(std::iter::repeat_n(c, count));
                continue;
            }
            _ => return None,
        };
        result.push_str(&format!("{number:0count$}"));
    }
    (!result.is_empty()).then_some(result)
}

fn clock_target(value: &str) -> Option<u32> {
    let upper = value.trim().to_ascii_uppercase();
    let meridiem = upper.strip_suffix("AM").map(|time| (time, 0))
        .or_else(|| upper.strip_suffix("PM").map(|time| (time, 12)));
    let mut parts = meridiem.map_or(upper.as_str(), |(time, _)| time.trim()).split(':');
    let mut hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    let second: u32 = parts.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    if let Some((_, offset)) = meridiem {
        if !(1..=12).contains(&hour) { return None; }
        hour = hour % 12 + offset;
    }
    if hour > 23 || minute > 59 || second > 59 || parts.next().is_some() { return None; }
    Some(((hour * 60 + minute) * 60 + second) * 1000)
}

#[test]
fn gt_scoreboard_clock_counts_down_to_local_deadline() {
    let value = "{0:4:00PM|mm:ss}";
    let deadline = 16 * 3_600_000;
    assert_eq!(clock_text(value, deadline - 90_000).as_deref(), Some("01:30"));
    assert_eq!(clock_text(value, deadline - 89_000).as_deref(), Some("01:29"));
    assert_eq!(clock_text(value, deadline + 1000).as_deref(), Some("00:00"));
    assert_eq!(clock_text("{0:HH:mm:ss}", deadline).as_deref(), Some("16:00:00"));
}

fn shadow_samples(shadow: Shadow, scale: Vec2, opacity: f32, mut draw: impl FnMut(Vec2, Color32)) {
    let taps = [
        (Vec2::ZERO, 0.24), (Vec2::X, 0.12), (-Vec2::X, 0.12), (Vec2::Y, 0.12), (-Vec2::Y, 0.12),
        (Vec2::new(1.0, 1.0), 0.07), (Vec2::new(-1.0, 1.0), 0.07),
        (Vec2::new(1.0, -1.0), 0.07), (Vec2::new(-1.0, -1.0), 0.07),
    ];
    if shadow.blur <= 0.0 {
        draw(shadow.offset * scale, shadow.color.gamma_multiply(opacity));
    } else {
        for (offset, weight) in taps {
            draw((shadow.offset + offset * shadow.blur * 0.55) * scale, shadow.color.gamma_multiply(opacity * weight));
        }
    }
}

/// A box whose front face is `view`. With a `tint` it is one colour, without it carries `texture`.
fn draw_cube(p: &Painter, view: View, texture: egui::TextureId, depth: f32, angles: [f32; 3], tint: Option<Color32>) {
    let bounds = view.bounds;
    let half = bounds.size() * 0.5;
    let half_depth = depth.max(0.0) * 0.5;
    let distance = 8.0 * half.x.max(half.y).max(half_depth);
    let [(sx, cx), (sy, cy), (sz, cz)] = angles.map(f32::sin_cos);
    let corners = [
        [-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0],
        [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
    ].map(|[x, y, z]| {
        let (x, y, z) = (x * half.x, y * half.y, z * half_depth);
        let (y, z) = (cx * y - sx * z, sx * y + cx * z);
        let (x, z) = (cy * x + sy * z, -sy * x + cy * z);
        let (x, y) = (cz * x - sz * y, sz * x + cz * y);
        bounds.center() + Vec2::new(x, y) * ((distance - half_depth) / (distance - z))
    });
    let faces = [[0, 1, 2, 3], [1, 5, 6, 2], [5, 4, 7, 6], [4, 0, 3, 7], [4, 5, 1, 0], [3, 2, 6, 7]];
    let uv = if tint.is_some() { [egui::epaint::WHITE_UV; 4] }
        else { [egui::pos2(0.0, 0.0), egui::pos2(1.0, 0.0), egui::pos2(1.0, 1.0), egui::pos2(0.0, 1.0)] };
    let mut mesh = egui::Mesh::with_texture(if tint.is_some() { egui::TextureId::default() } else { texture });
    let color = tint.unwrap_or(Color32::WHITE).gamma_multiply(view.opacity);
    for face in faces {
        let pts = face.map(|i| corners[i]);
        let a = pts[1] - pts[0];
        let b = pts[2] - pts[0];
        if a.x * b.y - a.y * b.x <= 0.0 { continue; }
        let first = mesh.vertices.len() as u32;
        mesh.vertices.extend(pts.into_iter().zip(uv).map(|(pos, uv)| egui::epaint::Vertex { pos, uv, color }));
        mesh.add_triangle(first, first + 1, first + 2);
        mesh.add_triangle(first, first + 2, first + 3);
    }
    p.add(egui::Shape::mesh(mesh));
}

#[derive(Clone, Copy)]
struct View { bounds: Rect, opacity: f32 }

/// A brush's plain colour. GraphicsBrush::.ctor (0x0600032c) defaults Type to Color (1);
/// GraphicsBrushType is Transparent, Color, LinearGradient, RadialGradient, Bitmap, and a
/// Transparent brush paints nothing whatever its Color says.
fn brush_color(node: Option<Xml<'_, '_>>, fallback: Color32) -> Color32 {
    match node {
        Some(node) if matches!(node.attribute("Type"), Some("Transparent" | "0")) => Color32::TRANSPARENT,
        Some(node) => color(node.attribute("Color"), fallback),
        None => fallback,
    }
}

fn parse_brush(node: Option<Xml<'_, '_>>, fallback: Color32) -> Brush {
    let Some(node) = node else { return Brush::Solid(fallback) };
    if matches!(node.attribute("Type"), Some("Transparent" | "0")) { return Brush::Solid(Color32::TRANSPARENT); }
    if let Some(source) = child(node, "Brush.Bitmap").and_then(|n| child(n, "Bitmap")).and_then(|n| n.attribute("Source")) {
        return Brush::Bitmap(source.into());
    }
    if let Some(stops) = child(node, "Brush.Stops") {
        // GraphicsGradientStop::.ctor (0x06000429): a stop without attributes is Position 0,
        // Color Transparent — not the brush's own colour.
        let mut stops: Vec<_> = stops.children().filter(|n| n.has_tag_name("GradientStop")).map(|n| {
            (number(n.attribute("Position"), 0.0).clamp(0.0, 1.0), color(n.attribute("Color"), Color32::TRANSPARENT))
        }).collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        if !stops.is_empty() {
            return Brush::Linear {
                start: pair(node.attribute("StartPoint"), Vec2::ZERO),
                end: pair(node.attribute("EndPoint"), Vec2::Y),
                stops,
                wrap: Wrap::parse(node.attribute("WrapX")),
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
    /// GraphicsEngine GraphicsAnimationHelper (Interpolate 0x0600076b): vMix's "CubicEasingIn"
    /// is 1 − (1 − t)³ (0x0600076f) and "CubicEasingOut" is t³ (0x06000770), the reverse of the
    /// usual names; BounceIn is `Bounce` (0x0600076c), BounceOut is 1 − Bounce(1 − t).
    fn apply(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::In => 1.0 - (1.0 - t).powi(3),
            Self::Out => t.powi(3),
            Self::InOut if t < 0.5 => 4.0 * t.powi(3),
            Self::InOut => 1.0 - (-2.0 * t + 2.0).powi(3) * 0.5,
            Self::BounceIn => bounce(t),
            Self::BounceOut => 1.0 - bounce(1.0 - t),
        }
    }
}

/// GraphicsAnimationHelper::Bounce (0x0600076c @0x1d620): 3 bounces with bounciness 2, each
/// half as high as the one before, ending at 1.
fn bounce(t: f32) -> f32 {
    let (bounces, b) = (3.0_f64, 2.0_f64);
    let pow = b.powf(bounces);
    let one_minus_b = 1.0 - b;
    let units = (1.0 - pow) / one_minus_b + pow * 0.5;
    let at = f64::from(t.clamp(0.0, 1.0)) * units;
    let index = (-at * one_minus_b + 1.0).log(b).floor();
    let start = (1.0 - b.powf(index)) / (one_minus_b * units);
    let end = (1.0 - b.powf(index + 1.0)) / (one_minus_b * units);
    let middle = (start + end) * 0.5;
    let peak = f64::from(t.clamp(0.0, 1.0)) - middle;
    let radius = middle - start;
    let amplitude = (1.0 / b).powf(bounces - index);
    (-amplitude / (radius * radius) * (peak - radius) * (peak + radius)) as f32
}

fn parse_storyboard(xml: Xml<'_, '_>, board: &mut Storyboard, unsupported: &mut BTreeSet<String>) {
    let Some(list) = child(xml, "Storyboard.Animations") else { return };
    for animation in list.children().filter(Node::is_element) {
        let delay = number(animation.attribute("Delay"), 0.0).clamp(0.0, 60.0);
        let duration = number(animation.attribute("Duration"), 1.0).clamp(0.0, 60.0);
        let effect = match animation.tag_name().name() {
            "Fade" => Motion::Fade, "Fly" => Motion::Fly, "Bounce" => Motion::Bounce, "Reveal" => Motion::Reveal,
            "Expand" => Motion::Expand, "Zoom" => Motion::Zoom, "ZoomFade" => Motion::ZoomFade,
            "Rotate" => Motion::Rotate, "Hidden" => Motion::Hidden, "Scroll" => Motion::Scroll,
            "None" => {
                if animation.attribute("Duration").is_some() {
                    board.seconds = board.seconds.max(delay + duration);
                }
                continue;
            }
            name => {
                unsupported.insert(format!("Animation {name}"));
                continue;
            }
        };
        let centered = animation.attribute("Direction") == Some("Center");
        let direction = match animation.attribute("Direction") {
            Some("Right") => Vec2::X, Some("Top") => -Vec2::Y, Some("Bottom") => Vec2::Y,
            Some("Center") if animation.attribute("CenterAxis") == Some("Y") => Vec2::Y,
            _ => -Vec2::X,
        };
        let ease = match animation.attribute("Interpolation") {
            Some("CubicEasingIn") => Ease::In, Some("CubicEasingOut") => Ease::Out,
            Some("CubicEasingInOut") => Ease::InOut, Some("BounceIn") => Ease::BounceIn,
            Some("BounceOut") => Ease::BounceOut, _ => Ease::Linear,
        };
        board.seconds = board.seconds.max(delay + duration);
        board.animations.entry(animation.attribute("Object").unwrap_or_default().into()).or_default()
            .push(Animation { effect, delay, duration: duration.max(0.001), direction, centered,
                reverse: animation.attribute("Reverse") == Some("True"), ease });
    }
}

fn reveal(bounds: Rect, direction: Vec2, centered: bool, shown: f32) -> Rect {
    if centered {
        let scale = if direction.x != 0.0 { Vec2::new(shown, 1.0) } else { Vec2::new(1.0, shown) };
        return Rect::from_center_size(bounds.center(), bounds.size() * scale);
    }
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
    let name = format!("gt:{}:{}:{}:{:?}", text.family, text.weight, text.italic, text.stretch);
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
            families: &families, weight: fontdb::Weight(text.weight), stretch: text.stretch,
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

fn draw_text(p: &Painter, view: View, text: &Text, value: &str, scale: f32, shadow: Option<Shadow>, skew: Vec2,
    mirror: [bool; 2], rotation: f32)
{
    if value.is_empty() || scale <= 0.0 { return; }
    let cased;
    let value = match text.case {
        TextCase::Normal => value,
        TextCase::Upper => { cased = value.to_uppercase(); cased.as_str() }
        TextCase::Lower => { cased = value.to_lowercase(); cased.as_str() }
    };
    let rect = view.bounds;
    // A turned text box clips to the area it covers on screen; the glyphs turn about its centre.
    let p = p.with_clip_rect(turned_bounds(rect, rotation));
    let turn = turn(rect.center(), rotation);
    let place = |pos: Pos2, galley: std::sync::Arc<egui::Galley>, color: Color32| {
        if skew != Vec2::ZERO || mirror != [false; 2] { p.add(skewed_text(&p, &galley, pos, color, skew, mirror, rect.center(), &turn)); }
        else if rotation == 0.0 { p.galley(pos, galley, color); }
        else { p.add(egui::epaint::TextShape::new(turn(pos), galley, color).with_angle(rotation)); }
    };
    let family = title_font(&p, text);
    let wrap = if text.wrap { rect.width() } else { f32::INFINITY };
    let layout = |size| {
        let mut job = egui::text::LayoutJob::simple(value.to_owned(), egui::FontId::new(size, family.clone()),
            Color32::PLACEHOLDER, wrap);
        job.sections[0].format.extra_letter_spacing = text.spacing * size / text.size;
        p.layout_job(job)
    };
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
    if let Some(shadow) = shadow {
        shadow_samples(shadow, Vec2::splat(scale), view.opacity, |offset, color| {
            place(pos + offset, galley.clone(), color);
        });
    }
    if text.thickness > 0.0 && text.stroke != Color32::TRANSPARENT {
        let stroke = text.stroke.gamma_multiply(view.opacity);
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::TAU / 8.0;
            place(pos + Vec2::new(a.cos(), a.sin()) * text.thickness * scale, galley.clone(), stroke);
        }
    }
    place(pos, galley, text.color.gamma_multiply(view.opacity));
}

/// Loads every `.gtzip` under LIVEMIX_GT_FIXTURES and prints how many titles use each feature
/// LiveMix can't draw yet, most common first.
#[test]
#[ignore = "requires operator-supplied GT title fixtures"]
fn report_unsupported_gt_features() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("LIVEMIX_GT_FIXTURES").expect("Set LIVEMIX_GT_FIXTURES"),
    );
    let mut pending = vec![directory];
    let (mut titles, mut failed) = (0, Vec::new());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut example: BTreeMap<String, String> = BTreeMap::new();
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() { pending.push(path); continue; }
            if path.extension().and_then(|e| e.to_str()) != Some("gtzip") { continue; }
            titles += 1;
            match Document::load(&path) {
                Ok(document) => {
                    fn turned(objects: &[Object]) -> usize {
                        objects.iter().map(|o| usize::from(o.rotation != 0.0)
                            + if let ObjectKind::Layer { objects, .. } = &o.kind { turned(objects) } else { 0 }).sum()
                    }
                    let count = turned(&document.objects);
                    if count >= 3 && document.unsupported.is_empty() { println!("rotated ×{count}: {}", path.display()); }
                    for feature in document.unsupported {
                        example.entry(feature.clone()).or_insert_with(|| path.display().to_string());
                        *counts.entry(feature).or_default() += 1;
                    }
                }
                Err(error) => failed.push(format!("{}: {error}", path.display())),
            }
        }
    }
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    println!("{titles} titles, {} could not load", failed.len());
    for (feature, count) in counts { println!("{count:5}  {feature}  (e.g. {})", example[&feature]); }
    for line in failed.iter().take(20) { println!("failed: {line}"); }
}

#[test]
#[ignore = "requires operator-supplied GT title fixtures"]
fn report_priority_gt_templates() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("LIVEMIX_GT_FIXTURES").expect("Set LIVEMIX_GT_FIXTURES"),
    );
    for name in [
        "GT Title/Spinner 180.gtzip",
        "GT Stingers/Stinger 2- Havin a blinder Blue.gtzip",
        "GT ScoreBoard/Scoreboard 3- Lineup 12.gtzip",
        "GT ScoreBoard/Scoreboard 13- The Extreme.gtzip",
        "GT Social/Social 1- Kimono.gtzip",
    ] {
        let document = Document::load(&directory.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
        println!("{name}: {} fields, unsupported: {:?}", document.fields.len(), document.unsupported);
    }
}

#[test]
fn transform_rotates_about_the_object_centre() {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("livemix-gt-rotate-{}.gtzip", std::process::id()));
    {
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        zip.start_file("document.xml", zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)).unwrap();
        zip.write_all(br#"<Composition Dimensions="1920,1080">
            <Rectangle Name="Bar" Location="100,100" Dimensions="200,100">
              <Rectangle.Transform><Transform Rotate="0,0,1.570796" /></Rectangle.Transform>
            </Rectangle>
            <Rectangle Name="Tilt" Dimensions="10,10">
              <Rectangle.Transform><Transform Rotate="0.5,0,0" Scale="2,2,1" /></Rectangle.Transform>
            </Rectangle>
          </Composition>"#).unwrap();
        zip.finish().unwrap();
    }
    let document = Document::load(&path).unwrap();
    assert!((document.objects[0].rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
    assert!(!document.unsupported.contains("Transform"));
    assert!(!document.unsupported.contains("Transform 3D rotation"));
    assert!(document.approximated.iter().any(|note| note.starts_with("3D rotation")));
    assert!(document.unsupported.contains("Transform scale, pan or origin"));
    // A quarter turn swaps the box's width and height about the same centre.
    let turned = turned_bounds(Rect::from_min_size(Pos2::new(100.0, 100.0), Vec2::new(200.0, 100.0)), std::f32::consts::FRAC_PI_2);
    assert!((turned.center() - Pos2::new(200.0, 150.0)).length() < 1e-3);
    assert!((turned.size() - Vec2::new(100.0, 200.0)).length() < 1e-3);
    // Clockwise on screen: the right side moves down.
    let point = turn(Pos2::ZERO, std::f32::consts::FRAC_PI_2)(Pos2::new(1.0, 0.0));
    assert!((point - Pos2::new(0.0, 1.0)).length() < 1e-5);
}

#[test]
fn gt_report_lists_unsupported_features_and_ignored_attributes_per_file() {
    use std::io::Write as _;
    let folder = std::env::temp_dir().join(format!("livemix-gt-report-{}", std::process::id()));
    let nested = folder.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    let package = |path: &Path, document: &str| {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        zip.start_file("document.xml", zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(document.as_bytes()).unwrap();
        zip.finish().unwrap();
    };
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/examples/timing-tower/timing-tower.gtzip"),
        folder.join("clean.gtzip"),
    ).unwrap();
    package(&nested.join("problems.gtzip"), r##"<Composition Width="640" Height="360">
        <Rectangle Name="Box" Location="0,0" Dimensions="100,50" Glow="3">
            <Rectangle.Fill><Brush Color="#ff0000"/></Rectangle.Fill>
            <Rectangle.Mask><Mask/></Rectangle.Mask>
            <Rectangle.Effects><Effect Type="Glow"/></Rectangle.Effects>
        </Rectangle>
        <TextBlock Name="Label" Text="x" Location="0,60" Dimensions="100,30" LineSpacing="2"><TextBlock.Highlight/></TextBlock>
        <Video Name="Clip" Location="0,0" Dimensions="10,10"/>
        <Layer Name="Empty" Location="0,0" Dimensions="10,10"/>
        <Storyboard Type="Wobble"><Storyboard.Animations><Spin Object="Box"/></Storyboard.Animations></Storyboard>
    </Composition>"##);
    std::fs::write(folder.join("broken.gtzip"), b"this is not a zip file").unwrap();
    std::fs::write(folder.join("notes.txt"), b"ignored").unwrap();

    let text = report(&folder).unwrap();
    assert!(text.contains("3 files"), "{text}");
    assert!(text.contains("OK   clean.gtzip  1920x1080"), "{text}");
    assert!(text.lines().any(|line| line.starts_with("FAIL broken.gtzip")), "{text}");
    let warning = text.lines().position(|line| line.starts_with("WARN") && line.contains("problems.gtzip")).expect(&text);
    let details: Vec<&str> = text.lines().skip(warning + 1).take(2).collect();
    for feature in ["Effect Glow", "Layer without composition", "Mask", "Storyboard Wobble", "Video"] {
        assert!(details[0].contains(feature), "{feature} missing: {text}");
    }
    for ignored in ["Rectangle.Glow attribute", "TextBlock.LineSpacing attribute", "TextBlock.Highlight property"] {
        assert!(details[1].contains(ignored), "{ignored} missing: {text}");
    }
    assert!(!text.contains("clean.gtzip\n       "), "the generated tower has nothing to report: {text}");
    assert!(text.contains("1 OK, 1 with unsupported features, 1 failed"), "{text}");
    assert!(text.contains("Unsupported features by number of files:\n     1  Effect Glow"), "{text}");
    // One file by path works too, and a missing path or an empty folder is an error.
    assert!(report(&folder.join("clean.gtzip")).unwrap().contains("1 file)"));
    assert!(report(&folder.join("missing")).is_err());
    std::fs::create_dir_all(folder.join("empty")).unwrap();
    assert!(report(&folder.join("empty")).is_err());
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn easing_matches_the_graphics_engine() {
    // vMix's names: CubicEasingIn starts fast, CubicEasingOut starts slow.
    assert!((Ease::In.apply(0.5) - 0.875).abs() < 1e-6);
    assert!((Ease::Out.apply(0.5) - 0.125).abs() < 1e-6);
    // Bounce ends on 1, starts at 0 and touches down between bounces.
    assert!(bounce(0.0).abs() < 1e-5 && (bounce(1.0) - 1.0).abs() < 1e-5);
    assert!((Ease::BounceOut.apply(0.0)).abs() < 1e-5 && (Ease::BounceOut.apply(1.0) - 1.0).abs() < 1e-5);
    assert!((0..=100).all(|i| (-1e-4..=1.0 + 1e-4).contains(&bounce(i as f32 / 100.0))));
    // BounceIn's touch-downs are where a new bounce starts: 1/11, 3/11 and 7/11 of the way.
    for touch in [1.0 / 11.0, 3.0 / 11.0, 7.0 / 11.0] {
        assert!(bounce(touch).abs() < 1e-4, "{touch}");
    }
}

#[test]
fn gradient_wrap_modes_and_default_stops_follow_the_brush() {
    assert_eq!(Wrap::parse(None), Wrap::Mirror);
    assert_eq!(Wrap::parse(Some("Wrap")), Wrap::Repeat);
    assert_eq!(Wrap::parse(Some("Clamp")), Wrap::Clamp);
    assert!((Wrap::Repeat.apply(1.25) - 0.25).abs() < 1e-6);
    assert!((Wrap::Mirror.apply(1.25) - 0.75).abs() < 1e-6);
    assert_eq!(Wrap::Clamp.apply(1.25), 1.0);
    // A repeated gradient restarts at every whole period; a clamped one only cuts inside 0…1.
    let stops = [(0.0, Color32::BLACK), (0.5, Color32::WHITE), (1.0, Color32::BLACK)];
    assert!(Wrap::Repeat.cuts(&stops, 0.2, 2.8).contains(&1.5));
    assert!(!Wrap::Clamp.cuts(&stops, 0.2, 2.8).contains(&1.5));
    // Type="Transparent" paints nothing even with a Color (GraphicsBrushType.Transparent).
    let clear = roxmltree::Document::parse(r##"<Brush Type="Transparent" Color="#FFFFFFFF" />"##).unwrap();
    assert_eq!(brush_color(Some(clear.root_element()), Color32::WHITE), Color32::TRANSPARENT);
    assert!(matches!(parse_brush(Some(clear.root_element()), Color32::WHITE), Brush::Solid(c) if c == Color32::TRANSPARENT));
    // A stop with no Color is transparent (GraphicsGradientStop default), not the brush colour.
    let xml = roxmltree::Document::parse(r##"<Brush Type="LinearGradient" Color="#FFFF0000" WrapX="Wrap">
        <Brush.Stops><GradientStop /><GradientStop Position="1" Color="#FFFFFFFF" /></Brush.Stops></Brush>"##).unwrap();
    match parse_brush(Some(xml.root_element()), Color32::RED) {
        Brush::Linear { stops, wrap, .. } => {
            assert_eq!(stops[0], (0.0, Color32::TRANSPARENT));
            assert_eq!(wrap, Wrap::Repeat);
        }
        other => panic!("expected a gradient, got {other:?}"),
    }
}

#[test]
fn gradients_mirror_and_scroll_like_gt_brushes() {
    assert_eq!(mirror(0.25), 0.25);
    assert!((mirror(1.25) - 0.75).abs() < 1e-6);
    assert!((mirror(-0.25) - 0.25).abs() < 1e-6);
    let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 10.0));
    let project = gradient_axis(rect, Vec2::ZERO, Vec2::X, Vec2::ZERO).unwrap();
    assert!((project(Pos2::new(50.0, 5.0)) - 0.5).abs() < 1e-6);
    // An Offset of 0.25 (object widths) samples a quarter further along.
    let shifted = gradient_axis(rect, Vec2::ZERO, Vec2::X, Vec2::new(0.25, 0.0)).unwrap();
    assert!((shifted(Pos2::new(50.0, 5.0)) - 0.75).abs() < 1e-6);
    // Speed 0.3 to the left (the default direction) for 2 s, folded into one mirror period.
    let drift = Drift { velocity: Vec2::new(0.3, 0.0), delay: 0.0 };
    assert!((Drift::at(Some(drift), 2.0).x - 0.6).abs() < 1e-5);
    assert!((Drift::at(Some(drift), 10.0).x - 1.0).abs() < 1e-5);
}


/// Writes a title package with `document` and extra entries into `folder`, then loads it.
#[cfg(test)]
fn synthetic_title(folder: &Path, name: &str, document: &str, entries: &[(&str, &[u8])], roots: &[PathBuf]) -> Result<Document> {
    use std::io::Write as _;
    std::fs::create_dir_all(folder).unwrap();
    let path = folder.join(name);
    let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
    zip.start_file("document.xml", zip::write::SimpleFileOptions::default()).unwrap();
    zip.write_all(document.as_bytes()).unwrap();
    for (entry, bytes) in entries {
        zip.start_file(*entry, zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    Document::load_with(&path, roots)
}

#[cfg(test)]
fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let picture = image::RgbaImage::from_fn(width, height, |x, _| image::Rgba([if x == 0 { 255 } else { 0 }, 0, 0, 255]));
    image::DynamicImage::ImageRgba8(picture).write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
    bytes
}

#[cfg(test)]
fn drawn_shapes(document: &Document, texts: &BTreeMap<String, String>) -> Vec<egui::Shape> {
    let context = egui::Context::default();
    let output = context.run(Default::default(), |ctx| {
        let painter = ctx.layer_painter(egui::LayerId::background());
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(document.size.x, document.size.y));
        document.draw(&painter, rect, 1.0, texts, Cue::Rest, 0.0);
    });
    output.shapes.into_iter().map(|clipped| clipped.shape).collect()
}

#[test]
fn gt_draws_rounded_corners_text_effects_flipped_pictures_and_condensed_fonts() {
    let folder = std::env::temp_dir().join(format!("livemix-gt-looks-{}", std::process::id()));
    let document = synthetic_title(&folder, "looks.gtzip", r##"<Composition Width="400" Height="200">
        <Rectangle Name="Round" Location="0,0" Dimensions="200,100,0" Radius="20,20">
            <Rectangle.Fill><Brush Color="#ff0000"/></Rectangle.Fill></Rectangle>
        <Rectangle Name="Square" Location="210,0" Dimensions="50,50,0" Radius="-5,0">
            <Rectangle.Fill><Brush Color="#00ff00"/></Rectangle.Fill></Rectangle>
        <Rectangle Name="Pill" Location="270,0" Dimensions="100,40,0" Radius="500,500">
            <Rectangle.Fill><Brush Color="#0000ff"/></Rectangle.Fill></Rectangle>
        <TextBlock Name="Shout" Text="hello" TextEffect="Uppercase" FontStretch="Condensed" Location="0,110" Dimensions="150,40" FontSize="30"/>
        <TextBlock Name="Whisper" Text="QUIET" TextEffect="Lowercase" Location="0,150" Dimensions="150,40" FontSize="30"/>
        <TextBlock Name="Odd" Text="x" TextEffect="Sparkle" Location="160,150" Dimensions="40,40"/>
        <Image Name="Mirror" Location="210,60" Dimensions="40,40,0">
            <Image.Effects><Effect Type="FlipX"/></Image.Effects>
            <Image.Bitmap><Bitmap Source="red-left.png"/></Image.Bitmap></Image>
        <Image Name="Plain" Location="260,60" Dimensions="40,40,0">
            <Image.Bitmap><Bitmap Source="red-left.png"/></Image.Bitmap></Image>
    </Composition>"##, &[("red-left.png", png_bytes(2, 2).as_slice())], &[]).unwrap();
    assert_eq!(document.unsupported.iter().map(String::as_str).collect::<Vec<_>>(), ["Text effect Sparkle"]);
    assert_eq!(document.ignored.iter().map(String::as_str).collect::<Vec<_>>(), ["Rectangle.Radius negative"]);
    assert_eq!(parse_stretch(Some("Condensed")), fontdb::Stretch::Condensed);

    let mut texts = BTreeMap::new();
    texts.insert("Odd".to_owned(), "y".to_owned());
    let shapes = drawn_shapes(&document, &texts);
    let outlines: Vec<Vec<Pos2>> = shapes.iter().filter_map(|shape| match shape {
        egui::Shape::Path(path) => Some(path.points.clone()),
        _ => None,
    }).collect();
    let [round, square, pill] = [&outlines[0], &outlines[1], &outlines[2]];
    assert_eq!(square.len(), 4, "a negative radius keeps the square corners");
    assert!(round.len() > 30 && !round.iter().any(|point| (point.x - 0.0).abs() < 0.01 && point.y.abs() < 0.01),
        "the corner of a rounded rectangle is cut off");
    let right_edge = round.iter().map(|point| point.x).fold(f32::MIN, f32::max);
    assert!((right_edge - 200.0).abs() < 0.01, "and the shape still fills its box: {right_edge}");
    let (top, bottom) = pill.iter().fold((f32::MAX, f32::MIN), |(lo, hi), point| (lo.min(point.y), hi.max(point.y)));
    assert!((bottom - top - 40.0).abs() < 0.01, "an oversized radius is limited to half the box");

    let words: Vec<String> = shapes.iter().filter_map(|shape| match shape {
        egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
        _ => None,
    }).collect();
    assert!(words.contains(&"HELLO".to_owned()) && words.contains(&"quiet".to_owned()), "{words:?}");
    assert!(words.contains(&"y".to_owned()), "an unknown text effect leaves the text as it is: {words:?}");

    // The picture is red on its left half. FlipX swaps the texture coordinates of the left and right edges.
    let corners: Vec<(f32, f32)> = shapes.iter().filter_map(|shape| match shape {
        egui::Shape::Mesh(mesh) if mesh.vertices.len() == 4 => {
            let left = mesh.vertices.iter().find(|vertex| vertex.pos.x < 255.0 && vertex.pos.y < 100.0)?;
            Some((left.pos.x, left.uv.x))
        }
        _ => None,
    }).collect();
    let (mirrored, plain): (Vec<_>, Vec<_>) = corners.iter().partition(|(x, _)| *x < 255.0);
    assert!(mirrored.iter().all(|(_, uv)| *uv == 1.0) && !mirrored.is_empty(), "{corners:?}");
    assert!(plain.is_empty() || plain.iter().all(|(_, uv)| *uv == 0.0), "{corners:?}");
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn gt_loads_textures_the_package_lacks_only_from_trusted_places() {
    let folder = std::env::temp_dir().join(format!("livemix-gt-textures-{}", std::process::id()));
    let library = folder.join("library");
    std::fs::create_dir_all(library.join("roof")).unwrap();
    std::fs::write(library.join("roof").join("Brown.png"), png_bytes(2, 2)).unwrap();
    std::fs::write(folder.join("next-to-title.png"), png_bytes(2, 2)).unwrap();
    std::fs::write(folder.join("secret.png"), png_bytes(2, 2)).unwrap();
    let title = folder.join("titles");
    let image = |name: &str, source: &str| format!(
        r#"<Image Name="{name}" Location="0,0" Dimensions="10,10,0"><Image.Bitmap><Bitmap Source="{source}"/></Image.Bitmap></Image>"#);
    let document = format!(r#"<Composition Width="100" Height="100">{}</Composition>"#, [
        image("Library", r"C:\Program Files (x86)\vMix\GT\textures\roof\Brown.png"),
        image("Beside", "next-to-title.png"),
        image("Placeholder", r"1f0c\Clear Image.png"),
        image("Up", r"..\secret.png"),
        image("Elsewhere", r"C:\Users\someone\Pictures\photo.png"),
        image("Script", r"C:\Program Files (x86)\vMix\GT\textures\roof\run.exe"),
    ].concat());
    std::fs::create_dir_all(&title).unwrap();
    std::fs::write(title.join("next-to-title.png"), png_bytes(2, 2)).unwrap();
    let loaded = synthetic_title(&title, "t.gtzip", &document, &[], &[library.clone()]).unwrap();
    let mut keys: Vec<&str> = loaded.assets.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, [r"1f0c\Clear Image.png", r"C:\Program Files (x86)\vMix\GT\textures\roof\Brown.png", "next-to-title.png"],
        "the library texture, the picture beside the title and the placeholder load");
    assert_eq!(loaded.unsupported.iter().map(String::as_str).collect::<Vec<_>>(), ["Missing or external bitmap"],
        "`..`, a random absolute path and a non-picture are refused");
    // Without the library the texture is reported as missing, as before.
    let without = synthetic_title(&title, "t.gtzip", &document, &[], &[]).unwrap();
    assert!(!without.assets.contains_key(r"C:\Program Files (x86)\vMix\GT\textures\roof\Brown.png"));
    std::fs::remove_dir_all(folder).unwrap();
}


#[test]
fn gt_tickers_scroll_their_items_in_a_loop_in_both_kinds_and_directions() {
    let folder = std::env::temp_dir().join(format!("livemix-gt-ticker-{}", std::process::id()));
    let document = synthetic_title(&folder, "ticker.gtzip", r##"<Composition Width="1000" Height="200">
        <Ticker Name="News" Location="0,0,0" Dimensions="400,40,0" FontFamily="Arial" FontSize="20" Speed="3"
                VerticalAlign="Center" TextWordWrapping="NoWrap" Type="Add">
            <Ticker.Fill><Brush Color="#FFFFFFFF"/></Ticker.Fill></Ticker>
        <Ticker Name="Feed" Location="500,0,0" Dimensions="200,150,0" Speed="2" Direction="Top">
            <Ticker.Template>
                <Layer Name="Card" Location="0,0,0" Dimensions="200,60,0"><Layer.Composition>
                    <Composition Width="200" Height="60">
                        <TextBlock Name="Who" Text="Who" Location="0,0,0" Dimensions="200,20,0"/>
                        <TextBlock Name="Msg" Text="Msg" Location="0,20,0" Dimensions="200,40,0"/>
                    </Composition></Layer.Composition></Layer>
            </Ticker.Template></Ticker>
        <Ticker Name="Odd" Location="0,100,0" Dimensions="100,40,0" Type="Replace"/>
    </Composition>"##, &[], &[]).unwrap();
    assert_eq!(document.unsupported.iter().map(String::as_str).collect::<Vec<_>>(), ["Ticker type Replace"]);
    assert!(document.ignored.is_empty(), "{:?}", document.ignored);
    assert_eq!(document.fields.get("News").map(String::as_str), Some("Ticker text"));
    assert_eq!(document.fields.get("Feed").map(String::as_str), Some("Who|Msg"));
    assert!(!document.fields.contains_key("Who") && !document.fields.contains_key("Msg"), "template fields stay inside the items");

    let mut texts = BTreeMap::new();
    texts.insert("News".to_owned(), "ALPHA\nBETA".to_owned());
    texts.insert("Feed".to_owned(), "Ann|Hello\nBob|World\nCy|Yo".to_owned());
    let at = |t: f64| -> Vec<(String, Pos2)> {
        let context = egui::Context::default();
        let output = context.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 200.0));
            document.draw(&painter, rect, 1.0, &texts, Cue::Rest, t);
        });
        output.shapes.into_iter().filter_map(|clipped| match clipped.shape {
            egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.pos)),
            _ => None,
        }).collect()
    };
    let find = |shapes: &[(String, Pos2)], word: &str| shapes.iter().find(|(text, _)| text == word).map(|(_, pos)| *pos);

    // A crawl: items queue behind the right edge, then move left 3 units a frame (180 a second).
    // The loop is always running, so the end of the previous lap is already on screen at t = 0.
    assert!((find(&at(0.0), "ALPHA").unwrap().x - 0.0).abs() < 0.5);
    let (one, two) = (at(1.0), at(2.0));
    let (alpha1, alpha2) = (find(&one, "ALPHA").unwrap(), find(&two, "ALPHA").unwrap());
    assert!((alpha1.x - 220.0).abs() < 0.5 && (alpha2.x - 40.0).abs() < 0.5, "{alpha1:?} {alpha2:?}");
    let beta = find(&two, "BETA").expect("the next item follows the first");
    assert!(beta.x > alpha2.x + 20.0 && beta.x < 400.0, "{beta:?}");
    for t in [37.3, 100.0, 250.9] {
        let shapes = at(t);
        assert!(find(&shapes, "ALPHA").is_some() || find(&shapes, "BETA").is_some(), "the loop never runs dry at {t}");
    }

    // A vertical feed of template items moving up 2 units a frame: one card of 60 is 0.5 s.
    let feed = at(1.0);
    let (ann, bob) = (find(&feed, "Ann").unwrap(), find(&feed, "Bob").unwrap());
    assert!((ann.y - 30.0).abs() < 0.5 && (bob.y - 90.0).abs() < 0.5, "{ann:?} {bob:?}");
    assert!((ann.x - 500.0).abs() < 0.5, "the cards sit in the ticker's own column");
    assert!(find(&feed, "Hello").is_some() && find(&feed, "Cy").is_none(), "the third card is still outside");
    assert!(find(&feed, "Who").is_none(), "the template's sample text is replaced");
    // With no text for the field the sample item is shown, so a new ticker is not blank.
    let empty = BTreeMap::new();
    let context = egui::Context::default();
    let output = context.run(Default::default(), |ctx| {
        let painter = ctx.layer_painter(egui::LayerId::background());
        document.draw(&painter, Rect::from_min_size(Pos2::ZERO, egui::vec2(1000.0, 200.0)), 1.0, &empty, Cue::Rest, 1.0);
    });
    let words: Vec<String> = output.shapes.into_iter().filter_map(|clipped| match clipped.shape {
        egui::Shape::Text(text) => Some(text.galley.text().to_owned()), _ => None,
    }).collect();
    assert!(words.contains(&"Ticker".to_owned()) || words.contains(&"text".to_owned()) || words.iter().any(|word| word.contains("Ticker")), "{words:?}");
    std::fs::remove_dir_all(folder).unwrap();
}


#[test]
fn gt_draws_qr_codes_skewed_text_and_pictures_mirrored_shapes_and_masks() {
    let folder = std::env::temp_dir().join(format!("livemix-gt-more-{}", std::process::id()));
    let document = synthetic_title(&folder, "more.gtzip", r##"<Composition Width="600" Height="400">
        <QRCode Name="Code" Location="0,0,0" Dimensions="100,100,0" Text="livemix.test"/>
        <TextBlock Name="Lean" Text="WIDE" Location="120,0,0" Dimensions="200,50,0" FontSize="40">
            <TextBlock.Effects><Effect Type="Skew" Angle="0,-30"/></TextBlock.Effects></TextBlock>
        <TextBlock Name="Upright" Text="WIDE" Location="120,60,0" Dimensions="200,50,0" FontSize="40"/>
        <Image Name="Photo" Location="330,0,0" Dimensions="100,50,0">
            <Image.Effects><Effect Type="Skew" Angle="20,0"/></Image.Effects>
            <Image.Bitmap><Bitmap Source="red.png"/></Image.Bitmap></Image>
        <RightTriangle Name="Corner" Location="0,120,0" Dimensions="200,100,0">
            <RightTriangle.Transform><Transform Rotate="0,3.141593,0"/></RightTriangle.Transform>
            <RightTriangle.Fill><Brush Color="#ff0000"/></RightTriangle.Fill></RightTriangle>
        <RightTriangle Name="Tilted" Location="220,120,0" Dimensions="50,50,0">
            <RightTriangle.Transform><Transform Rotate="0.5,0,0"/></RightTriangle.Transform></RightTriangle>
        <Rectangle Name="Hole" Location="100,260,0" Dimensions="50,50,0"/>
        <Rectangle Name="Big" Location="0,240,0" Dimensions="400,150,0">
            <Rectangle.Mask><Mask Object="Hole"/></Rectangle.Mask>
            <Rectangle.Fill><Brush Color="#00ff00"/></Rectangle.Fill></Rectangle>
        <Rectangle Name="Lost" Location="500,0,0" Dimensions="10,10,0"><Rectangle.Mask><Mask Object="Nowhere"/></Rectangle.Mask></Rectangle>
    </Composition>"##, &[("red.png", png_bytes(2, 2).as_slice())], &[]).unwrap();
    assert!(document.unsupported.is_empty(), "{:?}", document.unsupported);
    assert_eq!(document.approximated.iter().map(String::as_str).collect::<Vec<_>>(),
        ["3D rotation (the object is squashed, without perspective)", "Mask (limited to the area of the mask object)"]);
    assert_eq!(document.ignored.iter().map(String::as_str).collect::<Vec<_>>(), ["Mask object missing (nothing to limit the object)"]);
    assert_eq!(document.fields.get("Code").map(String::as_str), Some("livemix.test"));

    let context = egui::Context::default();
    let output = context.run(Default::default(), |ctx| {
        let painter = ctx.layer_painter(egui::LayerId::background());
        document.draw(&painter, Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0)), 1.0, &BTreeMap::new(), Cue::Rest, 0.0);
    });
    let shapes = output.shapes;

    // The QR code: a white square and a mesh of black modules, all inside its 100 x 100 area.
    let modules = shapes.iter().filter_map(|clipped| match &clipped.shape {
        egui::Shape::Mesh(mesh) if mesh.vertices.len() > 200 && mesh.vertices.iter().all(|v| v.pos.x <= 100.0 && v.pos.y <= 100.0) => Some(mesh.clone()),
        _ => None,
    }).next().expect("the QR code is drawn");
    assert!(modules.vertices.iter().all(|vertex| vertex.color == Color32::BLACK));
    let (min, max) = modules.vertices.iter().fold((f32::MAX, f32::MIN), |(lo, hi), vertex| (lo.min(vertex.pos.x), hi.max(vertex.pos.x)));
    assert!(min >= 100.0 * 2.0 / 33.0 - 1.0 && max <= 100.0 - 100.0 * 2.0 / 33.0 + 1.0, "the margin of two modules stays white: {min} {max}");

    // Skewed text: its glyph triangles lean, so height depends on position along the line.
    let slope = |points: &[Pos2]| {
        let n = points.len() as f32;
        let (mx, my) = (points.iter().map(|p| p.x).sum::<f32>() / n, points.iter().map(|p| p.y).sum::<f32>() / n);
        points.iter().map(|p| (p.x - mx) * (p.y - my)).sum::<f32>() / points.iter().map(|p| (p.x - mx).powi(2)).sum::<f32>()
    };
    let lean: Vec<Pos2> = shapes.iter().filter_map(|clipped| match &clipped.shape {
        egui::Shape::Mesh(mesh) if mesh.vertices.len() > 8 && mesh.vertices.iter().all(|v| v.pos.x >= 119.0 && v.pos.x < 330.0 && v.pos.y < 120.0) => {
            Some(mesh.vertices.iter().map(|v| v.pos).collect::<Vec<_>>())
        }
        _ => None,
    }).flatten().collect();
    assert!(lean.len() >= 16 && slope(&lean) < -0.4, "the text leans up to the right: {}", slope(&lean));
    assert!(shapes.iter().any(|clipped| matches!(&clipped.shape, egui::Shape::Text(text) if text.galley.text() == "WIDE")), "the upright text is a plain text shape");

    // A skewed picture: the left edge slants by tan(20 degrees) over its height.
    let photo = shapes.iter().find_map(|clipped| match &clipped.shape {
        egui::Shape::Mesh(mesh) if mesh.vertices.len() == 4 && mesh.vertices.iter().all(|v| v.pos.x > 300.0 && v.pos.y < 60.0) => Some(mesh.clone()),
        _ => None,
    }).expect("the picture is drawn");
    let left: Vec<Pos2> = photo.vertices.iter().map(|v| v.pos).filter(|pos| pos.x < 380.0).collect();
    let (top, bottom) = (*left.iter().min_by(|a, b| a.y.total_cmp(&b.y)).unwrap(), *left.iter().max_by(|a, b| a.y.total_cmp(&b.y)).unwrap());
    assert!((bottom.x - top.x - 50.0 * 20f32.to_radians().tan()).abs() < 1.0, "{top:?} {bottom:?}");

    // A half turn about Y mirrors the triangle: its right angle moves from the bottom left to the bottom right.
    let triangle = shapes.iter().find_map(|clipped| match &clipped.shape {
        egui::Shape::Path(path) if path.fill == Color32::RED => Some(path.points.clone()),
        _ => None,
    }).expect("the triangle is drawn");
    assert!(triangle.iter().any(|p| (p.x - 200.0).abs() < 0.01 && (p.y - 120.0).abs() < 0.01), "{triangle:?}");
    assert!(!triangle.iter().any(|p| p.x.abs() < 0.01 && (p.y - 120.0).abs() < 0.01), "{triangle:?}");

    // The masked rectangle may only show inside the mask object's area.
    let big = shapes.iter().find(|clipped| matches!(&clipped.shape, egui::Shape::Path(path) if path.fill == Color32::GREEN)).unwrap();
    assert_eq!(big.clip_rect, Rect::from_min_max(Pos2::new(100.0, 260.0), Pos2::new(150.0, 310.0)));

    let text = report(&folder.join("more.gtzip")).unwrap();
    assert!(text.contains("approximate: 3D rotation (the object is squashed, without perspective), Mask (limited to the area of the mask object)"), "{text}");
    assert!(text.contains("Approximated features by number of files:\n     1  3D rotation"), "{text}");
    assert!(text.contains("     1  Mask (limited"), "{text}");
    std::fs::remove_dir_all(folder).unwrap();
}


#[test]
fn gt_flat_rotation_is_not_reported_as_3d() {
    let folder = std::env::temp_dir().join(format!("livemix-gt-flat-{}", std::process::id()));
    let document = synthetic_title(&folder, "flat.gtzip", r##"<Composition Width="200" Height="100">
        <TextBlock Name="Turned" Text="x" Location="0,0,0" Dimensions="50,20,0">
            <TextBlock.Transform><Transform Rotate="0,0,0.5"/></TextBlock.Transform></TextBlock>
        <TextBlock Name="Flipped" Text="x" Location="0,30,0" Dimensions="50,20,0">
            <TextBlock.Transform><Transform Rotate="0,3.141593,0"/></TextBlock.Transform></TextBlock>
    </Composition>"##, &[], &[]).unwrap();
    assert!(document.unsupported.is_empty(), "a half turn about Y mirrors text; a turn about Z is plain rotation: {:?}", document.unsupported);
    assert!((document.objects[0].rotation - 0.5).abs() < 1e-6);
    assert!((document.objects[1].tilt[1] - std::f32::consts::PI).abs() < 1e-5, "{:?}", document.objects[1].tilt);
    assert!(document.approximated.is_empty(), "half turns are exact: {:?}", document.approximated);
    std::fs::remove_dir_all(folder).unwrap();
}


#[test]
fn gt_plays_pages_scrolls_brushes_cycles_sequences_and_draws_cubes_and_mirrored_text() {
    let folder = std::env::temp_dir().join(format!("livemix-gt-pages-{}", std::process::id()));
    let resources = br##"<resources><resource filename="logos\one.png">
        <source guid="g1">logos\one.png</source><source guid="g2">logos\two.png</source></resource></resources>"##;
    let (one, two) = (png_bytes(2, 2), png_bytes(3, 3));
    let document = synthetic_title(&folder, "pages.gtzip", r##"<Composition Width="600" Height="400">
        <Rectangle Name="Block" Location="0,0,0" Dimensions="100,100,60">
            <Rectangle.Geometry><Geometry Type="Cube"/></Rectangle.Geometry>
            <Rectangle.Fill><Brush Color="#FFFF8000"/></Rectangle.Fill></Rectangle>
        <Image Name="Logos" Location="120,0,0" Dimensions="50,50,0">
            <Image.Bitmap><Bitmap Source="logos\one.png"/></Image.Bitmap></Image>
        <Rectangle Name="Shimmer" Location="200,0,0" Dimensions="100,50,0">
            <Rectangle.Fill><Brush Type="LinearGradient" Color="#FFFFFFFF" StartPoint="0,0" EndPoint="1,0"><Brush.Stops>
                <GradientStop Color="#00FFFFFF"/><GradientStop Position="0.5" Color="#FFFFFFFF"/><GradientStop Position="1" Color="#00FFFFFF"/>
            </Brush.Stops></Brush></Rectangle.Fill></Rectangle>
        <TextBlock Name="Back" Text="L" Location="320,0,0" Dimensions="60,60,0" FontSize="40">
            <TextBlock.Transform><Transform Rotate="0,3.141593,0"/></TextBlock.Transform></TextBlock>
        <TextBlock Name="Plain" Text="L" Location="400,0,0" Dimensions="60,60,0" FontSize="40">
            <TextBlock.Effects><Effect Type="Skew" Angle="0,0.001"/></TextBlock.Effects></TextBlock>
        <Rectangle Name="Plate" Location="0,150,0" Dimensions="50,50,0" Style="Square"/>
        <Image Name="Pic" Location="60,150,0" Dimensions="20,20,0" SizeMode="Stretch" Locked="False" DataFlags="None">
            <Image.Bitmap><Bitmap Source="logos\one.png"/></Image.Bitmap></Image>
        <TextBlock Name="Quiet" Text="x" Location="100,150,0" Dimensions="20,20,0">
            <TextBlock.TextDecorations><TextDecoration/></TextBlock.TextDecorations></TextBlock>
        <TextBlock Name="Under" Text="x" Location="130,150,0" Dimensions="20,20,0">
            <TextBlock.TextDecorations><TextDecoration Location="Underline"/></TextBlock.TextDecorations></TextBlock>
        <Rectangle Name="Fit" Location="160,150,0" Dimensions="40,20,0">
            <Rectangle.Bounding><Bounding Object="Plate" Padding="1,1,1,1"/></Rectangle.Bounding></Rectangle>
        <Storyboard><Storyboard.Animations><Scroll Object="Shimmer" Duration="1" Direction="Right"/></Storyboard.Animations></Storyboard>
        <Storyboard Type="Page1"><Storyboard.Animations>
            <Fly Object="Block" Duration="0.5" Direction="Right"/><None Object="Plate"/></Storyboard.Animations></Storyboard>
        <Storyboard Type="Continuous"><Storyboard.Animations><ImageSequenceLoop Object="Logos"/></Storyboard.Animations></Storyboard>
    </Composition>"##, &[("resources.xml", resources.as_slice()), ("g1", one.as_slice()), ("g2", two.as_slice())], &[]).unwrap();
    assert!(document.unsupported.is_empty(), "{:?}", document.unsupported);
    assert_eq!(document.ignored.iter().map(String::as_str).collect::<Vec<_>>(), ["TextBlock.TextDecorations property"],
        "defaults and editor marks are known; a real decoration is not");
    assert_eq!(document.approximated.len(), 3, "{:?}", document.approximated);
    assert_eq!(document.pages(), [1]);
    assert_eq!(document.page_seconds(1), Some(0.5));
    assert_eq!(document.frames.get(r"logos\one.png"), Some(&2));

    // One context for every draw, so that each picture of a sequence gets its own texture id.
    let context = egui::Context::default();
    let draw = |cue: Cue<'_>, data: &[(Cue<'_>, &str)], t: f64| {
        let output = context.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0));
            document.draw_with(&painter, rect, 1.0, &BTreeMap::new(), cue, data, &HashMap::new(), t);
        });
        output.shapes.into_iter().map(|clipped| clipped.shape).collect::<Vec<_>>()
    };
    let meshes = |shapes: &[egui::Shape], inside: Rect| -> Vec<std::sync::Arc<egui::Mesh>> {
        shapes.iter().filter_map(|shape| match shape {
            egui::Shape::Mesh(mesh) if !mesh.vertices.is_empty() && mesh.vertices.iter().all(|v| inside.contains(v.pos)) => Some(mesh.clone()),
            _ => None,
        }).collect()
    };
    let area = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1)).expand(80.0);

    // A box shape: the front face of a cube, one colour, white texel of the font atlas.
    let rest = draw(Cue::Rest, &[], 0.0);
    let block = meshes(&rest, Rect::from_min_max(Pos2::new(-5.0, -5.0), Pos2::new(110.0, 110.0)));
    assert!(block.iter().any(|mesh| mesh.vertices.len() >= 4
        && mesh.vertices.iter().all(|v| v.color == Color32::from_rgb(255, 128, 0) && v.uv == egui::epaint::WHITE_UV)), "an orange box is drawn");

    // A page plays its storyboard: the block flies in from the right over half a second.
    let left = |shapes: &[egui::Shape]| meshes(shapes, area(0.0, 0.0, 100.0, 100.0)).iter()
        .flat_map(|mesh| mesh.vertices.iter().map(|v| v.pos.x)).fold(f32::MAX, f32::min);
    let halfway = draw(Cue::Rest, &[(Cue::Page(0.5, 1), "")], 0.0);
    assert!((left(&halfway) - left(&rest) - 50.0).abs() < 1.0, "{} {}", left(&halfway), left(&rest));
    assert!(draw(Cue::Rest, &[(Cue::Page(0.5, 9), "")], 0.0).len() == rest.len(), "a page that does not exist changes nothing");

    // Scroll moves the brush once over its duration while the title comes in, and rests afterwards.
    let shimmer = |shapes: &[egui::Shape]| meshes(shapes, Rect::from_min_max(Pos2::new(199.0, -1.0), Pos2::new(301.0, 51.0))).into_iter()
        .flat_map(|mesh| mesh.vertices.iter().map(|v| (v.pos.x as i32, v.color)).collect::<Vec<_>>()).collect::<Vec<_>>();
    let (resting, entering) = (shimmer(&rest), shimmer(&draw(Cue::In(0.5), &[], 0.0)));
    assert!(!resting.is_empty() && resting != entering, "the stripe has moved");
    assert_eq!(shimmer(&draw(Cue::In(0.0), &[], 0.0)), resting, "and starts where it rests");

    // An image sequence shows the next picture of its resource every second, and loops.
    let logo = |t: f64| {
        let shapes = draw(Cue::Rest, &[], t);
        meshes(&shapes, area(120.0, 0.0, 170.0, 50.0)).into_iter().find(|mesh| mesh.vertices.len() == 4 && mesh.vertices.iter().all(|v| v.pos.x > 110.0 && v.pos.x < 180.0 && v.pos.y < 60.0))
            .map(|mesh| mesh.texture_id)
    };
    let (first, second) = (logo(0.2).unwrap(), logo(1.2).unwrap());
    assert_ne!(first, second);
    assert_eq!(logo(2.2), Some(first), "after the last picture it starts over");

    // A half turn about Y mirrors text: the glyph's texture runs backwards along the line.
    let order = |inside: Rect, shapes: &[egui::Shape]| {
        let points: Vec<(f32, f32)> = meshes(shapes, inside).iter().flat_map(|mesh| mesh.vertices.iter().map(|v| (v.pos.x, v.uv.x)).collect::<Vec<_>>()).collect();
        let n = points.len() as f32;
        let (mx, mu) = (points.iter().map(|p| p.0).sum::<f32>() / n, points.iter().map(|p| p.1).sum::<f32>() / n);
        points.iter().map(|p| (p.0 - mx) * (p.1 - mu)).sum::<f32>()
    };
    assert!(order(area(320.0, 0.0, 380.0, 60.0).intersect(Rect::from_min_max(Pos2::new(300.0, -20.0), Pos2::new(390.0, 100.0))), &rest) < 0.0, "mirrored");
    assert!(order(Rect::from_min_max(Pos2::new(395.0, -20.0), Pos2::new(470.0, 100.0)), &rest) > 0.0, "not mirrored");

    // The same page and the shortcut that starts it, through an input.
    let mut switcher = crate::model::Switcher::empty();
    let id = switcher.add(crate::model::Input::new(0, "Titles", crate::model::Kind::GtTitle {
        path: folder.join("pages.gtzip").to_string_lossy().into_owned(), texts: Default::default(), document: None,
    }));
    switcher.input_mut(id).unwrap().reload_gt().unwrap();
    switcher.perform_action("TitleBeginAnimation 1 Page1", 12.5).unwrap();
    assert_eq!(switcher.input(id).unwrap().gt_page, Some((1, 12.5)));
    assert!(switcher.perform_action("TitleBeginAnimation 1 Page2", 13.0).is_err());
    assert!(switcher.perform_action("TitleBeginAnimation 1 In", 13.0).is_err());
    std::fs::remove_dir_all(folder).unwrap();
}
