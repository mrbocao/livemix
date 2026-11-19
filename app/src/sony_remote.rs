//! Optional local Alpha Camera REST API bridge to the Sony Camera Remote SDK.
use crate::egui;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROPERTIES: [(&str, &str); 4] = [
    ("Shutter", "shutter-speed"),
    ("Iris", "aperture"),
    ("ISO", "iso"),
    ("White balance (K)", "white-balance-color-temp"),
];
const RESPONSE_LIMIT: u64 = 1024 * 1024;

fn bridge() -> SocketAddr {
    let port = std::env::var("LIVEMIX_SONY_BRIDGE_PORT").ok()
        .and_then(|value| value.parse::<u16>().ok()).filter(|&port| port != 0).unwrap_or(8080);
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

fn request(address: SocketAddr, method: &str, path: &str, body: Value) -> Result<Value, String> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(1))
        .map_err(|_| format!("Sony control bridge unavailable on {address}"))?;
    let timeout = Some(Duration::from_secs(6));
    stream.set_read_timeout(timeout).map_err(|error| error.to_string())?;
    stream.set_write_timeout(timeout).map_err(|error| error.to_string())?;
    let body = if body.is_null() { String::new() } else { body.to_string() };
    write!(stream, "{method} {path} HTTP/1.0\r\nHost: {address}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
        .map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream.take(RESPONSE_LIMIT));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|error| error.to_string())?;
    let code = line.split_whitespace().nth(1).and_then(|value| value.parse::<u16>().ok())
        .ok_or("Invalid Sony bridge HTTP response")?;
    let mut length = None;
    loop {
        line.clear();
        if reader.read_line(&mut line).map_err(|error| error.to_string())? == 0 {
            return Err("Incomplete Sony bridge HTTP headers".into());
        }
        if line.trim().is_empty() { break; }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") { length = value.trim().parse::<u64>().ok(); }
        }
    }
    let mut bytes = Vec::new();
    reader.take(length.unwrap_or(RESPONSE_LIMIT).min(RESPONSE_LIMIT)).read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let data: Value = serde_json::from_slice(&bytes).map_err(|_| format!("Invalid Sony bridge JSON (HTTP {code})"))?;
    if !(200..300).contains(&code) || data["success"] != true {
        // Do not expose request credentials or arbitrary bridge responses in application logs.
        return Err(format!("Sony bridge rejected the camera command (HTTP {code}). Check the camera connection and controls."));
    }
    Ok(data)
}

fn camera_path(camera: &str) -> Result<String, String> {
    if camera.is_empty() || camera.len() > 100 || !camera.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
        return Err("Select a camera first".into());
    }
    Ok(format!("/api/cameras/{camera}"))
}

fn numeric(value: &str) -> Option<f64> {
    let value = value.trim().trim_end_matches(['K', 's']).trim();
    let result = if let Some((a, b)) = value.split_once('/') {
        a.parse::<f64>().ok()? / b.parse::<f64>().ok()?
    } else {
        value.parse().ok()?
    };
    (result.is_finite() && result > 0.0).then_some(result)
}

fn match_property(path: &str, calculate: impl FnOnce(f64) -> f64, distance: impl Fn(f64, f64) -> f64)
    -> Result<String, String>
{
    let info = request(bridge(), "GET", path, Value::Null)?;
    let info = &info["data"];
    if info["writable"] != true && info["writable"] != "true" { return Err("Control is read only".into()); }
    let current = info["formatted"].as_str().and_then(numeric).ok_or("Current value unavailable")?;
    let target = calculate(current);
    let (value, name, _) = info["available_values"].as_array().into_iter().flatten().filter_map(|entry| {
        let label = entry["formatted"].as_str()?;
        Some((entry["value"].as_str()?, label, numeric(label)?))
    }).min_by(|a, b| distance(a.2, target).total_cmp(&distance(b.2, target)))
        .ok_or("Camera did not enumerate supported values")?;
    if info["value"] != value { request(bridge(), "PUT", path, json!({"value":value}))?; }
    Ok(name.to_owned())
}

pub fn match_controls(ui: &mut egui::Ui, camera: &str, ev: f32, mired: f32, fps: f32, enabled: bool) {
    let status = ui.data_mut(|data| data.get_temp_mut_or_insert_with(ui.id().with(("sony-match", camera)), || {
        Arc::new(Mutex::new(Some(String::new())))
    }).clone());
    let mut displayed = status.lock().unwrap();
    if ui.add_enabled(enabled && displayed.is_some(), egui::Button::new("Apply to Sony camera")).clicked() {
        *displayed = None;
        let camera = camera.to_owned();
        let status = status.clone();
        let ctx = ui.ctx().clone();
        std::thread::spawn(move || {
            let result = camera_path(&camera).map(|path| {
                let shutter = match_property(&format!("{path}/properties/shutter-speed"),
                    |seconds| (seconds * 2f64.powf(f64::from(ev.clamp(-3.0, 3.0)))).min(1.0 / f64::from(fps.max(1.0))),
                    |a, b| (a / b).log2().abs());
                let balance = match_property(&format!("{path}/properties/white-balance-color-temp"),
                    |kelvin| 1.0e6 / (1.0e6 / kelvin - f64::from(mired.clamp(-100.0, 100.0))).clamp(60.0, 500.0),
                    |a, b| (a - b).abs());
                let display = |result: Result<String, String>| result.unwrap_or_else(|error| error);
                format!("Shutter: {} · White balance: {}", display(shutter), display(balance))
            }).unwrap_or_else(|error| error);
            *status.lock().unwrap() = Some(result);
            ctx.request_repaint();
        });
    }
    match displayed.as_deref() {
        Some(text) if !text.is_empty() => { ui.label(text); }
        None => { ui.label("Applying camera settings…"); }
        _ => {}
    }
}

#[derive(Default)]
struct Panel {
    busy: bool,
    status: String,
    cameras: Vec<(String, String)>,
    property: usize,
    values: Vec<(String, String)>,
    current: String,
    writable: bool,
    selected: String,
    user: String,
    password: String,
    fingerprint: String,
}

enum Job {
    Discover,
    Connect(Value),
    Disconnect,
    Read(usize),
    Set(usize, String),
    Record,
}

fn run(camera: &str, job: Job, panel: &Mutex<Panel>) -> Result<&'static str, String> {
    let address = bridge();
    if matches!(job, Job::Discover) {
        let data = request(address, "GET", "/api/cameras", Value::Null)?;
        let cameras = data["cameras"].as_array().ok_or("Invalid Sony camera list")?;
        panel.lock().unwrap().cameras = cameras.iter().take(64).filter_map(|camera| {
            let id = camera["id"].as_str()?;
            camera_path(id).ok()?;
            Some((id.to_owned(), format!("{} · {id}", camera["model"].as_str().unwrap_or("Sony"))))
        }).collect();
        return Ok("Camera list refreshed");
    }
    let path = camera_path(camera)?;
    match job {
        Job::Discover => unreachable!(),
        Job::Connect(auth) => {
            let mut body = json!({"mode":"remote"});
            if !auth.is_null() { body["auth"] = auth; }
            request(address, "POST", &format!("{path}/connection"), body)?;
            Ok("Sony camera connected")
        }
        Job::Disconnect => {
            request(address, "DELETE", &format!("{path}/connection"), Value::Null)?;
            Ok("Sony camera disconnected")
        }
        Job::Read(index) => {
            let data = request(address, "GET", &format!("{path}/properties/{}", PROPERTIES[index].1), Value::Null)?;
            let property = &data["data"];
            let mut panel = panel.lock().unwrap();
            panel.writable = property["writable"] == true || property["writable"] == "true";
            panel.selected = property["value"].as_str().unwrap_or_default().to_owned();
            panel.current = property["formatted"].as_str().unwrap_or_default().to_owned();
            panel.values = property["available_values"].as_array().into_iter().flatten().take(256).filter_map(|item| {
                Some((item["value"].as_str()?.to_owned(), item["formatted"].as_str()?.to_owned()))
            }).collect();
            Ok("Camera setting read")
        }
        Job::Set(index, value) => {
            request(address, "PUT", &format!("{path}/properties/{}", PROPERTIES[index].1), json!({"value":value}))?;
            run(camera, Job::Read(index), panel)
        }
        Job::Record => {
            request(address, "POST", &format!("{path}/actions/movie-rec"), Value::Null)?;
            Ok("Camera recording toggled")
        }
    }
}

pub fn show(ui: &mut egui::Ui, input: u32, camera: &mut String) {
    ui.collapsing("Sony Camera Remote", |ui| {
        ui.label(format!("Camera Remote SDK bridge · {}", bridge()));
        let state = ui.data_mut(|data| data.get_temp_mut_or_insert_with(ui.id().with(input), || {
            Arc::new(Mutex::new(Panel::default()))
        }).clone());
        let mut panel = state.lock().unwrap();
        let mut job = None;
        ui.add_enabled_ui(!panel.busy, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Discover Sony cameras").clicked() { job = Some(Job::Discover); }
                let mut id = camera.clone();
                egui::ComboBox::from_id_salt("sony-camera").selected_text(if id.is_empty() { "Choose a camera" } else { &id })
                    .show_ui(ui, |ui| {
                        for (key, name) in &panel.cameras { ui.selectable_value(&mut id, key.clone(), name); }
                    });
                if id != *camera {
                    *camera = id;
                    panel.values.clear();
                    panel.current.clear();
                    panel.writable = false;
                    panel.user.clear();
                    panel.password.clear();
                    panel.fingerprint.clear();
                }
            });
            ui.add_enabled_ui(camera_path(camera).is_ok(), |ui| {
                ui.collapsing("Connection credentials (temporary)", |ui| {
                    ui.add(egui::TextEdit::singleline(&mut panel.user).hint_text("Username").char_limit(128));
                    ui.add(egui::TextEdit::singleline(&mut panel.password).hint_text("Password").password(true).char_limit(128));
                    ui.add(egui::TextEdit::singleline(&mut panel.fingerprint).hint_text("Verified SDK fingerprint").char_limit(256));
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Connect").clicked() {
                        let auth = if panel.user.is_empty() && panel.password.is_empty() && panel.fingerprint.is_empty() { Value::Null }
                            else { json!({"username":panel.user,"password":std::mem::take(&mut panel.password),"fingerprint":panel.fingerprint}) };
                        job = Some(Job::Connect(auth));
                    }
                    if ui.button("Disconnect").clicked() { job = Some(Job::Disconnect); }
                    if ui.button("Toggle camera recording").clicked() { job = Some(Job::Record); }
                });
                ui.horizontal_wrapped(|ui| {
                    let mut index = panel.property;
                    egui::ComboBox::from_id_salt("sony-property").selected_text(PROPERTIES[index].0).show_ui(ui, |ui| {
                        for (index_value, (label, _)) in PROPERTIES.iter().enumerate() { ui.selectable_value(&mut index, index_value, *label); }
                    });
                    if index != panel.property || ui.button("Read setting").clicked() {
                        panel.property = index;
                        panel.values.clear();
                        panel.current.clear();
                        panel.writable = false;
                        job = Some(Job::Read(index));
                    }
                    if !panel.current.is_empty() { ui.label(&panel.current); }
                    let mut value = panel.selected.clone();
                    if panel.writable && !panel.values.is_empty() {
                        egui::ComboBox::from_id_salt("sony-property-value")
                            .selected_text(panel.values.iter().find(|(key, _)| key == &value).map_or("Choose value", |(_, label)| label))
                            .show_ui(ui, |ui| {
                                for (key, label) in &panel.values { ui.selectable_value(&mut value, key.clone(), label); }
                            });
                        panel.selected = value;
                        if ui.button("Apply").clicked() { job = Some(Job::Set(index, panel.selected.clone())); }
                    }
                });
            });
        });
        if let Some(job) = job {
            panel.busy = true;
            panel.status = "Waiting for Sony camera…".into();
            let camera = camera.clone();
            let state = state.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let result = run(&camera, job, &state);
                let mut panel = state.lock().unwrap();
                panel.status = result.map_or_else(|error| error, str::to_owned);
                panel.busy = false;
                ctx.request_repaint();
            });
        }
        ui.label(&panel.status);
    });
}

#[test]
fn reads_a_local_sony_bridge_response() {
    let server = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = server.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (stream, _) = server.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut socket = BufReader::new(stream);
        let mut line = String::new();
        socket.read_line(&mut line).unwrap();
        assert_eq!(line, "GET /api/cameras HTTP/1.0\r\n");
        loop {
            line.clear();
            assert!(socket.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" { break; }
        }
        let json = r#"{"success":true,"cameras":[{"id":"ABC123","model":"PXW-Z200"}]}"#;
        write!(socket.get_mut(), "HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n{json}", json.len()).unwrap();
    });
    let reply = request(address, "GET", "/api/cameras", Value::Null).unwrap();
    assert_eq!(reply["cameras"][0]["model"], "PXW-Z200");
    assert_eq!(camera_path("ABC123").unwrap(), "/api/cameras/ABC123");
    assert!(camera_path("ABC/123").is_err());
    assert_eq!(numeric("1/125"), Some(0.008));
    assert_eq!(numeric("5600 K"), Some(5600.0));
    worker.join().unwrap();
}
