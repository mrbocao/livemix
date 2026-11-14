//! Per-input luminance key and 3D LUTs.
use std::{io::Read, path::Path, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LumaKey {
    pub enabled: bool,
    pub threshold: u8,
    pub softness: u8,
    pub invert: bool,
}

impl Default for LumaKey {
    fn default() -> Self {
        Self { enabled: false, threshold: 64, softness: 24, invert: false }
    }
}

impl LumaKey {
    pub fn apply(self, pixels: &mut [u8]) {
        if !self.enabled { return; }
        for pixel in pixels.chunks_exact_mut(4) {
            let y = 0.2126 * f32::from(pixel[0]) + 0.7152 * f32::from(pixel[1]) + 0.0722 * f32::from(pixel[2]);
            let value = if self.invert { f32::from(self.threshold) - y } else { y - f32::from(self.threshold) };
            let alpha = if self.softness == 0 { f32::from(value >= 0.0) }
                else { (value / f32::from(self.softness)).clamp(0.0, 1.0) };
            pixel[3] = (f32::from(pixel[3]) * alpha).round() as u8;
        }
    }
}

#[derive(Debug)]
pub struct Lut {
    pub size: usize,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub cells: Vec<[f32; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct Effects {
    pub luma: LumaKey,
    pub lut_path: String,
    pub lut: Option<Arc<Lut>>,
}

impl PartialEq for Effects {
    fn eq(&self, other: &Self) -> bool {
        self.luma == other.luma && self.lut_path == other.lut_path
            && match (&self.lut, &other.lut) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl Effects {
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let path = std::path::absolute(path).map_err(|error| error.to_string())?;
        let lut = Lut::read(&path)?;
        self.lut_path = path.to_string_lossy().into_owned();
        self.lut = Some(Arc::new(lut));
        Ok(())
    }

    pub fn apply_lut(&self, pixels: &mut [u8]) {
        let Some(lut) = &self.lut else { return; };
        for pixel in pixels.chunks_exact_mut(4) {
            if pixel[3] == 0 { continue; }
            let rgb = lut.sample([pixel[0], pixel[1], pixel[2]].map(|v| f32::from(v) / 255.0));
            for (channel, value) in pixel.iter_mut().zip(rgb) {
                *channel = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
}

impl Lut {
    fn read(path: &Path) -> Result<Self, String> {
        if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("png")) {
            let mut reader = image::ImageReader::open(path).map_err(|error| error.to_string())?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(4225);
            limits.max_image_height = Some(4225);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode().map_err(|error| error.to_string())?.into_rgb8();
            Self::png(&image)
        } else if path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("cube")) {
            let mut text = String::new();
            std::fs::File::open(path).and_then(|file| file.take(16 * 1024 * 1024 + 1).read_to_string(&mut text))
                .map_err(|error| error.to_string())?;
            if text.len() > 16 * 1024 * 1024 { return Err("LUT is larger than 16 MiB".into()); }
            Self::cube(&text)
        } else {
            Err("Choose a .cube or PNG 3D LUT".into())
        }
    }

    fn png(image: &image::RgbImage) -> Result<Self, String> {
        let (w, h) = image.dimensions();
        let layout = (2u32..=65).find_map(|size| {
            if w % size != 0 || h % size != 0 || (w / size) * (h / size) != size { return None; }
            Some((size, w / size))
        }).ok_or("PNG LUT must be a strip or grid of square blue slices")?;
        let (size, columns) = layout;
        let mut cells = Vec::with_capacity((size * size * size) as usize);
        for blue in 0..size {
            for green in 0..size {
                for red in 0..size {
                    cells.push(image.get_pixel(blue % columns * size + red, blue / columns * size + green)
                        .0.map(|value| f32::from(value) / 255.0));
                }
            }
        }
        Ok(Self { size: size as usize, min: [0.0; 3], max: [1.0; 3], cells })
    }

    fn cube(text: &str) -> Result<Self, String> {
        let mut size = None;
        let mut min = [0.0; 3];
        let mut max = [1.0; 3];
        let mut cells = Vec::new();
        for (index, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            let mut fields = line.split('#').next().unwrap_or_default().split_whitespace();
            let Some(first) = fields.next() else { continue; };
            let invalid = || format!("Invalid 3D LUT line {}", index + 1);
            let triplet = |mut fields: std::str::SplitWhitespace<'_>| {
                let rgb = [0; 3].map(|_| fields.next().and_then(|value| value.parse::<f32>().ok()).filter(|v| v.is_finite()));
                match rgb { [Some(r), Some(g), Some(b)] if fields.next().is_none() => Some([r, g, b]), _ => None }
            };
            match first {
                "TITLE" => {}
                "LUT_3D_SIZE" => {
                    if size.is_some() { return Err(invalid()); }
                    let n = fields.next().and_then(|value| value.parse::<usize>().ok())
                        .filter(|value| (2..=65).contains(value)).ok_or_else(invalid)?;
                    size = Some(n);
                    cells.reserve(n * n * n);
                }
                "DOMAIN_MIN" => min = triplet(fields).ok_or_else(invalid)?,
                "DOMAIN_MAX" => max = triplet(fields).ok_or_else(invalid)?,
                _ => {
                    let n = size.ok_or_else(invalid)?;
                    if cells.len() >= n * n * n { return Err(invalid()); }
                    cells.push(triplet(line.split('#').next().unwrap_or_default().split_whitespace()).ok_or_else(invalid)?);
                }
            }
        }
        let size = size.ok_or("Missing LUT_3D_SIZE")?;
        if cells.len() != size * size * size || (0..3).any(|i| max[i] <= min[i] || !(max[i] - min[i]).is_finite()) {
            return Err("Invalid LUT domain or sample count".into());
        }
        Ok(Self { size, min, max, cells })
    }

    fn sample(&self, rgb: [f32; 3]) -> [f32; 3] {
        let limit = self.size - 1;
        let coordinate = std::array::from_fn::<_, 3, _>(|i| {
            ((rgb[i] - self.min[i]) / (self.max[i] - self.min[i])).clamp(0.0, 1.0) * limit as f32
        });
        let lower = coordinate.map(|v| v as usize);
        let fraction = std::array::from_fn::<_, 3, _>(|i| coordinate[i] - lower[i] as f32);
        let at = |r, g, b| self.cells[(b * self.size + g) * self.size + r];
        let mix = |a: [f32; 3], b: [f32; 3], t| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        let [r, g, b] = lower;
        let [rr, gg, bb] = lower.map(|v| (v + 1).min(limit));
        let slice = |z| mix(mix(at(r, g, z), at(rr, g, z), fraction[0]),
            mix(at(r, gg, z), at(rr, gg, z), fraction[0]), fraction[1]);
        mix(slice(b), slice(bb), fraction[2])
    }
}

#[test]
fn lut_interpolates_and_key_preserves_existing_alpha() {
    let lut = Lut::cube("LUT_3D_SIZE 2\n1 0 0\n0 0 0\n1 1 0\n0 1 0\n1 0 1\n0 0 1\n1 1 1\n0 1 1").unwrap();
    assert_eq!(lut.sample([0.25, 0.5, 0.75]), [0.75, 0.5, 0.75]);
    assert!(Lut::cube("LUT_3D_SIZE 2\n0 0 NaN").is_err());
    let image = image::RgbImage::from_fn(4, 2, |x, y| image::Rgb([(x % 2 * 255) as u8, (y * 255) as u8, (x / 2 * 255) as u8]));
    let png = Lut::png(&image).unwrap();
    assert_eq!(png.sample([0.25, 0.5, 0.75]), [0.25, 0.5, 0.75]);
    let mut pixels = [0, 0, 0, 255, 128, 128, 128, 100, 255, 255, 255, 80];
    LumaKey { enabled: true, threshold: 64, softness: 128, invert: false }.apply(&mut pixels);
    assert_eq!([pixels[3], pixels[7], pixels[11]], [0, 50, 80]);
}
