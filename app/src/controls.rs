use crate::{locale::tr, midi, model::{self, ShortcutBinding, Switcher}};
use eframe::egui::{self, Event, Key, Modifiers};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
enum State {
    #[default]
    Program,
    Preview,
    Recording,
    Streaming,
    MultiCorder,
    ReplayRecording,
    Overlay,
    FadeToBlack,
}

impl State {
    const ALL: [Self; 8] = [Self::Program, Self::Preview, Self::Recording, Self::Streaming,
        Self::MultiCorder, Self::ReplayRecording, Self::Overlay, Self::FadeToBlack];

    fn label(self) -> &'static str {
        match self {
            Self::Program => "Program", Self::Preview => "Preview", Self::Recording => "Recording",
            Self::Streaming => "Streaming", Self::MultiCorder => "MultiCorder",
            Self::ReplayRecording => "Replay recording", Self::Overlay => "Overlay",
            Self::FadeToBlack => "Fade to black",
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Feedback {
    device: String,
    channel: u8,
    number: u8,
    controller: bool,
    on: u8,
    off: u8,
    state: State,
    target: usize,
    disabled: bool,
}

impl Default for Feedback {
    fn default() -> Self {
        Self { device: String::new(), channel: 1, number: 0, controller: false,
            on: 127, off: 0, state: State::Program, target: 1, disabled: false }
    }
}

impl Feedback {
    fn active(&self, s: &Switcher) -> bool {
        let target = self.target.checked_sub(1);
        let input = || target.and_then(|index| s.inputs.get(index)).map(|input| input.id);
        match self.state {
            State::Program => input().is_some() && input() == s.output,
            State::Preview => input().is_some() && input() == s.preview,
            State::Recording => s.rec.is_some(),
            State::Streaming => s.stream_active.contains(&true),
            State::MultiCorder => s.multicorder.is_some(),
            State::ReplayRecording => s.replay_arm,
            State::Overlay => target.and_then(|index| s.overlays.get(index)).is_some_and(|overlay| overlay.on),
            State::FadeToBlack => s.ftb_on,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Learn { Key(usize), Midi(usize) }

struct Output {
    port: midi::Output,
    sent: BTreeMap<[u8; 2], (u8, u8)>,
}

impl Drop for Output {
    fn drop(&mut self) {
        for (&[status, number], &(_, off)) in &self.sent {
            let _ = self.port.send([status, number, off]);
        }
    }
}

#[derive(Default)]
pub struct Controls {
    input: Option<midi::Input>,
    selection: Vec<usize>,
    input_devices: Vec<midi::DeviceInfo>,
    output_devices: Vec<midi::DeviceInfo>,
    outputs: BTreeMap<String, Output>,
    enumerated: bool,
    output_retry: BTreeMap<String, std::time::Instant>,
    function_filter: String,
    learn: Option<Learn>,
    import: Option<String>,
    status: String,
}

fn chord(key: Key, modifiers: Modifiers) -> String {
    let mut result = String::new();
    for (active, label) in [(modifiers.ctrl, "Ctrl+"), (modifiers.mac_cmd, "Cmd+"),
        (modifiers.alt, "Alt+"), (modifiers.shift, "Shift+")] {
        if active { result.push_str(label); }
    }
    result.push_str(key.name());
    result
}

fn midi_action(binding: &ShortcutBinding, event: midi::Event) -> Option<String> {
    if binding.disabled || binding.midi_channel != event.channel + 1 || binding.midi_note != event.note
        || binding.midi_kind != event.kind || (binding.midi_value_enabled && binding.midi_value != event.value)
    {
        return None;
    }
    let percent = (u16::from(event.value) * 100 + 63) / 127;
    let delta = if event.value <= 63 { i16::from(event.value) } else { i16::from(event.value) - 128 };
    let mut action = binding.action.replace("{value}", &percent.to_string()).replace("{delta}", &delta.to_string());
    if binding.midi_value_type == 1 && action.eq_ignore_ascii_case("ReplayStep") {
        action = format!("ReplayStep {delta}");
    }
    Some(action)
}

impl Controls {
    fn refresh(&mut self) {
        self.input_devices = midi::devices();
        self.output_devices = midi::output_devices();
        self.enumerated = true;
        self.input = None;
        self.selection.clear();
        self.outputs.clear();
        self.output_retry.clear();
    }

    pub fn tick(&mut self, ctx: &egui::Context, s: &mut Switcher, now: f64) {
        if !self.enumerated { self.refresh(); }
        if !s.settings_open || s.cfg.page != 11 { self.learn = None; }
        if s.cfg.midi_enabled {
            if self.input.is_none() {
                match midi::Input::new() {
                    Ok(input) => self.input = Some(input),
                    Err(error) => { self.status = error; s.cfg.midi_enabled = false; }
                }
            }
            if let Some(input) = &mut self.input {
                if self.selection != s.cfg.midi_devices {
                    input.set_devices(&s.cfg.midi_devices);
                    self.selection.clone_from(&s.cfg.midi_devices);
                }
            }
        } else {
            self.input = None;
            self.selection.clear();
        }

        let keys = ctx.input(|input| input.events.iter().filter_map(|event| match event {
            Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
            _ => None,
        }).collect::<Vec<_>>());
        let midi = self.input.as_mut().map_or_else(Vec::new, midi::Input::poll);
        if let Some(learn) = self.learn {
            if keys.iter().any(|(key, _)| *key == Key::Escape) {
                self.learn = None;
                return;
            }
            match learn {
                Learn::Key(index) => {
                    if let (Some(binding), Some(&(key, modifiers))) = (s.cfg.shortcuts.get_mut(index), keys.first()) {
                        binding.key = chord(key, modifiers);
                        self.learn = None;
                    }
                }
                Learn::Midi(index) => {
                    if let (Some(binding), Some(event)) = (s.cfg.shortcuts.get_mut(index), midi.first()) {
                        binding.midi_channel = event.channel + 1;
                        binding.midi_kind = event.kind;
                        binding.midi_note = event.note;
                        binding.midi_value = event.value;
                        self.learn = None;
                    }
                }
            }
            if self.learn.is_none() { s.save_settings(); }
            return;
        }

        let mut actions = Vec::new();
        if !ctx.wants_keyboard_input() {
            for (key, modifiers) in keys {
                let key = chord(key, modifiers);
                if (key == "Ctrl+S" || key == "Cmd+S")
                    && !s.cfg.shortcuts.iter().any(|binding| binding.key.eq_ignore_ascii_case(&key))
                {
                    actions.push("SavePreset".into());
                }
                actions.extend(s.cfg.shortcuts.iter()
                    .filter(|binding| !binding.disabled && binding.key.eq_ignore_ascii_case(&key))
                    .map(|binding| binding.action.clone()));
            }
        }
        for event in midi {
            actions.extend(s.cfg.shortcuts.iter().filter_map(|binding| midi_action(binding, event)));
        }
        for action in actions {
            if let Err(error) = s.perform_action(&action, now) { s.notice(&error); }
        }
    }

    pub fn feedback(&mut self, s: &Switcher) {
        let devices: BTreeSet<_> = s.cfg.midi_feedback.iter().filter(|row| !row.disabled && !row.device.is_empty())
            .map(|row| row.device.as_str()).collect();
        self.outputs.retain(|name, _| s.cfg.midi_enabled && devices.contains(name.as_str()));
        if !s.cfg.midi_enabled { return; }
        self.output_retry.retain(|name, _| devices.contains(name.as_str()));
        for device in devices {
            if self.output_retry.get(device).is_some_and(|retry| std::time::Instant::now() < *retry) { continue; }
            let Some(index) = self.output_devices.iter().find(|item| item.name == device).map(|item| item.index) else { continue };
            if !self.outputs.contains_key(device) {
                match midi::Output::open(index) {
                    Ok(port) => { self.outputs.insert(device.to_owned(), Output { port, sent: BTreeMap::new() }); }
                    Err(error) => {
                        self.status = error;
                        self.output_retry.insert(device.to_owned(), std::time::Instant::now() + std::time::Duration::from_secs(2));
                        continue;
                    }
                }
            }
            let output = self.outputs.get_mut(device).unwrap();
            let wanted: BTreeMap<_, _> = s.cfg.midi_feedback.iter()
                .filter(|row| !row.disabled && row.device == device)
                .map(|row| {
                    let kind = if row.controller { midi::Kind::ControlChange } else { midi::Kind::NoteOn };
                    let [status, number, off] = midi::message(kind, row.channel.saturating_sub(1), row.number, row.off);
                    ([status, number], (if row.active(s) { row.on.min(127) } else { off }, off))
                }).collect();
            output.sent.retain(|&[status, number], &mut (_, off)| {
                if wanted.contains_key(&[status, number]) { return true; }
                let _ = output.port.send([status, number, off]);
                false
            });
            for (key @ [status, number], (value, off)) in wanted {
                if output.sent.get(&key).is_some_and(|&(previous, _)| previous == value) {
                    output.sent.insert(key, (value, off));
                    continue;
                }
                match output.port.send([status, number, value]) {
                    Ok(()) => { output.sent.insert(key, (value, off)); }
                    Err(error) => self.status = error,
                }
            }
        }
    }

    pub fn shortcuts(&mut self, ui: &mut egui::Ui, s: &mut Switcher) {
        crate::setting_toggle(ui, &mut s.cfg.midi_enabled, "Enable MIDI");
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr(ui, "Refresh MIDI devices")).clicked() { self.refresh(); }
            for device in &self.input_devices {
                let mut on = s.cfg.midi_devices.contains(&device.index);
                if crate::setting_toggle(ui, &mut on, &device.name).changed() {
                    if on { s.cfg.midi_devices.push(device.index); }
                    else { s.cfg.midi_devices.retain(|&index| index != device.index); }
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(tr(ui, "Add shortcut")).clicked() {
                s.cfg.shortcuts.insert(0, ShortcutBinding::default());
                self.learn = None;
            }
            if ui.button(tr(ui, "Default Keyboard")).clicked() { s.cfg.shortcuts = model::default_shortcuts(); self.learn = None; }
            if ui.button(tr(ui, "Import template XML")).clicked() { self.import = Some(String::new()); self.learn = None; }
        });
        if let Some(xml) = &mut self.import {
            ui.add(egui::TextEdit::multiline(xml).desired_rows(3).hint_text(tr(ui, "Paste a shortcut template XML")));
            if ui.button(tr(ui, "Import")).clicked() {
                match import_xml(xml, s) {
                    Ok((rows, skipped)) => {
                        self.status = format!("Imported {} shortcut(s), skipped {skipped}", rows.len());
                        s.cfg.shortcuts.extend(rows);
                        self.import = None;
                    }
                    Err(error) => self.status = error,
                }
            }
        }
        ui.collapsing(tr(ui, "Available commands"), |ui| {
            ui.label(COMMANDS);
            ui.label(crate::shortcut_ops::EXAMPLES);
            ui.label(tr(ui, "For MIDI CC, use {value} for 0–100 or {delta} for a relative encoder."));
        });
        ui.collapsing(tr(ui, "Shortcut function reference"), |ui| {
            ui.label(tr(ui, "Reference names include functions that LiveMix does not execute yet."));
            ui.add(egui::TextEdit::singleline(&mut self.function_filter).hint_text(tr(ui, "Search functions")));
            let query = self.function_filter.trim().to_ascii_lowercase();
            let names: Vec<_> = include_str!("shortcut-functions.txt").lines()
                .filter(|name| name.to_ascii_lowercase().contains(&query)).collect();
            egui::ScrollArea::vertical().id_salt("shortcut-reference").max_height(128.0)
                .show_rows(ui, 18.0, names.len(), |ui, range| {
                    for name in &names[range] { ui.monospace(*name); }
                });
        });
        let mut remove = None;
        for (index, binding) in s.cfg.shortcuts.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    crate::setting_toggle(ui, &mut binding.disabled, "Disabled");
                    if ui.button(tr(ui, "Delete")).clicked() { remove = Some(index); }
                    ui.add(egui::TextEdit::singleline(&mut binding.action).desired_width(175.0).hint_text(tr(ui, "Command")));
                    egui::ComboBox::from_id_salt("shortcut-actions").selected_text(tr(ui, "Commands")).show_ui(ui, |ui| {
                        for command in COMMANDS.lines().chain(crate::shortcut_ops::EXAMPLES.lines())
                            .flat_map(|line| line.split(" · ")) {
                            if ui.selectable_label(binding.action == command, command).clicked() {
                                binding.action = command.to_owned();
                            }
                        }
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button(if self.learn == Some(Learn::Key(index)) { tr(ui, "Press a key…") } else { tr(ui, "Learn key") }).clicked() {
                        self.learn = Some(Learn::Key(index));
                    }
                    ui.add(egui::TextEdit::singleline(&mut binding.key).desired_width(75.0));
                    if ui.add_enabled(s.cfg.midi_enabled, egui::Button::new(if self.learn == Some(Learn::Midi(index)) {
                        tr(ui, "Move MIDI control…")
                    } else { tr(ui, "Learn MIDI") })).clicked() {
                        self.learn = Some(Learn::Midi(index));
                    }
                    if binding.has_midi() {
                        ui.label(format!("{:?} {} / {}", binding.midi_kind, binding.midi_channel, binding.midi_note));
                        if ui.small_button("×").clicked() { binding.midi_channel = 0; }
                    }
                });
                if binding.has_midi() {
                    ui.horizontal_wrapped(|ui| {
                        crate::setting_toggle(ui, &mut binding.midi_value_enabled, "Match value");
                        ui.add_enabled(binding.midi_value_enabled, egui::DragValue::new(&mut binding.midi_value).range(0..=127));
                        let mut jog = binding.midi_value_type == 1;
                        if crate::setting_toggle(ui, &mut jog, "Jog wheel").changed() {
                            binding.midi_value_type = u8::from(jog);
                        }
                    });
                }
            });
        }
        if let Some(index) = remove { s.cfg.shortcuts.remove(index); self.learn = None; }
        if !self.status.is_empty() { ui.label(&self.status); }
    }

    pub fn activators(&mut self, ui: &mut egui::Ui, s: &mut Switcher) {
        crate::setting_toggle(ui, &mut s.cfg.midi_enabled, "Enable MIDI");
        ui.horizontal(|ui| {
            if ui.button(tr(ui, "Refresh MIDI devices")).clicked() { self.refresh(); }
            if ui.button(tr(ui, "Add activator")).clicked() { s.cfg.midi_feedback.push(Feedback::default()); }
        });
        let mut remove = None;
        for (index, row) in s.cfg.midi_feedback.iter_mut().enumerate() {
            ui.push_id(index, |ui| {
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("device").selected_text(if row.device.is_empty() {
                        tr(ui, "MIDI output")
                    } else { &row.device }).show_ui(ui, |ui| {
                        for device in &self.output_devices {
                            ui.selectable_value(&mut row.device, device.name.clone(), &device.name);
                        }
                    });
                    egui::ComboBox::from_id_salt("state").selected_text(tr(ui, row.state.label())).show_ui(ui, |ui| {
                        for state in State::ALL { ui.selectable_value(&mut row.state, state, tr(ui, state.label())); }
                    });
                    if matches!(row.state, State::Program | State::Preview | State::Overlay) {
                        ui.add(egui::DragValue::new(&mut row.target).range(1..=999).prefix(" #"));
                    }
                    if ui.button(tr(ui, "Delete")).clicked() { remove = Some(index); }
                });
                ui.horizontal_wrapped(|ui| {
                    crate::setting_toggle(ui, &mut row.controller, "CC");
                    ui.add(egui::DragValue::new(&mut row.channel).range(1..=16).prefix("Ch "));
                    ui.add(egui::DragValue::new(&mut row.number).range(0..=127).prefix("#"));
                    ui.add(egui::DragValue::new(&mut row.on).range(0..=127).prefix("On "));
                    ui.add(egui::DragValue::new(&mut row.off).range(0..=127).prefix("Off "));
                    crate::setting_toggle(ui, &mut row.disabled, "Disabled");
                });
            });
        }
        if let Some(index) = remove { s.cfg.midi_feedback.remove(index); }
        if !self.status.is_empty() { ui.label(&self.status); }
    }
}

const COMMANDS: &str = "Preview 1 · PreviewNext · PreviewPrevious · Cut 1 · Fade 1 500 · QuickPlay · Transition 1\n\
Play 1 · Pause 1 · PlayPause 1 · Restart 1 · Loop 1 · AudioOn 1 · AudioOff 1 · Volume 1 80\n\
StartRecording · StopRecording · StartStreaming · StopStreaming · StartMultiCorder · StopMultiCorder\n\
MasterVolume 80 · MasterAudio · Overlay 1 · External 1 · FadeToBlack · TBar 50 · Snapshot · SavePreset\n\
ReplayStartRecording · ReplayStopRecording · ReplayMarkIn · ReplayMarkOut · ReplayMarkLast 5 · ReplayPlayLast\n\
ReplaySelectChannelA · ReplaySelectChannelB · ReplaySelectChannelAB · ReplayPause · ReplayLive · ReplayReverse\n\
ReplayStep -1 · ReplaySetSpeed 50 · ReplaySelectCamera 1";

fn import_xml(xml: &str, s: &Switcher) -> Result<(Vec<ShortcutBinding>, usize), String> {
    let doc = roxmltree::Document::parse(xml).map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    let mut skipped = 0;
    for node in doc.descendants().filter(|node| node.has_tag_name("Shortcut")) {
        if let Some(binding) = parse_binding(node, s) { rows.push(binding); }
        else { skipped += 1; }
    }
    if rows.is_empty() && skipped == 0 { return Err("No shortcuts found in the XML".into()); }
    Ok((rows, skipped))
}

fn parse_binding(node: roxmltree::Node<'_, '_>, s: &Switcher) -> Option<ShortcutBinding> {
    let field = |name| node.children().find(|child| child.has_tag_name(name)).and_then(|child| child.text()).unwrap_or_default();
    let input_node = node.children().find(|child| child.has_tag_name("Input"));
    let input_field = |name| input_node.and_then(|input| input.children().find(|child| child.has_tag_name(name)))
        .and_then(|child| child.text()).unwrap_or_default();
    let input = input_field("Number").parse::<usize>().unwrap_or(0);
    let by_id = field("GetInputByNumber").eq_ignore_ascii_case("false");
    let key = input_field("Key");
    if by_id && !key.is_empty() && key != "00000000-0000-0000-0000-000000000000" { return None; }
    let target = if by_id {
        input.checked_sub(1).and_then(|index| s.inputs.get(index)).map_or_else(|| input.to_string(), |item| format!("@{}", item.id))
    } else { input.to_string() };
    let function = field("Function");
    if field("Mix").parse::<u32>().is_ok_and(|mix| mix != 0) { return None; }
    let duration = field("Duration").parse::<u32>().unwrap_or(500);
    let val = field("Value");
    let action = match function {
        "PreviewInput" if input > 0 => format!("Preview {target}"),
        "Cut" | "QuickPlay" if input > 0 => format!("{function} {target}"),
        "Fade" if input > 0 => format!("Fade {target} {duration}"),
        "Fade" => format!("Fade {duration}"),
        "CutDirect" if input > 0 => format!("CutDirect {target}"),
        "SetVolume" | "SetZoom" | "SetPanX" | "SetPanY" if input > 0 && !val.is_empty() => format!("{function} {target} {val}"),
        "SetMasterVolume" | "SetFader" | "ReplayJumpFrames" if !val.is_empty() => format!("{function} {val}"),
        name if name.starts_with("SetTransitionDuration") || name.starts_with("SetTransitionEffect") => {
            if val.is_empty() { return None; }
            format!("{name} {val}")
        }
        name if name.starts_with("OverlayInput") || name.starts_with("PreviewOverlayInput") => {
            if name.ends_with("Zoom") { return None; }
            if input > 0 { format!("{name} {target}") } else { name.into() }
        }
        name if ["ReplayACamera", "ReplayBCamera", "ReplayCamera"].into_iter()
            .any(|prefix| name.strip_prefix(prefix).is_some_and(|suffix| matches!(suffix.parse::<u8>(), Ok(1..=8)))) => name.into(),
        "Play" | "Pause" | "PlayPause" | "Restart" | "Loop" | "LoopOn" | "LoopOff"
        | "Audio" | "AudioOn" | "AudioOff" | "AudioAuto" | "AudioAutoOn" | "AudioAutoOff" if input > 0 => format!("{function} {target}"),
        "ReplayMarkInOut" | "ReplayMarkInOutLive" => format!("ReplayMarkLast {}", val.parse::<u32>().ok()?),
        "ReplaySetSpeed" | "ReplaySelectCamera" if !val.is_empty() => format!("{function} {val}"),
        "Cut" | "QuickPlay" | "PreviewInputNext" | "PreviewInputPrevious" | "FadeToBlack"
        | "StartRecording" | "StopRecording" | "StartStreaming" | "StopStreaming" | "StartMultiCorder"
        | "StopMultiCorder" | "ReplayMarkIn" | "ReplayMarkOut" | "ReplayStartRecording" | "ReplayStopRecording"
        | "ReplayPlayLastEvent" | "ReplayPlayLastEventToOutput" | "ReplayPlayPause"
        | "ReplaySelectChannelA" | "ReplaySelectChannelB" | "ReplaySelectChannelAB"
        | "StartStopRecording" | "StartStopStreaming" | "StartStopMultiCorder"
        | "Fullscreen" | "FullscreenOn" | "FullscreenOff" | "StartExternal" | "StopExternal"
        | "StartStopExternal" | "StartPlayList" | "StopPlayList" | "Snapshot" | "AudioMixerShowHide"
        | "ReplayShowHide" | "ReplayMarkCancel" | "ReplayStartStopRecording"
        | "ReplayPlaySelectedEvent" | "ReplayPlaySelectedEventToOutput" | "ReplayDeleteSelectedEvent"
        | "ReplaySelectFirstEvent" | "ReplaySelectLastEvent" | "ReplaySelectPreviousEvent" | "ReplaySelectNextEvent"
        | "ReplayLive" => function.into(),
        _ => crate::shortcut_ops::template_action(function, (input > 0).then_some(target.as_str()), val)?,
    };
    let midi_kind = match field("ExpandedKey") {
        "MIDINoteOff" => midi::Kind::NoteOff,
        "MIDIControlChange" | "MIDIController" => midi::Kind::ControlChange,
        _ => midi::Kind::NoteOn,
    };
    let expanded = field("ExpandedKey");
    let chord_text = if expanded.is_empty() || expanded == "None" || expanded.starts_with("MIDI") {
        field("Key")
    } else { expanded };
    let raw_key = chord_text.split([',', '+']).next().unwrap_or_default().trim();
    let modifiers = chord_text.split([',', '+']).skip(1).map(str::trim).fold(Modifiers::default(), |mut out, token| {
        match token {
            "Control" | "Ctrl" => out.ctrl = true,
            "Alt" => out.alt = true,
            "Shift" => out.shift = true,
            _ => {}
        }
        out
    });
    let key = match raw_key {
        "OemPeriod" => Some(Key::Period),
        "OemOpenBrackets" | "Oem4" => Some(Key::OpenBracket),
        "OemCloseBrackets" | "Oem6" => Some(Key::CloseBracket),
        raw => Key::from_name(raw.strip_prefix('D').filter(|s| s.len() == 1).unwrap_or(raw)),
    }.map_or_else(String::new, |key| chord(key, modifiers));
    let midi_channel = field("MIDIChannel").parse::<u8>().ok().filter(|value| *value <= 16).unwrap_or(0);
    if key.is_empty() && midi_channel == 0 { return None; }
    Some(ShortcutBinding {
        action, key, midi_channel, midi_kind,
        midi_note: field("MIDINote").parse::<u8>().unwrap_or(0).min(127),
        midi_value: field("MIDIValue").parse::<u8>().unwrap_or(0).min(127),
        midi_value_enabled: field("MIDIValueEnabled").eq_ignore_ascii_case("true"),
        midi_value_type: u8::from(field("MIDIValueType") == "JogWheel"),
        disabled: field("Disabled").eq_ignore_ascii_case("true"),
    })
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learned_note_and_cc_execute_existing_switcher_commands() {
        let mut s = Switcher::demo();
        let input = s.inputs[0].id;
        let binding = ShortcutBinding { key: chord(Key::Q, Modifiers::CTRL), action: "Preview 1".into(),
            midi_channel: 2, midi_note: 17, ..Default::default() };
        assert_eq!(binding.key, "Ctrl+Q");
        let event = midi::Event { kind: midi::Kind::NoteOn, channel: 1, note: 17, value: 120 };
        assert!(midi_action(&binding, midi::Event { kind: midi::Kind::NoteOff, ..event }).is_none());
        s.perform_action(&midi_action(&binding, event).unwrap(), 1.0).unwrap();
        assert_eq!(s.preview, Some(input));
        let slider = ShortcutBinding { action: "SetMasterVolume {value}".into(),
            midi_kind: midi::Kind::ControlChange, ..binding };
        let event = midi::Event { kind: midi::Kind::ControlChange, value: 64, ..event };
        s.perform_action(&midi_action(&slider, event).unwrap(), 1.0).unwrap();
        assert_eq!(s.master_volume, 50.0);
    }

    #[test]
    fn template_import_skips_unavailable_functions_and_keeps_note_off() {
        let s = Switcher::demo();
        let (rows, skipped) = import_xml(r#"<ShortcutsTemplate><Shortcuts>
          <Shortcut><Function>ReplaySelectChannelAB</Function><ExpandedKey>MIDINoteOff</ExpandedKey><MIDIChannel>2</MIDIChannel><MIDINote>4</MIDINote></Shortcut>
          <Shortcut><Function>PreviewInput</Function><GetInputByNumber>True</GetInputByNumber><ExpandedKey>Q, Control</ExpandedKey><Key>None</Key><Input><Number>1</Number></Input></Shortcut>
          <Shortcut><Function>ZoomJoinMeeting</Function><Key>A</Key></Shortcut>
        </Shortcuts></ShortcutsTemplate>"#, &s).unwrap();
        assert_eq!(skipped, 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].midi_kind, midi::Kind::NoteOff);
        assert_eq!(rows[1].key, "Ctrl+Q");
        assert_eq!(rows[1].action, "Preview 1");
        if let Some(file) = std::env::var_os("LIVEMIX_SHORTCUT_TEMPLATE_FIXTURE") {
            let xml = std::fs::read_to_string(file).unwrap();
            let (rows, skipped) = import_xml(&xml, &s).unwrap();
            assert_eq!((rows.len(), skipped), (36, 0));
        }
    }

    #[test]
    fn activator_changes_with_tally_and_encodes_midi_values() {
        let mut s = Switcher::demo();
        let feedback = Feedback::default();
        s.output = Some(s.inputs[0].id);
        assert!(feedback.active(&s));
        s.output = Some(s.inputs[1].id);
        assert!(!feedback.active(&s));
        assert_eq!(midi::message(midi::Kind::NoteOn, 2, 32, 127), [0x92, 32, 127]);
        assert_eq!(midi::message(midi::Kind::ControlChange, 15, 32, 0), [0xbf, 32, 0]);
    }

    #[test]
    fn numbered_shortcuts_operate_on_replay_and_overlays() {
        let mut s = Switcher::demo();
        s.perform_action("ReplayBCamera8", 1.0).unwrap();
        assert_eq!(s.replay_cameras[1], 7);
        s.perform_action("OverlayInput8In 1", 1.0).unwrap();
        assert!(s.overlays[7].on);
        s.perform_action("OverlayInput8Out", 1.0).unwrap();
        assert!(!s.overlays[7].on);
        s.perform_action("SetTransitionDuration2 120", 1.0).unwrap();
        assert_eq!(s.transitions[1].ms, 120);
    }
}
