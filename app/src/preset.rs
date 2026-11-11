//! Version 9 XML project preset interchange.

use crate::model::*;
use crate::media::RecordFormat;
use eframe::egui::Color32;
use roxmltree::Node;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::Path;

const LOCAL: &str = "LiveMix/1 ";

fn xml(value: &str) -> String {
    let mut escaped = String::new();
    for c in value.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\r' => escaped.push_str("&#13;"),
            '\n' => escaped.push_str("&#10;"),
            '\t' => escaped.push_str("&#9;"),
            c if c >= ' ' && !matches!(c, '\u{fffe}' | '\u{ffff}') => escaped.push(c),
            _ => {}
        }
    }
    escaped
}

fn attr(out: &mut String, name: &str, value: impl std::fmt::Display) {
    write!(out, " {name}=\"{}\"", xml(&value.to_string())).unwrap();
}

fn tf(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

#[cfg(windows)]
fn guid() -> Result<String, String> {
    unsafe { windows::Win32::System::Com::CoCreateGuid() }
        .map(|id| format!("{id:?}")).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
fn guid() -> Result<String, String> {
    extern "C" { fn uuid_generate_random(bytes: *mut u8); }
    let mut bytes = [0_u8; 16];
    unsafe { uuid_generate_random(bytes.as_mut_ptr()) };
    let mut id = String::with_capacity(36);
    for (i, byte) in bytes.into_iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) { id.push('-'); }
        write!(id, "{byte:02x}").unwrap();
    }
    Ok(id)
}

pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", guid()?));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| error.to_string())
}

pub fn encode(s: &Switcher) -> Result<String, String> {
    let mut out = String::from("<XML>\n  <Version>9</Version>\n  <ZoomManager />\n");
    if !s.script.is_empty() { writeln!(out, "  <LiveMixScript>{}</LiveMixScript>", xml(&s.script)).unwrap(); }
    let replay_angles = s.replay_angles.map(|source| {
        if matches!(source, SRC_OUTPUT | MV_AUTO) { return source; }
        s.inputs.iter().position(|input| input.id + 1 == source)
            .map_or(MV_AUTO, |index| index as u32 + 1)
    });
    let replay_angles = serde_json::to_string(&replay_angles).map_err(|error| error.to_string())?;
    writeln!(out, "  <LiveMixReplayAngles>{replay_angles}</LiveMixReplayAngles>").unwrap();
    for i in &s.inputs {
        let (kind, filename, colour, bars) = match &i.kind {
            Kind::VideoFile { path } => (0, path.as_str(), None, false),
            Kind::ImageFile { path } => (1, path.as_str(), None, false),
            Kind::Photos { folder } => (2, folder.as_str(), None, false),
            Kind::ImageSequence { folder } => (21, folder.as_str(), None, false),
            Kind::GtTitle { path, .. } => (9000, path.as_str(), None, false),
            Kind::VideoList { .. } => (14, "", None, false),
            Kind::VideoDelay { .. } => (10, "", None, false),
            Kind::Telestrator { .. } => (5001, "", None, false),
            Kind::Device { .. } => (5, "", None, false),
            Kind::Colour(c) => (12, "", Some(*c), false),
            Kind::Bars => (12, "", Some(Color32::BLACK), true),
            _ => (100, "", None, false),
        };
        out.push_str("  <Input");
        attr(&mut out, "Type", kind);
        attr(&mut out, "Key", guid()?);
        attr(&mut out, "State", 1);
        attr(&mut out, "OriginalTitle", &i.title);
        attr(&mut out, "Title", &i.title);
        attr(&mut out, "Loop", tf(i.loop_on));
        attr(&mut out, "VolumeF", i.volume / 100.0);
        if !i.ptz_address.is_empty() { attr(&mut out, "LiveMixPtzAddress", &i.ptz_address); }
        if !i.on_program.is_empty() { attr(&mut out, "LiveMixOnProgram", &i.on_program); }
        // Input Settings > General / Colour Correction / Tally Lights / Advanced.
        let flags: [(&str, bool); 9] = [
            ("LiveMixMirror", i.mirror), ("LiveMixSharpen", i.sharpen),
            ("LiveMixDeinterlace", i.deinterlace), ("LiveMixPlayActive", i.play_active),
            ("LiveMixPausePreview", i.pause_preview), ("LiveMixRestartActive", i.restart_active),
            ("LiveMixAutoMixAudio", i.auto_mix_audio), ("LiveMixRec601", i.rec601),
            ("LiveMixPremultiplied", i.premultiplied),
        ];
        for (name, on) in flags {
            if on { attr(&mut out, name, "True"); }
        }
        if i.aspect != 0.0 { attr(&mut out, "LiveMixAspectRatio", i.aspect); }
        if let Some([r, g, b]) = i.tally_colour { attr(&mut out, "LiveMixTallyColour", format!("{r},{g},{b}")); }
        if i.flatten { attr(&mut out, "LiveMixFlatten", "True"); }
        if i.effects_source { attr(&mut out, "LiveMixUseSourceEffects", "True"); }
        if i.click_action != 0 { attr(&mut out, "LiveMixClickAction", i.click_action); }
        if i.delay_frames != 0 { attr(&mut out, "LiveMixDelayFrames", i.delay_frames); }
        if i.colour_adjust != ColourAdjust::default() {
            if let Ok(value) = serde_json::to_string(&i.colour_adjust) { attr(&mut out, "LiveMixColourAdjust", value); }
        }
        if i.audio_effects != crate::audio::Effects::default() {
            if let Ok(effects) = serde_json::to_string(&i.audio_effects) {
                attr(&mut out, "LiveMixAudioEffects", effects);
            }
        }
        attr(&mut out, "Muted", tf(!i.audio_on));
        attr(&mut out, "BusMaster", tf(i.bus_m));
        attr(&mut out, "Collapsed", tf(i.collapsed));
        attr(&mut out, "Category", i.category.map_or(0, |n| n + 1));
        if let Some(c) = colour {
            let argb = i32::from_be_bytes([c.a(), c.r(), c.g(), c.b()]);
            attr(&mut out, "Colour", argb);
            attr(&mut out, "ColourBars", tf(bars));
        }
        if let Kind::VideoList { paths, .. } = &i.kind {
            attr(&mut out, "Videos", paths.join("\r\n"));
        }
        if let Kind::GtTitle { texts, .. } = &i.kind {
            // mm::get_XML → mo::c, item serializer 0x06002b0c.
            let mut items = String::from("<items>");
            for (field, value) in texts {
                write!(items, "<item name=\"{}.Text\" version=\"2\"><value>{}</value></item>",
                    xml(field), xml(value)).unwrap();
            }
            items.push_str("</items>");
            attr(&mut out, "XML", items);
        }
        if let Kind::Device { link, .. } = &i.kind {
            attr(&mut out, "CapturePath", capture_path(link));
        }
        if let Kind::VideoDelay { source, seconds, resolution } = &i.kind {
            if let Some(number) = source.and_then(|id| s.number_of(id).checked_sub(1)) {
                attr(&mut out, "LiveMixDelaySource", number + 1);
            }
            attr(&mut out, "LiveMixDelaySeconds", seconds);
            attr(&mut out, "LiveMixDelayResolution", format!("{}x{}", resolution[0], resolution[1]));
        }
        if let Kind::Telestrator { strokes } = &i.kind {
            if let Ok(value) = serde_json::to_string(strokes) {
                attr(&mut out, "LiveMixTelestrator", value);
            }
        }
        if i.place != Position::default() {
            let p = i.place;
            attr(&mut out, "Positions", format!(
                "<ArrayOfMatrixPosition><MatrixPosition><ZoomX>{}</ZoomX><ZoomY>{}</ZoomY><PanX>{}</PanX><PanY>{}</PanY></MatrixPosition></ArrayOfMatrixPosition>",
                p.zoom, p.zoom, p.pan_x, p.pan_y,
            ));
            for (label, value) in ["X1", "Y1", "X2", "Y2"].into_iter().zip(p.crop) {
                attr(&mut out, &format!("VideoShader_Clipping{label}"), value);
            }
        }
        writeln!(out, ">{}</Input>", xml(filename)).unwrap();
    }
    let index = |id: Option<u32>| id.and_then(|id| s.number_of(id).checked_sub(1)).map_or(-1, |n| n as i64);
    writeln!(out, "  <State Main=\"{}\" Preview=\"{}\" />", index(s.output), index(s.preview)).unwrap();
    let (w, h) = s.cfg.out_dims();
    let (n, d) = crate::media::rate_fraction(s.cfg.frame_rate());
    let interval = (10_000_000.0 * d as f64 / n as f64).round() as u32;
    let write_recording = |out: &mut String, tag: &str, slot: Option<(usize, RecordFormat, u32)>| {
        writeln!(out, "  <{tag}><Size>{w}x{h}</Size><FrameRate>{interval}</FrameRate><FrameRateInterlaced>{}</FrameRateInterlaced>", u8::from(s.cfg.interlaced)).unwrap();
        if let Some((channel, codec, bitrate)) = slot {
            let folder = if s.cfg.rec_folder.trim().is_empty() { crate::media::storage_folder() }
                else { s.cfg.rec_folder.clone().into() };
            let name = if tag == "RecordingSettings2" { format!("{} - 2", s.cfg.rec_name) }
                else { s.cfg.rec_name.clone() };
            let file = folder.join(format!("{name}.{}", codec.extension()));
            let tab = if codec == RecordFormat::Wmv { 2 } else { 4 };
            writeln!(out, "    <Filename>{}</Filename><SelectedTab>{tab}</SelectedTab><MPEGBitRate>{bitrate}</MPEGBitRate><Interval>{}</Interval><Channel>{channel}</Channel>",
                xml(&file.to_string_lossy()), s.cfg.rec_split_min).unwrap();
            if codec != RecordFormat::Wmv {
                writeln!(out, "    <MP4Format>{}</MP4Format><HardwareAcceleration>{}</HardwareAcceleration>",
                    u8::from(codec == RecordFormat::Hevc), u8::from(s.cfg.hw_encoder)).unwrap();
            } else {
                writeln!(out, "    <WMVBitRate>{}</WMVBitRate>", bitrate * 1_000_000).unwrap();
            }
        }
        writeln!(out, "  </{tag}>").unwrap();
    };
    write_recording(&mut out, "RecordingSettings", Some((s.cfg.rec_source, s.cfg.rec_codec, s.cfg.rec_mbps)));
    write_recording(&mut out, "RecordingSettings2", s.cfg.rec_second.then_some((s.cfg.rec_second_source, s.cfg.rec_second_codec, s.cfg.rec_second_mbps)));
    writeln!(out, "  <OutputFormat OutputSize=\"{w}x{h}\" OutputFrameRate=\"{interval}\" OutputFrameRateInterlaced=\"{}\">", tf(s.cfg.interlaced)).unwrap();
    for (index, route) in s.cfg.outputs.iter().enumerate() {
        let tag = if index == 0 { "OutputsExternal".into() } else { format!("OutputsExternal{}", index + 1) };
        let input = route.source.checked_sub(1).map_or(0, |id| s.number_of(id));
        writeln!(out, "    <{tag}><Input>{input}</Input><Mix>0</Mix>").unwrap();
        for channel in 0..8 {
            writeln!(out, "      <Overlay{channel}>{}</Overlay{channel}>", u8::from(route.overlays)).unwrap();
        }
        writeln!(out, "    </{tag}>").unwrap();
    }
    out.push_str("  </OutputFormat>\n");

    // Preserve LiveMix settings that have no equivalent in the XML schema.
    let mut local = s.preset_text();
    let route_code = |source: u32| {
        if matches!(source, SRC_OUTPUT | SRC_PREVIEW | SRC_MV1 | SRC_MV2) { source }
        else { s.number_of(source - 1) as u32 + 1 }
    };
    let mut routes = s.cfg.outputs.iter().map(|route| (route_code(route.source), route.overlays)).collect::<Vec<_>>();
    routes.push((route_code(s.cfg.fs_source), s.cfg.fs_overlays));
    writeln!(local, "routes\t{}", serde_json::to_string(&routes).unwrap()).unwrap();
    let multi = (&s.cfg.mc_folder, s.cfg.mc_mbps, s.cfg.mc_split_min, s.cfg.mc_codec.key(), s.cfg.mc_outputs);
    writeln!(local, "multicorder\t{}", serde_json::to_string(&multi).unwrap()).unwrap();
    write!(out, "  <!--{LOCAL}").unwrap();
    for byte in local.bytes() {
        write!(out, "{byte:02x}").unwrap();
    }
    out.push_str("-->\n</XML>\n");
    Ok(out)
}

fn capture_path(link: &str) -> String {
    if link.starts_with(r"\\?\") { format!("@device:pnp:{link}") } else { link.into() }
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name(name))
}

fn bool_value(text: Option<&str>) -> bool {
    text.is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

pub struct ImportReport {
    pub path: std::path::PathBuf,
    pub inputs: usize,
    pub unsupported: Vec<String>,
    pub missing: Vec<(String, String)>,
}

impl ImportReport {
    pub fn collect(path: &Path, inputs: &[Input], unsupported: Vec<String>) -> Self {
        let mut missing = Vec::new();
        for input in inputs {
            let paths: &[String] = match &input.kind {
                Kind::VideoFile { path } | Kind::ImageFile { path } | Kind::GtTitle { path, .. } => std::slice::from_ref(path),
                Kind::Photos { folder } | Kind::ImageSequence { folder } => std::slice::from_ref(folder),
                Kind::VideoList { paths, .. } => paths,
                _ => &[],
            };
            for asset in paths {
                if !Path::new(asset).exists() {
                    missing.push((input.title.clone(), asset.clone()));
                }
            }
        }
        Self { path: path.to_owned(), inputs: inputs.len(), unsupported, missing }
    }
}

fn recording_slot<'a>(root: Node<'a, '_>, tag: &str) -> Option<(RecordFormat, usize, u32, u32, &'a str)> {
    let node = child(root, tag)?;
    let read = |tag| child(node, tag).and_then(|n| n.text());
    let file = read("Filename").filter(|s| !s.is_empty())?;
    let tab = read("SelectedTab")?.parse::<u32>().ok()?;
    let codec = match tab {
        2 => RecordFormat::Wmv,
        4 if read("MP4Format") == Some("1") => RecordFormat::Hevc,
        4 => RecordFormat::H264,
        _ => return None,
    };
    let channel = read("Channel").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(OUTPUT_COUNT - 1);
    let mbps = if codec == RecordFormat::Wmv {
        read("WMVBitRate").and_then(|v| v.parse::<u32>().ok()).map(|v| v.saturating_add(500_000) / 1_000_000)
    } else {
        read("MPEGBitRate").and_then(|v| v.parse::<u32>().ok())
    };
    let split = read("Interval").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    Some((codec, channel, mbps.unwrap_or(8).clamp(1, 100), split, file))
}

pub fn decode(s: &mut Switcher, path: &Path, text: &str) -> Result<Vec<String>, String> {
    let doc = roxmltree::Document::parse(text).map_err(|error| error.to_string())?;
    let root = doc.root_element();
    if !root.has_tag_name("XML") || child(root, "Version").and_then(|n| n.text()).and_then(|v| v.parse::<u32>().ok()).is_none() {
        return Err("Expected a project preset in XML with a Version element.".into());
    }
    let local = root.children().filter(Node::is_comment)
        .filter_map(|n| n.text()).find_map(|s| s.strip_prefix(LOCAL))
        .filter(|hex| hex.len() % 2 == 0)
        .and_then(|hex| {
            let bytes: Option<Vec<_>> = hex.as_bytes().chunks(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()).collect();
            String::from_utf8(bytes?).ok().filter(|s| s.starts_with("livemix-preset 1\n"))
        });
    let mut warnings = Vec::new();
    let mut imported = Switcher::empty();
    imported.cfg = s.cfg.clone();
    imported.show_mixer = s.show_mixer;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let resolve = |file: &str| parent.join(file.trim()).to_string_lossy().into_owned();
    if let Some(local) = &local {
        imported.load_preset_text(local);
    } else {
        for node in root.children().filter(|n| n.has_tag_name("Input")) {
            let ty = node.attribute("Type").unwrap_or("");
            let name = node.attribute("Title").or_else(|| node.attribute("OriginalTitle")).unwrap_or("Input");
            let file = node.text().unwrap_or("").trim();
            let kind = match ty {
                "0" | "16" => Kind::VideoFile { path: resolve(file) },
                "1" => Kind::ImageFile { path: resolve(file) },
                "2" => Kind::Photos { folder: resolve(file) },
                "21" => Kind::ImageSequence { folder: resolve(file) },
                "5" => Kind::Device {
                    link: node.attribute("CapturePath").unwrap_or_default().trim_start_matches("@device:pnp:").into(),
                    blackmagic: name.to_lowercase().contains("decklink"),
                },
                "6" => Kind::Desktop,
                "14" => Kind::VideoList {
                    paths: node.attribute("Videos").unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).map(resolve).collect(),
                    idx: 0,
                },
                "10" => {
                    let resolution = node.attribute("LiveMixDelayResolution").and_then(|v| {
                        let (w, h) = v.split_once('x')?;
                        Some([w.parse().ok()?, h.parse().ok()?])
                    }).unwrap_or(crate::model::DELAY_RESOLUTIONS[3]);
                    Kind::VideoDelay {
                        source: None,
                        seconds: node.attribute("LiveMixDelaySeconds").and_then(|v| v.parse().ok()).unwrap_or(1.0),
                        resolution,
                    }
                }
                "5001" => Kind::Telestrator {
                    strokes: node.attribute("LiveMixTelestrator")
                        .and_then(|v| serde_json::from_str(v).ok()).unwrap_or_default(),
                },
                "9000" => Kind::GtTitle { path: resolve(file), texts: title_texts(node), document: None },
                "12" if bool_value(node.attribute("ColourBars")) => Kind::Bars,
                "12" => {
                    let [a, r, g, b] = node.attribute("Colour").and_then(|v| v.parse::<i32>().ok()).unwrap_or(-16777216).to_be_bytes();
                    if a == 0 { Kind::Blank } else { Kind::Colour(Color32::from_rgb(r, g, b)) }
                }
                "100" => Kind::Blank,
                _ => {
                    warnings.push(format!("{name} (type {ty})"));
                    Kind::Blank
                }
            };
            let mut input = Input::new(0, name, kind);
            input.loop_on = ty == "16" || bool_value(node.attribute("Loop"));
            input.audio_on = !bool_value(node.attribute("Muted"));
            input.bus_m = node.attribute("BusMaster").map_or(true, |v| bool_value(Some(v)));
            input.collapsed = bool_value(node.attribute("Collapsed"));
            input.volume = node.attribute("VolumeF").and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite()).map_or(80.0, |v| (v * 100.0).clamp(0.0, 100.0));
            input.category = node.attribute("Category").and_then(|v| v.parse::<u8>().ok())
                .and_then(|v| v.checked_sub(1)).filter(|&v| (v as usize) < CATEGORIES.len());
            if let Some(pos) = node.attribute("Positions").and_then(|xml| roxmltree::Document::parse(xml).ok()) {
                if let Some(matrix) = child(pos.root_element(), "MatrixPosition") {
                    let read = |tag| child(matrix, tag).and_then(|n| n.text()).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
                    input.place.zoom = read("ZoomX").unwrap_or(1.0);
                    input.place.pan_x = read("PanX").unwrap_or(0.0);
                    input.place.pan_y = read("PanY").unwrap_or(0.0);
                }
            }
            for (index, suffix) in ["X1", "Y1", "X2", "Y2"].into_iter().enumerate() {
                if let Some(v) = node.attribute(format!("VideoShader_Clipping{suffix}").as_str()).and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite()) {
                    input.place.crop[index] = v.clamp(0.0, 1.0);
                }
            }
            if matches!(input.kind, Kind::GtTitle { .. }) {
                if let Err(error) = input.reload_gt() { input.status = error; }
            }
            imported.add(input);
        }
        if let Some(state) = child(root, "State") {
            let selected = |key| state.attribute(key).and_then(|v| v.parse::<usize>().ok())
                .and_then(|index| imported.inputs.get(index)).map(|i| i.id);
            (imported.output, imported.preview) = (selected("Main"), selected("Preview"));
        }
    }

    let input_ids: Vec<u32> = imported.inputs.iter().map(|input| input.id).collect();
    for (input, node) in imported.inputs.iter_mut().zip(root.children().filter(|node| node.has_tag_name("Input"))) {
        input.ptz_address = node.attribute("LiveMixPtzAddress").unwrap_or_default().into();
        input.on_program = node.attribute("LiveMixOnProgram").unwrap_or_default().into();
        input.mirror = bool_value(node.attribute("LiveMixMirror"));
        input.sharpen = bool_value(node.attribute("LiveMixSharpen"));
        input.deinterlace = bool_value(node.attribute("LiveMixDeinterlace"));
        input.play_active = bool_value(node.attribute("LiveMixPlayActive"));
        input.pause_preview = bool_value(node.attribute("LiveMixPausePreview"));
        input.restart_active = bool_value(node.attribute("LiveMixRestartActive"));
        input.auto_mix_audio = bool_value(node.attribute("LiveMixAutoMixAudio"));
        input.rec601 = bool_value(node.attribute("LiveMixRec601"));
        input.premultiplied = bool_value(node.attribute("LiveMixPremultiplied"));
        input.flatten = bool_value(node.attribute("LiveMixFlatten"));
        input.effects_source = bool_value(node.attribute("LiveMixUseSourceEffects"));
        input.aspect = node.attribute("LiveMixAspectRatio").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        input.click_action = node.attribute("LiveMixClickAction").and_then(|v| v.parse().ok()).unwrap_or(0);
        input.delay_frames = node.attribute("LiveMixDelayFrames").and_then(|v| v.parse().ok()).unwrap_or(0);
        input.tally_colour = node.attribute("LiveMixTallyColour").and_then(|v| {
            let c: Vec<u8> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            (c.len() == 3).then(|| [c[0], c[1], c[2]])
        });
        input.colour_adjust = node.attribute("LiveMixColourAdjust")
            .and_then(|value| serde_json::from_str(value).ok()).unwrap_or_default();
        if let Some(effects) = node.attribute("LiveMixAudioEffects")
            .and_then(|value| serde_json::from_str::<crate::audio::Effects>(value).ok())
        {
            input.audio_effects = effects.normalized();
        }
        if let Kind::VideoDelay { source, seconds, resolution } = &mut input.kind {
            *source = node.attribute("LiveMixDelaySource").and_then(|v| v.parse::<usize>().ok())
                .and_then(|n| n.checked_sub(1)).and_then(|n| input_ids.get(n)).copied();
            *seconds = node.attribute("LiveMixDelaySeconds").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            if let Some(size) = node.attribute("LiveMixDelayResolution").and_then(|v| {
                let (w, h) = v.split_once('x')?;
                Some([w.parse().ok()?, h.parse().ok()?])
            }) {
                *resolution = size;
            }
        }
    }

    if let Some(format) = child(root, "OutputFormat") {
        if let Some(size) = format.attribute("OutputSize") {
            if let Some(index) = OUTPUT_SIZES.iter().position(|(w, h)| size == format!("{w}x{h}")) {
                imported.cfg.out_size = index as u8;
            }
        }
        if let Some(interval) = format.attribute("OutputFrameRate").and_then(|v| v.parse::<u32>().ok()).filter(|&v| v > 0) {
            imported.cfg.interlaced = bool_value(format.attribute("OutputFrameRateInterlaced"));
            let fps = (if imported.cfg.interlaced { 20_000_000.0 } else { 10_000_000.0 }) / interval as f32;
            if let Some(index) = FRAME_RATES.iter().position(|(_, rate)| (*rate - fps).abs() < 0.025) {
                imported.cfg.out_rate = index as u8;
            }
        }
        for (index, route) in imported.cfg.outputs.iter_mut().enumerate() {
            let tag = if index == 0 { "OutputsExternal".into() } else { format!("OutputsExternal{}", index + 1) };
            if let Some(channel) = child(format, &tag) {
                route.source = child(channel, "Input").and_then(|n| n.text()).and_then(|v| v.parse::<usize>().ok())
                    .and_then(|v| v.checked_sub(1)).and_then(|v| imported.inputs.get(v)).map_or(SRC_OUTPUT, |i| i.id + 1);
                route.overlays = (0..8).any(|c| bool_value(child(channel, &format!("Overlay{c}")).and_then(|n| n.text())));
            }
        }
    }
    if let Some((codec, channel, bitrate, split, file)) = recording_slot(root, "RecordingSettings") {
        let file = parent.join(file);
        imported.cfg.rec_folder = file.parent().unwrap_or(parent).to_string_lossy().into_owned();
        imported.cfg.rec_name = file.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        imported.cfg.rec_codec = codec;
        imported.cfg.rec_source = channel;
        imported.cfg.rec_mbps = bitrate;
        imported.cfg.rec_split_min = split;
    }
    if child(root, "RecordingSettings2").is_some() {
        imported.cfg.rec_second = false;
        if let Some((codec, channel, bitrate, _, _)) = recording_slot(root, "RecordingSettings2") {
            imported.cfg.rec_second = true;
            imported.cfg.rec_second_codec = codec;
            imported.cfg.rec_second_source = channel;
            imported.cfg.rec_second_mbps = bitrate;
        }
    }
    if let Some(routes) = local.as_ref().and_then(|s| s.lines().find_map(|line| line.strip_prefix("routes\t")))
        .and_then(|v| serde_json::from_str::<Vec<(u32, bool)>>(v).ok())
    {
        let route_source = |source: u32| {
            if matches!(source, SRC_OUTPUT | SRC_PREVIEW | SRC_MV1 | SRC_MV2) { source }
            else { imported.inputs.get((source - 2) as usize).map_or(SRC_OUTPUT, |i| i.id + 1) }
        };
        for (route, (source, overlays)) in imported.cfg.outputs.iter_mut().zip(routes.iter().copied()) {
            route.source = route_source(source);
            route.overlays = overlays;
        }
        if let Some(&(source, overlays)) = routes.get(OUTPUT_COUNT) {
            imported.cfg.fs_source = route_source(source);
            imported.cfg.fs_overlays = overlays;
        }
    }
    if let Some((folder, bitrate, interval, codec, outputs)) = local.as_ref()
        .and_then(|s| s.lines().find_map(|line| line.strip_prefix("multicorder\t")))
        .and_then(|v| serde_json::from_str::<(String, u32, u32, String, [bool; OUTPUT_COUNT])>(v).ok())
    {
        imported.cfg.mc_folder = folder;
        imported.cfg.mc_mbps = bitrate.clamp(1, 100);
        imported.cfg.mc_split_min = interval;
        imported.cfg.mc_codec = RecordFormat::parse(&codec);
        imported.cfg.mc_outputs = outputs;
    }
    imported.script = child(root, "LiveMixScript").and_then(|node| node.text()).unwrap_or_default().into();
    if let Some(angles) = child(root, "LiveMixReplayAngles").and_then(|node| node.text())
        .and_then(|value| serde_json::from_str::<[u32; REPLAY_ANGLES]>(value).ok())
    {
        imported.replay_angles = angles.map(|source| {
            if matches!(source, SRC_OUTPUT | MV_AUTO) { return source; }
            imported.inputs.get((source - 1) as usize).map_or(MV_AUTO, |input| input.id + 1)
        });
    }
    imported.remote = s.remote.take();
    *s = imported;
    Ok(warnings)
}

fn title_texts(input: Node<'_, '_>) -> std::collections::BTreeMap<String, String> {
    let Some(items) = input.attribute("XML").and_then(|data| roxmltree::Document::parse(data).ok()) else {
        return Default::default();
    };
    items.root_element().children().filter(|node| node.has_tag_name("item")).filter_map(|node| {
        let field = node.attribute("name")?.strip_suffix(".Text")?;
        let value = child(node, "value")?.text().unwrap_or_default();
        Some((field.to_owned(), value.to_owned()))
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires externally supplied preset and GT title fixtures"]
    fn preset_and_title_fixtures() {
        let dir = std::env::temp_dir();
        let path = PathBuf::from(std::env::var_os("LIVEMIX_PRESET_FIXTURE").expect("Set LIVEMIX_PRESET_FIXTURE"));
        let installed = PathBuf::from(std::env::var_os("LIVEMIX_GT_FIXTURES").expect("Set LIVEMIX_GT_FIXTURES"));
        let mut s = Switcher::empty();
        decode(&mut s, &path, &fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(s.inputs.len(), 2);
        let path = installed.join("GT Title/Spinner 180.gtzip");
        let mut title = Input::new(0, "Spinner 180", Kind::GtTitle {
            path: path.to_string_lossy().into_owned(), texts: Default::default(), document: None,
        });
        title.reload_gt().unwrap();
        let Kind::GtTitle { document: Some(document), .. } = &title.kind else { unreachable!() };
        assert!(document.unsupported.is_empty(), "{:?}", document.unsupported);
        let context = eframe::egui::Context::default();
        let points_at = |t| {
            let draw = context.run(Default::default(), |ctx| {
                let painter = ctx.layer_painter(eframe::egui::LayerId::background());
                document.draw(&painter, eframe::egui::Rect::from_min_size(
                    eframe::egui::Pos2::ZERO, eframe::egui::vec2(1920.0, 1080.0),
                ), 1.0, &Default::default(), crate::gt::Cue::Rest, t);
            });
            draw.shapes.into_iter().find_map(|clipped| {
                if let eframe::egui::Shape::Mesh(mesh) = clipped.shape {
                    Some(mesh.vertices.iter().map(|vertex| vertex.pos).collect::<Vec<_>>())
                } else { None }
            }).unwrap()
        };
        assert_ne!(points_at(0.0), points_at(2.0));
        for name in [
            "GT Stingers/Stinger 2- Havin a blinder Blue.gtzip",
            "GT Social/Social 1- Kimono.gtzip",
            "GT ScoreBoard/Scoreboard 13- The Extreme.gtzip",
            "GT ScoreBoard/Scoreboard 3- Lineup 12.gtzip",
        ] {
            let document = crate::gt::Document::load(&installed.join(name)).unwrap();
            assert!(document.unsupported.is_empty(), "{name}: {:?}", document.unsupported);
            if name.contains("The Extreme") {
                assert_eq!(document.fields.get("Clock").map(String::as_str), Some("{0:4:00PM|mm:ss}"));
            }
            if name.contains("Lineup 12") {
                assert!(document.fields.len() >= 27);
            }
            if name.contains("Havin a blinder") {
                assert!((document.seconds(false).unwrap() - 0.35).abs() < 0.001);
                assert!((document.seconds(true).unwrap() - 0.8).abs() < 0.001);
                let mut switcher = Switcher::empty();
                switcher.output = Some(switcher.add(Input::new(0, "Before", Kind::Blank)));
                switcher.preview = Some(switcher.add(Input::new(0, "After", Kind::Blank)));
                let (before, after) = (switcher.output, switcher.preview);
                let stinger = Kind::GtTitle {
                    path: installed.join(name).to_string_lossy().into_owned(),
                    texts: Default::default(),
                    document: Some(std::sync::Arc::new(document)),
                };
                switcher.transitions[0].stinger = Some(switcher.add(Input::new(0, "Stinger", stinger)));
                switcher.start_transition_button(0);
                switcher.tick(0.3, 0.3);
                assert_eq!(switcher.output, before);
                switcher.tick(0.4, 0.1);
                assert_eq!(switcher.output, after);
                assert!(switcher.trans.is_some());
                switcher.tick(0.8, 0.4);
                switcher.tick(1.2, 0.4);
                assert_eq!(switcher.preview, before);
                assert!(switcher.trans.is_none());
            }
        }
        println!("DeckLink SDK sees {} device(s)", crate::decklink::refresh_devices().len());
        s.preview = Some(s.add(title));
        write_file(&dir.join("livemix-export-test.vmix"), encode(&s).unwrap().as_bytes()).unwrap();
    }

    #[test]
    fn native_preset_and_live_mix_round_trip() {
        let mut s = Switcher::empty();
        let folder = std::env::temp_dir();
        let preset = folder.join("test.vmix");
        let path = preset.as_path();
        let xml = r#"<XML><Version>9</Version>
          <Input Type="100" OriginalTitle="Blank" />
          <Input Type="0" Title="Vídeo &amp; som" Loop="True">clip.mp4</Input>
          <State Main="0" Preview="1" />
          <OutputFormat OutputSize="1280x720" OutputFrameRate="333667" OutputFrameRateInterlaced="True" />
        </XML>"#;
        assert!(decode(&mut s, path, xml).unwrap().is_empty());
        assert_eq!(s.cfg.out_dims(), (1280, 720));
        assert_eq!(s.cfg.out_rate, 6);
        assert_eq!(s.output, Some(s.inputs[0].id));
        assert_eq!(s.preview, Some(s.inputs[1].id));
        assert_eq!(s.inputs[1].title, "Vídeo & som");
        assert!(matches!(&s.inputs[1].kind, Kind::VideoFile { path } if Path::new(path) == folder.join("clip.mp4")));
        let id = s.inputs[0].id;
        s.remove_input(id);
        s.cfg.outputs[1].source = s.inputs[0].id + 1;
        s.replay_angles[1] = s.inputs[0].id + 1;
        s.script = "Preview 1\nWait 100\nCut".into();
        s.inputs[0].on_program = "Wait 20\nPreview 1".into();
        s.inputs[0].ptz_address = "192.0.2.1:52381".into();
        s.inputs[0].colour_adjust.saturation = -0.5;
        s.assign_overlay(7, s.inputs[0].id, true);
        let saved = encode(&s).unwrap();
        let mut reopened = Switcher::empty();
        decode(&mut reopened, path, &saved).unwrap();
        assert_eq!(reopened.inputs[0].title, s.inputs[0].title);
        assert_eq!(reopened.script, s.script);
        assert_eq!(reopened.inputs[0].on_program, s.inputs[0].on_program);
        assert_eq!(reopened.inputs[0].ptz_address, s.inputs[0].ptz_address);
        assert_eq!(reopened.inputs[0].colour_adjust, s.inputs[0].colour_adjust);
        assert_eq!(reopened.cfg.outputs[1].source, reopened.inputs[0].id + 1);
        assert_eq!(reopened.replay_angles[1], reopened.inputs[0].id + 1);
        assert_eq!(reopened.overlays[7].assigned.map(|source| source.input), Some(reopened.inputs[0].id));
        let mut native_only = Switcher::empty();
        let start = saved.find("<!--").unwrap();
        let end = saved.find("-->").unwrap() + 3;
        let xml = [&saved[..start], &saved[end..]].concat();
        decode(&mut native_only, path, &xml).unwrap();
        assert_eq!(native_only.inputs[0].title, "Vídeo & som");
        assert!(native_only.inputs[0].loop_on);
        assert_eq!(native_only.cfg.outputs[1].source, native_only.inputs[0].id + 1);
        assert!(decode(&mut s, path, "<unrelated />").is_err());
        assert_eq!(s.inputs[0].title, "Vídeo & som");
    }

    #[test]
    fn write_replaces_previous_preset() {
        let mut path = PathBuf::from(std::env::temp_dir());
        path.push(format!("livemix-{}.vmix", guid().unwrap()));
        write_file(&path, b"first").unwrap();
        write_file(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        fs::remove_file(path).unwrap();
    }
}
