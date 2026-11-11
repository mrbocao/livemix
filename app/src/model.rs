// Switcher state and rules. Numbers come from MECHANICS.md.

use eframe::egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Effect {
    Fade,
    Zoom,
    Wipe,
    Slide,
    Fly,
    CrossZoom,
    FlyRotate,
    Cube,
    CubeZoom,
    VerticalWipe,
    VerticalSlide,
    Merge,
    WipeReverse,
    SlideReverse,
    VerticalWipeReverse,
    VerticalSlideReverse,
    BarnDoor,
    RollerDoor,
    Cut,
}

/// Same order as the original's effect list (19 effects).
pub const ALL_EFFECTS: [Effect; 19] = [
    Effect::Fade,
    Effect::Zoom,
    Effect::Wipe,
    Effect::Slide,
    Effect::Fly,
    Effect::CrossZoom,
    Effect::FlyRotate,
    Effect::Cube,
    Effect::CubeZoom,
    Effect::VerticalWipe,
    Effect::VerticalSlide,
    Effect::Merge,
    Effect::WipeReverse,
    Effect::SlideReverse,
    Effect::VerticalWipeReverse,
    Effect::VerticalSlideReverse,
    Effect::BarnDoor,
    Effect::RollerDoor,
    Effect::Cut,
];

pub const OVERLAY_EFFECTS: [Effect; 6] = [Effect::Fade, Effect::Zoom, Effect::Wipe, Effect::Slide, Effect::VerticalSlide, Effect::Cut];

impl Effect {
    pub fn name(self) -> &'static str {
        match self {
            Effect::Fade => "Fade",
            Effect::Zoom => "Zoom",
            Effect::Wipe => "Wipe",
            Effect::Slide => "Slide",
            Effect::Fly => "Fly",
            Effect::CrossZoom => "CrossZoom",
            Effect::FlyRotate => "FlyRotate",
            Effect::Cube => "Cube",
            Effect::CubeZoom => "CubeZoom",
            Effect::VerticalWipe => "VerticalWipe",
            Effect::VerticalSlide => "VerticalSlide",
            Effect::Merge => "Merge",
            Effect::WipeReverse => "WipeReverse",
            Effect::SlideReverse => "SlideReverse",
            Effect::VerticalWipeReverse => "VWipeReverse",
            Effect::VerticalSlideReverse => "VSlideReverse",
            Effect::BarnDoor => "BarnDoor",
            Effect::RollerDoor => "RollerDoor",
            Effect::Cut => "Cut",
        }
    }
    fn code(self) -> usize {
        ALL_EFFECTS.iter().position(|e| *e == self).unwrap_or(0)
    }
    fn from_code(c: usize) -> Effect {
        ALL_EFFECTS.get(c).copied().unwrap_or(Effect::Fade)
    }
    pub fn code_pub(self) -> usize {
        self.code()
    }
    pub fn from_code_pub(c: usize) -> Effect {
        Self::from_code(c)
    }
}

pub const CATEGORIES: [(&str, Color32); 6] = [
    ("Green", Color32::from_rgb(0x3c, 0xc0, 0x4a)),
    ("Red", Color32::from_rgb(0xe0, 0x40, 0x40)),
    ("Purple", Color32::from_rgb(0xa0, 0x5c, 0xe0)),
    ("Orange", Color32::from_rgb(0xff, 0x8c, 0x00)),
    ("Blue", Color32::from_rgb(0x40, 0x8c, 0xff)),
    ("Aqua", Color32::from_rgb(0x30, 0xd0, 0xd0)),
];

#[derive(Clone, Debug)]
pub enum Kind {
    Colour(Color32),
    Bars,
    Title { text: String, sub: String },
    GtTitle {
        path: String,
        texts: std::collections::BTreeMap<String, String>,
        document: Option<std::sync::Arc<crate::gt::Document>>,
    },
    Blank,
    Clock,
    Camera(u32),
    Video { hue: f32 },
    Image(u32),
    /// A real capture device (webcam, capture card, Blackmagic device).
    Device { link: String, blackmagic: bool },
    /// A real video file.
    VideoFile { path: String },
    /// A real picture file.
    ImageFile { path: String },
    /// "List": several video files played one after another (original's Video List input).
    VideoList { paths: Vec<String>, idx: usize },
    /// "Instant Replay": the selected replay event angle, played like a video file.
    Replay { path: String },
    /// "NDI / Desktop Capture": the whole Windows desktop.
    Desktop,
    /// "Image Sequence / Stinger": a folder of numbered pictures played as video.
    ImageSequence { folder: String },
    /// "Photos": a slideshow of the pictures in a folder.
    Photos { folder: String },
    /// "Video Delay" (vMix.VideoType 10): a delayed copy of another input. The original
    /// buffers the source at the chosen resolution for `seconds` (frmInput tVideoDelay).
    VideoDelay { source: Option<u32>, seconds: f32, resolution: [u32; 2] },
    /// "Telestrator" (vMix.VideoType 5001): a transparent drawing layer over the program.
    Telestrator { strokes: Vec<Telestroke> },
}

/// One freehand line of a Telestrator input. Points are 0..1 in the frame, like the original's
/// web page canvases (1920x1080 scaled to the frame).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Telestroke {
    pub points: Vec<[f32; 2]>,
    pub colour: [u8; 3],
    /// Line width as a fraction of the frame height (0.004 = ~4 px at 1080p).
    pub width: f32,
}

/// One trigger of Input Settings → Triggers: run an automation script when the input goes
/// to or leaves Program, after an optional delay. The original's lstTriggers holds these.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputTrigger {
    /// 0 = On Transition In, 1 = On Transition Out.
    pub kind: u8,
    pub command: String,
    #[serde(default)]
    pub delay_ms: u32,
}

impl Kind {
    pub fn playable(&self) -> bool {
        match self {
            Kind::Replay { path } => !path.is_empty(),
            _ => matches!(self, Kind::Video { .. } | Kind::VideoFile { .. } | Kind::VideoList { .. } | Kind::ImageSequence { .. } | Kind::Photos { .. }),
        }
    }
    pub fn has_audio(&self) -> bool {
        match self {
            Kind::Replay { path } => !path.is_empty(),
            _ => matches!(self, Kind::Device { .. } | Kind::VideoFile { .. } | Kind::VideoList { .. }),
        }
    }
    /// The input a Video Delay copies (None for every other kind).
    pub fn transparent(&self) -> bool {
        matches!(self, Kind::Title { .. } | Kind::GtTitle { .. } | Kind::Blank | Kind::ImageFile { .. } | Kind::ImageSequence { .. })
    }
    pub fn keyable(&self) -> bool {
        match self {
            Kind::Replay { path } => !path.is_empty(),
            _ => matches!(self, Kind::Device { .. } | Kind::VideoFile { .. } | Kind::VideoList { .. } | Kind::Desktop),
        }
    }

    pub fn gt_seconds(&self, exiting: bool) -> Option<f32> {
        match self {
            Self::GtTitle { document: Some(doc), .. } => doc.seconds(exiting),
            _ => None,
        }
    }
    /// Inputs whose picture comes from a media thread (camera, files, desktop…).
    pub fn streamed(&self) -> bool {
        match self {
            Kind::Replay { path } => !path.is_empty(),
            _ => matches!(
                self,
                Kind::Device { .. } | Kind::VideoFile { .. } | Kind::VideoList { .. } | Kind::Desktop | Kind::ImageSequence { .. } | Kind::Photos { .. }
            ),
        }
    }
}

/// Green-screen (chroma key) settings of an input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub on: bool,
    pub colour: [u8; 3],
    pub tolerance: u8,
}

impl Default for Key {
    fn default() -> Self {
        Key { on: false, colour: [0, 177, 64], tolerance: 60 }
    }
}

/// Where an input (or a layer) sits in the frame — the original's MatrixPosition
/// (ctor @0x2bc140: zoom 1, pan 0; matrix @0x2bc778: scale by zoom, then move by
/// Pan X × half the width and Pan Y × half the height, up positive). Crop is the original's
/// 0–10000 trackbar scale (frmInputSettings tCropX1..Y2) stored as 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub zoom: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    /// x1, y1, x2, y2 (0..1); default 0, 0, 1, 1 = no crop.
    pub crop: [f32; 4],
}

impl Default for Position {
    fn default() -> Self {
        Position { zoom: 1.0, pan_x: 0.0, pan_y: 0.0, crop: [0.0, 0.0, 1.0, 1.0] }
    }
}

/// Position editor trackbar ranges (ozzzzzq layout code, round 19): Zoom −3990…4000,
/// Pan X/Y −2000…2000.
pub const ZOOM_SLIDER: (i32, i32) = (-3990, 4000);
pub const PAN_SLIDER: (i32, i32) = (-2000, 2000);

/// Zoom trackbar → zoom (ozzzzzq::k 0x060039a4): below centre value/4000 + 1, above centre
/// value/1000 + 1, so the left half covers 0.0025…1 and the right half 1…5. The original
/// writes 0 ("unset" = 1) inside the dead band −1…1; here that is zoom 1.
pub fn zoom_from_slider(v: i32) -> f32 {
    if v < -1 {
        v as f32 / 4000.0 + 1.0
    } else if v > 1 {
        v as f32 / 1000.0 + 1.0
    } else {
        1.0
    }
}

/// Zoom → trackbar (ozzzzzq::h 0x060039a1): (z − 1)·4000 below 1, (z − 1)·1000 above, 0 at 1.
pub fn zoom_to_slider(z: f32) -> i32 {
    let v = if z < 1.0 {
        ((z - 1.0) * 4000.0).round() as i32
    } else if z > 1.0 {
        ((z - 1.0) * 1000.0).round() as i32
    } else {
        0
    };
    v.clamp(ZOOM_SLIDER.0, ZOOM_SLIDER.1)
}

/// Pan trackbar ↔ pan (ozzzzzq::n/o and ::i): value/1000, so ±2000 = ±2 half-frames.
pub fn pan_from_slider(v: i32) -> f32 {
    v as f32 / 1000.0
}
pub fn pan_to_slider(p: f32) -> i32 {
    ((p * 1000.0).round() as i32).clamp(PAN_SLIDER.0, PAN_SLIDER.1)
}

/// One of an input's layers (the original has 10: ShortcutFunction SetLayer1..10Zoom/PanX/…).
/// The border is the Layer Designer's Border mode (LayerEditor::LayerBorder): a frame drawn
/// around the layer's visible rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub on: bool,
    pub input: Option<u32>,
    pub place: Position,
    pub border: bool,
    pub border_colour: [u8; 3],
    /// Border thickness as a fraction of the layer's width (0.02 = 2 %).
    pub border_width: f32,
}

impl Default for Layer {
    fn default() -> Self {
        Layer { on: false, input: None, place: Position::default(), border: false,
            border_colour: [0xff, 0xff, 0xff], border_width: 0.02 }
    }
}

/// Number of layers per input (ShortcutFunction SetLayer1..SetLayer10).
pub const LAYERS: usize = 10;

/// Picture treatment of one input. `brightness`/`contrast`/`saturation` are the Colour Adjust
/// page; `red`/`green`/`blue`/`alpha`/`black`/`white` are the Colour Correction page's
/// tbRed..tbWhite (the original's -100..100 trackbars, stored as -1..1).
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ColourAdjust {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
    pub black: f32,
    pub white: f32,
}

impl ColourAdjust {
    pub fn apply(self, pixels: &mut [u8]) {
        if self == Self::default() { return; }
        let brightness = self.brightness.clamp(-1.0, 1.0);
        let contrast = 1.0 + self.contrast.clamp(-1.0, 1.0);
        let saturation = 1.0 + self.saturation.clamp(-1.0, 1.0);
        let gain = [
            1.0 + self.red.clamp(-1.0, 1.0),
            1.0 + self.green.clamp(-1.0, 1.0),
            1.0 + self.blue.clamp(-1.0, 1.0),
        ];
        let black = self.black.clamp(-1.0, 1.0) * 0.5;
        let white = 1.0 + self.white.clamp(-1.0, 1.0) * 0.5;
        let span = (white - black).max(0.1);
        let alpha = 1.0 + self.alpha.clamp(-1.0, 1.0);
        for pixel in pixels.chunks_exact_mut(4) {
            let luma = (f32::from(pixel[0]) * 0.2126 + f32::from(pixel[1]) * 0.7152
                + f32::from(pixel[2]) * 0.0722) / 255.0;
            for (channel, gain) in pixel[..3].iter_mut().zip(gain) {
                let mut v = f32::from(*channel) / 255.0;
                v = luma + (v - luma) * saturation;
                v = (v - 0.5) * contrast + 0.5 + brightness;
                v *= gain;
                v = (v - black) / span;
                *channel = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            pixel[3] = (f32::from(pixel[3]) * alpha).clamp(0.0, 255.0).round() as u8;
        }
    }
}

#[derive(Clone, Debug)]
pub struct Input {
    /// Position in the frame (Input Settings → Position).
    pub place: Position,
    /// Tile shrunk to its title bar (double-click the title).
    pub collapsed: bool,
    /// Up to 10 other inputs drawn on top (Input Settings → Layers).
    pub layers: [Layer; LAYERS],
    pub id: u32,
    pub title: String,
    pub kind: Kind,
    pub category: Option<u8>,
    pub loop_on: bool,
    pub playing: bool,
    pub pos: f32,
    pub duration: f32,
    pub audio_on: bool,
    pub volume: f32,
    pub audio_effects: crate::audio::Effects,
    pub ptz_address: String,
    pub on_program: String,
    pub colour_adjust: ColourAdjust,
    /// Input Settings > General (frmInputSettings chkMirror/chkSharpen/chkDeinterlace).
    pub mirror: bool,
    pub sharpen: bool,
    pub deinterlace: bool,
    /// Automatically Play / Pause / Restart with Transition (chkPlayActive/chkPausePreview/chkRestart).
    pub play_active: bool,
    pub pause_preview: bool,
    pub restart_active: bool,
    /// Automatically mix audio (chkAutoMixAudio).
    pub auto_mix_audio: bool,
    /// Forced display aspect ratio; 0 = the source's own (cboAspectRatio "Default").
    pub aspect: f32,
    /// Tally Lights page: this input's tally colour; None = the default orange.
    pub tally_colour: Option<[u8; 3]>,
    /// Colour Correction page: Rec. 601 to 709 and Premultiplied Alpha.
    pub rec601: bool,
    pub premultiplied: bool,
    /// Advanced page: Flatten Layers, Mouse Click Action and video Delay in frames.
    pub flatten: bool,
    /// Effects page: chkUseSourceRenderEffects ("Use Source Settings").
    pub effects_source: bool,
    pub click_action: u8,
    pub delay_frames: u32,
    /// Live dBFS peak, filled in each frame for the Audio page's meter.
    pub level: f32,
    pub bus_m: bool,
    pub bus_a: bool,
    pub bus_b: bool,
    /// Live picture of a real input (texture id and pixel size), filled in by the UI.
    pub tex: Option<(eframe::egui::TextureId, [usize; 2])>,
    /// Photos: previous picture during the fade, with fade progress 0..1.
    pub prev_tex: Option<(eframe::egui::TextureId, [usize; 2], f32)>,
    /// "Starting…", "No signal …" etc. for real inputs.
    pub status: String,
    pub restart_req: bool,
    pub seek_ms: Option<u64>,
    pub key: Key,
    /// Camera: the capture mode chosen in Input Settings (None = Auto), every mode the camera
    /// offers and the one in use (filled in from the camera reader).
    pub cam_mode: Option<crate::media::CamMode>,
    pub decklink_port: u32,
    pub decklink_mode: u32,
    pub gt_change: Option<crate::gt::DataChange>,
    pub cam_modes: Vec<crate::media::CamMode>,
    pub cam_current: Option<crate::media::CamMode>,
}

impl Input {
    pub fn reload_gt(&mut self) -> Result<(), String> {
        let Kind::GtTitle { path, document, .. } = &mut self.kind else {
            return Err("This input is not a GT Title.".into());
        };
        let loaded = crate::gt::Document::load(std::path::Path::new(path))?;
        *document = Some(std::sync::Arc::new(loaded));
        self.status.clear();
        Ok(())
    }

    pub fn new(id: u32, title: &str, kind: Kind) -> Self {
        Input {
            place: Position::default(),
            collapsed: false,
            layers: [Layer::default(); LAYERS],
            id,
            title: title.to_string(),
            kind,
            category: None,
            loop_on: false,
            playing: false,
            pos: 0.0,
            duration: 60.0,
            audio_on: true,
            volume: 80.0,
            audio_effects: Default::default(),
            ptz_address: String::new(),
            on_program: String::new(),
            colour_adjust: Default::default(),
            mirror: false,
            sharpen: false,
            deinterlace: false,
            play_active: false,
            pause_preview: false,
            restart_active: false,
            auto_mix_audio: false,
            aspect: 0.0,
            tally_colour: None,
            rec601: false,
            premultiplied: false,
            flatten: false,
            effects_source: false,
            click_action: 0,
            delay_frames: 0,
            level: 0.0,
            bus_m: true,
            bus_a: false,
            bus_b: false,
            tex: None,
            prev_tex: None,
            status: String::new(),
            restart_req: false,
            seek_ms: None,
            key: Key::default(),
            cam_mode: None,
            decklink_port: 0,
            decklink_mode: 0,
            gt_change: None,
            cam_modes: Vec::new(),
            cam_current: None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct TransBtn {
    pub effect: Effect,
    pub ms: u32,
    pub stinger: Option<u32>,
}

/// A running transition. `progress` is on the original's 0–255 scale (hm::r @0x8b760).
pub struct Trans {
    pub from: Option<u32>,
    pub to: u32,
    pub effect: Effect,
    pub progress: f32,
    pub step: f32,
    pub manual: bool,
    /// Per-input Quick Play / Cut: Output becomes `to`, Preview is kept (ms::w @0x12e904).
    pub direct: bool,
    pub stinger: Option<Stinger>,
}

pub struct Stinger {
    pub input: u32,
    pub cut: f32,
    switched: bool,
}

pub const OVERLAY_CHANNELS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlaySource {
    pub input: u32,
    pub on_preview: bool,
}

/// An overlay layer; `level` is 0–255 like the original's `rs::b` (rs::t @0x1b0890).
pub struct Overlay {
    /// Source selected for the next activation.
    pub assigned: Option<OverlaySource>,
    /// Source currently being rendered, including during fade-out.
    pub input: Option<u32>,
    pub on: bool,
    pub level: f32,
    pub effect: Effect,
    pub ms: u32,
    /// Right-click puts the overlay on Preview instead of Output (ms::zzo @0x1301ac).
    pub on_preview: bool,
}

// ---- numbers from the mechanics sheet ----
/// Output render rate for the frame-based step: the project's Master Frame Rate (Settings →
/// Display). Default 59.94 (1080i59.94 in the screenshot of the original); interlaced formats
/// render per field (md::e @0x11ebe8 halves the frame duration), so the field rate is used.
static OUTPUT_FPS_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
pub const DEFAULT_FPS: f32 = 59.94;
pub fn output_fps() -> f32 {
    match OUTPUT_FPS_BITS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => DEFAULT_FPS,
        b => f32::from_bits(b),
    }
}
pub fn set_output_fps(fps: f32) {
    OUTPUT_FPS_BITS.store(fps.clamp(1.0, 240.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

/// Output sizes offered for the project (frmInput's list 640x480 … 1920x1080, plus the New
/// Preset sizes 2560x1440, 3840x2160, 2276x1280 and 3412x1920 from ss::e @0x1c9128).
pub const OUTPUT_SIZES: [(u32, u32); 11] = [(640, 480), (720, 480), (720, 576), (768, 480), (768, 576), (1280, 720), (1920, 1080), (2560, 1440), (3840, 2160), (2276, 1280), (3412, 1920)];
/// Resolutions of the New Preset window, in the original's order (ss::e 0x060046cf @0x1c9128:
/// 720x480, 720x576, 1280x720, 1920x1080, 2560x1440, 3840x2160, 2276x1280, 3412x1920), as
/// OUTPUT_SIZES indexes.
pub const NEW_PRESET_SIZES: [u8; 8] = [1, 2, 5, 6, 7, 8, 9, 10];
/// Master Frame Rate choices (frames per second; for interlaced output: fields per second).
pub const FRAME_RATES: [(&str, f32); 8] = [("23.976", 23.976), ("24", 24.0), ("25", 25.0), ("29.97", 29.97), ("30", 30.0), ("50", 50.0), ("59.94", 59.94), ("60", 60.0)];

/// Steps of the New button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewStep {
    Idle,
    /// "Do you wish to save changes to the current preset?" (frmCloseConfirm lblMessage:
    /// Save / Discard changes / Cancel).
    SaveChanges,
    WaitingForSave,
    /// The "New Preset" window (frmNewPreset: Resolution, Frame Rate, OK, Cancel).
    Format,
}

/// Language file STOP_RECORDING: why New / Open is refused while outputs run.
pub const STOP_OUTPUTS_MSG: &str = "Stop Recording, External Output, MultiCorder and Streaming before Opening a Preset.";

/// Frame rate choices of the New Preset window: (name, FRAME_RATES index, interlaced) — ss::e
/// (0x060046cf @0x1c9128) adds, in 100 ns frame durations: 400000 (25p), 333667 (29.97p),
/// 200000 (50p), 166833 (59.94p), then interlaced 400000 (25 frames = 50i) and 333667
/// (29.97 frames = 59.94i). Interlaced is kept as its field rate here, as the status bar shows it.
pub const NEW_PRESET_RATES: [(&str, u8, bool); 6] = [
    ("25p", 2, false),
    ("29.97p", 3, false),
    ("50p", 5, false),
    ("59.94p", 6, false),
    ("50i", 5, true),
    ("59.94i", 6, true),
];

/// Safe area menu of the Preview: names and SafeAreasType values, in the original's menu
/// order (kzzzzzj::b @0xf68ec builds the menu; kzzzzzj::d @0xf6d78 draws each type).
pub const SAFE_AREAS: [(&str, u16); 12] = [
    ("None", 0),
    ("SMPTE/EBU", 10),
    ("SMPTE/EBU with 4:3", 20),
    ("90% / 80%", 30),
    ("90% / 80% with 4:3", 32),
    ("GRID 3", 33),
    ("GRID 4", 40),
    ("GRID 8 16:9", 80),
    ("GRID 16", 160),
    ("GRID 32", 320),
    ("Square 1:1", 400),
    ("Vertical 9:16", 402),
];

/// "Waveform Monitor" menu of the Preview, in the original's order with the value each item
/// carries — hd::k (0x060012bb @0x87cb4).
pub const WAVEFORMS: [(&str, u8); 11] = [
    ("None", 0),
    ("Waveform RGB + Preview", 20),
    ("Waveform RGB", 5),
    ("Waveform Y + Preview", 25),
    ("Waveform Y", 6),
    ("Parade RGB + Preview", 30),
    ("Parade RGB", 10),
    ("Vectorscope + Preview", 40),
    ("Vectorscope", 15),
    ("Vectorscope + Waveform Y", 50),
    ("Vectorscope + Waveform Y + Preview", 60),
];
/// Scope pictures: Waveform RGB, Waveform Y, Parade RGB, Vectorscope.
pub const SCOPE_RGB: u8 = 5;
pub const SCOPE_Y: u8 = 6;
pub const SCOPE_PARADE: u8 = 10;
pub const SCOPE_VECTOR: u8 = 15;

/// Where the Preview picture and the scopes go in the Preview monitor for a Waveform Monitor
/// choice, as [x, y, w, h] in 0..1 of the picture (y down) — hc::a (0x06001292 @0x87134), which
/// places layer 8 (the picture), 9 (vectorscope) and 10 (waveform) with Position zoom / pan
/// (pan 1 = half the frame, pan Y up):
/// - Waveform RGB / Y / Parade alone: picture and scope both full size, the scope on top;
/// - "+ Preview": waveform zoom 0.5 pan X −0.5 (left half), picture zoom 0.5 pan X 0.5 (right);
/// - Vectorscope: zoom 0.5625 × 1, centred, picture hidden;
/// - Vectorscope + Preview: vectorscope 0.28125 × 0.5 pan X −0.5, picture right half;
/// - Vectorscope + Waveform Y: vectorscope right (pan X 0.5), waveform left; picture hidden;
/// - … + Preview: vectorscope top left (pan −0.5, 0.5), waveform bottom left (−0.5, −0.5),
///   picture right half.
/// Returns (picture place or None when hidden, [(scope, place)]).
pub fn scope_layout(code: u8) -> (Option<[f32; 4]>, Vec<(u8, [f32; 4])>) {
    let full = [0.0, 0.0, 1.0, 1.0];
    let left = [0.0, 0.25, 0.5, 0.5];
    let right = [0.5, 0.25, 0.5, 0.5];
    // A 0.28125 × 0.5 square (9:16 of half the width) centred in a half.
    let vs_at = |cx: f32, cy: f32| [cx - 0.140625, cy - 0.25, 0.28125, 0.5];
    match code {
        5 | 6 | 10 => (Some(full), vec![(code, full)]),
        20 => (Some(right), vec![(SCOPE_RGB, left)]),
        25 => (Some(right), vec![(SCOPE_Y, left)]),
        30 => (Some(right), vec![(SCOPE_PARADE, left)]),
        15 => (None, vec![(SCOPE_VECTOR, [0.21875, 0.0, 0.5625, 1.0])]),
        40 => (Some(right), vec![(SCOPE_VECTOR, vs_at(0.25, 0.5))]),
        50 => (None, vec![(SCOPE_Y, left), (SCOPE_VECTOR, vs_at(0.75, 0.5))]),
        60 => (Some(right), vec![(SCOPE_VECTOR, vs_at(0.25, 0.25)), (SCOPE_Y, [0.0, 0.5, 0.5, 0.5])]),
        _ => (Some(full), Vec::new()),
    }
}

/// One guide line, from (x0, y0) to (x1, y1), in pixels of a w × h picture.
pub type Guide = [f32; 4];

/// The guide lines and boxes of a safe area type for a w × h picture — kzzzzzj::d
/// (0x06002543 @0xf6d78):
/// - SMPTE/EBU: title safe 0.90 and action safe 0.93 boxes, centred; "90% / 80%": 0.80 and 0.90;
///   "with 4:3": two upright lines at the 4:3 width 0.675 (across the title box's height).
/// - GRID n: n columns × n rows (GRID 16 = 16 × 9, GRID 32 = 32 × 18); on pictures narrower than
///   1.7 : 1 a grid wider than 4 gets round(n / 4 × 3) rows (not GRID 8 16:9).
/// - Square 1:1 / Vertical 9:16: upright lines left and right of a centred square / 9:16 area
///   (9:16 width = h / 16 × 9, raised to a multiple of 4).
/// - The safe area types also draw a centre cross 0.05 × h long.
pub fn safe_area_guides(kind: u16, w: f32, h: f32) -> Vec<Guide> {
    let mut g: Vec<Guide> = Vec::new();
    let boxed = |g: &mut Vec<Guide>, share: f32| {
        let (x, y) = ((1.0 - share) / 2.0 * w, (1.0 - share) / 2.0 * h);
        let (bw, bh) = (share * w, share * h);
        g.push([x, y, x + bw, y]);
        g.push([x + bw, y, x + bw, y + bh]);
        g.push([x + bw, y + bh, x, y + bh]);
        g.push([x, y + bh, x, y]);
        (y, bh)
    };
    match kind {
        10 | 20 | 30 | 32 => {
            let (title, action) = if kind == 30 || kind == 32 { (0.80, 0.90) } else { (0.90, 0.93) };
            let (ty, th) = boxed(&mut g, title);
            boxed(&mut g, action);
            if kind == 20 || kind == 32 {
                let x = (1.0 - 0.675) / 2.0 * w;
                g.push([x, ty, x, ty + th]);
                g.push([x + 0.675 * w, ty, x + 0.675 * w, ty + th]);
            }
            let len = h * 0.05;
            g.push([w / 2.0, (h - len) / 2.0, w / 2.0, (h + len) / 2.0]);
            g.push([(w - len) / 2.0, h / 2.0, (w + len) / 2.0, h / 2.0]);
        }
        33 | 40 | 80 | 160 | 320 => {
            let (cols, mut rows) = match kind {
                33 => (3, 3),
                40 => (4, 4),
                80 => (8, 8),
                160 => (16, 9),
                _ => (32, 18),
            };
            if kind != 80 && w / h < 1.7 && cols > 4 {
                rows = (cols as f32 / 4.0 * 3.0).round() as i32;
            }
            for c in 1..cols {
                let x = (w / cols as f32 * c as f32).round();
                g.push([x, 0.0, x, h - 1.0]);
            }
            for r in 1..rows {
                let y = (h / rows as f32 * r as f32).round();
                g.push([0.0, y, w - 1.0, y]);
            }
        }
        400 | 402 => {
            let mut aw = h.round() as i32;
            if kind == 402 {
                aw = (h / 16.0 * 9.0).round() as i32;
                while aw % 4 != 0 {
                    aw += 1;
                }
            }
            let x0 = ((w - aw as f32) / 2.0).round();
            g.push([x0, 0.0, x0, h - 1.0]);
            g.push([x0 + aw as f32, 0.0, x0 + aw as f32, h - 1.0]);
        }
        _ => {}
    }
    g
}
/// Full scale of transition progress, overlay level and T-bar (hm::r, rs::t, ms::zzzzzze).
pub const FULL: f32 = 255.0;
/// mr::zzy @0x12ce3c: step = 256 / round(ms * fps / 1000).
const STEP_NUMERATOR: f32 = 256.0;
/// ms::zzzz @0x131c20: durations <= 0 or >= 10000 are replaced with 500.
pub const MAX_DURATION_MS: u32 = 10000;
const INVALID_DURATION_MS: u32 = 500;

/// mr::zzy: frames-based step on the 0–255 scale.
pub fn step_for(ms: u32) -> f32 {
    let frames = (ms as f32 * output_fps() / 1000.0).round();
    if frames <= 0.0 {
        // hm::f: a zero step becomes 1; a zero frame count makes the step "infinite",
        // which finishes on the next frame.
        return FULL;
    }
    STEP_NUMERATOR / frames
}

/// ms::zzzz duration rule.
pub fn sanitize_ms(ms: u32) -> u32 {
    if ms == 0 || ms >= MAX_DURATION_MS {
        INVALID_DURATION_MS
    } else {
        ms
    }
}

/// Input types of the Add Input window, in the original's order (language file
/// frmInput.tMenu.*), followed by this app's extras.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddKind {
    VideoFile,
    Dvd,
    List,
    Camera,
    Desktop,
    StreamSrt,
    Replay,
    ImageSequence,
    VideoDelay,
    ImageFile,
    Photos,
    PowerPoint,
    Colour,
    AudioFile,
    AudioInput,
    Title,
    VirtualSet,
    WebBrowser,
    VideoCall,
    Zoom,
    Telestrator,
    // extras of this app
    Blackmagic,
    Clock,
    TestCamera,
    TestVideo,
    TestImage,
}

pub const ADD_KINDS: [AddKind; 26] = [
    AddKind::VideoFile,
    AddKind::Dvd,
    AddKind::List,
    AddKind::Camera,
    AddKind::Desktop,
    AddKind::StreamSrt,
    AddKind::Replay,
    AddKind::ImageSequence,
    AddKind::VideoDelay,
    AddKind::ImageFile,
    AddKind::Photos,
    AddKind::PowerPoint,
    AddKind::Colour,
    AddKind::AudioFile,
    AddKind::AudioInput,
    AddKind::Title,
    AddKind::VirtualSet,
    AddKind::WebBrowser,
    AddKind::VideoCall,
    AddKind::Zoom,
    AddKind::Telestrator,
    AddKind::Blackmagic,
    AddKind::Clock,
    AddKind::TestCamera,
    AddKind::TestVideo,
    AddKind::TestImage,
];

/// Number of entries that come from the original's list (the rest are this app's extras).
pub const ORIGINAL_ADD_KINDS: usize = 21;

impl AddKind {
    pub fn label(self) -> &'static str {
        match self {
            AddKind::VideoFile => "Video",
            AddKind::Dvd => "DVD",
            AddKind::List => "List",
            AddKind::Camera => "Camera",
            AddKind::Desktop => "NDI / OMT / Desktop Capture",
            AddKind::StreamSrt => "Stream / SRT",
            AddKind::Replay => "Instant Replay",
            AddKind::ImageSequence => "Image Sequence / Stinger",
            AddKind::VideoDelay => "Video Delay",
            AddKind::ImageFile => "Image",
            AddKind::Photos => "Photos",
            AddKind::PowerPoint => "PowerPoint",
            AddKind::Colour => "Colour",
            AddKind::AudioFile => "Audio",
            AddKind::AudioInput => "Audio Input",
            AddKind::Title => "Title",
            AddKind::VirtualSet => "Virtual Set",
            AddKind::WebBrowser => "Web Browser",
            AddKind::VideoCall => "Video Call",
            AddKind::Zoom => "Zoom",
            AddKind::Telestrator => "Telestrator",
            AddKind::Blackmagic => "Blackmagic",
            AddKind::Clock => "Production Clock",
            AddKind::TestCamera => "Test Camera",
            AddKind::TestVideo => "Test Video",
            AddKind::TestImage => "Test Image",
        }
    }
    /// Built in this version?
    pub fn available(self) -> bool {
        !matches!(
            self,
            AddKind::Dvd
                | AddKind::PowerPoint
                | AddKind::AudioFile
                | AddKind::VirtualSet
                | AddKind::WebBrowser
                | AddKind::VideoCall
                | AddKind::Zoom
        )
    }
    pub fn help(self) -> &'static str {
        match self {
            AddKind::VideoFile => "A video file (MP4, MOV, WMV, AVI, MKV…). Plays, pauses, restarts and loops.",
            AddKind::List => "Several video files played one after another. Loop repeats the whole list.",
            AddKind::Camera => "A webcam, USB camera, HDMI/SDI capture card, Blackmagic device or NDI Webcam on this computer.",
            AddKind::Desktop => "Choose an NDI or OMT source or capture this computer's main screen.",
            AddKind::ImageSequence => "A folder of numbered pictures (PNG with transparency works as a stinger) played as a clip at the output frame rate.",
            AddKind::ImageFile => "A picture file (PNG, JPG, BMP). PNG transparency is kept, so logos work as Overlays.",
            AddKind::Photos => "A slideshow of every picture in a folder (JPG, PNG, BMP, TIF, GIF); while playing it moves to the next picture every 2 seconds.",
            AddKind::Colour => "A solid colour, a transparent (empty) input or colour bars.",
            AddKind::Title => "Open a .gtzip template created in GT Designer. Edit its text in Input Settings.",
            AddKind::Blackmagic => "Blackmagic DeckLink, Intensity, UltraStudio, ATEM Mini or Web Presenter. DeckLink/Intensity/UltraStudio need Blackmagic Desktop Video installed.",
            AddKind::Clock => "A clock counting up from when the show started.",
            AddKind::TestCamera => "A drawn stand-in camera picture (for practice without hardware).",
            AddKind::TestVideo => "A drawn stand-in clip with a play position.",
            AddKind::TestImage => "A drawn stand-in still picture.",
            AddKind::Dvd => "Play a DVD disc — not available in this version.",
            AddKind::StreamSrt => "Receive H.264 or HEVC video from an SRT source.",
            AddKind::Replay => "Show the selected Instant Replay event angle as an input. Configure angles and mark events in the Replay window.",
            AddKind::VideoDelay => "A delayed copy of another input. Pick the source, the buffer length and the resolution the picture is stored at.",
            AddKind::PowerPoint => "PowerPoint slides — not available in this version (export the slides as pictures and use Photos).",
            AddKind::AudioFile => "An audio file — not available in this version (audio is not built yet).",
            AddKind::AudioInput => "A microphone, USB audio interface or line input.",
            AddKind::VirtualSet => "Virtual studio sets — not available in this version.",
            AddKind::WebBrowser => "A web page as an input — not available in this version.",
            AddKind::VideoCall => "Remote guests by video call — not available in this version.",
            AddKind::Zoom => "Zoom meeting participants — not available in this version.",
            AddKind::Telestrator => "Draw over the program from a web browser. Enable the Web Controller and open /telestrator/ on your tablet or phone.",
        }
    }
    pub fn uses_device(self) -> bool {
        matches!(self, AddKind::Camera | AddKind::Blackmagic | AddKind::AudioInput)
    }
}

#[derive(Clone, Debug)]
pub struct DeviceChoice {
    pub name: String,
    pub link: String,
    pub blackmagic: bool,
}

pub struct AddDialog {
    pub decklink_port: u32,
    pub decklink_mode: u32,
    pub open: bool,
    pub kind: AddKind,
    pub name: String,
    pub colour: Color32,
    pub length: f32,
    pub devices: Vec<DeviceChoice>,
    pub devices_loaded: bool,
    pub device: usize,
    pub ndi_source: Option<String>,
    pub ndi_sources: Vec<String>,
    pub ndi_scan: bool,
    pub ndi_status: String,
    pub omt_sources: Vec<String>,
    pub omt_status: String,
    pub srt: crate::srt::Connection,
    pub path: String,
    /// Colour type options of the original (frmInput rColour / rTransparent / rColourBars):
    /// 0 = Colour, 1 = Transparent, 2 = Colour Bars.
    pub colour_mode: u8,
    /// Files of a List input.
    pub list: Vec<String>,
    /// Folder of an Image Sequence or Photos input.
    pub folder: String,
    /// Video Delay: the input copied, the buffer length in seconds and the buffer resolution
    /// (index into DELAY_RESOLUTIONS, the original's cboVideoDelayResolution).
    pub delay_source: Option<u32>,
    pub delay_seconds: f32,
    pub delay_resolution: usize,
}

/// cboVideoDelayResolution entries in the original (frmInput tVideoDelay).
pub const DELAY_RESOLUTIONS: [[u32; 2]; 8] = [
    [640, 480], [720, 480], [720, 486], [720, 576],
    [768, 480], [768, 576], [1280, 720], [1920, 1080],
];

pub struct Switcher {
    pub inputs: Vec<Input>,
    next_id: u32,
    pub preview: Option<u32>,
    pub input_preview: Option<u32>,
    pub output: Option<u32>,
    pub trans: Option<Trans>,
    pub transitions: [TransBtn; 4],
    pub quickplay_effect: Effect,
    pub quickplay_ms: u32,
    pub ftb_on: bool,
    pub ftb_level: f32,
    pub ftb_ms: u32,
    pub ftb_audio: bool,
    pub overlays: [Overlay; OVERLAY_CHANNELS],
    pub rec: Option<f64>,
    pub multicorder: Option<f64>,
    pub playlist: Option<f64>,
    pub playlist_secs: f32,
    /// Fullscreen display (top bar "Fullscreen"): its own borderless window filling the chosen screen.
    pub fullscreen: bool,
    /// Setup windows of the gear buttons next to Record, Stream, MultiCorder and PlayList
    /// (frmmain cmdRecSetup, cmdStreamingSetup, cmdMultiCorderSetup, cmdPlayListSetup).
    pub rec_setup: bool,
    pub stream_setup: bool,
    pub stream_slot: usize,
    pub stream_active: [bool; crate::stream::COUNT],
    pub stream_status: [String; crate::stream::COUNT],
    pub mc_setup: bool,
    pub playlist_setup: bool,
    /// Inputs left out of the MultiCorder / the PlayList (ticked off in their setup windows).
    pub mc_off: std::collections::HashSet<u32>,
    pub playlist_off: std::collections::HashSet<u32>,
    /// Which field a folder picked with Browse goes to: 0 = Add Input, 1 = Recording, 2 = MultiCorder.
    pub browse_for: u8,
    pub show_mixer: bool,
    pub settings_open: bool,
    pub overlay_dialog: bool,
    pub add_dialog: AddDialog,
    pub filter: Option<u8>,
    pub confirm_close: Option<u32>,
    pub master_volume: f32,
    pub master_on: bool,
    pub pause_preview: bool,
    pub play_active: bool,
    pub restart_active: bool,
    pub preset_name: String,
    pub preset_file: Option<std::path::PathBuf>,
    /// The preset as last saved / opened / made new (None: never saved), for "save changes?".
    pub saved_text: Option<String>,
    /// Where the New button is: asking to save changes, or showing the New Preset window.
    pub new_step: NewStep,
    /// Video format picked in the New Preset window: (size, rate, interlaced) indexes.
    pub new_format: (u8, u8, bool),
    pub notice: String,
    notice_until: f64,
    now: f64,
    /// T-bar value on the 0–255 scale (ms::zzzzzze @0x133cc0).
    tbar: f32,
    tbar_rest_bottom: bool,
    frame_acc: f32,
    // ---- requests from the UI to the app (real media, files, windows) ----
    /// Where the Output monitor is on screen (for snapshots and recording).
    pub output_rect: eframe::egui::Rect,
    /// Scope pictures of the composed Preview by kind (SCOPE_RGB …).
    pub scope_tex: Vec<(u8, eframe::egui::TextureId)>,
    /// Interface frame counter (Preview Skip).
    pub ui_frame: u64,
    pub snapshot_req: bool,
    /// Some(true) = pick a video file, Some(false) = pick a picture.
    pub browse_req: Option<crate::media::PickKind>,
    pub refresh_devices: bool,
    pub dialog_error: String,
    pub external: [ExternalWindow; OUTPUT_COUNT],
    pub decklink_active: [bool; OUTPUT_COUNT],
    pub decklink_status: [String; OUTPUT_COUNT],
    pub ndi_status: [String; OUTPUT_COUNT],
    pub omt_status: [String; OUTPUT_COUNT],
    pub srt_active: [bool; OUTPUT_COUNT],
    pub srt_status: [String; OUTPUT_COUNT],
    pub srt_setup: Option<usize>,
    /// Input whose settings window (name, green-screen key) is open.
    pub settings_input: Option<u32>,
    pub import_report: Option<crate::preset::ImportReport>,
    pub automation: crate::automation::State,
    pub script: String,
    pub remote: Option<crate::automation::Remote>,
    pub replay_open: bool,
    pub replay_arm: bool,
    pub replay_angles: [u32; REPLAY_ANGLES],
    pub replay_mark_in: bool,
    pub replay_mark: bool,
    pub replay_add: bool,
    pub replay_available: bool,
    pub replay_status: String,
    pub replay_events: Vec<(String, Vec<String>)>,
    pub replay_event: usize,
    pub replay_angle: usize,
    pub replay_marking: bool,
    /// "Stop Recording?" / "Stop MultiCorder?" question (frmmain::ConfirmRecording @0x250d10).
    pub confirm_stop: Option<&'static str>,
    /// Position copied in Input Settings (Copy / Paste buttons of the original).
    pub position_clipboard: Option<Position>,
    /// Options from the original's Settings window (defaults from its settings file).
    pub cfg: Settings,
}

// What an output window or a MultiView box shows: 0 = Output, 1 = Preview, an input id + 1,
// or MultiView 1 / 2 (Settings → Outputs "Source" column of the original).
pub const SRC_OUTPUT: u32 = 0;
pub const SRC_PREVIEW: u32 = 1;
pub const SRC_MV1: u32 = u32::MAX - 1;
pub const SRC_MV2: u32 = u32::MAX - 2;
/// MultiView box left on automatic: the next input in list order.
pub const MV_AUTO: u32 = u32::MAX;
pub const OUTPUT_COUNT: usize = 4;
pub const REPLAY_ANGLES: usize = 8;

#[derive(Clone, Copy, Default)]
pub struct ExternalWindow {
    pub open: bool,
    pub fullscreen: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeckLinkRoute {
    pub device: Option<usize>,
    pub port: u32,
    pub mode: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputRoute {
    pub source: u32,
    pub overlays: bool,
}

impl Default for OutputRoute {
    fn default() -> Self {
        Self { source: SRC_OUTPUT, overlays: true }
    }
}

/// The original's enum MultiViewLayout (token 0x0200105c), in its order:
/// Pro (= 0, the config default), Standard, Quad, Sixteen, One.
pub const MV_LAYOUTS: [&str; 5] = ["Pro", "Standard", "Quad", "Sixteen", "One"];

/// MultiView frame the original lays out in (output format 1080i59.94).
const MV_W: i32 = 1920;
const MV_H: i32 = 1080;

/// .NET Math.Round(double): halves go to the even number.
fn round_net(v: f64) -> i32 {
    v.round_ties_even() as i32
}

/// Sizes of a MultiView layout — `qu::b` (0x06003e56 @ 0x193cf4) called with
/// (rows of title strips, small boxes per column):
/// returns (small box w, h), title strip height, column pitch, left margin of the small grid,
/// big box pitch and left margin of the big boxes.
fn mv_sizes(rows: i32, cols: i32) -> ((i32, i32), i32, i32, i32, i32, i32) {
    // Title strip first guess = round(H / 36); the pictures share what is left.
    let b0 = round_net(MV_H as f64 / 36.0);
    let mut h6 = MV_H - b0 * rows;
    while h6 % cols != 0 {
        h6 -= 1;
    }
    let aspect = (MV_W as f64 / MV_H as f64) as f32;
    let width = |h6: i32| ((h6 as f64 / cols as f64) * aspect as f64) as f32;
    let mut w = width(h6);
    // For 16:9 and 4:3 frames the picture height drops until the box width is a whole pixel.
    if aspect == 16.0_f32 / 9.0 || aspect == 4.0_f32 / 3.0 {
        while (w as f64) != (w as f64).round_ties_even() {
            h6 -= 1;
            w = width(h6);
        }
    }
    let total_w = round_net(w as f64);
    // The title strips share the remaining height evenly (rounded down to fit).
    let mut rest = MV_H - h6;
    while rest % rows != 0 {
        rest -= 1;
    }
    let b = round_net(rest as f64 / rows as f64);
    let gap = round_net(b as f64 / 2.0); // e: space between columns
    let small = (total_w, round_net(h6 as f64 / cols as f64));
    let pitch = small.0 + gap; // i
    let left = round_net((MV_W - pitch * cols) as f64 / 2.0); // j
    let big_pitch = small.0 * 2 + b; // k = m.Width + g
    let big_left = round_net((MV_W - big_pitch * 2) as f64 / 2.0); // l
    (small, b, pitch, left, big_pitch, big_left)
}

/// Boxes of a MultiView layout as [x, y, w, h] in 0..1 of the frame (the picture only; its
/// name strip is right below it, `mv_title_height` tall), each with what it shows by default.
/// From the original's layout builders in class `qu`:
/// - Pro / Standard → `qu::f` (@ 0x1944c4, b(10, 8, 3, 4)): Preview and Output twice the small
///   size side by side on top, then two rows of four inputs.
/// - Quad → `qu::d` (@ 0x194124, b(4, 2, 2, 2)): 2×2 — Preview, Output, then two inputs.
/// - Sixteen → `qu::e` (@ 0x1942e4, b(16, 14, 4, 4)): 4×4 — Preview, Output, then 14 inputs.
/// - One → `qu::c` (@ 0x193fa4, b(1, 1, 1, 1)): the Output alone.
pub fn mv_layout_boxes(layout: u8) -> Vec<([f32; 4], u32)> {
    let norm = |x: i32, y: i32, w: i32, h: i32| [x as f32 / MV_W as f32, y as f32 / MV_H as f32, w as f32 / MV_W as f32, h as f32 / MV_H as f32];
    let grid = |rows: i32, cols: i32, count: i32| -> Vec<([f32; 4], u32)> {
        let ((sw, sh), b, pitch, left, _, _) = mv_sizes(rows, cols);
        (0..count)
            .map(|k| {
                let (r, c) = (k / cols, k % cols);
                let def = match k {
                    0 => SRC_PREVIEW,
                    1 => SRC_OUTPUT,
                    _ => MV_AUTO,
                };
                (norm(left + c * pitch, r * (sh + b), sw, sh), def)
            })
            .collect()
    };
    match layout {
        0 | 1 => {
            let ((sw, sh), b, pitch, left, big_pitch, big_left) = mv_sizes(3, 4);
            let (bw, bh) = (sw * 2, sh * 2);
            let mut v = vec![(norm(big_left, 0, bw, bh), SRC_PREVIEW), (norm(big_left + big_pitch, 0, bw, bh), SRC_OUTPUT)];
            for k in 0..8 {
                let (r, c) = (k / 4, k % 4);
                v.push((norm(left + c * pitch, bh + b + r * (sh + b), sw, sh), MV_AUTO));
            }
            v
        }
        2 => grid(2, 2, 4),
        3 => grid(4, 4, 16),
        _ => {
            let ((sw, sh), _, pitch, left, _, _) = mv_sizes(1, 1);
            let _ = pitch;
            vec![(norm(left, 0, sw, sh), SRC_OUTPUT)]
        }
    }
}

/// Height of the name strip under each MultiView box, as a share of the frame height.
pub fn mv_title_height(layout: u8) -> f32 {
    let (rows, cols) = match layout {
        0 | 1 => (3, 4),
        2 => (2, 2),
        3 => (4, 4),
        _ => (1, 1),
    };
    mv_sizes(rows, cols).1 as f32 / MV_H as f32
}

/// One row of Settings > Shortcuts. The original's `vMix.Shortcut` carries either a
/// keyboard key (`m_Key`) or MIDI data (`m_MIDIChannel`, `m_MIDINote`, `m_MIDIValue`)
/// and one action (`m_Function` + input/duration); this keeps both in the same row.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ShortcutBinding {
    /// Chord written with `egui::Key::name`, e.g. "Q", "PageUp" or "Ctrl+S".
    /// Empty means keyboard is not bound.
    pub key: String,
    /// Action executed by the key or MIDI message (see `Switcher::perform_action`).
    pub action: String,
    /// 0 = no MIDI binding, 1..=16 = MIDI channel as shown in the original.
    pub midi_channel: u8,
    /// Note number (0..=127). For ControlChange this is the controller number.
    pub midi_note: u8,
    /// Optional value match (velocity / controller value).
    pub midi_value: u8,
    pub midi_value_enabled: bool,
    /// `MIDIValueType`: 0 Default, 1 JogWheel, 2..=4 reserved by the original.
    pub midi_value_type: u8,
    pub disabled: bool,
}

impl ShortcutBinding {
    pub fn has_midi(&self) -> bool {
        self.midi_channel > 0
    }
}

/// The original's "Default Keyboard" shortcut template (36 shortcuts).
pub fn default_shortcuts() -> Vec<ShortcutBinding> {
    let mut rows = Vec::new();
    let mut add = |key: &str, action: String| rows.push(ShortcutBinding {
        key: key.into(),
        action,
        ..Default::default()
    });
    add("PageUp", "PreviewPrevious".into());
    add("PageDown", "PreviewNext".into());
    add("Period", "Fade 500".into());
    add("0", "Cut".into());
    for (key, ms) in [("1", 1000), ("2", 2000), ("3", 3000), ("4", 4000)] {
        add(key, format!("Fade {ms}"));
    }
    add("OpenBracket", "PreviewPrevious".into());
    add("CloseBracket", "PreviewNext".into());
    for (n, key) in ["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"].into_iter().enumerate() {
        add(key, format!("Preview {}", n + 1));
    }
    for (n, key) in ["A", "S", "D", "F", "G", "H", "J", "K", "L"].into_iter().enumerate() {
        add(key, format!("CutDirect {}", n + 1));
    }
    for (n, key) in ["Z", "X", "C", "V", "B", "N", "M"].into_iter().enumerate() {
        add(key, format!("FadeDirect {} 1000", n + 1));
    }
    rows
}

/// Settings window options. Defaults are the original's config defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InterfaceTheme {
    #[default]
    Classic,
    Modern,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub theme: InterfaceTheme,
    /// InputSize = 208x156 (ms::.cctor @0x12e000 falls back to 200x150).
    pub input_w: f32,
    pub input_h: f32,
    /// InputNumbers = True.
    pub input_numbers: bool,
    /// InputCloseConfirm = False ("Display confirmation for Input Close button").
    pub input_close_confirm: bool,
    /// RecordingConfirm = True ("Display confirmation for Record, External, Stream and MultiCorder").
    pub recording_confirm: bool,
    /// PreviewClick = False ("Activate QuickPlay by clicking Preview Window").
    pub preview_click: bool,
    /// Transition Display: ShowQuickPlay / ShowTransitions24 / ShowFTB / ShowTBar = True.
    pub show_quickplay: bool,
    pub show_trans24: bool,
    pub show_ftb: bool,
    pub show_tbar: bool,
    /// Overlay buttons in the transition column (no default in config — estimated True).
    pub show_overlays: bool,
    /// MasterAudioMeter = True, InputAudioMeter = True.
    pub master_meter: bool,
    pub input_meter: bool,
    /// Alerts (AlertsXML empty = off): no-recording alert, FTB alert.
    pub recording_alert: bool,
    pub ftb_alert: bool,
    /// Fullscreen = False, External = False (start-up options), Maximise = True.
    pub start_fullscreen: bool,
    pub start_external: bool,
    pub start_maximised: bool,
    /// External window source (config OutputsExternal, empty = Output): SRC_OUTPUT, SRC_PREVIEW,
    /// SRC_MV1 / SRC_MV2 (MultiView 1 / 2) or an input id + 1.
    pub outputs: [OutputRoute; OUTPUT_COUNT],
    pub decklink_outputs: [DeckLinkRoute; OUTPUT_COUNT],
    pub ndi_outputs: [bool; OUTPUT_COUNT],
    pub omt_outputs: [bool; OUTPUT_COUNT],
    pub omt_quality: [u8; OUTPUT_COUNT],
    pub srt_outputs: [crate::srt::Settings; OUTPUT_COUNT],
    /// Fullscreen display source (config OutputsFullscreen, empty = Output), same codes.
    pub fs_source: u32,
    pub fs_overlays: bool,
    /// Screen the Fullscreen display fills (frmSettings lblOutputScreen "Display:"),
    /// index into the monitor list; estimated default: the second screen when there is one.
    pub fs_display: u8,
    /// MultiView 1 and 2 layout (config MultiViewLayout / MultiViewLayout2 = 0 = Pro).
    pub mv_layout: [u8; 2],
    /// "Customise Layout" (config MultiViewInputs): what each small box shows.
    /// MV_AUTO = the next input in list order, otherwise a source code as above.
    pub mv_boxes: [Vec<u32>; 2],
    /// OutputAlwaysOnTop = False, OutputHideCursor = False (config defaults).
    pub ext_on_top: bool,
    pub ext_hide_cursor: bool,
    /// Window size: 0 = 960x540, 1 = 1280x720, 2 = 1920x1080.
    pub ext_size: u8,
    /// Recording Setup (frmRecordingSetup): folder ("" = Documents\LiveMixStorage), the name
    /// that goes in front of the date (RecordingFilenameFormat "{0} - {1:dd MMMM yyyy - hh-mm-ss tt}"),
    /// MP4 video bit rate in Mbps and "New File Every" minutes (0 = one file).
    pub rec_folder: String,
    pub rec_name: String,
    pub rec_mbps: u32,
    pub rec_split_min: u32,
    pub rec_codec: crate::media::RecordFormat,
    pub rec_source: usize,
    pub rec_second: bool,
    pub rec_second_codec: crate::media::RecordFormat,
    pub rec_second_source: usize,
    pub rec_second_mbps: u32,
    /// MultiCorder (frmMultiCorder): folder ("Change Recording Folder"), video bit rate, new file every.
    pub mc_folder: String,
    pub mc_mbps: u32,
    pub mc_split_min: u32,
    pub mc_codec: crate::media::RecordFormat,
    pub mc_outputs: [bool; OUTPUT_COUNT],
    /// Streaming Settings (frmStreaming): destination, URL, stream name or key, quality.
    pub streams: [crate::stream::Destination; crate::stream::COUNT],
    /// PlayList (frmPlaylist chkLoop): start again after the last input.
    pub playlist_loop: bool,
    /// Project format (Settings → Display "Output Size", "Master Frame Rate", interlaced):
    /// indexes into OUTPUT_SIZES / FRAME_RATES. Default 1920x1080, 59.94, interlaced (the
    /// original's status bar shows 1080i59,94; config Interlaced = True).
    pub out_size: u8,
    pub out_rate: u8,
    pub interlaced: bool,
    /// Performance (Settings → Performance): Preview Skip (draw the input boxes every Nth frame,
    /// 1 = every frame), Synchronize Output with Refresh Rate (config LockBackBuffer = False:
    /// otherwise the window redraws at the Master Frame Rate), Disable Effects on Input Previews
    /// (input boxes skip layers and colour key), hardware encoder for recordings
    /// (recording key "HardwareAcceleration" = 1, kh::get_n @0xe4334), and the largest camera
    /// mode Auto picks.
    pub preview_skip: u8,
    pub lock_refresh: bool,
    pub no_input_effects: bool,
    pub hw_encoder: bool,
    pub video_adapter: String,
    pub auto_cam_max_h: u32,
    /// Preview monitor tools: safe area type (SafeAreasType value, 0 = None — see
    /// SAFE_AREAS) and Waveform Monitor choice (value from WAVEFORMS, 0 = None), with the
    /// "Vectorscope 2X Zoom" switch.
    pub safe_type: u16,
    pub wave_type: u8,
    pub vs_zoom: bool,
    /// Selected section of the Input Settings window.
    pub input_page: usize,
    /// Selected page in the Settings window.
    pub page: usize,
    /// Settings > Shortcuts (keyboard + MIDI), based on the original's Shortcut list.
    pub shortcuts: Vec<ShortcutBinding>,
    /// Settings > Shortcuts > MIDI Input master switch.
    pub midi_enabled: bool,
    /// Checked MIDI devices (indexes from `midi::Input::devices`).
    pub midi_devices: Vec<usize>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: InterfaceTheme::Classic,
            input_w: 208.0,
            input_h: 156.0,
            input_numbers: true,
            input_close_confirm: false,
            recording_confirm: true,
            preview_click: false,
            show_quickplay: true,
            show_trans24: true,
            show_ftb: true,
            show_tbar: true,
            show_overlays: true,
            master_meter: true,
            input_meter: true,
            recording_alert: false,
            ftb_alert: false,
            start_fullscreen: false,
            start_external: false,
            start_maximised: true,
            outputs: [OutputRoute::default(); OUTPUT_COUNT],
            decklink_outputs: [DeckLinkRoute::default(); OUTPUT_COUNT],
            ndi_outputs: [false; OUTPUT_COUNT],
            omt_outputs: [false; OUTPUT_COUNT],
            omt_quality: [0; OUTPUT_COUNT],
            srt_outputs: Default::default(),
            fs_source: SRC_OUTPUT,
            fs_overlays: true,
            fs_display: 1,
            mv_layout: [0, 0],
            mv_boxes: [Vec::new(), Vec::new()],
            ext_on_top: false,
            ext_hide_cursor: false,
            ext_size: 0,
            rec_folder: String::new(),
            rec_name: "capture".into(),
            rec_mbps: crate::media::DEFAULT_MBPS,
            rec_split_min: 0,
            rec_codec: Default::default(),
            rec_source: 0,
            rec_second: false,
            rec_second_codec: Default::default(),
            rec_second_source: 0,
            rec_second_mbps: crate::media::DEFAULT_MBPS,
            mc_folder: String::new(),
            mc_mbps: crate::media::DEFAULT_MBPS,
            mc_split_min: 0,
            mc_codec: Default::default(),
            mc_outputs: [false; OUTPUT_COUNT],
            streams: Default::default(),
            playlist_loop: false,
            out_size: 6,
            out_rate: 6,
            interlaced: true,
            preview_skip: 1,
            lock_refresh: false,
            no_input_effects: false,
            hw_encoder: true,
            video_adapter: String::new(),
            auto_cam_max_h: 1080,
            safe_type: 0,
            wave_type: 0,
            vs_zoom: false,
            input_page: 0,
            page: 0,
            shortcuts: default_shortcuts(),
            midi_enabled: false,
            midi_devices: Vec::new(),
        }
    }
}

impl Settings {
    /// Project picture size in pixels.
    pub fn out_dims(&self) -> (u32, u32) {
        OUTPUT_SIZES[(self.out_size as usize).min(OUTPUT_SIZES.len() - 1)]
    }
    /// Master Frame Rate (fields per second when interlaced).
    pub fn rate(&self) -> f32 {
        FRAME_RATES[(self.out_rate as usize).min(FRAME_RATES.len() - 1)].1
    }
    /// Pictures per second written to a recording: interlaced formats carry two fields per frame.
    pub fn frame_rate(&self) -> f32 {
        if self.interlaced { self.rate() / 2.0 } else { self.rate() }
    }
    /// Status bar name of the format, as the original writes it: "1080i59,94".
    pub fn format_label(&self) -> String {
        let (_, h) = self.out_dims();
        let rate = FRAME_RATES[(self.out_rate as usize).min(FRAME_RATES.len() - 1)].0.replace('.', ",");
        format!("{}{}{}", h, if self.interlaced { "i" } else { "p" }, rate)
    }
}

impl Switcher {
    fn settings_path() -> std::path::PathBuf {
        crate::media::storage_folder().join("settings.txt")
    }

    /// Saves the Settings window options (key=value lines).
    pub fn save_settings(&self) {
        let c = &self.cfg;
        let b = |v: bool| if v { "1" } else { "0" };
        let lines = [
            format!("theme={}", if c.theme == InterfaceTheme::Modern { "modern" } else { "classic" }),
            format!("input_size={}x{}", c.input_w, c.input_h),
            format!("input_numbers={}", b(c.input_numbers)),
            format!("input_close_confirm={}", b(c.input_close_confirm)),
            format!("recording_confirm={}", b(c.recording_confirm)),
            format!("preview_click={}", b(c.preview_click)),
            format!("show_quickplay={}", b(c.show_quickplay)),
            format!("show_trans24={}", b(c.show_trans24)),
            format!("show_ftb={}", b(c.show_ftb)),
            format!("show_tbar={}", b(c.show_tbar)),
            format!("show_overlays={}", b(c.show_overlays)),
            format!("master_meter={}", b(c.master_meter)),
            format!("input_meter={}", b(c.input_meter)),
            format!("recording_alert={}", b(c.recording_alert)),
            format!("ftb_alert={}", b(c.ftb_alert)),
            format!("start_fullscreen={}", b(c.start_fullscreen)),
            format!("start_external={}", b(c.start_external)),
            format!("start_maximised={}", b(c.start_maximised)),
            format!("ext_source={}", c.outputs[0].source),
            format!("ndi_outputs={}", c.ndi_outputs.iter().map(|on| if *on { '1' } else { '0' }).collect::<String>()),
            format!("omt_outputs={}", c.omt_outputs.iter().map(|on| if *on { '1' } else { '0' }).collect::<String>()),
            format!("omt_quality={}", serde_json::to_string(&c.omt_quality).unwrap_or_default()),
            format!("srt_outputs={}", serde_json::to_string(&c.srt_outputs).unwrap_or_default()),
            format!("decklink_outputs={}", c.decklink_outputs.iter().map(|route| {
                format!("{},{},{}", route.device.map_or(-1, |n| n as i64), route.port, route.mode)
            }).collect::<Vec<_>>().join(";")),
            format!("ext_overlays={}", b(c.outputs[0].overlays)),
            format!("additional_outputs={}", c.outputs[1..].iter()
                .map(|out| format!("{},{}", out.source, b(out.overlays))).collect::<Vec<_>>().join(";")),
            format!("ext_on_top={}", b(c.ext_on_top)),
            format!("ext_hide_cursor={}", b(c.ext_hide_cursor)),
            format!("ext_size={}", c.ext_size),
            format!("fs_source={}", c.fs_source),
            format!("fs_overlays={}", b(c.fs_overlays)),
            format!("fs_display={}", c.fs_display),
            format!("mv_layout={},{}", c.mv_layout[0], c.mv_layout[1]),
            format!("mv_boxes1={}", c.mv_boxes[0].iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")),
            format!("mv_boxes2={}", c.mv_boxes[1].iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")),
            format!("rec_folder={}", c.rec_folder),
            format!("rec_name={}", c.rec_name),
            format!("rec_mbps={}", c.rec_mbps),
            format!("rec_split_min={}", c.rec_split_min),
            format!("rec_codec={}", c.rec_codec.key()),
            format!("rec_source={}", c.rec_source),
            format!("rec_second={}", b(c.rec_second)),
            format!("rec_second_codec={}", c.rec_second_codec.key()),
            format!("rec_second_source={}", c.rec_second_source),
            format!("rec_second_mbps={}", c.rec_second_mbps),
            format!("mc_folder={}", c.mc_folder),
            format!("mc_mbps={}", c.mc_mbps),
            format!("mc_split_min={}", c.mc_split_min),
            format!("mc_codec={}", c.mc_codec.key()),
            format!("mc_outputs={}", c.mc_outputs.iter().map(|on| if *on { '1' } else { '0' }).collect::<String>()),
            format!("streams={}", serde_json::Value::Array(c.streams.iter().map(|d|
                serde_json::json!({"provider": d.provider, "url": d.url, "key": d.key,
                    "quality": d.quality, "source": d.source, "hardware": d.hardware})).collect())),
            format!("playlist_loop={}", b(c.playlist_loop)),
            format!("out_size={}", c.out_size),
            format!("out_rate={}", c.out_rate),
            format!("interlaced={}", b(c.interlaced)),
            format!("preview_skip={}", c.preview_skip),
            format!("lock_refresh={}", b(c.lock_refresh)),
            format!("no_input_effects={}", b(c.no_input_effects)),
            format!("hw_encoder={}", b(c.hw_encoder)),
            format!("video_adapter={}", c.video_adapter),
            format!("auto_cam_max_h={}", c.auto_cam_max_h),
            format!("safe_type={}", c.safe_type),
            format!("wave_type={}", c.wave_type),
            format!("vs_zoom={}", b(c.vs_zoom)),
            format!("quickplay={},{}", self.quickplay_effect.code_pub(), self.quickplay_ms),
            format!("ftb_ms={}", self.ftb_ms),
            format!("ftb_audio={}", b(self.ftb_audio)),
            format!("pause_preview={}", b(self.pause_preview)),
            format!("play_active={}", b(self.play_active)),
            format!("restart_active={}", b(self.restart_active)),
            format!("playlist_secs={}", self.playlist_secs),
            format!("shortcuts={}", serde_json::to_string(&c.shortcuts).unwrap_or_default()),
            format!("midi_enabled={}", b(c.midi_enabled)),
            format!("midi_devices={}", serde_json::to_string(&c.midi_devices).unwrap_or_default()),
        ];
        let _ = std::fs::write(Self::settings_path(), lines.join("\n"));
    }

    /// Loads saved Settings window options, if any.
    pub fn load_settings(&mut self) {
        let Ok(text) = std::fs::read_to_string(Self::settings_path()) else { return };
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let on = v == "1";
            let c = &mut self.cfg;
            match k {
                "theme" => c.theme = if v == "modern" { InterfaceTheme::Modern } else { InterfaceTheme::Classic },
                "input_size" => {
                    if let Some((w, h)) = v.split_once('x') {
                        c.input_w = w.parse::<f32>().unwrap_or(208.0).clamp(120.0, 480.0);
                        c.input_h = h.parse::<f32>().unwrap_or(156.0).clamp(68.0, 360.0);
                    }
                }
                "input_numbers" => c.input_numbers = on,
                "input_close_confirm" => c.input_close_confirm = on,
                "recording_confirm" => c.recording_confirm = on,
                "preview_click" => c.preview_click = on,
                "show_quickplay" => c.show_quickplay = on,
                "show_trans24" => c.show_trans24 = on,
                "show_ftb" => c.show_ftb = on,
                "show_tbar" => c.show_tbar = on,
                "show_overlays" => c.show_overlays = on,
                "master_meter" => c.master_meter = on,
                "input_meter" => c.input_meter = on,
                "recording_alert" => c.recording_alert = on,
                "ftb_alert" => c.ftb_alert = on,
                "start_fullscreen" => c.start_fullscreen = on,
                "start_external" => c.start_external = on,
                "start_maximised" => c.start_maximised = on,
                "ext_source" => c.outputs[0].source = v.parse().unwrap_or(SRC_OUTPUT),
                "ndi_outputs" => {
                    for (enabled, value) in c.ndi_outputs.iter_mut().zip(v.bytes()) {
                        *enabled = value == b'1';
                    }
                }
                "omt_outputs" => {
                    for (enabled, value) in c.omt_outputs.iter_mut().zip(v.bytes()) {
                        *enabled = value == b'1';
                    }
                }
                "omt_quality" => c.omt_quality = serde_json::from_str(v).unwrap_or_default(),
                "srt_outputs" => c.srt_outputs = serde_json::from_str(v).unwrap_or_default(),
                "decklink_outputs" => {
                    for (route, entry) in c.decklink_outputs.iter_mut().zip(v.split(';')) {
                        let fields: Vec<_> = entry.split(',').collect();
                        if let [device, port, mode] = fields.as_slice() {
                            route.device = device.parse().ok();
                            route.port = port.parse().unwrap_or(0);
                            route.mode = mode.parse().unwrap_or(0);
                        }
                    }
                }
                "ext_overlays" => c.outputs[0].overlays = on,
                "additional_outputs" => {
                    for (out, setting) in c.outputs[1..].iter_mut().zip(v.split(';')) {
                        if let Some((source, overlays)) = setting.split_once(',') {
                            out.source = source.parse().unwrap_or(SRC_OUTPUT);
                            out.overlays = overlays == "1";
                        }
                    }
                }
                "ext_on_top" => c.ext_on_top = on,
                "ext_hide_cursor" => c.ext_hide_cursor = on,
                "ext_size" => c.ext_size = v.parse().unwrap_or(0),
                "fs_source" => c.fs_source = v.parse().unwrap_or(SRC_OUTPUT),
                "fs_overlays" => c.fs_overlays = on,
                "fs_display" => c.fs_display = v.parse().unwrap_or(1),
                "mv_layout" => {
                    for (k, p) in v.split(',').take(2).enumerate() {
                        c.mv_layout[k] = p.parse::<u8>().unwrap_or(0).min(MV_LAYOUTS.len() as u8 - 1);
                    }
                }
                "rec_folder" => c.rec_folder = v.to_string(),
                "rec_name" => c.rec_name = if v.trim().is_empty() { "capture".into() } else { v.to_string() },
                "rec_mbps" => c.rec_mbps = v.parse().unwrap_or(crate::media::DEFAULT_MBPS).clamp(1, 100),
                "rec_split_min" => c.rec_split_min = v.parse().unwrap_or(0),
                "rec_codec" => c.rec_codec = crate::media::RecordFormat::parse(v),
                "rec_source" => c.rec_source = v.parse::<usize>().unwrap_or(0).min(OUTPUT_COUNT - 1),
                "rec_second" => c.rec_second = on,
                "rec_second_codec" => c.rec_second_codec = crate::media::RecordFormat::parse(v),
                "rec_second_source" => c.rec_second_source = v.parse::<usize>().unwrap_or(0).min(OUTPUT_COUNT - 1),
                "rec_second_mbps" => c.rec_second_mbps = v.parse().unwrap_or(crate::media::DEFAULT_MBPS).clamp(1, 100),
                "mc_folder" => c.mc_folder = v.to_string(),
                "mc_mbps" => c.mc_mbps = v.parse().unwrap_or(crate::media::DEFAULT_MBPS).clamp(1, 100),
                "mc_split_min" => c.mc_split_min = v.parse().unwrap_or(0),
                "mc_codec" => c.mc_codec = crate::media::RecordFormat::parse(v),
                "mc_outputs" => {
                    for (enabled, value) in c.mc_outputs.iter_mut().zip(v.bytes()) {
                        *enabled = value == b'1';
                    }
                }
                "stream_dest" => c.streams[0].provider = v.parse().unwrap_or(0),
                "stream_url" => c.streams[0].url = v.to_string(),
                "stream_key" => c.streams[0].key = v.to_string(),
                "stream_quality" => c.streams[0].quality = v.parse().unwrap_or(1),
                "streams" => {
                    if let Ok(streams) = serde_json::from_str::<Vec<serde_json::Value>>(v) {
                        for (destination, stream) in c.streams.iter_mut().zip(streams) {
                            destination.provider = stream["provider"].as_u64().unwrap_or(0).min(3) as u8;
                            destination.url = stream["url"].as_str().unwrap_or("").into();
                            destination.key = stream["key"].as_str().unwrap_or("").into();
                            destination.quality = stream["quality"].as_u64().unwrap_or(1).min(2) as u8;
                            destination.source = stream["source"].as_u64().unwrap_or(0).min((OUTPUT_COUNT - 1) as u64) as usize;
                            destination.hardware = stream["hardware"].as_bool().unwrap_or(false);
                        }
                    }
                }
                "playlist_loop" => c.playlist_loop = on,
                "out_size" => c.out_size = v.parse::<u8>().unwrap_or(6).min(OUTPUT_SIZES.len() as u8 - 1),
                "out_rate" => c.out_rate = v.parse::<u8>().unwrap_or(6).min(FRAME_RATES.len() as u8 - 1),
                "interlaced" => c.interlaced = on,
                "preview_skip" => c.preview_skip = v.parse::<u8>().unwrap_or(1).clamp(1, 8),
                "lock_refresh" => c.lock_refresh = on,
                "no_input_effects" => c.no_input_effects = on,
                "hw_encoder" => c.hw_encoder = on,
                "video_adapter" => c.video_adapter = v.to_owned(),
                "auto_cam_max_h" => c.auto_cam_max_h = v.parse().unwrap_or(1080),
                "safe_type" => c.safe_type = v.parse().ok().filter(|t| SAFE_AREAS.iter().any(|s| s.1 == *t)).unwrap_or(0),
                "wave_type" => c.wave_type = v.parse().ok().filter(|t| WAVEFORMS.iter().any(|w| w.1 == *t)).unwrap_or(0),
                "vs_zoom" => c.vs_zoom = on,
                "mv_boxes1" | "mv_boxes2" => {
                    let k = if k == "mv_boxes1" { 0 } else { 1 };
                    c.mv_boxes[k] = v.split(',').filter_map(|p| p.parse().ok()).collect();
                }
                "quickplay" => {
                    if let Some((e, ms)) = v.split_once(',') {
                        self.quickplay_effect = Effect::from_code_pub(e.parse().unwrap_or(0));
                        self.quickplay_ms = ms.parse().unwrap_or(500);
                    }
                }
                "ftb_ms" => self.ftb_ms = v.parse().unwrap_or(500),
                "ftb_audio" => self.ftb_audio = on,
                "pause_preview" => self.pause_preview = on,
                "play_active" => self.play_active = on,
                "restart_active" => self.restart_active = on,
                "playlist_secs" => self.playlist_secs = v.parse().unwrap_or(PLAYLIST_DEFAULT_SECS),
                "shortcuts" => {
                    if let Ok(rows) = serde_json::from_str(v) { c.shortcuts = rows; }
                }
                "midi_enabled" => c.midi_enabled = on,
                "midi_devices" => {
                    if let Ok(devices) = serde_json::from_str(v) { c.midi_devices = devices; }
                }
                _ => {}
            }
        }
    }
}

// Values from the mechanics sheet.
const QUICKPLAY_MS: u32 = 500; // config QuickPlayDuration (read by ms::zzzzj @0xdd55)
const FTB_MS: u32 = 500; // config FTBDuration (read by me::.ctor @0x11edbc)
const OVERLAY_MS: u32 = 500; // na::.ctor field e
/// ii::.ctor @0xad634: default transition buttons.
const DEFAULT_TRANSITIONS: [TransBtn; 4] = [
    TransBtn { effect: Effect::Fade, ms: 500, stinger: None },
    TransBtn { effect: Effect::Merge, ms: 1000, stinger: None },
    TransBtn { effect: Effect::Wipe, ms: 1000, stinger: None },
    TransBtn { effect: Effect::CubeZoom, ms: 1000, stinger: None },
];
const DEFAULT_TRANSITION_MS: u32 = 500; // ii::e fallback for an unknown button
/// PlayList item defaults — PlaylistItem::.ctor (0x06006d87 @0x2c201c): m_Transition = 100
/// (Cut), m_TransitionDuration = 500 ms, and an input that can't seek gets TimeSpan(0, 0, 5).
pub const PLAYLIST_EFFECT: Effect = Effect::Cut;
pub const PLAYLIST_TRANSITION_MS: u32 = 500;
pub const PLAYLIST_DEFAULT_SECS: f32 = 5.0;

impl Switcher {
    pub(crate) fn empty() -> Self {
        let ov = || Overlay { assigned: None, input: None, on: false, level: 0.0, effect: Effect::Fade, ms: OVERLAY_MS, on_preview: false };
        Switcher {
            inputs: Vec::new(),
            next_id: 1,
            preview: None,
            input_preview: None,
            output: None,
            trans: None,
            transitions: DEFAULT_TRANSITIONS,
            quickplay_effect: Effect::Fade,
            quickplay_ms: QUICKPLAY_MS,
            ftb_on: false,
            ftb_level: 0.0,
            ftb_ms: FTB_MS,
            ftb_audio: true,
            overlays: std::array::from_fn(|_| ov()),
            rec: None,
            multicorder: None,
            playlist: None,
            playlist_secs: PLAYLIST_DEFAULT_SECS,
            fullscreen: false,
            rec_setup: false,
            stream_setup: false,
            stream_slot: 0,
            stream_active: Default::default(),
            stream_status: Default::default(),
            mc_setup: false,
            playlist_setup: false,
            mc_off: Default::default(),
            playlist_off: Default::default(),
            browse_for: 0,
            show_mixer: true,
            settings_open: false,
            overlay_dialog: false,
            add_dialog: AddDialog {
                decklink_port: 0,
                decklink_mode: 0,
                open: false,
                kind: AddKind::Camera,
                name: String::new(),
                colour: Color32::from_rgb(0x20, 0x40, 0x80),
                length: 60.0,
                devices: Vec::new(),
                devices_loaded: false,
                device: 0,
                ndi_source: None,
                ndi_sources: Vec::new(),
                ndi_scan: true,
                ndi_status: String::new(),
                omt_sources: Vec::new(),
                omt_status: String::new(),
                srt: crate::srt::Settings::default().connection,
                path: String::new(),
                colour_mode: 0,
                list: Vec::new(),
                folder: String::new(),
                delay_source: None,
                delay_seconds: 1.0,
                delay_resolution: 3,
            },
            filter: None,
            confirm_close: None,
            master_volume: 85.0,
            master_on: true,
            pause_preview: true,
            play_active: true,
            restart_active: false,
            preset_name: "Untitled".into(),
            saved_text: None,
            preset_file: None,
            new_step: NewStep::Idle,
            new_format: (6, 6, true),
            notice: String::new(),
            notice_until: 0.0,
            now: 0.0,
            tbar: 0.0,
            tbar_rest_bottom: true,
            frame_acc: 0.0,
            output_rect: eframe::egui::Rect::NOTHING,
            scope_tex: Vec::new(),
            ui_frame: 0,
            snapshot_req: false,
            browse_req: None,
            refresh_devices: false,
            dialog_error: String::new(),
            external: [ExternalWindow::default(); OUTPUT_COUNT],
            decklink_active: [false; OUTPUT_COUNT],
            decklink_status: Default::default(),
            ndi_status: Default::default(),
            omt_status: Default::default(),
            srt_active: Default::default(),
            srt_status: Default::default(),
            srt_setup: None,
            settings_input: None,
            import_report: None,
            automation: Default::default(),
            script: String::new(),
            remote: None,
            replay_open: false,
            replay_arm: false,
            replay_angles: [SRC_OUTPUT, MV_AUTO, MV_AUTO, MV_AUTO, MV_AUTO, MV_AUTO, MV_AUTO, MV_AUTO],
            replay_mark_in: false,
            replay_mark: false,
            replay_add: false,
            replay_available: false,
            replay_status: String::new(),
            replay_events: Vec::new(),
            replay_event: 0,
            replay_angle: 0,
            replay_marking: false,
            confirm_stop: None,
            position_clipboard: None,
            cfg: Settings::default(),
        }
    }

    pub fn notice(&mut self, text: &str) {
        self.notify(text);
    }

    pub fn demo() -> Self {
        let mut s = Self::empty();
        s.add(Input::new(0, "Colour Bars", Kind::Bars));
        let c1 = s.add(Input::new(0, "Camera 1", Kind::Camera(1)));
        let c2 = s.add(Input::new(0, "Camera 2", Kind::Camera(2)));
        let mut v = Input::new(0, "Intro Video", Kind::Video { hue: 0.58 });
        v.duration = 95.0;
        v.pos = 12.0;
        v.loop_on = true;
        v.playing = true;
        v.category = Some(1);
        let vid = s.add(v);
        let mut lt = Input::new(0, "Lower Third", Kind::Title { text: "Alex Morgan".into(), sub: "Host · Live from the studio".into() });
        lt.category = Some(4);
        let lt = s.add(lt);
        s.add(Input::new(0, "Blue", Kind::Colour(Color32::from_rgb(0x1e, 0x3c, 0x78))));
        s.add(Input::new(0, "Production Clock", Kind::Clock));
        s.add(Input::new(0, "Holding Slide", Kind::Image(3)));
        for id in [c1, c2] {
            if let Some(i) = s.input_mut(id) {
                i.category = Some(0);
            }
        }
        if let Some(i) = s.input_mut(c2) {
            i.volume = 65.0;
        }
        if let Some(i) = s.input_mut(vid) {
            i.volume = 55.0;
        }
        s.output = Some(c1);
        s.preview = Some(c2);
        s.assign_overlay(0, lt, false);
        s.overlays[0].input = Some(lt);
        s.overlays[0].on = true;
        s.overlays[0].level = FULL;
        s
    }

    pub fn add(&mut self, mut i: Input) -> u32 {
        i.id = self.next_id;
        self.next_id += 1;
        let id = i.id;
        self.inputs.push(i);
        id
    }

    pub fn input(&self, id: u32) -> Option<&Input> {
        self.inputs.iter().find(|i| i.id == id)
    }
    pub fn input_mut(&mut self, id: u32) -> Option<&mut Input> {
        self.inputs.iter_mut().find(|i| i.id == id)
    }
    pub fn number_of(&self, id: u32) -> usize {
        self.inputs.iter().position(|i| i.id == id).map(|p| p + 1).unwrap_or(0)
    }

    fn notify(&mut self, text: &str) {
        self.notice = text.to_string();
        self.notice_until = self.now + 4.0;
    }

    // ------------------------------------------------------------ transitions

    fn on_transition_start(&mut self, to: u32) {
        let (play, restart) = (self.play_active, self.restart_active);
        if let Some(i) = self.input_mut(to) {
            if i.kind.playable() {
                if restart || i.restart_active {
                    i.pos = 0.0;
                    i.restart_req = true;
                }
                if play || i.play_active {
                    i.playing = true;
                }
            }
            // Input Settings > General: Automatically mix audio.
            if i.auto_mix_audio {
                i.audio_on = true;
                i.bus_m = true;
            }
        }
    }

    /// End of a transition: ms::u @0x12e840 swaps the buses; ms::w @0x12e904 (direct)
    /// only replaces Output.
    fn finish_transition(&mut self, to: u32, direct: bool) {
        let old = self.output;
        self.output = Some(to);
        if !direct {
            self.preview = old.or(Some(to));
        }
        // Automatically Pause after Transition: the Settings option for every input, or this
        // input's own Input Settings flag.
        if self.pause_preview {
            if let Some(p) = old {
                if p != to && (!direct || Some(p) != self.preview) {
                    if let Some(i) = self.input_mut(p) {
                        i.playing = false;
                    }
                }
            }
        } else if let Some(p) = old {
            let per_input = self.input(p).is_some_and(|i| i.pause_preview);
            if per_input && p != to && (!direct || Some(p) != self.preview) {
                if let Some(i) = self.input_mut(p) {
                    i.playing = false;
                }
            }
        }
        self.trans = None;
    }

    /// ms::zzzz @0x131c20: start a transition to `to`.
    fn begin(&mut self, to: u32, effect: Effect, ms: u32, direct: bool) {
        if self.trans.is_some() {
            return;
        }
        let ms = sanitize_ms(ms);
        // Effect 100 (Cut) runs with step 255: it lands on the next output frame.
        let step = if effect == Effect::Cut { FULL } else { step_for(ms) };
        self.on_transition_start(to);
        self.trans = Some(Trans { from: self.output, to, effect, progress: 0.0, step, manual: false, direct, stinger: None });
    }

    pub fn start_transition(&mut self, effect: Effect, ms: u32) {
        let Some(to) = self.preview else { return };
        self.begin(to, effect, ms, false);
    }

    pub fn start_transition_button(&mut self, index: usize) {
        let button = self.transitions[index];
        let Some(source) = button.stinger else {
            self.start_transition(button.effect, button.ms);
            return;
        };
        let Some(to) = self.preview else { return };
        if self.trans.is_some() {
            return;
        }
        if source == to || self.output == Some(source) {
            self.notify("Select another input as Preview/Output to use this GT Stinger.");
            return;
        }
        let durations = self.input(source).and_then(|input| {
            Some((input.kind.gt_seconds(false)?, input.kind.gt_seconds(true)?))
        });
        let Some((enter, exit)) = durations else {
            self.notify("A GT Stinger requires both Transition In and Transition Out.");
            return;
        };
        let frames = |seconds: f32| (seconds * output_fps()).round().max(1.0);
        let (enter, exit) = (frames(enter), frames(exit));
        let step = FULL / (enter + exit);
        self.trans = Some(Trans {
            from: self.output, to, effect: Effect::Cut, progress: 0.0, step,
            manual: false, direct: false, stinger: Some(Stinger { input: source, cut: enter * step, switched: false }),
        });
    }

    /// Per-input Quick Play: that input goes to Output with the Quick Play effect; Preview kept.
    pub fn quick_play_input(&mut self, id: u32) {
        let (e, ms) = (self.quickplay_effect, self.quickplay_ms);
        self.begin(id, e, ms, true);
    }

    /// Per-input Cut: straight to Output (effect 100), Preview kept.
    pub fn cut_direct(&mut self, id: u32) {
        self.begin(id, Effect::Cut, 0, true);
    }

    /// A given input straight to Output with an effect and duration (shortcut functions with an
    /// input number, e.g. "Fade" input 1 1000 ms in the Default Keyboard template).
    pub fn transition_input(&mut self, id: u32, effect: Effect, ms: u32) {
        self.begin(id, effect, ms, true);
    }

    /// Shortcut functions PreviewInputNext / PreviewInputPrevious.
    pub fn preview_step(&mut self, delta: i32) {
        if self.inputs.is_empty() {
            return;
        }
        let n = self.inputs.len() as i32;
        let cur = self.preview.map(|p| self.number_of(p) as i32 - 1).unwrap_or(-1);
        let next = (cur + delta).rem_euclid(n) as usize;
        self.preview = Some(self.inputs[next].id);
    }

    /// Runs one shortcut / MIDI / Trigger action. Keyboard shortcuts, MIDI notes and
    /// input Triggers all use these action strings (the original's `Shortcut.Function`
    /// plus input/duration, e.g. "Preview 1", "CutDirect 2", "FadeDirect 1 500",
    /// "Fade 500", "SavePreset").
    pub fn perform_action(&mut self, action: &str, now: f64) -> Result<(), String> {
        let text = action.trim();
        if text.is_empty() {
            return Err("Empty action".into());
        }
        if text.len() > 256 {
            return Err("Action is too long".into());
        }
        let fields: Vec<&str> = text.split_whitespace().collect();
        let op = fields.first().copied().unwrap_or_default().to_ascii_lowercase();
        let number = |text: &str| text.parse::<usize>().ok().filter(|n| *n >= 1);
        let duration = |text: &str| text.parse::<u32>().ok().filter(|ms| *ms <= 60_000);
        let target = |s: &Self, n: usize| s.inputs.get(n - 1).map(|input| input.id);
        match (op.as_str(), fields.len()) {
            ("cut", 1) | ("cutpreview", 1) => self.start_transition(Effect::Cut, 0),
            ("previewnext", 1) | ("previewinputnext", 1) => self.preview_step(1),
            ("previewprevious", 1) | ("previewinputprevious", 1) => self.preview_step(-1),
            ("savepreset", 1) => {
                if !self.save_preset() {
                    return Err("Preset could not be saved".into());
                }
            }
            ("startrecording", 1) => {
                if self.rec.is_none() {
                    self.rec = Some(now);
                }
            }
            ("stoprecording", 1) => self.rec = None,
            ("startstreaming", 1) => self.stream_active[0] = true,
            ("stopstreaming", 1) => self.stream_active[0] = false,
            ("startmulticorder", 1) => {
                if self.multicorder.is_none() {
                    self.multicorder = Some(now);
                }
            }
            ("stopmulticorder", 1) => self.multicorder = None,
            ("fadetoblack", 1) | ("ftb", 1) => self.ftb_on = !self.ftb_on,
            ("preview", 2) => {
                let n = number(fields[1]).ok_or_else(|| format!("Invalid input in {text:?}"))?;
                let id = target(self, n).ok_or_else(|| format!("Input {n} is unavailable"))?;
                self.preview = Some(id);
            }
            ("cutdirect", 2) | ("cut", 2) => {
                let n = number(fields[1]).ok_or_else(|| format!("Invalid input in {text:?}"))?;
                if n == 0 {
                    self.start_transition(Effect::Cut, 0);
                } else {
                    let id = target(self, n).ok_or_else(|| format!("Input {n} is unavailable"))?;
                    self.cut_direct(id);
                }
            }
            ("fade", 2) => {
                let ms = duration(fields[1]).ok_or_else(|| format!("Invalid duration in {text:?}"))?;
                self.start_transition(Effect::Fade, ms);
            }
            ("fadedirect", 3) | ("fade", 3) => {
                let n = number(fields[1]).ok_or_else(|| format!("Invalid input in {text:?}"))?;
                let ms = duration(fields[2]).ok_or_else(|| format!("Invalid duration in {text:?}"))?;
                if n == 0 {
                    self.start_transition(Effect::Fade, ms);
                } else {
                    let id = target(self, n).ok_or_else(|| format!("Input {n} is unavailable"))?;
                    self.transition_input(id, Effect::Fade, ms);
                }
            }
            ("overlay", 2) => {
                let n = number(fields[1]).ok_or_else(|| format!("Invalid overlay in {text:?}"))?;
                let channel = (n - 1).min(OVERLAY_CHANNELS - 1);
                self.toggle_overlay_channel(channel);
            }
            ("external", 2) => {
                let n = number(fields[1]).ok_or_else(|| format!("Invalid output in {text:?}"))?;
                if n <= OUTPUT_COUNT {
                    self.toggle_external_route(n - 1);
                } else {
                    return Err(format!("Output {n} does not exist"));
                }
            }
            _ => return Err(format!("Unknown action {text:?}")),
        }
        Ok(())
    }

    /// T-bar position as 0..1 for drawing.
    pub fn tbar_value(&self) -> f32 {
        self.tbar / FULL
    }

    /// ms::zzzzzze @0x133cc0 + hm::set_s @0x8ad04. `v` is the bar position 0..1.
    pub fn set_tbar(&mut self, v: f32) {
        if matches!(&self.trans, Some(t) if !t.manual) {
            return;
        }
        let value = (v * FULL).round().clamp(0.0, FULL);
        self.tbar = value;
        let progress = if self.tbar_rest_bottom { value } else { FULL - value };
        if self.trans.is_none() {
            if progress <= 0.0 {
                return;
            }
            let Some(to) = self.preview else { return };
            self.on_transition_start(to);
            // hm::set_s uses the effect of transition 1 (ii::get_a).
            let e = self.transitions[0].effect;
            self.trans = Some(Trans { from: self.output, to, effect: if e == Effect::Cut { Effect::Fade } else { e }, progress, step: 0.0, manual: true, direct: false, stinger: None });
        }
        if let Some(t) = &mut self.trans {
            t.progress = progress;
            if progress >= FULL {
                let to = t.to;
                self.finish_transition(to, false);
                self.tbar_rest_bottom = !self.tbar_rest_bottom;
            } else if progress <= 0.0 {
                self.trans = None;
            }
        }
    }

    /// Assign a source without affecting the current output of the channel.
    pub fn assign_overlay(&mut self, channel: usize, id: u32, on_preview: bool) {
        if self.input(id).is_some() {
            if let Some(ch) = self.overlays.get_mut(channel) {
                ch.assigned = Some(OverlaySource { input: id, on_preview });
            }
        }
    }

    /// An input button selects and activates its source, or clears it on a second click.
    pub fn press_input_overlay(&mut self, channel: usize, id: u32, on_preview: bool) {
        let Some(ch) = self.overlays.get_mut(channel) else { return };
        if ch.assigned.is_some_and(|source| source.input == id) {
            ch.assigned = None;
            if ch.input == Some(id) {
                ch.on = false;
            }
            return;
        }
        if self.input(id).is_none() { return; }
        self.assign_overlay(channel, id, on_preview);
        self.activate_overlay(channel);
    }

    /// Centre overlay buttons. Turning a channel off leaves its assignment ready for the next use.
    pub fn toggle_overlay_channel(&mut self, c: usize) -> bool {
        let Some(ch) = self.overlays.get_mut(c) else { return false };
        if ch.on {
            ch.on = false;
            return true;
        }
        self.activate_overlay(c)
    }

    fn activate_overlay(&mut self, channel: usize) -> bool {
        let Some(source) = self.overlays[channel].assigned else { return false };
        let authored_in = self.input(source.input).and_then(|inp| inp.kind.gt_seconds(false)).is_some();
        let ch = &mut self.overlays[channel];
        if ch.effect == Effect::Cut && !authored_in {
            ch.level = FULL;
        } else if ch.input != Some(source.input) || ch.on_preview != source.on_preview {
            ch.level = 0.0;
        }
        ch.input = Some(source.input);
        ch.on_preview = source.on_preview;
        ch.on = true;
        true
    }

    /// MultiView `mv` (0 or 1) resolved: each box with the source it shows (None = empty).
    /// Automatic boxes take the inputs in list order.
    pub fn mv_resolve(&self, mv: usize) -> Vec<([f32; 4], Option<u32>)> {
        let mv = mv.min(1);
        let custom = &self.cfg.mv_boxes[mv];
        let mut next = self.inputs.iter();
        mv_layout_boxes(self.cfg.mv_layout[mv])
            .into_iter()
            .enumerate()
            .map(|(k, (r, def))| {
                let v = custom.get(k).copied().unwrap_or(def);
                let src = if v == MV_AUTO {
                    next.next().map(|i| i.id + 1)
                } else if v == SRC_OUTPUT || v == SRC_PREVIEW || self.input(v.wrapping_sub(1)).is_some() {
                    Some(v)
                } else {
                    None
                };
                (r, src)
            })
            .collect()
    }

    /// How long the PlayList stays on the input now on Output: a video file's own length,
    /// otherwise the Duration (PlaylistItem: inputs that can't seek get 5 s).
    pub fn playlist_item_secs(&self) -> f32 {
        match self.output.and_then(|o| self.input(o)) {
            Some(i) if matches!(i.kind, Kind::VideoFile { .. } | Kind::VideoList { .. }) && i.duration > 0.0 => i.duration,
            _ => self.playlist_secs,
        }
    }

    /// The input the PlayList goes to next: the next ticked input after the one on Output, in
    /// list order. After the last one it starts again when Loop is on, otherwise it ends (None).
    pub fn playlist_next(&self) -> Option<u32> {
        let items: Vec<u32> = self.inputs.iter().map(|i| i.id).filter(|id| !self.playlist_off.contains(id)).collect();
        if items.is_empty() {
            return None;
        }
        let cur = self.output.and_then(|o| self.inputs.iter().position(|i| i.id == o));
        let after = cur.and_then(|c| self.inputs[c + 1..].iter().map(|i| i.id).find(|id| items.contains(id)));
        match (cur, after) {
            (_, Some(n)) => Some(n),
            (None, None) => Some(items[0]),
            (Some(_), None) if self.cfg.playlist_loop => Some(items[0]),
            _ => None,
        }
    }

    /// Drag and drop of an input onto another (ms::zzzzzb @0x1324bc → ms::zzzzzc @0x132560):
    /// the dragged input takes the place (list index) of the input it was dropped on, the
    /// others shift along. Input numbers follow the new order.
    pub fn move_input(&mut self, from: u32, onto: u32) {
        if from == onto {
            return;
        }
        let (Some(a), Some(b)) = (self.inputs.iter().position(|i| i.id == from), self.inputs.iter().position(|i| i.id == onto)) else { return };
        let inp = self.inputs.remove(a);
        self.inputs.insert(b, inp);
    }

    pub fn remove_input(&mut self, id: u32) {
        for button in &mut self.transitions {
            if button.stinger == Some(id) {
                button.stinger = None;
            }
        }
        if self.trans.as_ref().and_then(|tr| tr.stinger.as_ref()).is_some_and(|stinger| stinger.input == id) {
            self.trans = None;
        }
        self.inputs.retain(|i| i.id != id);
        for mv in &mut self.cfg.mv_boxes {
            for b in mv.iter_mut().filter(|b| **b == id + 1) {
                *b = MV_AUTO;
            }
        }
        for src in self.cfg.outputs.iter_mut().map(|out| &mut out.source)
            .chain(std::iter::once(&mut self.cfg.fs_source))
        {
            if *src == id + 1 {
                *src = SRC_OUTPUT;
            }
        }
        for i in &mut self.inputs {
            for l in i.layers.iter_mut().filter(|l| l.input == Some(id)) {
                *l = Layer::default();
            }
        }
        if self.preview == Some(id) {
            self.preview = None;
        }
        if self.input_preview == Some(id) {
            self.input_preview = None;
        }
        if self.output == Some(id) {
            self.output = None;
        }
        if matches!(&self.trans, Some(t) if t.to == id || t.from == Some(id)) {
            self.trans = None;
        }
        for ch in &mut self.overlays {
            if ch.assigned.is_some_and(|source| source.input == id) {
                ch.assigned = None;
            }
            if ch.input == Some(id) {
                ch.input = None;
                ch.on = false;
                ch.level = 0.0;
            }
        }
        self.confirm_close = None;
    }

    // ------------------------------------------------------------ time

    /// One output frame: hm::r @0x8b760 (transition) and rs::t @0x1b0890 (layers).
    fn output_frame(&mut self) {
        let mut done = None;
        let mut stinger_cut = None;
        let mut stinger_done = false;
        if let Some(tr) = &mut self.trans {
            if !tr.manual {
                tr.progress += tr.step;
                if let Some(stinger) = &mut tr.stinger {
                    if !stinger.switched && tr.progress + 0.001 >= stinger.cut {
                        stinger.switched = true;
                        stinger_cut = Some(tr.to);
                    }
                    stinger_done = tr.progress + 0.001 >= FULL;
                } else if tr.progress >= FULL {
                    done = Some((tr.to, tr.direct));
                }
            }
        }
        if let Some(to) = stinger_cut {
            let current = self.trans.take();
            self.on_transition_start(to);
            self.finish_transition(to, false);
            self.trans = current;
        }
        if stinger_done {
            self.trans = None;
        }
        if let Some((to, direct)) = done {
            self.finish_transition(to, direct);
        }
        // Fade To Black: a black colour layer faded with the FTB duration (me::.ctor @0x11edbc).
        let ftb_step = step_for(sanitize_ms(self.ftb_ms));
        self.ftb_level = if self.ftb_on { (self.ftb_level + ftb_step).min(FULL) } else { (self.ftb_level - ftb_step).max(0.0) };
        for ch in &mut self.overlays {
            let storyboard = ch.input.and_then(|id| self.inputs.iter().find(|i| i.id == id))
                .and_then(|inp| inp.kind.gt_seconds(!ch.on));
            let step = match storyboard {
                Some(seconds) => FULL / (seconds * output_fps() as f32).round().max(1.0),
                None if ch.effect == Effect::Cut => FULL,
                None => step_for(sanitize_ms(ch.ms)),
            };
            if ch.on {
                ch.level = (ch.level + step).min(FULL);
            } else {
                ch.level = (ch.level - step).max(0.0);
                if ch.level <= 0.0 {
                    ch.input = None;
                }
            }
        }
    }

    pub fn tick(&mut self, t: f64, dt: f32) {
        self.now = t;
        if !self.notice.is_empty() && t > self.notice_until {
            self.notice.clear();
        }
        // Advance on the output frame clock.
        self.frame_acc += dt.min(0.5);
        let frame = 1.0 / output_fps();
        while self.frame_acc >= frame {
            self.frame_acc -= frame;
            self.output_frame();
        }
        // playback
        for i in &mut self.inputs {
            if i.playing && matches!(i.kind, Kind::Video { .. }) {
                i.pos += dt;
                if i.pos >= i.duration {
                    if i.loop_on {
                        i.pos = 0.0;
                    } else {
                        i.pos = i.duration;
                        i.playing = false;
                    }
                }
            }
        }
        // PlayList: when the input on Output has had its time, the next one goes to Output with
        // the item transition — PlaylistItem::.ctor (0x06006d87 @0x2c201c): Cut (100), 500 ms;
        // a video plays its own length, anything that can't seek plays the Duration (5 s).
        if let Some(last) = self.playlist {
            if t - last >= self.playlist_item_secs() as f64 && self.trans.is_none() {
                match self.playlist_next() {
                    Some(next) => {
                        self.preview = Some(next);
                        self.start_transition(PLAYLIST_EFFECT, PLAYLIST_TRANSITION_MS);
                        self.playlist = Some(t);
                    }
                    None => self.playlist = None,
                }
            }
        }
    }

    // ------------------------------------------------------------ add input

    /// Devices offered for the selected kind (Blackmagic tab shows only Blackmagic devices).
    pub fn dialog_devices(&self) -> Vec<DeviceChoice> {
        let d = &self.add_dialog;
        d.devices.iter().filter(|x| match d.kind {
            AddKind::AudioInput => x.link.starts_with("audio:"),
            AddKind::Blackmagic => x.blackmagic,
            _ => !x.link.starts_with("audio:"),
        }).cloned().collect()
    }

    /// Adds the input described by the Add Input dialog. Returns an error to show, if any.
    pub fn add_from_dialog(&mut self) -> Result<(), String> {
        let devices = self.dialog_devices();
        let d = &self.add_dialog;
        let n = self.inputs.len() + 1;
        let seed = self.next_id;
        let file_name = std::path::Path::new(d.path.trim()).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let (kind, default_name) = match d.kind {
            AddKind::Camera | AddKind::Blackmagic | AddKind::AudioInput => {
                let Some(dev) = devices.get(d.device) else {
                    return Err(if d.kind == AddKind::Blackmagic {
                        "No Blackmagic device found. Connect it and install Blackmagic Desktop Video, then press Refresh.".into()
                    } else if d.kind == AddKind::AudioInput {
                        "No audio input found. Check microphone permissions and connected devices, then press Refresh.".into()
                    } else {
                        "No camera or capture device found. Connect one and press Refresh.".into()
                    });
                };
                (Kind::Device { link: dev.link.clone(), blackmagic: dev.blackmagic }, dev.name.clone())
            }
            AddKind::VideoFile | AddKind::ImageFile => {
                let p = d.path.trim().to_string();
                if p.is_empty() || !std::path::Path::new(&p).exists() {
                    return Err("Choose a file first (Browse…).".into());
                }
                if d.kind == AddKind::VideoFile {
                    (Kind::VideoFile { path: p }, file_name)
                } else {
                    (Kind::ImageFile { path: p }, file_name)
                }
            }
            AddKind::List => {
                let paths: Vec<String> = d.list.iter().filter(|p| std::path::Path::new(p).exists()).cloned().collect();
                if paths.is_empty() {
                    return Err("Add one or more video files to the list first (Add…).".into());
                }
                (Kind::VideoList { paths, idx: 0 }, "List".into())
            }
            AddKind::Desktop => match &d.ndi_source {
                Some(link) => (Kind::Device { link: link.clone(), blackmagic: false }, link[4..].to_owned()),
                None => (Kind::Desktop, "Desktop Capture".into()),
            },
            AddKind::StreamSrt => {
                d.srt.url()?;
                let link = format!("srt:{}", serde_json::to_string(&d.srt).map_err(|e| e.to_string())?);
                (Kind::Device { link, blackmagic: false }, d.srt.label())
            }
            AddKind::Replay => (Kind::Replay { path: String::new() }, "Instant Replay".into()),
            AddKind::ImageSequence | AddKind::Photos => {
                let f = d.folder.trim().to_string();
                if f.is_empty() || !std::path::Path::new(&f).is_dir() {
                    return Err("Choose a folder with pictures first (Browse…).".into());
                }
                let fname = std::path::Path::new(&f).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                if d.kind == AddKind::Photos {
                    (Kind::Photos { folder: f }, fname)
                } else {
                    (Kind::ImageSequence { folder: f }, fname)
                }
            }
            AddKind::TestVideo => (Kind::Video { hue: (seed as f32 * 0.17) % 1.0 }, "Test Video".into()),
            AddKind::TestCamera => (Kind::Camera(seed), format!("Test Camera {n}")),
            AddKind::TestImage => (Kind::Image(seed), "Test Image".into()),
            AddKind::Colour => match d.colour_mode {
                1 => (Kind::Blank, "Transparent".into()),
                2 => (Kind::Bars, "Colour Bars".into()),
                _ => (Kind::Colour(d.colour), "Colour".into()),
            },
            AddKind::Title => {
                let path = std::path::Path::new(d.path.trim());
                if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("gtzip")) {
                    return Err("Choose a GT Designer title (.gtzip).".into());
                }
                let document = crate::gt::Document::load(path)?;
                let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
                (Kind::GtTitle {
                    path: path.to_string_lossy().into_owned(),
                    texts: Default::default(),
                    document: Some(std::sync::Arc::new(document)),
                }, name)
            }
            AddKind::Clock => (Kind::Clock, "Production Clock".into()),
            AddKind::VideoDelay => {
                let Some(source) = d.delay_source.filter(|id| self.inputs.iter().any(|i| i.id == *id)) else {
                    return Err("Choose the input the Video Delay copies first.".into());
                };
                let resolution = DELAY_RESOLUTIONS[d.delay_resolution.min(DELAY_RESOLUTIONS.len() - 1)];
                let seconds = d.delay_seconds.clamp(0.0, 120.0);
                (Kind::VideoDelay { source: Some(source), seconds, resolution },
                    format!("Video Delay ({seconds:.1}s)"))
            }
            AddKind::Telestrator => (Kind::Telestrator { strokes: Vec::new() }, "Telestrator".into()),
            k => return Err(format!("{} is not available in this version yet.", k.label())),
        };
        let name = if d.name.trim().is_empty() { default_name } else { d.name.trim().to_string() };
        let mut i = Input::new(0, &name, kind);
        if d.kind == AddKind::Replay {
            i.playing = true;
        }
        i.decklink_port = d.decklink_port;
        i.decklink_mode = d.decklink_mode;
        if d.kind == AddKind::TestVideo {
            i.duration = d.length;
        }
        if matches!(d.kind, AddKind::VideoFile | AddKind::List | AddKind::Replay | AddKind::ImageSequence | AddKind::Photos) {
            i.duration = 0.0;
        }
        if matches!(d.kind, AddKind::ImageSequence | AddKind::Photos) {
            i.loop_on = true;
        }
        let id = self.add(i);
        if self.preview.is_none() {
            self.preview = Some(id);
        }
        self.add_dialog.name.clear();
        Ok(())
    }

    // ------------------------------------------------------------ presets

    fn legacy_preset_path() -> std::path::PathBuf {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("livemix_preset.txt")
    }

    fn last_preset_path() -> std::path::PathBuf {
        crate::media::storage_folder().join("last-preset.txt")
    }

    fn remember_preset(&mut self, path: &std::path::Path) -> Result<(), String> {
        self.preset_name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        self.preset_file = Some(path.to_path_buf());
        self.saved_text = Some(self.preset_text());
        crate::preset::write_file(&Self::last_preset_path(), path.to_string_lossy().as_bytes())
    }

    pub fn save_preset(&mut self) -> bool {
        match self.preset_file.clone().filter(|path| path.extension().is_some_and(|v| v.eq_ignore_ascii_case("vmix"))) {
            Some(path) => self.save_preset_to(&path),
            None => {
                self.browse_req = Some(crate::media::PickKind::PresetSave);
                false
            }
        }
    }

    pub fn save_preset_to(&mut self, path: &std::path::Path) -> bool {
        if !path.extension().is_some_and(|v| v.eq_ignore_ascii_case("vmix")) {
            self.notify("Save the project with the .vmix extension.");
            return false;
        }
        match crate::preset::encode(self).and_then(|xml| crate::preset::write_file(path, xml.as_bytes())) {
            Ok(_) => {
                let last = self.remember_preset(path);
                self.notify(&match last {
                    Ok(()) => format!("Preset saved: {}", path.display()),
                    Err(error) => format!("Preset saved. Could not update Last: {error}"),
                });
                true
            }
            Err(e) => {
                self.notify(&format!("Could not save preset: {e}"));
                false
            }
        }
    }

    /// Unsaved changes — sr::c (0x06004664 @0x1beab4) with sr::get_e (@0x1bea6c): no inputs →
    /// nothing to save; otherwise the preset is compared with the copy kept when it was last
    /// saved / opened / made new, and a preset never saved counts as changed.
    pub fn preset_changed(&self) -> bool {
        if self.inputs.is_empty() {
            return false;
        }
        match &self.saved_text {
            Some(t) => *t != self.preset_text(),
            None => true,
        }
    }

    /// Recording, External Output, MultiCorder (or Streaming) running: the original refuses a
    /// new or opened preset then (frmmain::zzr @0x24ffe8, error 11004 "ClosePresets").
    pub fn outputs_busy(&self) -> bool {
        self.rec.is_some() || self.multicorder.is_some() || self.replay_arm || self.external_active()
            || self.stream_active.contains(&true)
            || self.srt_active.contains(&true)
    }

    pub fn external_active(&self) -> bool {
        self.external.iter().any(|window| window.open) || self.decklink_active.contains(&true)
    }

    pub fn toggle_external_route(&mut self, channel: usize) {
        if self.cfg.decklink_outputs[channel].device.is_some() {
            self.decklink_active[channel] = !self.decklink_active[channel];
        } else {
            self.toggle_external(channel);
        }
    }

    pub fn toggle_external(&mut self, channel: usize) {
        let window = &mut self.external[channel];
        window.open = !window.open;
        window.fullscreen = false;
    }

    /// The New button — frmmain::zzr (0x06005ae8 @0x24ffe8): refused while outputs run, then
    /// "save changes?" when there are unsaved changes, then the New Preset window.
    pub fn press_new(&mut self) {
        if self.outputs_busy() {
            self.notify(STOP_OUTPUTS_MSG);
            return;
        }
        // frmNewPreset::d (@0x259698) preselects the current OutputSize / FrameRate when they are
        // in its lists; otherwise the app falls back to 1920x1080 59.94i.
        let size = if NEW_PRESET_SIZES.contains(&self.cfg.out_size) { self.cfg.out_size } else { 6 };
        let (rate, il) = if NEW_PRESET_RATES.iter().any(|r| r.1 == self.cfg.out_rate && r.2 == self.cfg.interlaced) {
            (self.cfg.out_rate, self.cfg.interlaced)
        } else {
            (6, true)
        };
        self.new_format = (size, rate, il);
        self.new_step = if self.preset_changed() { NewStep::SaveChanges } else { NewStep::Format };
    }

    /// New Preset OK — frmNewPreset::c (0x06005c39 @0x25960c): set the chosen video format
    /// (ss::d) and start a blank preset (ss::c).
    pub fn new_preset_with(&mut self, size: u8, rate: u8, interlaced: bool) {
        self.cfg.out_size = size.min(OUTPUT_SIZES.len() as u8 - 1);
        self.cfg.out_rate = rate.min(FRAME_RATES.len() as u8 - 1);
        self.cfg.interlaced = interlaced;
        set_output_fps(self.cfg.rate());
        self.save_settings();
        self.new_preset();
    }

    /// The preset as text (inputs, Preview/Output, positions, layers, transitions).
    pub fn preset_text(&self) -> String {
        let mut out = String::from("livemix-preset 1\n");
        for i in &self.inputs {
            let (code, a, b) = match &i.kind {
                Kind::Colour(c) => ("colour", format!("{},{},{}", c.r(), c.g(), c.b()), String::new()),
                Kind::Bars => ("bars", String::new(), String::new()),
                Kind::Title { text, sub } => ("title", text.clone(), sub.clone()),
                Kind::GtTitle { path, texts, .. } => (
                    "gttitle", path.clone(), serde_json::to_string(texts).unwrap_or_default(),
                ),
                Kind::Blank => ("blank", String::new(), String::new()),
                Kind::Clock => ("clock", String::new(), String::new()),
                Kind::Camera(s) => ("camera", s.to_string(), String::new()),
                Kind::Video { hue } => ("video", hue.to_string(), String::new()),
                Kind::Image(s) => ("image", s.to_string(), String::new()),
                Kind::Device { link, blackmagic } => ("device", link.clone(), (*blackmagic as u8).to_string()),
                Kind::VideoFile { path } => ("videofile", path.clone(), String::new()),
                Kind::ImageFile { path } => ("imagefile", path.clone(), String::new()),
                Kind::VideoList { paths, .. } => ("list", paths.join("|"), String::new()),
                Kind::Replay { path } => ("replay", path.clone(), String::new()),
                Kind::Desktop => ("desktop", String::new(), String::new()),
                Kind::ImageSequence { folder } => ("imagesequence", folder.clone(), String::new()),
                Kind::Photos { folder } => ("photos", folder.clone(), String::new()),
                Kind::VideoDelay { source, seconds, resolution } => ("videodelay",
                    self.number_of(source.unwrap_or(0)).to_string(),
                    format!("{},{},{}", resolution[0], resolution[1], seconds)),
                Kind::Telestrator { strokes } => ("telestrator",
                    serde_json::to_string(strokes).unwrap_or_else(|_| "[]".into()), String::new()),
            };
            let cat = i.category.map(|c| c.to_string()).unwrap_or_default();
            let k = i.key;
            out.push_str(&format!(
                "input\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{},{},{}\t{}\n",
                code,
                clean(&i.title),
                clean(&a),
                clean(&b),
                i.duration,
                i.volume,
                cat,
                i.loop_on as u8,
                k.on as u8,
                k.colour[0],
                k.colour[1],
                k.colour[2],
                k.tolerance
            ));
        }
        let num = |o: Option<u32>| o.map(|i| self.number_of(i)).unwrap_or(0);
        out.push_str(&format!("preview\t{}\noutput\t{}\n", num(self.preview), num(self.output)));
        // Position and layers of each input (by input number).
        let fmt_pos = |p: &Position| format!("{},{},{},{},{},{},{}", p.zoom, p.pan_x, p.pan_y, p.crop[0], p.crop[1], p.crop[2], p.crop[3]);
        for (n, i) in self.inputs.iter().enumerate() {
            out.push_str(&format!("input_options\t{}\t{}\t{}\t{}\t{}\t{}\n",
                n + 1, i.collapsed as u8, i.audio_on as u8, i.bus_m as u8, i.bus_a as u8, i.bus_b as u8));
            if i.place != Position::default() {
                out.push_str(&format!("place\t{}\t{}\n", n + 1, fmt_pos(&i.place)));
            }
            let bits = i.mirror as u16
                | (i.sharpen as u16) << 1
                | (i.deinterlace as u16) << 2
                | (i.play_active as u16) << 3
                | (i.pause_preview as u16) << 4
                | (i.restart_active as u16) << 5
                | (i.auto_mix_audio as u16) << 6
                | (i.rec601 as u16) << 7
                | (i.premultiplied as u16) << 8
                | (i.flatten as u16) << 9
                | (i.effects_source as u16) << 10;
            if bits != 0 {
                out.push_str(&format!("input_flags\t{}\t{}\n", n + 1, bits));
            }
            if i.aspect != 0.0 {
                out.push_str(&format!("aspect\t{}\t{}\n", n + 1, i.aspect));
            }
            if let Some([r, g, b]) = i.tally_colour {
                out.push_str(&format!("tally\t{}\t{r}\t{g}\t{b}\n", n + 1));
            }
            if i.click_action != 0 || i.delay_frames != 0 {
                out.push_str(&format!("advanced\t{}\t{}\t{}\n", n + 1, i.click_action, i.delay_frames));
            }
            let c = i.colour_adjust;
            if c != ColourAdjust::default() {
                out.push_str(&format!("colour\t{}\t{},{},{},{},{},{},{},{},{}\n", n + 1, c.brightness,
                    c.contrast, c.saturation, c.red, c.green, c.blue, c.alpha, c.black, c.white));
            }
            if i.audio_effects != crate::audio::Effects::default() {
                if let Ok(fx) = serde_json::to_string(&i.audio_effects) {
                    out.push_str(&format!("audio_effects\t{}\t{fx}\n", n + 1));
                }
            }
            if let Some(m) = &i.cam_mode {
                out.push_str(&format!("cammode\t{}\t{}\n", n + 1, m.code()));
            }
            if i.decklink_port != 0 || i.decklink_mode != 0 {
                out.push_str(&format!("decklink_port\t{}\t{}\t{}\n", n + 1, i.decklink_port, i.decklink_mode));
            }
            for (k, l) in i.layers.iter().enumerate() {
                if l.input.is_some() {
                    out.push_str(&format!("layer\t{}\t{}\t{}\t{}\t{}\t{}\t{},{},{}\t{}\n", n + 1, k,
                        l.on as u8, num(l.input), fmt_pos(&l.place), l.border as u8,
                        l.border_colour[0], l.border_colour[1], l.border_colour[2], l.border_width));
                }
            }
        }
        for (k, tb) in self.transitions.iter().enumerate() {
            out.push_str(&format!("transition\t{}\t{}\t{}\t{}\n", k, tb.effect.code(), tb.ms, num(tb.stinger)));
        }
        for (channel, overlay) in self.overlays.iter().enumerate() {
            if let Some(source) = overlay.assigned {
                out.push_str(&format!("overlay_source\t{}\t{}\t{}\n",
                    channel, self.number_of(source.input), source.on_preview as u8));
            }
        }
        out
    }

    pub fn load_last_preset(&mut self) {
        let path = std::fs::read_to_string(Self::last_preset_path()).ok()
            .filter(|p| !p.trim().is_empty()).map(std::path::PathBuf::from);
        let path = path.or_else(|| {
            let legacy = Self::legacy_preset_path();
            legacy.is_file().then_some(legacy)
        });
        if let Some(path) = path {
            self.load_preset_from(&path);
        } else {
            self.notify("No recent preset yet. Open or save a project first.");
        }
    }

    pub fn load_preset_from(&mut self, path: &std::path::Path) {
        if self.outputs_busy() {
            self.notify(STOP_OUTPUTS_MSG);
            return;
        }
        let now = self.now;
        let result = std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|text| {
            if text.starts_with("livemix-preset 1") {
                self.load_preset_text(&text);
                Ok(Vec::new())
            } else {
                crate::preset::decode(self, path, &text)
            }
        });
        match result {
            Ok(warnings) => {
                if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("vmix")) {
                    self.import_report = Some(crate::preset::ImportReport::collect(path, &self.inputs, warnings.clone()));
                }
                self.now = now;
                let last = self.remember_preset(path);
                set_output_fps(self.cfg.rate());
                self.notify(&match (last, warnings.is_empty()) {
                    (_, false) => format!("Preset opened. Unsupported inputs shown as blank: {}", warnings.join(", ")),
                    (Err(error), _) => format!("Preset opened. Could not update Last: {error}"),
                    (Ok(()), _) => format!("Preset opened: {}", path.display()),
                });
            }
            Err(error) => self.notify(&format!("Could not open preset: {error}")),
        }
    }

    pub(crate) fn load_preset_text(&mut self, text: &str) {
        let mut s = Self::empty();
        let mut ids = Vec::new();
        // Video Delay source input numbers, resolved once every input exists.
        let mut delay_sources: Vec<(usize, usize)> = Vec::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.first().copied() {
                Some("input") if f.len() >= 9 => {
                    let kind = match f[1] {
                        "colour" => {
                            let c: Vec<u8> = f[3].split(',').filter_map(|x| x.parse().ok()).collect();
                            Kind::Colour(if c.len() == 3 { Color32::from_rgb(c[0], c[1], c[2]) } else { Color32::BLACK })
                        }
                        "bars" => Kind::Bars,
                        "title" => Kind::Title { text: f[3].into(), sub: f[4].into() },
                        "gttitle" => Kind::GtTitle {
                            path: f[3].into(),
                            texts: serde_json::from_str(f[4]).unwrap_or_default(),
                            document: None,
                        },
                        "clock" => Kind::Clock,
                        "camera" => Kind::Camera(f[3].parse().unwrap_or(1)),
                        "video" => Kind::Video { hue: f[3].parse().unwrap_or(0.5) },
                        "image" => Kind::Image(f[3].parse().unwrap_or(1)),
                        "device" => Kind::Device { link: f[3].into(), blackmagic: f[4] == "1" },
                        "videofile" => Kind::VideoFile { path: f[3].into() },
                        "imagefile" => Kind::ImageFile { path: f[3].into() },
                        "list" => Kind::VideoList { paths: f[3].split('|').filter(|p| !p.is_empty()).map(String::from).collect(), idx: 0 },
                        "replay" => Kind::Replay { path: f[3].into() },
                        "desktop" => Kind::Desktop,
                        "imagesequence" => Kind::ImageSequence { folder: f[3].into() },
                        "photos" => Kind::Photos { folder: f[3].into() },
                        "videodelay" => {
                            let r: Vec<u32> = f[4].split(',').take(2).filter_map(|x| x.parse().ok()).collect();
                            Kind::VideoDelay {
                                source: None,
                                seconds: f[4].split(',').nth(2).and_then(|x| x.parse().ok()).unwrap_or(1.0),
                                resolution: if r.len() == 2 { [r[0], r[1]] } else { DELAY_RESOLUTIONS[3] },
                            }
                        }
                        "telestrator" => Kind::Telestrator { strokes: serde_json::from_str(f[3]).unwrap_or_default() },
                        _ => Kind::Blank,
                    };
                    let mut i = Input::new(0, f[2], kind);
                    if matches!(i.kind, Kind::GtTitle { .. }) {
                        if let Err(error) = i.reload_gt() {
                            i.status = error;
                        }
                    }
                    i.duration = f[5].parse().unwrap_or(60.0);
                    i.volume = f[6].parse().unwrap_or(80.0);
                    i.category = f[7].parse().ok();
                    i.loop_on = f[8] == "1";
                    if f.len() >= 12 {
                        let c: Vec<u8> = f[10].split(',').filter_map(|x| x.parse().ok()).collect();
                        i.key = Key {
                            on: f[9] == "1",
                            colour: if c.len() == 3 { [c[0], c[1], c[2]] } else { Key::default().colour },
                            tolerance: f[11].parse().unwrap_or(60),
                        };
                    }
                    if matches!(i.kind, Kind::VideoDelay { .. }) {
                        let n = f[3].parse::<usize>().unwrap_or(0);
                        if n > 0 {
                            delay_sources.push((ids.len(), n));
                        }
                    }
                    ids.push(s.add(i));
                }
                Some("cammode") if f.len() >= 3 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    if let (Some(id), Some(m)) = (n.checked_sub(1).and_then(|k| ids.get(k)).copied(), crate::media::CamMode::parse(f[2])) {
                        if let Some(i) = s.input_mut(id) {
                            i.cam_mode = Some(m);
                        }
                    }
                }
                Some("input_options") if f.len() >= 7 => {
                    if let Some(i) = f[1].parse::<usize>().ok()
                        .and_then(|n| n.checked_sub(1)).and_then(|n| ids.get(n)).and_then(|&id| s.input_mut(id))
                    {
                        i.collapsed = f[2] == "1";
                        i.audio_on = f[3] == "1";
                        i.bus_m = f[4] == "1";
                        i.bus_a = f[5] == "1";
                        i.bus_b = f[6] == "1";
                    }
                }
                Some("decklink_port") if f.len() >= 3 => {
                    if let Some(i) = f[1].parse::<usize>().ok()
                        .and_then(|n| n.checked_sub(1)).and_then(|n| ids.get(n)).and_then(|&id| s.input_mut(id))
                    {
                        i.decklink_port = f[2].parse().unwrap_or(0);
                        i.decklink_mode = f.get(3).and_then(|v| v.parse().ok()).unwrap_or(0);
                    }
                }
                Some("place") if f.len() >= 3 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    if let (Some(id), Some(p)) = (ids.get(n.wrapping_sub(1)).copied(), parse_pos(f[2])) {
                        if let Some(i) = s.input_mut(id) {
                            i.place = p;
                        }
                    }
                }
                Some("layer") if f.len() >= 6 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    let k: usize = f[2].parse().unwrap_or(LAYERS);
                    let target: usize = f[4].parse().unwrap_or(0);
                    if let (Some(id), Some(p)) = (ids.get(n.wrapping_sub(1)).copied(), parse_pos(f[5])) {
                        let tid = ids.get(target.wrapping_sub(1)).copied();
                        if let Some(l) = s.input_mut(id).and_then(|i| i.layers.get_mut(k)) {
                            let mut layer = Layer { on: f[3] == "1", input: tid, place: p, ..Layer::default() };
                            if let Some(v) = f.get(6) {
                                layer.border = *v == "1";
                            }
                            if let Some(v) = f.get(7) {
                                let c: Vec<u8> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                                if c.len() == 3 {
                                    layer.border_colour = [c[0], c[1], c[2]];
                                }
                            }
                            if let Some(v) = f.get(8).and_then(|v| v.parse().ok()) {
                                layer.border_width = v;
                            }
                            *l = layer;
                        }
                    }
                }
                Some("input_flags") if f.len() >= 3 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    let bits: u16 = f[2].parse().unwrap_or(0);
                    if let Some(i) = ids.get(n.wrapping_sub(1)).copied().and_then(|id| s.input_mut(id)) {
                        i.mirror = bits & 1 != 0;
                        i.sharpen = bits & 2 != 0;
                        i.deinterlace = bits & 4 != 0;
                        i.play_active = bits & 8 != 0;
                        i.pause_preview = bits & 16 != 0;
                        i.restart_active = bits & 32 != 0;
                        i.auto_mix_audio = bits & 64 != 0;
                        i.rec601 = bits & 128 != 0;
                        i.premultiplied = bits & 256 != 0;
                        i.flatten = bits & 512 != 0;
                        i.effects_source = bits & 1024 != 0;
                    }
                }
                Some("aspect") if f.len() >= 3 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    if let Some(i) = ids.get(n.wrapping_sub(1)).copied().and_then(|id| s.input_mut(id)) {
                        i.aspect = f[2].parse().unwrap_or(0.0);
                    }
                }
                Some("tally") if f.len() >= 5 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    let v: Vec<u8> = f[2..5].iter().filter_map(|x| x.parse().ok()).collect();
                    if let Some(i) = ids.get(n.wrapping_sub(1)).copied().and_then(|id| s.input_mut(id)) {
                        i.tally_colour = (v.len() == 3).then(|| [v[0], v[1], v[2]]);
                    }
                }
                Some("advanced") if f.len() >= 4 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    if let Some(i) = ids.get(n.wrapping_sub(1)).copied().and_then(|id| s.input_mut(id)) {
                        i.click_action = f[2].parse().unwrap_or(0);
                        i.delay_frames = f[3].parse().unwrap_or(0);
                    }
                }
                Some("colour") if f.len() >= 3 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    let v: Vec<f32> = f[2].split(',').filter_map(|x| x.parse().ok()).collect();
                    if v.len() == 9 {
                        if let Some(i) = ids.get(n.wrapping_sub(1)).copied().and_then(|id| s.input_mut(id)) {
                            i.colour_adjust = ColourAdjust { brightness: v[0], contrast: v[1], saturation: v[2],
                                red: v[3], green: v[4], blue: v[5], alpha: v[6], black: v[7], white: v[8] };
                        }
                    }
                }
                Some("audio_effects") if f.len() >= 3 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    if let (Some(i), Ok(fx)) = (ids.get(n.wrapping_sub(1)).copied().and_then(|id| s.input_mut(id)),
                        serde_json::from_str::<crate::audio::Effects>(f[2]))
                    {
                        i.audio_effects = fx.normalized();
                    }
                }
                Some("preview") if f.len() >= 2 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    s.preview = ids.get(n.wrapping_sub(1)).copied();
                }
                Some("output") if f.len() >= 2 => {
                    let n: usize = f[1].parse().unwrap_or(0);
                    s.output = ids.get(n.wrapping_sub(1)).copied();
                }
                Some("transition") if f.len() >= 4 => {
                    if let Ok(k) = f[1].parse::<usize>() {
                        if k < 4 {
                            let stinger = f.get(4).and_then(|n| n.parse::<usize>().ok()).and_then(|n| n.checked_sub(1))
                                .and_then(|n| ids.get(n)).copied();
                            s.transitions[k] = TransBtn { effect: Effect::from_code(f[2].parse().unwrap_or(0)), ms: f[3].parse().unwrap_or(DEFAULT_TRANSITION_MS), stinger };
                        }
                    }
                }
                Some("overlay_source") if f.len() >= 4 => {
                    let channel = f[1].parse().unwrap_or(OVERLAY_CHANNELS);
                    let input = f[2].parse::<usize>().ok()
                        .and_then(|n| n.checked_sub(1)).and_then(|n| ids.get(n)).copied();
                    if let Some(id) = input {
                        s.assign_overlay(channel, id, f[3] == "1");
                    }
                }
                _ => {}
            }
        }
        for (index, number) in delay_sources {
            let source = number.checked_sub(1).and_then(|k| ids.get(k)).copied();
            if let Some(input) = ids.get(index).copied().and_then(|id| s.input_mut(id)) {
                if let Kind::VideoDelay { source: field, .. } = &mut input.kind {
                    *field = source;
                }
            }
        }
        s.preset_name = "livemix_preset".into();
        s.show_mixer = self.show_mixer;
        s.cfg = self.cfg.clone();
        s.now = self.now;
        *self = s;
        self.saved_text = Some(self.preset_text());
        self.notify("Preset opened");
    }

    /// A blank preset: no inputs, default transitions, the Settings kept.
    pub fn new_preset(&mut self) {
        let mixer = self.show_mixer;
        let now = self.now;
        let cfg = self.cfg.clone();
        *self = Self::empty();
        self.show_mixer = mixer;
        self.cfg = cfg;
        self.now = now;
        self.saved_text = Some(self.preset_text());
        self.notify(&format!("New preset — {}", self.cfg.format_label()));
    }
}

fn parse_pos(s: &str) -> Option<Position> {
    let v: Vec<f32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
    (v.len() == 7).then(|| Position { zoom: v[0], pan_x: v[1], pan_y: v[2], crop: [v[3], v[4], v[5], v[6]] })
}

fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_sliders_follow_the_original() {
        assert_eq!(zoom_from_slider(0), 1.0);
        assert!((zoom_from_slider(-3990) - 0.0025).abs() < 1e-6);
        assert_eq!(zoom_from_slider(4000), 5.0);
        assert_eq!(zoom_from_slider(-2000), 0.5);
        assert_eq!(zoom_to_slider(0.5), -2000);
        assert_eq!(zoom_to_slider(2.0), 1000);
        assert_eq!(zoom_to_slider(1.0), 0);
        assert_eq!(pan_from_slider(-2000), -2.0);
        assert_eq!(pan_to_slider(0.5), 500);
        assert_eq!(pan_to_slider(9.0), 2000);
    }

    fn frames(s: &mut Switcher, n: usize) {
        for _ in 0..n {
            s.output_frame();
        }
    }

    #[test]
    fn defaults_match_ii_ctor() {
        let s = Switcher::demo();
        let t: Vec<(Effect, u32)> = s.transitions.iter().map(|b| (b.effect, b.ms)).collect();
        assert_eq!(t, vec![(Effect::Fade, 500), (Effect::Merge, 1000), (Effect::Wipe, 1000), (Effect::CubeZoom, 1000)]);
    }

    #[test]
    fn duration_rule_and_step_formula() {
        assert_eq!(sanitize_ms(0), 500);
        assert_eq!(sanitize_ms(10000), 500);
        assert_eq!(sanitize_ms(9999), 9999);
        assert!((step_for(500) - 256.0 / 30.0).abs() < 1e-4);
        assert!((step_for(1000) - 256.0 / 60.0).abs() < 1e-4);
    }

    #[test]
    fn fade_500ms_takes_30_frames_and_swaps() {
        let mut s = Switcher::demo();
        let (p, o) = (s.preview, s.output);
        let tb = s.transitions[0];
        s.start_transition(tb.effect, tb.ms);
        frames(&mut s, 29);
        assert!(s.trans.is_some());
        frames(&mut s, 1);
        assert!(s.trans.is_none());
        assert_eq!(s.output, p);
        assert_eq!(s.preview, o);
    }

    #[test]
    fn cut_lands_next_frame_and_tbar_completes_at_255() {
        let mut s = Switcher::demo();
        let p = s.preview;
        s.start_transition(Effect::Cut, 0);
        frames(&mut s, 1);
        assert_eq!(s.output, p);
        let p2 = s.preview;
        s.set_tbar(0.5);
        assert!(s.trans.is_some());
        s.set_tbar(1.0);
        assert_eq!(s.output, p2);
    }

    #[test]
    fn new_button_follows_frmmain_zzr() {
        let mut s = Switcher::demo();
        // Outputs running: refused.
        s.rec = Some(0.0);
        s.press_new();
        assert_eq!(s.new_step, NewStep::Idle);
        s.rec = None;
        s.external[0].open = false;
        // Unsaved inputs: ask to save first, with the current format preselected.
        s.press_new();
        assert_eq!(s.new_step, NewStep::SaveChanges);
        assert_eq!(s.new_format, (s.cfg.out_size, s.cfg.out_rate, s.cfg.interlaced));
        // OK: blank preset in the chosen format (720p25), nothing unsaved.
        s.new_step = NewStep::Idle;
        s.cfg = Settings::default();
        let mut cfg = s.cfg.clone();
        cfg.out_size = 5;
        cfg.out_rate = 2;
        cfg.interlaced = false;
        s.cfg.out_size = 5;
        s.cfg.out_rate = 2;
        s.cfg.interlaced = false;
        s.new_preset();
        assert!(s.inputs.is_empty());
        assert_eq!(s.cfg.format_label(), "720p25");
        assert!(!s.preset_changed());
        // A blank preset goes straight to the format window, 720p25 preselected (both listed).
        s.press_new();
        assert_eq!(s.new_step, NewStep::Format);
        assert_eq!(s.new_format, (5, 2, false));
        // 30p is not in the original's list (ss::e): falls back to 1080 59.94i.
        s.cfg.out_rate = 4;
        s.press_new();
        assert_eq!(s.new_format, (5, 6, true));
        assert_eq!(NEW_PRESET_RATES.len(), 6);
        assert_eq!(NEW_PRESET_SIZES.len(), 8);
        let _ = cfg;
        set_output_fps(DEFAULT_FPS);
    }

    #[test]
    fn waveform_monitor_layouts_follow_hc_a() {
        // "+ Preview": waveform on the left half, the picture on the right half.
        let (pic, sc) = scope_layout(25);
        assert_eq!(pic, Some([0.5, 0.25, 0.5, 0.5]));
        assert_eq!(sc, vec![(SCOPE_Y, [0.0, 0.25, 0.5, 0.5])]);
        // Alone: both full size, scope on top.
        assert_eq!(scope_layout(10), (Some([0.0, 0.0, 1.0, 1.0]), vec![(SCOPE_PARADE, [0.0, 0.0, 1.0, 1.0])]));
        // Vectorscope alone: 0.5625 × 1 centred, picture hidden.
        assert_eq!(scope_layout(15), (None, vec![(SCOPE_VECTOR, [0.21875, 0.0, 0.5625, 1.0])]));
        // Vectorscope + Waveform Y + Preview: three parts.
        let (pic, sc) = scope_layout(60);
        assert_eq!(pic, Some([0.5, 0.25, 0.5, 0.5]));
        assert_eq!(sc.len(), 2);
        assert_eq!(WAVEFORMS.len(), 11);
    }

    #[test]
    fn safe_areas_follow_kzzzzzj_d() {
        // SMPTE/EBU on 1920x1080: title box from 96 to 1824 (0.90), action from 67.2 (0.93).
        let g = safe_area_guides(10, 1920.0, 1080.0);
        assert_eq!(g.len(), 10);
        assert!((g[0][0] - 96.0).abs() < 0.01 && (g[0][1] - 54.0).abs() < 0.01);
        assert!((g[4][0] - 67.2).abs() < 0.01);
        // Centre cross 0.05 of the height = 54 px long.
        assert!((g[8][3] - g[8][1] - 54.0).abs() < 0.01);
        // 90 % / 80 % with 4:3 adds the two 4:3 lines.
        assert_eq!(safe_area_guides(32, 1920.0, 1080.0).len(), 12);
        // GRID 16 = 15 + 8 lines; GRID 3 = 2 + 2.
        assert_eq!(safe_area_guides(160, 1920.0, 1080.0).len(), 23);
        assert_eq!(safe_area_guides(33, 1920.0, 1080.0).len(), 4);
        // Vertical 9:16 on 1080 lines: 607.5 → 608 wide, centred.
        let v = safe_area_guides(402, 1920.0, 1080.0);
        assert_eq!(v[1][0] - v[0][0], 608.0);
        assert!(safe_area_guides(0, 1920.0, 1080.0).is_empty());
    }

    #[test]
    fn project_format_names_and_rates() {
        let mut c = Settings::default();
        assert_eq!(c.format_label(), "1080i59,94");
        assert!((c.frame_rate() - 29.97).abs() < 1e-3);
        c.out_size = 5;
        c.out_rate = 2;
        c.interlaced = false;
        assert_eq!(c.format_label(), "720p25");
        assert_eq!(c.out_dims(), (1280, 720));
        assert_eq!(crate::media::rate_fraction(29.97), (30000, 1001));
        assert_eq!(crate::media::rate_fraction(25.0), (25, 1));
        let m = crate::media::CamMode { w: 1280, h: 720, fps_n: 30, fps_d: 1, format: "MJPG".into() };
        assert_eq!(crate::media::CamMode::parse(&m.code()), Some(m.clone()));
        assert_eq!(m.label(), "1280x720 30 fps MJPG");
    }

    #[test]
    fn centre_overlay_buttons_only_switch_the_channel() {
        let mut s = Switcher::demo();
        let (a, p) = (s.inputs[4].id, s.preview);
        let channel = OVERLAY_CHANNELS - 1;
        assert!(!s.toggle_overlay_channel(channel));
        s.assign_overlay(channel, a, true);
        frames(&mut s, 30);
        assert!(!s.overlays[channel].on);
        assert!(s.overlays[channel].input.is_none());
        assert!(s.toggle_overlay_channel(channel));
        assert_eq!(s.overlays[channel].input, Some(a));
        assert!(s.overlays[channel].on_preview);
        frames(&mut s, 30);
        assert!(s.toggle_overlay_channel(channel));
        frames(&mut s, 30);
        assert!(s.overlays[channel].input.is_none());
        assert!(s.toggle_overlay_channel(channel));
        assert_eq!(s.overlays[channel].input, Some(a));
        assert_eq!(s.preview, p);
    }

    #[test]
    fn preset_restores_overlay_sources_with_channels_off() {
        let mut s = Switcher::demo();
        let channel = OVERLAY_CHANNELS - 1;
        s.assign_overlay(channel, s.inputs[3].id, true);
        s.toggle_overlay_channel(channel);
        let preset = s.preset_text();
        s.load_preset_text(&preset);
        assert_eq!(s.overlays[channel].assigned, Some(OverlaySource { input: s.inputs[3].id, on_preview: true }));
        assert!(!s.overlays[channel].on);
        assert!(s.overlays[channel].input.is_none());
        s.remove_input(s.inputs[3].id);
        assert!(s.overlays[channel].assigned.is_none());
    }

    #[test]
    fn preset_keeps_input_settings_and_layer_border() {
        let mut s = Switcher::demo();
        let (a, b) = (s.inputs[1].id, s.inputs[2].id);
        {
            let i = s.input_mut(a).unwrap();
            i.mirror = true;
            i.sharpen = true;
            i.deinterlace = true;
            i.play_active = true;
            i.pause_preview = true;
            i.restart_active = true;
            i.auto_mix_audio = true;
            i.rec601 = true;
            i.premultiplied = true;
            i.flatten = true;
            i.effects_source = true;
            i.aspect = 4.0 / 3.0;
            i.tally_colour = Some([1, 2, 3]);
            i.click_action = 2;
            i.delay_frames = 7;
            i.audio_effects.pan = -0.4;
            i.colour_adjust = ColourAdjust { brightness: 0.1, contrast: -0.2, saturation: 0.3,
                red: 0.4, green: -0.5, blue: 0.6, alpha: -0.7, black: 0.125, white: -0.157 };
            i.layers[3] = Layer { on: true, input: Some(b),
                place: Position { zoom: 0.25, pan_x: 0.5, pan_y: -0.5, crop: [0.1, 0.2, 0.9, 0.8] },
                border: true, border_colour: [9, 8, 7], border_width: 0.05 };
        }
        let text = s.preset_text();
        let mut reopened = Switcher::empty();
        reopened.load_preset_text(&text);
        let i = &reopened.inputs[1];
        assert!(i.mirror && i.sharpen && i.deinterlace);
        assert!(i.play_active && i.pause_preview && i.restart_active && i.auto_mix_audio);
        assert!(i.rec601 && i.premultiplied && i.flatten);
        assert!(i.effects_source);
        assert!((i.aspect - 4.0 / 3.0).abs() < 0.001);
        assert_eq!(i.tally_colour, Some([1, 2, 3]));
        assert_eq!((i.click_action, i.delay_frames), (2, 7));
        assert!((i.audio_effects.pan + 0.4).abs() < 1e-6);
        assert_eq!(i.colour_adjust, s.input(a).unwrap().colour_adjust);
        let l = i.layers[3];
        assert!(l.on && l.border);
        assert_eq!(l.border_colour, [9, 8, 7]);
        assert!((l.border_width - 0.05).abs() < 0.001);
        assert_eq!(l.input, Some(reopened.inputs[2].id));
        assert_eq!(l.place, s.input(a).unwrap().layers[3].place);
    }

    #[test]
    fn preset_keeps_video_delay_and_telestrator() {
        let mut s = Switcher::demo();
        let source = s.inputs[0].id;
        s.add(Input::new(0, "Delay", Kind::VideoDelay {
            source: Some(source), seconds: 4.5, resolution: [1280, 720],
        }));
        s.add(Input::new(0, "Telestrator", Kind::Telestrator { strokes: vec![Telestroke {
            points: vec![[0.1, 0.2], [0.3, 0.4]], colour: [10, 20, 30], width: 0.008,
        }] }));
        let text = s.preset_text();
        let mut reopened = Switcher::empty();
        reopened.load_preset_text(&text);
        let delay = reopened.inputs.iter().find(|i| i.title == "Delay").unwrap();
        let Kind::VideoDelay { source: restored, seconds, resolution } = &delay.kind else {
            panic!("Video Delay lost its kind")
        };
        assert_eq!(*restored, Some(reopened.inputs[0].id));
        assert!((seconds - 4.5).abs() < 0.001);
        assert_eq!(*resolution, [1280, 720]);
        let tele = reopened.inputs.iter().find(|i| i.title == "Telestrator").unwrap();
        let Kind::Telestrator { strokes } = &tele.kind else {
            panic!("Telestrator lost its kind")
        };
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points, vec![[0.1, 0.2], [0.3, 0.4]]);
        assert_eq!(strokes[0].colour, [10, 20, 30]);
        assert!((strokes[0].width - 0.008).abs() < 1e-6);
    }

    #[test]
    fn playlist_skips_unticked_inputs_and_loops() {
        let mut s = Switcher::demo();
        let ids: Vec<u32> = s.inputs.iter().map(|i| i.id).collect();
        // The original's PlayList starts with Loop off, 5 s per still input (PlayList /
        // PlaylistItem constructors).
        assert!(!Settings::default().playlist_loop);
        assert_eq!(s.playlist_secs, PLAYLIST_DEFAULT_SECS);
        s.output = Some(ids[0]);
        assert_eq!(s.playlist_item_secs(), 5.0);
        s.playlist_off.insert(ids[1]);
        assert_eq!(s.playlist_next(), Some(ids[2]));
        s.output = ids.last().copied();
        assert_eq!(s.playlist_next(), None);
        s.cfg.playlist_loop = true;
        assert_eq!(s.playlist_next(), Some(ids[0]));
    }

    #[test]
    fn dropping_an_input_takes_the_targets_place() {
        let mut s = Switcher::demo();
        let ids: Vec<u32> = s.inputs.iter().map(|i| i.id).collect();
        s.move_input(ids[0], ids[2]);
        let now: Vec<u32> = s.inputs.iter().map(|i| i.id).collect();
        assert_eq!(&now[..3], &[ids[1], ids[2], ids[0]]);
        s.move_input(ids[0], ids[1]);
        assert_eq!(s.inputs[0].id, ids[0]);
    }

    #[test]
    fn multiview_sizes_follow_qu_b() {
        // Pro at 1920x1080: pictures 436x245, title strips 33, columns 452 apart from x 56,
        // big boxes 872x490 from x 55, 905 apart.
        assert_eq!(mv_sizes(3, 4), ((436, 245), 33, 452, 56, 905, 55));
        // Sixteen: 4x4 boxes; One: a single 1856x1044 picture with a 36 px strip.
        assert_eq!(mv_sizes(1, 1).0, (1856, 1044));
        assert_eq!(mv_sizes(1, 1).1, 36);
        let pro = mv_layout_boxes(0);
        assert_eq!(pro.len(), 10);
        // Everything fits in the frame, names included.
        let th = mv_title_height(0);
        for (r, _) in &pro {
            assert!(r[0] >= 0.0 && r[0] + r[2] <= 1.0 && r[1] + r[3] + th <= 1.0 + 1e-6, "{r:?}");
        }
        assert_eq!(mv_layout_boxes(2).iter().map(|b| b.1).collect::<Vec<_>>(), vec![SRC_PREVIEW, SRC_OUTPUT, MV_AUTO, MV_AUTO]);
        assert_eq!(mv_layout_boxes(3).len(), 16);
        assert_eq!(mv_layout_boxes(4).len(), 1);
    }

    #[test]
    fn multiview_pro_has_preview_output_and_eight_inputs() {
        let mut s = Switcher::demo();
        let boxes = s.mv_resolve(0);
        assert_eq!(boxes.len(), 10);
        assert_eq!(boxes[0].1, Some(SRC_PREVIEW));
        assert_eq!(boxes[1].1, Some(SRC_OUTPUT));
        assert_eq!(boxes[2].1, Some(s.inputs[0].id + 1));
        // A customised box shows its own choice; the automatic ones keep the list order.
        s.cfg.mv_boxes[0] = vec![SRC_PREVIEW, SRC_OUTPUT, s.inputs[3].id + 1];
        let boxes = s.mv_resolve(0);
        assert_eq!(boxes[2].1, Some(s.inputs[3].id + 1));
        assert_eq!(boxes[3].1, Some(s.inputs[0].id + 1));
        s.cfg.mv_layout[0] = 3;
        assert_eq!(s.mv_resolve(0).len(), 16);
    }

    #[test]
    fn per_input_quick_play_keeps_preview() {
        let mut s = Switcher::demo();
        let p = s.preview;
        s.quick_play_input(1);
        frames(&mut s, 30);
        assert_eq!(s.output, Some(1));
        assert_eq!(s.preview, p);
    }

    #[test]
    fn overlay_and_ftb_reach_full_after_30_frames() {
        let mut s = Switcher::demo();
        s.assign_overlay(1, 1, false);
        s.toggle_overlay_channel(1);
        s.ftb_on = true;
        frames(&mut s, 15);
        assert!(s.overlays[1].level > 100.0 && s.overlays[1].level < 130.0);
        frames(&mut s, 15);
        assert_eq!(s.overlays[1].level, FULL);
        assert_eq!(s.ftb_level, FULL);
        s.toggle_overlay_channel(1);
        frames(&mut s, 30);
        assert_eq!(s.overlays[1].input, None);
    }
}
