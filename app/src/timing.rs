//! Live timing for GT titles: a JSON or XML file or URL mapped onto a title's text fields.
//!
//! A timing tower template is an ordinary GT title whose rows are named `Row1` to `RowN`,
//! with text fields `Row1.Pos`, `Row1.Name`, `Row1.Time` and so on. LiveMix reads the data
//! on a background thread, ranks the cars, and writes the text of every field. Changed fields
//! play the template's own data-change animations. With `slide` on, each row is bound to a
//! car and the row objects slide between positions when the order changes.
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::model::{Input, Kind};

pub const MODE_NAMES: [&str; 3] = ["Lap time", "Gap to leader", "Interval"];
const BODY_LIMIT: usize = 8 * 1024 * 1024;
const MAX_ROWS: usize = 64;

/// Where the data comes from and how it maps onto the template.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub enabled: bool,
    /// A file path or an http(s) URL.
    pub location: String,
    /// Seconds between reads.
    pub interval: f32,
    /// Path to the array of cars. Empty finds the first array of objects.
    pub rows: String,
    /// Key that orders the cars, ascending.
    pub sort: String,
    /// Key that identifies a car across reads, used by sliding rows.
    pub identity: String,
    /// 0 lap time, 1 gap to the leader, 2 interval to the car ahead.
    pub mode: u8,
    /// Keys read in each mode.
    pub mode_keys: [String; 3],
    /// Numeric key (milliseconds or seconds) used to compute a gap or interval the data lacks.
    pub order: String,
    /// Key shown by the leader in the gap and interval modes.
    pub leader: String,
    /// Gap key used instead of the gap mode key while a class is shown.
    pub class_gap: String,
    /// `Suffix=key` lines for the `RowN.Suffix` fields. `{rank}`, `{mode}`, `{lap}`, `{gap}`
    /// and `{interval}` are available as keys.
    pub row_map: String,
    /// `Field=path` lines for the other fields. `@mode`, `@class` and `@rows` are available.
    pub field_map: String,
    /// Key that holds the class.
    pub class_key: String,
    /// Empty shows every car, `@rotate` cycles the classes, anything else shows one class.
    pub class: String,
    /// Seconds each class stays on screen when rotating.
    pub rotate: f32,
    /// Slide the rows when the order changes instead of rewriting their text.
    pub slide: bool,
    pub slide_ms: u32,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            enabled: true,
            location: String::new(),
            interval: 2.0,
            rows: String::new(),
            sort: "position".into(),
            identity: "no".into(),
            mode: 0,
            mode_keys: ["lasttime".into(), "gap".into(), "interval".into()],
            order: "total_ms".into(),
            leader: "total".into(),
            class_gap: "gap_in_class".into(),
            row_map: "Pos={rank}\nNo=no\nName=fullname\nClass=class\nTime={mode}\n".into(),
            field_map: "Header.Event=meta.eventname\nHeader.Run=meta.runname\nHeader.Mode=@mode\nHeader.Class=@class\n".into(),
            class_key: "class".into(),
            class: String::new(),
            rotate: 5.0,
            slide: true,
            slide_ms: 700,
        }
    }
}

impl Source {
    pub fn normalized(mut self) -> Self {
        self.interval = if self.interval.is_finite() { self.interval.clamp(0.25, 300.0) } else { 2.0 };
        self.rotate = if self.rotate.is_finite() { self.rotate.clamp(1.0, 300.0) } else { 5.0 };
        self.slide_ms = self.slide_ms.clamp(50, 5000);
        self.mode = self.mode.min(2);
        self
    }

    pub fn mode_label(&self) -> &'static str {
        MODE_NAMES[usize::from(self.mode.min(2))]
    }

    /// The feed restarts when the location or the interval change; other edits apply live.
    fn same_feed(&self, other: &Source) -> bool {
        self.location == other.location && self.interval == other.interval
    }
}

// ---------------------------------------------------------------- parsing

fn is_url(location: &str) -> bool {
    let lower = location.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .user_agent("LiveMix")
        .build()
        .into();
    let mut response = agent.get(url.trim()).call().map_err(|error| error.to_string())?;
    response.body_mut().with_config().limit(BODY_LIMIT as u64).read_to_vec().map_err(|error| error.to_string())
}

fn read_file(location: &str) -> Result<Vec<u8>, String> {
    let path = location.trim();
    let path = path.strip_prefix("file:///").or_else(|| path.strip_prefix("file://")).unwrap_or(path);
    let length = std::fs::metadata(path).map_err(|error| format!("{path}: {error}"))?.len();
    if length > BODY_LIMIT as u64 { return Err("The timing file is larger than 8 MB".into()); }
    std::fs::read(path).map_err(|error| format!("{path}: {error}"))
}

/// Reads and parses one snapshot. JSON and XML are told apart by their first character.
pub fn fetch(location: &str) -> Result<Value, String> {
    if location.trim().is_empty() { return Err("Enter a file or URL".into()); }
    let bytes = if is_url(location) { http_get(location)? } else { read_file(location)? };
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> Result<Value, String> {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| "The timing data is not UTF-8 text".to_owned())?;
    if text.trim_start().starts_with('<') {
        let document = roxmltree::Document::parse(text).map_err(|error| format!("Invalid XML: {error}"))?;
        let root = document.root_element();
        let mut map = Map::new();
        map.insert(root.tag_name().name().to_owned(), element(root));
        Ok(Value::Object(map))
    } else {
        serde_json::from_str(text).map_err(|error| format!("Invalid JSON: {error}"))
    }
}

/// XML becomes JSON: attributes and child elements are keys, repeated elements are arrays.
/// An element with a `type` or `name` attribute and only text, such as
/// `<label type="eventname">Race</label>`, is also reachable as `label_by.eventname`.
fn element(node: roxmltree::Node<'_, '_>) -> Value {
    let mut map = Map::new();
    for attribute in node.attributes() {
        map.insert(attribute.name().to_owned(), Value::String(attribute.value().to_owned()));
    }
    let mut by_label: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for child in node.children().filter(roxmltree::Node::is_element) {
        let name = child.tag_name().name().to_owned();
        let value = element(child);
        if let Some(label) = child.attribute("type").or_else(|| child.attribute("name")) {
            if !child.children().any(|node| node.is_element()) {
                let text = child.text().unwrap_or_default().trim().to_owned();
                by_label.entry(format!("{name}_by")).or_default().insert(label.to_owned(), Value::String(text));
            }
        }
        match map.get_mut(&name) {
            Some(Value::Array(items)) => items.push(value),
            Some(existing) => {
                let first = existing.take();
                *existing = Value::Array(vec![first, value]);
            }
            None => { map.insert(name, value); }
        }
    }
    for (name, labels) in by_label { map.insert(name, Value::Object(labels)); }
    let text: String = node.children().filter(roxmltree::Node::is_text).filter_map(|n| n.text()).collect::<String>();
    let text = text.trim();
    if map.is_empty() { return Value::String(text.to_owned()); }
    if !text.is_empty() { map.insert("#text".into(), Value::String(text.to_owned())); }
    Value::Object(map)
}

fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in path.split('.').map(str::trim).filter(|part| !part.is_empty()) {
        current = match current {
            Value::Object(map) => map.get(part)?,
            Value::Array(items) => items.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    (!current.is_null()).then_some(current)
}

fn text_of(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => match number.as_f64() {
            Some(float) if float.fract() != 0.0 => format!("{float:.3}").trim_end_matches('0').trim_end_matches('.').to_owned(),
            _ => number.to_string(),
        },
        Value::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

fn number_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().replace(',', ".").parse().ok(),
        _ => None,
    }
}

/// `m:ss.mmm`, or `s.mmm` under a minute, with an optional sign.
pub fn format_ms(ms: f64, plus: bool) -> String {
    let sign = if ms < 0.0 { "-" } else if plus { "+" } else { "" };
    let ms = ms.abs().round() as u64;
    let (minutes, seconds, millis) = (ms / 60_000, (ms / 1000) % 60, ms % 1000);
    if minutes > 0 { format!("{sign}{minutes}:{seconds:02}.{millis:03}") } else { format!("{sign}{seconds}.{millis:03}") }
}

/// A time from the data. Text passes through; a number is milliseconds when its key ends in
/// `ms` and seconds otherwise.
fn time_text(value: &Value, key: &str, plus: bool) -> String {
    match value {
        Value::Number(number) => {
            let value = number.as_f64().unwrap_or(0.0);
            let milliseconds = if key.to_ascii_lowercase().ends_with("ms") { value } else { value * 1000.0 };
            format_ms(milliseconds, plus)
        }
        other => text_of(other),
    }
}

fn rows_of<'a>(data: &'a Value, path: &str) -> Vec<&'a Value> {
    fn first_array(value: &Value) -> Option<&Vec<Value>> {
        match value {
            Value::Array(items) if items.iter().any(Value::is_object) => Some(items),
            Value::Object(map) => map.values().find_map(first_array),
            _ => None,
        }
    }
    if path.trim().is_empty() {
        return first_array(data).map(|items| items.iter().filter(|item| item.is_object()).collect()).unwrap_or_default();
    }
    match lookup(data, path) {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(object @ Value::Object(_)) => vec![object],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- template and state

/// The rows and fields a GT title offers.
#[derive(Clone, Debug, Default)]
pub struct Template {
    pub rows: usize,
    pub fields: Vec<String>,
}

fn split_row(name: &str) -> Option<(usize, &str)> {
    let rest = name.get(..3).filter(|prefix| prefix.eq_ignore_ascii_case("row")).map(|_| &name[3..])?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let row: usize = rest[..digits].parse().ok().filter(|&row| (1..=MAX_ROWS).contains(&row))?;
    let suffix = rest[digits..].strip_prefix('.').filter(|suffix| !suffix.is_empty())?;
    Some((row, suffix))
}

impl Template {
    pub fn of(fields: &BTreeMap<String, String>) -> Self {
        let rows = fields.keys().filter_map(|name| split_row(name)).map(|(row, _)| row).max().unwrap_or(0);
        Self { rows, fields: fields.keys().cloned().collect() }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Slide { from: f32, to: f32, started: f64 }

/// Per-title timing state: which car each row shows, row movement and what the last read did.
#[derive(Clone, Debug, Default)]
pub struct Runtime {
    bound: Vec<String>,
    slides: Vec<Slide>,
    /// Whether each row shows a car. Rows without one are hidden.
    occupied: Vec<bool>,
    rotation: usize,
    rotated_at: Option<f64>,
    pub classes: Vec<String>,
    pub class: String,
    pub shown: usize,
    pub total: usize,
    pub updated: Option<f64>,
    pub error: String,
}

fn ease_out(x: f32) -> f32 { 1.0 - (1.0 - x.clamp(0.0, 1.0)).powi(3) }

impl Runtime {
    fn slot(&self, row: usize, now: f64, ms: u32) -> f32 {
        let slide = self.slides[row];
        let progress = ((now - slide.started) / (f64::from(ms) / 1000.0)) as f32;
        slide.from + (slide.to - slide.from) * ease_out(progress)
    }

    /// Vertical offsets, in template units, of the rows that are away from their own place.
    /// Rows without a car get NaN, which hides them.
    pub fn offsets(&self, now: f64, pitch: f32, ms: u32) -> HashMap<String, f32> {
        (0..self.occupied.len()).filter_map(|row| {
            if !self.occupied[row] { return Some((format!("Row{}", row + 1), f32::NAN)); }
            let slot = if row < self.slides.len() { self.slot(row, now, ms) } else { row as f32 };
            let offset = (slot - row as f32) * pitch;
            (offset.abs() > 0.01).then(|| (format!("Row{}", row + 1), offset))
        }).collect()
    }

    /// True when rotating classes should advance to the next one.
    pub fn rotation_due(&self, source: &Source, now: f64) -> bool {
        source.class == "@rotate" && self.classes.len() > 1
            && self.rotated_at.is_some_and(|at| now - at >= f64::from(source.rotate))
    }
}

fn mapping(lines: &str) -> Vec<(String, String)> {
    lines.lines().filter_map(|line| {
        let line = line.trim();
        let (name, key) = line.split_once('=')?;
        (!line.starts_with('#') && !name.trim().is_empty()).then(|| (name.trim().to_owned(), key.trim().to_owned()))
    }).collect()
}

struct Context<'a> {
    source: &'a Source,
    cars: &'a [&'a Value],
    filtered: bool,
}

impl Context<'_> {
    fn order_ms(&self, car: &Value) -> Option<f64> {
        let key = self.source.order.trim();
        if key.is_empty() { return None; }
        let value = number_of(lookup(car, key)?)?;
        Some(if key.to_ascii_lowercase().ends_with("ms") { value } else { value * 1000.0 })
    }

    fn lap(&self, car: &Value) -> String {
        let key = self.source.mode_keys[0].trim();
        lookup(car, key).map(|value| time_text(value, key, false)).unwrap_or_default()
    }

    fn gap_like(&self, rank: usize, car: &Value, interval: bool) -> String {
        let source = self.source;
        if rank == 0 {
            let key = source.leader.trim();
            return lookup(car, key).map(|value| time_text(value, key, false)).unwrap_or_default();
        }
        let mut key = source.mode_keys[if interval { 2 } else { 1 }].trim();
        if self.filtered && !interval && !source.class_gap.trim().is_empty() { key = source.class_gap.trim(); }
        if let Some(text) = lookup(car, key).map(|value| time_text(value, key, true)).filter(|text| !text.is_empty()) {
            return text;
        }
        let reference = if interval { self.cars[rank - 1] } else { self.cars[0] };
        match (self.order_ms(car), self.order_ms(reference)) {
            (Some(own), Some(other)) => format_ms(own - other, true),
            _ => String::new(),
        }
    }

    fn mode(&self, mode: u8, rank: usize, car: &Value) -> String {
        match mode {
            0 => self.lap(car),
            1 => self.gap_like(rank, car, false),
            _ => self.gap_like(rank, car, true),
        }
    }

    fn row_value(&self, key: &str, rank: usize, car: &Value) -> String {
        match key.to_ascii_lowercase().as_str() {
            "{rank}" => (rank + 1).to_string(),
            "{mode}" => self.mode(self.source.mode, rank, car),
            "{lap}" => self.mode(0, rank, car),
            "{gap}" => self.mode(1, rank, car),
            "{interval}" => self.mode(2, rank, car),
            _ => lookup(car, key).map(text_of).unwrap_or_default(),
        }
    }
}

/// Ranks the cars and returns the text of every template field. `run` keeps the row bindings,
/// the slide animations and the class rotation between calls.
pub fn evaluate(source: &Source, data: &Value, template: &Template, run: &mut Runtime, now: f64) -> Vec<(String, String)> {
    let mut sorted = rows_of(data, &source.rows);
    sorted.sort_by(|a, b| {
        let key = |car: &Value| lookup(car, &source.sort).and_then(number_of).unwrap_or(f64::INFINITY);
        key(a).total_cmp(&key(b))
    });
    // Classes in the order of their best car.
    run.classes.clear();
    if !source.class_key.trim().is_empty() {
        for car in &sorted {
            let class = lookup(car, &source.class_key).map(text_of).unwrap_or_default();
            if !class.is_empty() && !run.classes.contains(&class) { run.classes.push(class); }
        }
    }
    let rotating = source.class == "@rotate" && !run.classes.is_empty();
    if rotating {
        let started = *run.rotated_at.get_or_insert(now);
        if now - started >= f64::from(source.rotate) {
            run.rotation += 1;
            run.rotated_at = Some(now);
        }
        run.class = run.classes[run.rotation % run.classes.len()].clone();
    } else {
        run.rotated_at = None;
        run.class = if source.class.is_empty() || source.class == "@rotate" { String::new() } else {
            run.classes.iter().find(|class| class.eq_ignore_ascii_case(source.class.trim()))
                .cloned().unwrap_or_else(|| source.class.trim().to_owned())
        };
    }
    let filtered = !run.class.is_empty();
    let cars: Vec<&Value> = sorted.into_iter().filter(|car| {
        !filtered || lookup(car, &source.class_key).is_some_and(|value| text_of(value).eq_ignore_ascii_case(&run.class))
    }).collect();
    run.total = cars.len();
    let rows = template.rows;

    // Which car each row shows, and where each row belongs.
    let mut assigned: Vec<Option<usize>> = vec![None; rows];
    if source.slide {
        let ids: Vec<String> = cars.iter().enumerate().map(|(index, car)| {
            lookup(car, &source.identity).map(text_of).filter(|id| !id.is_empty()).unwrap_or_else(|| format!("#{index}"))
        }).collect();
        let top = &ids[..ids.len().min(rows)];
        let same = run.bound.len() == rows && run.bound.iter().filter(|id| !id.is_empty()).count() == top.len()
            && top.iter().all(|id| run.bound.contains(id));
        if !same {
            run.bound = (0..rows).map(|row| top.get(row).cloned().unwrap_or_default()).collect();
            run.slides = (0..rows).map(|row| Slide { from: row as f32, to: row as f32, started: f64::NEG_INFINITY }).collect();
        }
        for row in 0..rows {
            let Some(index) = (!run.bound[row].is_empty()).then(|| ids.iter().position(|id| *id == run.bound[row])).flatten() else { continue };
            assigned[row] = Some(index);
            let target = index as f32;
            if run.slides[row].to != target {
                let from = run.slot(row, now, source.slide_ms);
                run.slides[row] = Slide { from, to: target, started: now };
            }
        }
    } else {
        run.bound.clear();
        run.slides.clear();
        for (row, slot) in assigned.iter_mut().enumerate() { *slot = (row < cars.len()).then_some(row); }
    }
    run.shown = assigned.iter().flatten().count();
    run.occupied = assigned.iter().map(Option::is_some).collect();

    let context = Context { source, cars: &cars, filtered };
    let row_map = mapping(&source.row_map);
    let field_map = mapping(&source.field_map);
    let mut texts = Vec::with_capacity(template.fields.len());
    for field in &template.fields {
        if let Some((row, suffix)) = split_row(field) {
            let value = assigned.get(row - 1).copied().flatten().map(|index| {
                let key = row_map.iter().find(|(name, _)| name.eq_ignore_ascii_case(suffix))
                    .map_or_else(|| suffix.to_ascii_lowercase(), |(_, key)| key.clone());
                context.row_value(&key, index, cars[index])
            });
            texts.push((field.clone(), value.unwrap_or_default()));
        } else if let Some((_, path)) = field_map.iter().find(|(name, _)| name.eq_ignore_ascii_case(field)) {
            let value = match path.as_str() {
                "@mode" => source.mode_label().to_ascii_uppercase(),
                "@class" => if filtered { run.class.to_ascii_uppercase() } else { "ALL".into() },
                "@rows" => run.shown.to_string(),
                path => lookup(data, path).map(text_of).unwrap_or_default(),
            };
            texts.push((field.clone(), value));
        }
    }
    texts
}

/// Applies one snapshot to a GT title: every changed field starts its data-change animation.
pub fn apply(input: &mut Input, data: &Value, now: f64) {
    let Some(source) = input.timing.clone() else { return };
    let Kind::GtTitle { document: Some(document), .. } = &input.kind else {
        input.timing_run.error = "Add a GT title to show live timing".into();
        return;
    };
    let document = document.clone();
    let template = Template::of(&document.fields);
    let updates = evaluate(&source, data, &template, &mut input.timing_run, now);
    for (field, value) in updates {
        let _ = input.set_gt_field(&field, value, now);
    }
    input.timing_run.updated = Some(now);
    input.timing_run.error.clear();
}

// ---------------------------------------------------------------- reading thread

struct Shared {
    latest: Option<Result<Arc<Value>, String>>,
}

/// Reads a file or URL on its own thread. Only the newest result is kept.
pub struct Feed {
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    source: Source,
}

impl Feed {
    pub fn start(source: &Source) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(Shared { latest: None }));
        let (location, interval) = (source.location.clone(), Duration::from_secs_f32(source.interval.clamp(0.25, 300.0)));
        let (thread_stop, thread_shared) = (stop.clone(), shared.clone());
        std::thread::spawn(move || {
            while !thread_stop.load(Relaxed) {
                let result = fetch(&location).map(Arc::new);
                thread_shared.lock().unwrap().latest = Some(result);
                let mut waited = Duration::ZERO;
                while waited < interval && !thread_stop.load(Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                    waited += Duration::from_millis(50);
                }
            }
        });
        Self { stop, shared, source: source.clone() }
    }

    pub fn take(&self) -> Option<Result<Arc<Value>, String>> {
        self.shared.lock().unwrap().latest.take()
    }

    pub fn matches(&self, source: &Source) -> bool {
        self.source.same_feed(source)
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
    }
}

/// One title's reader and the last snapshot it delivered.
pub struct Tracker {
    feed: Feed,
    data: Option<Arc<Value>>,
    applied: Option<Source>,
}

/// Starts, restarts and stops the readers and writes new data into the titles. Called every frame.
pub fn update(inputs: &mut [Input], trackers: &mut HashMap<u32, Tracker>, now: f64) {
    fn live(input: &Input) -> Option<&Source> {
        input.timing.as_ref().filter(|source| source.enabled && matches!(input.kind, Kind::GtTitle { .. }))
    }
    trackers.retain(|id, tracker| inputs.iter().any(|input| input.id == *id && live(input).is_some_and(|source| tracker.feed.matches(source))));
    for input in inputs.iter_mut() {
        let Some(source) = live(input).cloned() else { continue };
        let tracker = trackers.entry(input.id).or_insert_with(|| Tracker { feed: Feed::start(&source), data: None, applied: None });
        match tracker.feed.take() {
            Some(Ok(data)) => {
                tracker.data = Some(data);
                tracker.applied = None;
            }
            Some(Err(error)) => input.timing_run.error = error,
            None => {}
        }
        let Some(data) = tracker.data.clone() else { continue };
        if tracker.applied.as_ref() != Some(&source) || input.timing_run.rotation_due(&source, now) {
            apply(input, &data, now);
            tracker.applied = Some(source);
        }
    }
}

// ---------------------------------------------------------------- interface

/// The Live Timing section of a GT title's settings.
pub fn settings(ui: &mut egui::Ui, input: &mut Input, now: f64) {
    let has_rows = matches!(&input.kind, Kind::GtTitle { document: Some(document), .. }
        if Template::of(&document.fields).rows > 0);
    let mut enabled = input.timing.as_ref().is_some_and(|source| source.enabled);
    ui.horizontal(|ui| {
        ui.heading("Live Timing");
        if ui.checkbox(&mut enabled, "On").changed() {
            input.timing.get_or_insert_with(Source::default).enabled = enabled;
        }
    });
    let Some(source) = input.timing.as_mut() else {
        ui.label(egui::RichText::new("Read a JSON or XML file or URL into the Row1…RowN fields of this title.")
            .size(11.0).color(egui::Color32::GRAY));
        return;
    };
    if !has_rows {
        ui.label(egui::RichText::new("This title has no Row1.Name style fields. Rows show data only when it does.")
            .size(11.0).color(egui::Color32::LIGHT_YELLOW));
    }
    let before = source.clone();
    egui::Grid::new(("timing-grid", input.id)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        ui.label("File or URL");
        ui.add(egui::TextEdit::singleline(&mut source.location).desired_width(300.0)
            .hint_text("D:\\data.json or https://…"));
        ui.end_row();
        ui.label("Read every");
        ui.add(egui::DragValue::new(&mut source.interval).range(0.25..=300.0).speed(0.1).suffix(" s"));
        ui.end_row();
        ui.label("Show");
        egui::ComboBox::from_id_salt(("timing-mode", input.id)).selected_text(source.mode_label()).show_ui(ui, |ui| {
            for (index, name) in MODE_NAMES.iter().enumerate() {
                ui.selectable_value(&mut source.mode, index as u8, *name);
            }
        });
        ui.end_row();
        ui.label("Class");
        let label = match source.class.as_str() { "" => "All".to_owned(), "@rotate" => "Rotate".to_owned(), other => other.to_owned() };
        egui::ComboBox::from_id_salt(("timing-class", input.id)).selected_text(label).show_ui(ui, |ui| {
            ui.selectable_value(&mut source.class, String::new(), "All");
            ui.selectable_value(&mut source.class, "@rotate".into(), "Rotate");
            for class in &input.timing_run.classes {
                ui.selectable_value(&mut source.class, class.clone(), class);
            }
        });
        ui.end_row();
        ui.label("Rows");
        ui.horizontal(|ui| {
            ui.checkbox(&mut source.slide, "Slide on position change");
            ui.add_enabled(source.slide, egui::DragValue::new(&mut source.slide_ms).range(50..=5000).speed(10.0).suffix(" ms"));
        });
        ui.end_row();
        if source.class == "@rotate" {
            ui.label("Rotate every");
            ui.add(egui::DragValue::new(&mut source.rotate).range(1.0..=300.0).speed(0.1).suffix(" s"));
            ui.end_row();
        }
    });
    ui.collapsing("Mapping", |ui| {
        egui::Grid::new(("timing-map", input.id)).num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
            for (label, value) in [("Cars at", &mut source.rows), ("Order by", &mut source.sort),
                ("Car identity", &mut source.identity), ("Class key", &mut source.class_key)]
            {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(value).desired_width(160.0));
                ui.end_row();
            }
            for (index, label) in ["Lap time key", "Gap key", "Interval key"].into_iter().enumerate() {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(&mut source.mode_keys[index]).desired_width(160.0));
                ui.end_row();
            }
            for (label, value) in [("Total time (ms) key", &mut source.order), ("Leader shows", &mut source.leader),
                ("Class gap key", &mut source.class_gap)]
            {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(value).desired_width(160.0));
                ui.end_row();
            }
        });
        ui.label("Row fields (Suffix=key)");
        ui.add(egui::TextEdit::multiline(&mut source.row_map).desired_rows(4).desired_width(300.0).code_editor());
        ui.label("Other fields (Field=path)");
        ui.add(egui::TextEdit::multiline(&mut source.field_map).desired_rows(4).desired_width(300.0).code_editor());
        if ui.button("Restore mapping defaults").clicked() {
            let defaults = Source::default();
            source.row_map = defaults.row_map;
            source.field_map = defaults.field_map;
        }
    });
    let changed = *source != before;
    let run = &input.timing_run;
    let status = if !run.error.is_empty() { run.error.clone() } else if let Some(at) = run.updated {
        format!("{} cars · {} rows · updated {:.0} s ago", run.total, run.shown, (now - at).max(0.0))
    } else if source.enabled { "Waiting for data…".into() } else { "Off".into() };
    ui.label(egui::RichText::new(status).size(11.0).color(if run.error.is_empty() {
        egui::Color32::GRAY } else { egui::Color32::LIGHT_RED }));
    if changed {
        input.timing = input.timing.take().map(Source::normalized);
        input.timing_run.updated = None;
    }
}

/// A command from a shortcut or script: `lap`, `gap`, `interval` or `next` for the mode, and
/// `all`, `rotate` or a class name for the class.
pub fn command(input: &mut Input, what: &str, value: &str) -> Result<(), String> {
    let source = input.timing.as_mut().ok_or("This input has no live timing")?;
    match what {
        "mode" => {
            source.mode = match value.to_ascii_lowercase().as_str() {
                "lap" | "laptime" | "0" => 0,
                "gap" | "leader" | "1" => 1,
                "interval" | "2" => 2,
                "next" | "toggle" => (source.mode + 1) % 3,
                _ => return Err("Use lap, gap, interval or next".into()),
            };
        }
        _ => {
            source.class = match value.to_ascii_lowercase().as_str() {
                "" | "all" => String::new(),
                "rotate" | "@rotate" => "@rotate".into(),
                _ => value.to_owned(),
            };
        }
    }
    input.timing_run.updated = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fields(rows: usize) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for row in 1..=rows {
            for suffix in ["Pos", "No", "Name", "Time"] { map.insert(format!("Row{row}.{suffix}"), String::new()); }
        }
        for field in ["Header.Event", "Header.Mode", "Header.Class"] { map.insert(field.into(), String::new()); }
        map
    }

    fn sample() -> Value {
        json!({
            "meta": {"eventname": "GRAND PRIX", "runname": "Race 1"},
            "drivers": [
                {"no": "7", "fullname": "SECOND", "class": "A", "position": 2, "lasttime": "1:31.200", "gap": "+0.800", "total_ms": 100800.0},
                {"no": "1", "fullname": "LEADER", "class": "A", "position": 1, "lasttime": "1:31.000", "gap": "", "total": "1:40.000", "total_ms": 100000.0},
                {"no": "9", "fullname": "THIRD", "class": "B", "position": 3, "lasttime": 92.5, "gap": null, "total_ms": 103500.0},
            ]
        })
    }

    fn get<'a>(texts: &'a [(String, String)], field: &str) -> &'a str {
        texts.iter().find(|(name, _)| name == field).map(|(_, value)| value.as_str()).unwrap()
    }

    #[test]
    fn json_cars_are_ranked_and_mapped_onto_the_row_fields() {
        let source = Source { slide: false, ..Default::default() };
        let template = Template::of(&fields(3));
        let texts = evaluate(&source, &sample(), &template, &mut Runtime::default(), 0.0);
        assert_eq!(get(&texts, "Row1.Name"), "LEADER");
        assert_eq!(get(&texts, "Row2.Name"), "SECOND");
        assert_eq!(get(&texts, "Row3.No"), "9");
        assert_eq!(get(&texts, "Row2.Pos"), "2");
        assert_eq!(get(&texts, "Row1.Time"), "1:31.000", "lap time mode");
        assert_eq!(get(&texts, "Row3.Time"), "1:32.500", "a number of seconds becomes a lap time");
        assert_eq!(get(&texts, "Header.Event"), "GRAND PRIX");
        assert_eq!(get(&texts, "Header.Mode"), "LAP TIME");
    }

    #[test]
    fn gap_and_interval_come_from_the_data_or_are_computed_from_total_time() {
        let template = Template::of(&fields(3));
        let mut source = Source { slide: false, mode: 1, ..Default::default() };
        let gap = evaluate(&source, &sample(), &template, &mut Runtime::default(), 0.0);
        assert_eq!(get(&gap, "Row1.Time"), "1:40.000", "the leader shows its total time");
        assert_eq!(get(&gap, "Row2.Time"), "+0.800", "the data's own gap is kept");
        assert_eq!(get(&gap, "Row3.Time"), "+3.500", "a missing gap is computed from total_ms");
        assert_eq!(get(&gap, "Header.Mode"), "GAP TO LEADER");
        source.mode = 2;
        let interval = evaluate(&source, &sample(), &template, &mut Runtime::default(), 0.0);
        assert_eq!(get(&interval, "Row2.Time"), "+0.800");
        assert_eq!(get(&interval, "Row3.Time"), "+2.700", "interval is the distance to the car ahead");
        assert_eq!(get(&interval, "Header.Mode"), "INTERVAL");
    }

    #[test]
    fn xml_is_read_like_json_including_labelled_values() {
        let xml = r#"<resultspage><label type="eventname">ROUND 3</label><results>
            <result no="5" fullname="ONE" position="1" lasttime="59.100"/>
            <result no="6" fullname="TWO" position="2" lasttime="59.900"/></results></resultspage>"#;
        let data = parse(xml.as_bytes()).unwrap();
        assert_eq!(lookup(&data, "resultspage.label_by.eventname").map(text_of).as_deref(), Some("ROUND 3"));
        let source = Source { slide: false, field_map: "Header.Event=resultspage.label_by.eventname".into(), ..Default::default() };
        let texts = evaluate(&source, &data, &Template::of(&fields(2)), &mut Runtime::default(), 0.0);
        assert_eq!(get(&texts, "Row2.Name"), "TWO");
        assert_eq!(get(&texts, "Row1.Time"), "59.100");
        assert_eq!(get(&texts, "Header.Event"), "ROUND 3");
        assert!(parse(b"{not json").is_err() && parse(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn classes_filter_and_rotate() {
        let template = Template::of(&fields(3));
        let mut run = Runtime::default();
        let mut source = Source { slide: false, class: "B".into(), ..Default::default() };
        let texts = evaluate(&source, &sample(), &template, &mut run, 0.0);
        assert_eq!((get(&texts, "Row1.Name"), get(&texts, "Row2.Name")), ("THIRD", ""));
        assert_eq!(get(&texts, "Row1.Pos"), "1", "the rank counts inside the class");
        assert_eq!(get(&texts, "Header.Class"), "B");
        source.class = "@rotate".into();
        let mut run = Runtime::default();
        assert_eq!(get(&evaluate(&source, &sample(), &template, &mut run, 10.0), "Header.Class"), "A");
        assert!(!run.rotation_due(&source, 12.0) && run.rotation_due(&source, 15.5));
        assert_eq!(get(&evaluate(&source, &sample(), &template, &mut run, 15.5), "Header.Class"), "B");
        assert_eq!(run.classes, ["A", "B"]);
    }

    #[test]
    fn rows_stay_with_their_car_and_slide_when_the_order_changes() {
        let template = Template::of(&fields(3));
        let source = Source::default();
        let mut run = Runtime::default();
        let first = evaluate(&source, &sample(), &template, &mut run, 0.0);
        assert_eq!(get(&first, "Row1.Name"), "LEADER");
        assert!(run.offsets(0.0, 40.0, 700).is_empty(), "nothing moves on the first read and every row has a car");
        // The cars in positions 1 and 2 swap.
        let mut swapped = sample();
        swapped["drivers"][0]["position"] = json!(1);
        swapped["drivers"][1]["position"] = json!(2);
        let texts = evaluate(&source, &swapped, &template, &mut run, 5.0);
        assert_eq!(get(&texts, "Row1.Name"), "LEADER", "row 1 still shows the same car");
        assert_eq!(get(&texts, "Row1.Pos"), "2", "with its new position");
        assert_eq!(get(&texts, "Row2.Name"), "SECOND");
        let start = run.offsets(5.0, 40.0, 700);
        assert!(start.is_empty(), "the move starts from where the row is");
        let middle = run.offsets(5.35, 40.0, 700);
        let (leader, second) = (middle["Row1"], middle["Row2"]);
        assert!(leader > 0.0 && leader < 40.0, "the leader's row moves down: {leader}");
        assert!(second < 0.0 && second > -40.0, "the other row moves up: {second}");
        let end = run.offsets(6.0, 40.0, 700);
        assert!((end["Row1"] - 40.0).abs() < 0.01 && (end["Row2"] + 40.0).abs() < 0.01);
        // A different set of cars rebinds the rows at once instead of sliding.
        let mut other = sample();
        other["drivers"][2]["no"] = json!("99");
        evaluate(&source, &other, &template, &mut run, 9.0);
        assert!(run.offsets(9.0, 40.0, 700).is_empty());
    }

    #[test]
    fn template_rows_are_found_from_field_names() {
        let mut names = fields(4);
        names.insert("Rowing.Name".into(), String::new());
        names.insert("row12.time".into(), String::new());
        assert_eq!(Template::of(&names).rows, 12);
        assert_eq!(split_row("Row3.Name"), Some((3, "Name")));
        assert_eq!(split_row("Row.Name"), None);
        assert_eq!(split_row("Header.Name"), None);
        assert_eq!(format_ms(61_234.0, true), "+1:01.234");
        assert_eq!(format_ms(-500.0, true), "-0.500");
    }

    /// Every text the template draws at `t`, with the y position it is drawn at.
    fn drawn(input: &Input, t: f64) -> Vec<(String, f32)> {
        let Kind::GtTitle { document: Some(document), texts, .. } = &input.kind else { unreachable!() };
        let data: Vec<(crate::gt::Cue<'_>, &str)> = input.gt_changes.iter().filter_map(|change| {
            document.data_cue((t - change.started).max(0.0) as f32, &change.field).map(|cue| (cue, change.previous.as_str()))
        }).collect();
        let offsets = input.timing_run.offsets(t, document.row_pitch().unwrap(), 700);
        let context = egui::Context::default();
        let output = context.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::background());
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0));
            document.draw_with(&painter, rect, 1.0, texts, crate::gt::Cue::Rest, &data, &offsets, t);
        });
        output.shapes.into_iter().filter_map(|clipped| match clipped.shape {
            egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.pos.y)),
            _ => None,
        }).collect()
    }

    #[test]
    fn the_example_tower_fills_from_data_animates_changed_fields_and_slides_rows() {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/examples/timing-tower");
        let mut input = Input::new(0, "Tower", Kind::GtTitle {
            path: folder.join("timing-tower.gtzip").to_string_lossy().into_owned(), texts: Default::default(), document: None,
        });
        input.reload_gt().unwrap();
        input.timing = Some(Source::default());
        let Kind::GtTitle { document: Some(document), .. } = &input.kind else { unreachable!() };
        assert!(document.unsupported.is_empty(), "{:?}", document.unsupported);
        assert_eq!(Template::of(&document.fields).rows, 20);
        assert_eq!(document.row_pitch(), Some(34.0));
        assert!(document.seconds(false).is_some(), "the template has its own entrance");

        let sample = parse(&std::fs::read(folder.join("timing-sample.json")).unwrap()).unwrap();
        let (first, second) = (text_of(&sample["drivers"][0]["fullname"]), text_of(&sample["drivers"][1]["fullname"]));
        apply(&mut input, &sample, 10.0);
        let Kind::GtTitle { texts, .. } = &input.kind else { unreachable!() };
        assert_eq!((texts["Row1.Name"].as_str(), texts["Row2.Name"].as_str()), (first.as_str(), second.as_str()));
        assert_eq!(texts["Header.Event"], "SAMPLE GRAND PRIX");
        assert!(input.gt_changes.len() > 20, "the first read changes many fields at once");
        assert_eq!(input.timing_run.total, 12);
        // Twelve cars fill twelve rows; the other eight are blank.
        assert_eq!(drawn(&input, 20.0).iter().filter(|(text, _)| *text == first || *text == second).count(), 2);
        let hidden = input.timing_run.offsets(20.0, 34.0, 700);
        assert!(hidden["Row13"].is_nan() && hidden["Row20"].is_nan() && !hidden.contains_key("Row12"), "rows without a car are hidden");

        // The cars in positions 1 and 2 swap.
        let mut swapped = sample.clone();
        swapped["drivers"][0]["position"] = json!(2);
        swapped["drivers"][1]["position"] = json!(1);
        apply(&mut input, &swapped, 20.0);
        let Kind::GtTitle { texts, .. } = &input.kind else { unreachable!() };
        assert_eq!(texts["Row1.Name"], first, "the row keeps its car");
        assert_eq!(texts["Row1.Pos"], "2");
        let name_y = |t: f64, name: &str| drawn(&input, t).into_iter().find(|(text, _)| text == name).unwrap().1;
        let pos_y = |t: f64| drawn(&input, t).into_iter().filter(|(text, _)| text == "1" || text == "2").map(|(text, _)| text).collect::<Vec<_>>();
        let home = name_y(19.9, &first);
        let (early, late) = (name_y(20.25, &first), name_y(21.0, &first));
        assert!(early > home && early < home + 34.0, "the row is on its way: {home} {early}");
        assert!((late - home - 34.0).abs() < 0.5, "and arrives one row lower: {home} {late}");
        assert!((name_y(21.0, &second) - (name_y(19.9, &second) - 34.0)).abs() < 0.5, "the other row moves up");
        // While the position number animates out it still shows the old one; afterwards the new one.
        assert!(input.gt_changes.iter().any(|change| change.field == "Row1.Pos" && change.previous == "1"));
        assert!(pos_y(20.05).contains(&"1".to_owned()), "the old position fades out first");
        assert!(pos_y(20.6).contains(&"2".to_owned()), "then the new one is shown");
    }

    #[test]
    fn the_timing_setup_survives_saving_and_reopening_and_shortcuts_change_the_view() {
        use crate::model::Switcher;
        let template = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/examples/timing-tower/timing-tower.gtzip");
        let source = Source {
            location: "https://example.test/live.json".into(), interval: 1.5, mode: 2, class: "@rotate".into(),
            slide: false, slide_ms: 450, row_map: "Pos={rank}\nName=driver".into(), ..Default::default()
        }.normalized();
        let mut switcher = Switcher::empty();
        let mut tower = Input::new(0, "Tower", Kind::GtTitle {
            path: template.to_string_lossy().into_owned(), texts: Default::default(), document: None,
        });
        tower.timing = Some(source.clone());
        switcher.add(tower);
        let find = |switcher: &Switcher| switcher.inputs.iter().find(|input| input.title == "Tower").unwrap().timing.clone();

        let project = crate::preset::encode(&switcher).unwrap();
        let mut reopened = Switcher::empty();
        crate::preset::decode(&mut reopened, std::path::Path::new("tower.livemix"), &project).unwrap();
        assert_eq!(find(&reopened), Some(source.clone()), "project file");
        assert!(reopened.inputs.iter().any(|input| matches!(&input.kind, Kind::GtTitle { document: Some(_), .. })));
        let mut recovered = Switcher::empty();
        recovered.load_preset_text(&switcher.preset_text());
        assert_eq!(find(&recovered), Some(source), "recovery copy");

        let number = switcher.inputs.iter().position(|input| input.title == "Tower").unwrap() + 1;
        for (command, check) in [
            (format!("SetTimingMode {number} gap"), (1, "@rotate")),
            (format!("SetTimingMode {number} next"), (2, "@rotate")),
            (format!("SetTimingClass {number} BI500"), (2, "BI500")),
            (format!("SetTimingClass {number} all"), (2, "")),
        ] {
            switcher.perform_action(&command, 0.0).unwrap_or_else(|error| panic!("{command}: {error}"));
            let source = find(&switcher).unwrap();
            assert_eq!((source.mode, source.class.as_str()), check, "{command}");
        }
        assert!(switcher.perform_action(&format!("SetTimingMode {number} sideways"), 0.0).is_err());
    }

    #[test]
    fn feed_reads_a_changing_file_without_blocking() {
        let path = std::env::temp_dir().join(format!("livemix-timing-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"drivers":[{"no":"1","position":1}]}"#).unwrap();
        let source = Source { location: path.to_string_lossy().into_owned(), interval: 0.25, ..Default::default() };
        let feed = Feed::start(&source);
        let wait = |feed: &Feed| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(result) = feed.take() { return result; }
                assert!(std::time::Instant::now() < deadline, "no data arrived");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        assert_eq!(rows_of(&wait(&feed).unwrap(), "").len(), 1);
        std::fs::write(&path, r#"{"drivers":[{"no":"1","position":1},{"no":"2","position":2}]}"#).unwrap();
        loop {
            if rows_of(&wait(&feed).unwrap(), "").len() == 2 { break; }
        }
        std::fs::write(&path, "not json").unwrap();
        let error = loop { if let Err(error) = wait(&feed) { break error; } };
        assert!(error.contains("JSON"), "{error}");
        assert!(feed.matches(&Source { location: source.location.clone(), interval: 0.25, mode: 2, ..Default::default() }));
        assert!(!feed.matches(&Source { interval: 1.0, ..source }));
        drop(feed);
        let _ = std::fs::remove_file(path);
    }
}
