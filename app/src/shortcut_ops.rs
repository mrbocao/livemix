//! Commands that use legacy function names and LiveMix's existing inputs and routes.
use crate::model::{Input, Kind, Layer, Switcher, LAYERS, MV_AUTO, OUTPUT_COUNT, mv_layout_boxes,
    SRC_MV1, SRC_MV2, SRC_OUTPUT, SRC_PREVIEW};
use crate::replay::Action;

pub const EXAMPLES: &str = "ActiveInput 1 · Effect1 · MasterAudioOn · SoloOn 1 · AudioBusOn 1 A · SetBalance 1 0 · SetGain 1 3\n\
InputPreviewShow 1 · InputPreviewHide · SetInputName 1 Camera One · SetText 1 Title.Text Updated headline\n\
SetLayer 1 2 3 · LayerOnOff 1 2 · SetLayer2Width 1 640 · SetLayer2Crop 1 0,0,1,1 · SetLayerDynamicZoom 1 2 0.5\n\
SetOutput2 Preview · SetOutputFullscreen2 MultiView1 · StartSRTOutput 2 · StopSRTOutput 2\n\
ReplayPlayEventToOutput 0 · ReplaySetSelectedEventText Great save · ReplayToggleSelectedEventCamera2 · ReplayDeleteLastEvent";

fn parse_index(text: &str, count: usize) -> Result<usize, String> {
    text.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).filter(|&n| n < count)
        .ok_or_else(|| format!("Expected a number from 1 to {count}, got {text:?}"))
}

pub fn replay_speed(text: &str) -> Result<u8, String> {
    let value = scalar(text)?;
    if !(0.0..=100.0).contains(&value) { return Err("Invalid Replay speed".into()); }
    Ok((if value <= 1.0 { value * 100.0 } else { value }).round() as u8)
}

fn scalar(text: &str) -> Result<f32, String> {
    text.parse::<f32>().ok().filter(|value| value.is_finite())
        .ok_or_else(|| format!("Invalid value {text:?}"))
}

fn crop(text: &str) -> Result<[f32; 4], String> {
    let mut coordinates = text.split(',');
    let mut result = [0.0; 4];
    for value in &mut result {
        *value = scalar(coordinates.next().ok_or("Enter four crop coordinates")?)?.clamp(0.0, 1.0);
    }
    if coordinates.next().is_some() || result[0] > result[2] || result[1] > result[3] {
        return Err("Crop must be x1,y1,x2,y2 with the first corner before the second".into());
    }
    Ok(result)
}

fn layer_transform(layer: &mut Layer, property: &str, value: &str, size: (u32, u32)) -> Result<(), String> {
    if property == "crop" {
        layer.place.crop = crop(value)?;
        return Ok(());
    }
    let [width, height] = [size.0 as f32, size.1 as f32];
    if property == "rectangle" {
        let values = value.split(',').map(scalar).collect::<Result<Vec<_>, _>>()?;
        let [x, y, w, h] = values.as_slice() else { return Err("Enter X,Y,Width,Height in pixels".into()); };
        if *w <= 0.0 || *h <= 0.0 { return Err("Layer width and height must be positive".into()); }
        layer.scale = [(*w / width / layer.place.zoom).clamp(0.0025, 5.0),
            (*h / height / layer.place.zoom).clamp(0.0025, 5.0)];
        layer.place.pan_x = (2.0 * (*x + *w / 2.0) / width - 1.0).clamp(-2.0, 2.0);
        layer.place.pan_y = (1.0 - 2.0 * (*y + *h / 2.0) / height).clamp(-2.0, 2.0);
        return Ok(());
    }
    let value = scalar(value)?;
    match property {
        "zoom" => layer.place.zoom = value.clamp(0.0025, 5.0),
        "panx" => layer.place.pan_x = value.clamp(-2.0, 2.0),
        "pany" => layer.place.pan_y = value.clamp(-2.0, 2.0),
        "x" => layer.place.pan_x = (2.0 * value / width + layer.scale[0] * layer.place.zoom - 1.0).clamp(-2.0, 2.0),
        "y" => layer.place.pan_y = (1.0 - 2.0 * value / height - layer.scale[1] * layer.place.zoom).clamp(-2.0, 2.0),
        "width" | "height" => {
            if value <= 0.0 { return Err("Layer width and height must be positive".into()); }
            let axis = usize::from(property == "height");
            layer.scale[axis] = (value / [width, height][axis] / layer.place.zoom).clamp(0.0025, 5.0);
        }
        "cropx1" => layer.place.crop[0] = value.clamp(0.0, layer.place.crop[2]),
        "cropy1" => layer.place.crop[1] = value.clamp(0.0, layer.place.crop[3]),
        "cropx2" => layer.place.crop[2] = value.clamp(layer.place.crop[0], 1.0),
        "cropy2" => layer.place.crop[3] = value.clamp(layer.place.crop[1], 1.0),
        _ => return Err(format!("Unknown layer property {property:?}")),
    }
    Ok(())
}

fn layer_property(name: &str) -> bool {
    matches!(name, "zoom" | "panx" | "pany" | "x" | "y" | "width" | "height" | "crop"
        | "cropx1" | "cropx2" | "cropy1" | "cropy2" | "rectangle")
}

fn has_input(op: &str) -> bool {
    matches!(op, "activeinput" | "inputpreviewshow" | "inputpreviewshowhide" | "mirroron" | "mirroroff"
        | "deinterlaceon" | "deinterlaceoff" | "autoplayon" | "autoplayoff" | "autopauseon" | "autopauseoff"
        | "autorestarton" | "autorestartoff" | "soloon" | "solooff" | "solo" | "solopfl")
}

fn has_value(op: &str) -> bool {
    matches!(op, "setgain" | "setbalance" | "setalpha" | "setccsaturation" | "setframedelay"
        | "setposition" | "setcrop" | "setinputname" | "settext" | "setcolor" | "moveinput"
        | "setlayer" | "layeron" | "layeroff" | "layeronoff" | "movelayer" | "audiobus" | "audiobuson"
        | "audiobusoff" | "setmultiviewoverlay" | "multiviewoverlay" | "multiviewoverlayon" | "multiviewoverlayoff")
        || op.strip_prefix("setlayer").is_some_and(|rest| {
            if let Some(property) = rest.strip_prefix("dynamic") { return layer_property(property); }
            let digit_count = rest.bytes().take_while(u8::is_ascii_digit).count();
            parse_index(&rest[..digit_count], LAYERS).is_ok() && layer_property(&rest[digit_count..])
        })
}

pub fn template_action(function: &str, input: Option<&str>, value: &str) -> Option<String> {
    let op = function.to_ascii_lowercase();
    if has_input(&op) { return Some(format!("{function} {}", input?)); }
    if has_value(&op) && !value.trim().is_empty() {
        return Some(format!("{function} {} {}", input?, value));
    }
    let param = if value.is_empty() { input } else { Some(value) };
    match op.as_str() {
        "setoutput2" | "setoutput3" | "setoutput4" | "setoutputexternal2"
        | "setoutputfullscreen" | "setoutputfullscreen2" | "startsrtoutput" | "stopsrtoutput"
        | "replayplayevent" | "replayplayeventtooutput" | "replaysetselectedeventtext"
        | "replaysetlasteventtext" | "replayupdateselectedspeedfromvalue" =>
            Some(format!("{function} {}", param?)),
        "addinput" | "openpreset" | "lastpreset" | "inputpreviewhide" | "masteraudioon" | "masteraudiooff"
        | "soloalloff" | "effect1" | "effect2" | "effect3" | "effect4"
        | "replayswapchannels" | "replaydeletelastevent" | "replayupdateselectedspeed"
        | "replayupdateselectedspeeddefault" | "replaystopevents" | "replayplay" | "replaypause" => Some(function.into()),
        _ if replay_angle(&op).is_some() => Some(function.into()),
        _ => None,
    }
}

fn replay_angle(op: &str) -> Option<(bool, usize)> {
    for (prefix, selected) in [("replaytoggleselectedeventcamera", true), ("replaytogglelasteventcamera", false)] {
        if let Some(camera) = op.strip_prefix(prefix).and_then(|arg| parse_index(arg, 8).ok()) {
            return Some((selected, camera));
        }
    }
    None
}

fn event_for(s: &Switcher, selected: bool) -> Result<crate::replay::Event, String> {
    let event = if selected {
        s.replay_events.iter().find(|event| Some(event.id) == s.replay_event)
    } else { s.replay_events.iter().max_by_key(|event| (event.end, event.id)) };
    event.cloned().ok_or_else(|| "Replay event not available".into())
}

pub fn run(
    s: &mut Switcher, op: &str, args: &[&str], target: impl Fn(&Switcher, &str) -> Option<u32>, now: f64,
) -> Result<bool, String> {
    let arg = |index| args.get(index).copied().ok_or_else(|| format!("Missing argument {}", index + 1));
    let input = |s: &Switcher, index| {
        let name = arg(index)?;
        target(s, name).ok_or_else(|| format!("Input {name} is unavailable"))
    };
    let text = |start| args.get(start..).filter(|words| !words.is_empty())
        .map(|words: &[&str]| words.join(" ")).ok_or_else(|| "Missing text".to_owned());
    let source = |s: &Switcher, index| -> Result<u32, String> {
        match arg(index)?.to_ascii_lowercase().as_str() {
            "program" | "output" | "0" => Ok(SRC_OUTPUT),
            "preview" => Ok(SRC_PREVIEW),
            "multiview1" => Ok(SRC_MV1),
            "multiview2" => Ok(SRC_MV2),
            _ => Ok(input(s, index)? + 1),
        }
    };
    if let Some(slot) = op.strip_prefix("effect").and_then(|n| parse_index(n, s.transitions.len()).ok()) {
        s.start_transition_button(slot);
        return Ok(true);
    }
    if let Some((selected, camera)) = replay_angle(op) {
        if s.replay_read_only { return Err("Replay session is read-only".into()); }
        let mut event = event_for(s, selected)?;
        event.angles[camera] = !event.angles[camera];
        s.replay_command = Some(Action::SaveEvent(event));
        return Ok(true);
    }
    match op {
        "activeinput" => s.cut_direct(input(s, 0)?),
        "addinput" => s.add_dialog.open = true,
        "openpreset" | "lastpreset" => {
            if s.outputs_busy() { return Err("Stop active outputs before opening a preset".into()); }
            if op == "lastpreset" { s.load_last_preset(); }
            else { s.browse_req = Some(crate::media::PickKind::PresetOpen); }
        }
        "inputpreviewhide" => s.input_preview = None,
        "inputpreviewshow" | "inputpreviewshowhide" => {
            let id = input(s, 0)?;
            s.input_preview = if op == "inputpreviewshowhide" && s.input_preview == Some(id) { None } else { Some(id) };
        }
        "masteraudioon" | "masteraudiooff" => s.master_on = op == "masteraudioon",
        "soloalloff" => { for item in &mut s.inputs { item.solo = false; } }
        "mirroron" | "mirroroff" | "deinterlaceon" | "deinterlaceoff"
        | "autoplayon" | "autoplayoff" | "autopauseon" | "autopauseoff" | "autorestarton" | "autorestartoff"
        | "soloon" | "solooff" | "solo" | "solopfl" => {
            let id = input(s, 0)?;
            let item = s.input_mut(id).unwrap();
            let on = op.ends_with("on");
            match op {
                "mirroron" | "mirroroff" => item.mirror = on,
                "deinterlaceon" | "deinterlaceoff" => item.deinterlace = on,
                "autoplayon" | "autoplayoff" => item.play_active = on,
                "autopauseon" | "autopauseoff" => item.pause_preview = on,
                "autorestarton" | "autorestartoff" => item.restart_active = on,
                "solo" => item.solo = !item.solo,
                "solopfl" => { item.solo_pfl = true; item.solo = !item.solo; }
                _ => item.solo = on,
            }
        }
        "setgain" | "setbalance" | "setalpha" | "setccsaturation" | "setframedelay" | "setposition" | "setcrop" => {
            let id = input(s, 0)?;
            let item = s.input_mut(id).unwrap();
            if op == "setcrop" { item.place.crop = crop(arg(1)?)?; }
            else {
                let value = scalar(arg(1)?)?;
                match op {
                    "setgain" => item.audio_effects.gain_db = value.clamp(-60.0, 24.0),
                    "setbalance" => item.audio_effects.pan = value.clamp(-1.0, 1.0),
                    "setalpha" => item.colour_adjust.alpha = value.clamp(0.0, 1.0) - 1.0,
                    "setccsaturation" => item.colour_adjust.saturation = value.clamp(-1.0, 1.0),
                    "setframedelay" => item.delay_frames = value.clamp(0.0, 300.0).round() as u32,
                    _ => {
                        item.seek_ms = Some(value.clamp(0.0, item.duration.max(0.0) * 1000.0) as u64);
                    }
                }
            }
        }
        "audiobus" | "audiobuson" | "audiobusoff" => {
            let id = input(s, 0)?;
            let item = s.input_mut(id).unwrap();
            let bus = match arg(1)?.to_ascii_lowercase().as_str() {
                "m" | "master" => &mut item.bus_m, "a" => &mut item.bus_a, "b" => &mut item.bus_b,
                _ => return Err("Available audio buses are M, A and B".into()),
            };
            *bus = if op == "audiobus" { !*bus } else { op == "audiobuson" };
        }
        "setinputname" | "settext" | "setcolor" => {
            let id = input(s, 0)?;
            let item = s.input_mut(id).unwrap();
            match op {
                "setinputname" => item.title = text(1)?,
                "settext" => set_title_text(item, arg(1)?, text(2)?, now)?,
                _ => {
                    let hex = arg(1)?.trim_start_matches('#');
                    let rgb = u32::from_str_radix(hex, 16).ok().filter(|_| hex.len() == 6)
                        .ok_or("Expected #RRGGBB")?;
                    if let Kind::Colour(colour) = &mut item.kind {
                        let [_, r, g, b] = rgb.to_be_bytes();
                        *colour = eframe::egui::Color32::from_rgb(r, g, b);
                    } else { return Err("SetColor requires a Colour input".into()); }
                }
            }
        }
        "moveinput" => s.move_input(input(s, 0)?, input(s, 1)?),
        "setlayer" | "layeron" | "layeroff" | "layeronoff" | "movelayer" => {
            let id = input(s, 0)?;
            let layer = parse_index(arg(1)?, LAYERS)?;
            if op == "setlayer" {
                let source = if arg(2)? == "0" { None } else { Some(input(s, 2)?) };
                if source == Some(id) { return Err("An input cannot be its own layer".into()); }
                let entry = &mut s.input_mut(id).unwrap().layers[layer];
                entry.input = source;
                entry.on = source.is_some();
            } else {
                let layers = &mut s.input_mut(id).unwrap().layers;
                if op == "movelayer" { layers.swap(layer, parse_index(arg(2)?, LAYERS)?); }
                else { layers[layer].on = if op == "layeronoff" { !layers[layer].on } else { op == "layeron" }; }
            }
        }
        "setoutput2" | "setoutput3" | "setoutput4" | "setoutputexternal2" => {
            let channel = match op { "setoutput3" => 2, "setoutput4" => 3, _ => 1 };
            s.cfg.outputs[channel].source = source(s, 0)?;
        }
        "setoutputfullscreen" | "setoutputfullscreen2" => {
            let slot = usize::from(op.ends_with('2'));
            s.cfg.fs_source[slot] = source(s, 0)?;
        }
        "setmultiviewoverlay" | "multiviewoverlay" | "multiviewoverlayon" | "multiviewoverlayoff" => {
            let view = parse_index(arg(0)?, 2)?;
            let defaults = mv_layout_boxes(s.cfg.mv_layout[view]);
            let slot = parse_index(arg(1)?, defaults.len())?;
            let current = s.cfg.mv_boxes[view].get(slot).copied().unwrap_or(defaults[slot].1);
            let route = match op {
                "setmultiviewoverlay" | "multiviewoverlayon" => source(s, 2)?,
                "multiviewoverlayoff" => MV_AUTO,
                _ if current != MV_AUTO => MV_AUTO,
                _ => source(s, 2)?,
            };
            if matches!(route, SRC_MV1 | SRC_MV2) { return Err("MultiView boxes cannot contain a MultiView".into()); }
            let boxes = &mut s.cfg.mv_boxes[view];
            boxes.extend(defaults.iter().skip(boxes.len()).map(|entry| entry.1));
            boxes[slot] = route;
        }
        "startsrtoutput" | "stopsrtoutput" => {
            let channel = parse_index(arg(0)?, OUTPUT_COUNT)?;
            s.srt_active[channel] = op == "startsrtoutput";
        }
        "replayswapchannels" => {
            s.replay_cameras.swap(0, 1);
            s.replay_speed.swap(0, 1);
        }
        "replayplayevent" | "replayplayeventtooutput" => {
            let offset = arg(0)?.parse::<usize>().map_err(|_| "Expected a Replay event offset")?;
            let mut events: Vec<_> = s.replay_events.iter().collect();
            events.sort_unstable_by_key(|event| std::cmp::Reverse((event.end, event.id)));
            let event = events.get(offset).ok_or("Replay event not available")?;
            s.replay_command = Some(Action::Play { event: event.id, program: op.ends_with("tooutput") });
        }
        "replaydeletelastevent" | "replaysetselectedeventtext" | "replaysetlasteventtext"
        | "replayupdateselectedspeed" | "replayupdateselectedspeeddefault" | "replayupdateselectedspeedfromvalue" => {
            if s.replay_read_only { return Err("Replay session is read-only".into()); }
            let mut event = event_for(s, op.contains("selected"))?;
            s.replay_command = Some(match op {
                "replaydeletelastevent" => Action::DeleteEvent(event.id),
                "replayupdateselectedspeeddefault" => { event.speed = 100; Action::SaveEvent(event) }
                "replayupdateselectedspeed" => {
                    event.speed = s.replay_speed[s.replay_channel.min(1)].max(1);
                    Action::SaveEvent(event)
                }
                "replayupdateselectedspeedfromvalue" => {
                    event.speed = replay_speed(arg(0)?)?.max(1);
                    Action::SaveEvent(event)
                }
                _ => { event.description = text(0)?; Action::SaveEvent(event) }
            });
        }
        "replayexportlastevent" => {
            let event = event_for(s, false)?.id;
            s.replay_command = Some(Action::Export { event, angle: s.replay_cameras[s.replay_channel.min(1)] });
        }
        "replayjumptonow" => s.replay_command = Some(Action::Live),
        _ => {
            let Some(rest) = op.strip_prefix("setlayer") else { return Ok(false); };
            let (layer, property, value_index) = if let Some(property) = rest.strip_prefix("dynamic") {
                (parse_index(arg(1)?, LAYERS)?, property, 2)
            } else {
                let digit_count = rest.bytes().take_while(u8::is_ascii_digit).count();
                let Ok(layer) = parse_index(&rest[..digit_count], LAYERS) else { return Ok(false); };
                (layer, &rest[digit_count..], 1)
            };
            if !layer_property(property) { return Ok(false); }
            let id = input(s, 0)?;
            let size = s.cfg.out_dims();
            layer_transform(&mut s.input_mut(id).unwrap().layers[layer], property, arg(value_index)?, size)?;
        }
    }
    Ok(true)
}

fn set_title_text(input: &mut Input, field: &str, value: String, now: f64) -> Result<(), String> {
    match &mut input.kind {
        Kind::Title { text, sub } => match field.to_ascii_lowercase().as_str() {
            "text" | "title" => *text = value,
            "sub" | "subtitle" => *sub = value,
            _ => return Err("Unknown title field".into()),
        },
        Kind::GtTitle { texts, document: Some(document), .. } => {
            let (field, default) = document.fields.iter().find(|(name, _)| {
                name.eq_ignore_ascii_case(field)
                    || field.get(..field.len().saturating_sub(5)).is_some_and(|base| base.eq_ignore_ascii_case(name))
                        && field.to_ascii_lowercase().ends_with(".text")
            }).ok_or("Unknown GT title field")?;
            let previous = input.gt_change.as_ref().filter(|change| {
                matches!(document.data_cue((now - change.started) as f32, &change.field), Some(crate::gt::Cue::DataOut(_, _)))
            }).map_or_else(|| texts.clone(), |change| change.previous.clone());
            if &value == default { texts.remove(field); } else { texts.insert(field.clone(), value); }
            input.gt_change = Some(crate::gt::DataChange { started: now, field: field.clone(), previous });
        }
        _ => return Err("SetText requires a title input".into()),
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_transform_and_persist_layer_dimensions() {
        let mut switcher = Switcher::demo();
        let (w, h) = switcher.cfg.out_dims();
        switcher.perform_action("SetLayer 1 10 2", 1.0).unwrap();
        switcher.perform_action(&format!("SetLayer10Width 1 {}", w / 2), 1.0).unwrap();
        switcher.perform_action(&format!("SetLayer10Height 1 {}", h / 2), 1.0).unwrap();
        switcher.perform_action("SetLayer10Crop 1 0.1,0.2,0.8,0.9", 1.0).unwrap();
        assert!(switcher.perform_action("SetLayer10Crop 1 0.9,0,0.1,1", 1.0).is_err());
        let expected = switcher.inputs[0].layers[9];
        assert_eq!(expected.scale, [0.5, 0.5]);
        assert_eq!(expected.place.crop, [0.1, 0.2, 0.8, 0.9]);
        let mut loaded = Switcher::empty();
        loaded.load_preset_text(&switcher.preset_text());
        assert_eq!(loaded.inputs[0].layers[9].scale, expected.scale);
        assert_eq!(loaded.inputs[0].layers[9].place, expected.place);
    }

    #[test]
    fn routing_and_replay_accept_legacy_names() {
        let mut switcher = Switcher::demo();
        switcher.perform_action("SetOutput3 Preview", 1.0).unwrap();
        assert_eq!(switcher.cfg.outputs[2].source, SRC_PREVIEW);
        switcher.perform_action("SetOutputFullscreen2 MultiView1", 1.0).unwrap();
        assert_eq!(switcher.cfg.fs_source[1], SRC_MV1);
        switcher.perform_action("SetMultiViewOverlay 1 1 Preview", 1.0).unwrap();
        assert_eq!(switcher.cfg.mv_boxes[0][0], SRC_PREVIEW);
        for id in [4, 9] {
            switcher.replay_events.push(crate::replay::Event {
                id, start: 0, end: 100, description: String::new(), speed: 100, angles: [true; 8],
                ..Default::default()
            });
        }
        switcher.perform_action("ReplayPlayEventToOutput 0", 1.0).unwrap();
        assert!(matches!(switcher.replay_command, Some(Action::Play { event: 9, program: true })));
        switcher.replay_event = Some(4);
        switcher.perform_action("ReplayToggleSelectedEventCamera2", 1.0).unwrap();
        assert!(matches!(&switcher.replay_command, Some(Action::SaveEvent(event)) if event.id == 4 && !event.angles[1]));
    }
}
