//! Interchange for the .NET userSettings section exported by vMix 28.
use crate::model::{Settings, FRAME_RATES, OUTPUT_SIZES};
use eframe::egui::Color32;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct DirectShow {
    pub enabled: bool,
    pub mpeg_video: String,
    pub mpeg_audio: String,
    pub h264_video: String,
    pub blocked: Vec<String>,
}

impl Default for DirectShow {
    fn default() -> Self {
        Self {
            enabled: false,
            mpeg_video: String::new(),
            mpeg_audio: String::new(),
            h264_video: String::new(),
            blocked: vec!["ffdshow Video Decoder".into(), "ffdshow Audio Decoder".into()],
        }
    }
}

pub struct Report {
    pub settings: Settings,
    pub imported: usize,
    pub skipped: usize,
}

fn boolean(value: &str) -> Option<bool> {
    match value { "True" | "true" | "1" => Some(true), "False" | "false" | "0" => Some(false), _ => None }
}

fn color(value: &str) -> Option<Color32> {
    match value {
        "DarkOrange" => Some(Color32::from_rgb(255, 140, 0)),
        "DarkGreen" => Some(Color32::from_rgb(0, 100, 0)),
        _ => {
            let numbers = value.split(',').map(|part| part.trim().parse::<u8>())
                .collect::<Result<Vec<_>, _>>().ok()?;
            match numbers.as_slice() {
                [r, g, b] | [255, r, g, b] => Some(Color32::from_rgb(*r, *g, *b)),
                _ => None,
            }
        }
    }
}

fn rate_period(rate: f32, interlaced: bool) -> u64 {
    let hz = match rate {
        23.976 => 24_000.0 / 1001.0,
        29.97 => 30_000.0 / 1001.0,
        59.94 => 60_000.0 / 1001.0,
        rate => f64::from(rate),
    };
    (10_000_000.0 / hz * if interlaced { 2.0 } else { 1.0 }).round() as u64
}

fn decoder(value: &str) -> String {
    if value.eq_ignore_ascii_case("Auto") { String::new() } else { value.into() }
}

pub fn import(text: &str, current: &Settings) -> Result<Report, String> {
    let doc = roxmltree::Document::parse(text).map_err(|error| format!("Invalid vMix XML: {error}"))?;
    fn child<'a, 'input>(parent: roxmltree::Node<'a, 'input>, name: &str) -> Option<roxmltree::Node<'a, 'input>> {
        parent.children().find(|node| node.has_tag_name(name))
    }
    let root = doc.root_element();
    let section = root.has_tag_name("configuration").then(|| child(root, "userSettings")).flatten()
        .and_then(|node| child(node, "vMix.My.MySettings"))
        .ok_or("Expected a vMix settings .config file")?;
    let mut report = Report { settings: current.clone(), imported: 0, skipped: 0 };
    let mut seen = std::collections::HashSet::new();
    let mut format = (None, None);
    let settings = &mut report.settings;
    macro_rules! assign_bool {
        ($field:ident, $value:expr) => { boolean($value).map(|value| settings.$field = value).is_some() };
    }
    for entry in section.children().filter(|node| node.has_tag_name("setting")) {
        let name = entry.attribute("name").ok_or("Missing vMix setting name")?;
        if !seen.insert(name) { return Err(format!("Duplicate vMix setting: {name}")); }
        let Some(node) = child(entry, "value") else { report.skipped += 1; continue; };
        let value = node.text().unwrap_or_default().trim();
        let array = || child(node, "ArrayOfString").map(|array| array.children()
            .filter(|item| item.has_tag_name("string"))
            .filter_map(|item| item.text().map(str::to_owned)).collect::<Vec<_>>());
        let imported = match name {
            "OutputSize" => OUTPUT_SIZES.iter().position(|&(w, h)| value == format!("{w}x{h}"))
                .map(|index| settings.out_size = index as u8).is_some(),
            "FrameRate" => { format.0 = value.parse::<u64>().ok(); continue; }
            "Interlaced" => { format.1 = boolean(value); continue; }
            "PreviewColour" => color(value).map(|color| settings.preview_colour = color).is_some(),
            "ActiveColour" => color(value).map(|color| settings.output_colour = color).is_some(),
            "PreviewSkip" => value.parse::<u8>().ok().filter(|skip| *skip < 8)
                .map(|skip| settings.preview_skip = skip + 1).is_some(),
            "FFMpegExtensions" => array().map(|items| {
                settings.ffmpeg_extensions = items.join(",");
                settings.ffmpeg_all_files = false;
            }).is_some(),
            "MpegVideoDecoder" => { settings.directshow.mpeg_video = decoder(value); true }
            "MpegAudioDecoder" => { settings.directshow.mpeg_audio = decoder(value); true }
            "H264VideoDecoder" => { settings.directshow.h264_video = decoder(value); true }
            "BlockedFilters" => array().map(|items| settings.directshow.blocked = items).is_some(),
            "CaptureLowLatency" => assign_bool!(low_latency_capture, value),
            "DisableInputEffects" => assign_bool!(no_input_effects, value),
            "RecordingConfirm" => assign_bool!(recording_confirm, value),
            "InputCloseConfirm" => assign_bool!(input_close_confirm, value),
            "InputNumbers" => assign_bool!(input_numbers, value),
            "PreviewClick" => assign_bool!(preview_click, value),
            "ShowQuickPlay" => assign_bool!(show_quickplay, value),
            "ShowTransitions24" => assign_bool!(show_trans24, value),
            "ShowFTB" => assign_bool!(show_ftb, value),
            "ShowTBar" => assign_bool!(show_tbar, value),
            "MasterAudioMeter" => assign_bool!(master_meter, value),
            "InputAudioMeter" => assign_bool!(input_meter, value),
            "Maximise" => assign_bool!(start_maximised, value),
            "AutoAudioMixing" => assign_bool!(default_auto_mix, value),
            "OutputHideCursor" => assign_bool!(ext_hide_cursor, value),
            "OutputAlwaysOnTop" => assign_bool!(ext_on_top, value),
            _ => false,
        };
        if imported { report.imported += 1; } else { report.skipped += 1; }
    }
    if seen.contains("FrameRate") || seen.contains("Interlaced") {
        let interlaced = format.1.unwrap_or(settings.interlaced);
        let index = format.0.and_then(|period| FRAME_RATES.iter().position(|&(_, rate)| {
            rate_period(rate, interlaced).abs_diff(period) <= 1
        }));
        if let Some(index) = index {
            settings.out_rate = index as u8;
            settings.interlaced = interlaced;
            report.imported += 1 + usize::from(format.1.is_some());
            report.skipped += usize::from(seen.contains("Interlaced") && format.1.is_none());
        } else {
            report.skipped += usize::from(seen.contains("FrameRate")) + usize::from(seen.contains("Interlaced"));
        }
    }
    if report.imported == 0 { return Err("The vMix config contains no supported settings".into()); }
    Ok(report)
}

pub fn export(settings: &Settings) -> String {
    let mut result = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<configuration>\n  <userSettings>\n    <vMix.My.MySettings>\n");
    let (width, height) = settings.out_dims();
    let rgb = |c: Color32| format!("{}, {}, {}", c.r(), c.g(), c.b());
    let mut scalar = |name: &str, value: &str| write_setting(&mut result, name, "String", &escape(value));
    scalar("OutputSize", &format!("{width}x{height}"));
    scalar("FrameRate", &rate_period(settings.rate(), settings.interlaced).to_string());
    scalar("PreviewColour", &rgb(settings.preview_colour));
    scalar("ActiveColour", &rgb(settings.output_colour));
    scalar("PreviewSkip", &settings.preview_skip.saturating_sub(1).to_string());
    for (name, value) in [
        ("MpegVideoDecoder", settings.directshow.mpeg_video.as_str()),
        ("MpegAudioDecoder", settings.directshow.mpeg_audio.as_str()),
        ("H264VideoDecoder", settings.directshow.h264_video.as_str()),
    ] { scalar(name, if value.is_empty() { "Auto" } else { value }); }
    for (name, on) in [
        ("Interlaced", settings.interlaced), ("CaptureLowLatency", settings.low_latency_capture),
        ("DisableInputEffects", settings.no_input_effects), ("RecordingConfirm", settings.recording_confirm),
        ("InputCloseConfirm", settings.input_close_confirm), ("InputNumbers", settings.input_numbers),
        ("PreviewClick", settings.preview_click), ("ShowQuickPlay", settings.show_quickplay),
        ("ShowTransitions24", settings.show_trans24), ("ShowFTB", settings.show_ftb),
        ("ShowTBar", settings.show_tbar), ("MasterAudioMeter", settings.master_meter),
        ("InputAudioMeter", settings.input_meter), ("Maximise", settings.start_maximised),
        ("AutoAudioMixing", settings.default_auto_mix), ("OutputHideCursor", settings.ext_hide_cursor),
        ("OutputAlwaysOnTop", settings.ext_on_top),
    ] { scalar(name, if on { "True" } else { "False" }); }
    let extensions: Vec<_> = settings.ffmpeg_extensions.split([',', ';', ' ']).filter(|item| !item.is_empty())
        .map(|item| format!("*.{}", item.trim_start_matches(['*', '.']))).collect();
    for (name, items) in [("FFMpegExtensions", &extensions), ("BlockedFilters", &settings.directshow.blocked)] {
        let mut array = String::from("<ArrayOfString>");
        for item in items { array.push_str(&format!("<string>{}</string>", escape(item))); }
        array.push_str("</ArrayOfString>");
        write_setting(&mut result, name, "Xml", &array);
    }
    result.push_str("    </vMix.My.MySettings>\n  </userSettings>\n</configuration>\n");
    result
}

fn escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn write_setting(xml: &mut String, name: &str, kind: &str, value: &str) {
    xml.push_str(&format!("      <setting name=\"{name}\" serializeAs=\"{kind}\"><value>{value}</value></setting>\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires a vMix 28 settings export"]
    fn reads_a_real_v28_export() {
        let path = std::env::var_os("LIVEMIX_VMIX_SETTINGS_FIXTURE").expect("Set LIVEMIX_VMIX_SETTINGS_FIXTURE");
        let xml = std::fs::read_to_string(path).unwrap();
        let report = import(xml.trim_start_matches('\u{feff}'), &Settings::default()).unwrap();
        assert!(report.imported >= 12 && report.skipped > 0);
        let exported = export(&report.settings);
        let round_trip = import(&exported, &Settings::default()).unwrap();
        assert_eq!(round_trip.skipped, 0);
        assert_eq!(round_trip.settings.out_dims(), report.settings.out_dims());
        println!("{} fields imported, {} skipped", report.imported, report.skipped);
    }

    #[test]
    fn v28_settings_round_trip_preserves_current_unmapped_fields() {
        let mut original = Settings::default();
        original.out_rate = 6;
        original.interlaced = true;
        original.preview_skip = 3;
        original.directshow.blocked = vec!["A & B <decoder>".into()];
        let xml = export(&original);
        assert!(xml.contains(">333667<"));
        let mut existing = Settings::default();
        existing.rec_folder = "Keep this folder".into();
        let result = import(&xml, &existing).unwrap();
        assert!(result.imported >= 20);
        assert_eq!(result.skipped, 0);
        assert_eq!(result.settings.out_rate, 6);
        assert!(result.settings.interlaced);
        assert_eq!(result.settings.preview_skip, 3);
        assert_eq!(result.settings.directshow.blocked, original.directshow.blocked);
        assert_eq!(result.settings.rec_folder, existing.rec_folder);
    }
}
