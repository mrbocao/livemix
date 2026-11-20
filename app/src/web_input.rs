//! Chromium pages rendered in an isolated, shared browser process.
use crate::media_common::{Frame, Shared};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command as ProcessCommand, Stdio},
    sync::{atomic::Ordering::Relaxed, Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};
use crate::egui;

#[derive(Debug)]
pub struct Command(&'static str, Value);

fn modifiers(value: egui::Modifiers) -> u8 {
    u8::from(value.alt) | u8::from(value.ctrl) << 1 | u8::from(value.shift) << 3 | u8::from(value.mac_cmd) << 2
}

pub fn interact(ui: &mut egui::Ui, rect: egui::Rect, id: u32, shared: &Shared) {
    let response = ui.interact(rect, ui.id().with(("browser-interact", id)), egui::Sense::click_and_drag());
    let (events, pointer, mods, pressed) = ui.input(|i| (i.events.clone(), i.pointer.hover_pos(), i.modifiers, i.pointer.any_pressed()));
    if pressed {
        if pointer.is_some_and(|pos| rect.contains(pos)) { response.request_focus(); }
        else { response.surrender_focus(); }
    }
    let Ok(mut queue) = shared.browser_input.try_lock() else { return };
    let coords = |pos: egui::Pos2| ((pos.x - rect.left()) * 1280.0 / rect.width(), (pos.y - rect.top()) * 720.0 / rect.height());
    for event in events {
        if queue.len() >= 96 { break; }
        let command = match event {
            egui::Event::PointerMoved(pos) if rect.contains(pos) || response.dragged() => {
                let (x,y) = coords(pos);
                Command("Input.dispatchMouseEvent", json!({"type":"mouseMoved","x":x,"y":y,"modifiers":modifiers(mods)}))
            }
            egui::Event::PointerButton { pos, button, pressed, modifiers: mods }
                if rect.contains(pos) || (!pressed && response.has_focus()) =>
            {
                let button = match button { egui::PointerButton::Primary => "left", egui::PointerButton::Secondary => "right",
                    egui::PointerButton::Middle => "middle", _ => continue };
                let (x,y) = coords(pos);
                Command("Input.dispatchMouseEvent", json!({"type":if pressed {"mousePressed"} else {"mouseReleased"},
                    "x":x,"y":y,"button":button,"clickCount":1,"modifiers":modifiers(mods)}))
            }
            egui::Event::MouseWheel { unit, delta, modifiers: mods } if pointer.is_some_and(|pos| rect.contains(pos)) => {
                let (x,y) = coords(pointer.unwrap());
                let scale = match unit { egui::MouseWheelUnit::Point => 1.0, egui::MouseWheelUnit::Line => 32.0, egui::MouseWheelUnit::Page => 720.0 };
                Command("Input.dispatchMouseEvent", json!({"type":"mouseWheel","x":x,"y":y,
                    "deltaX":-delta.x*scale,"deltaY":-delta.y*scale,"modifiers":modifiers(mods)}))
            }
            egui::Event::Text(text) | egui::Event::Paste(text) if response.has_focus() =>
                Command("Input.insertText", json!({"text":text.chars().take(16384).collect::<String>()})),
            egui::Event::Key { key, pressed, repeat, modifiers: mods, .. } if response.has_focus() && key != egui::Key::Escape => {
                let name = key.name();
                let virtual_key = match key {
                    egui::Key::Backspace => 8, egui::Key::Tab => 9, egui::Key::Enter => 13, egui::Key::Space => 32,
                    egui::Key::PageUp => 33, egui::Key::PageDown => 34, egui::Key::End => 35, egui::Key::Home => 36,
                    egui::Key::ArrowLeft => 37, egui::Key::ArrowUp => 38, egui::Key::ArrowRight => 39, egui::Key::ArrowDown => 40,
                    egui::Key::Delete => 46, _ if name.len() == 1 => u32::from(name.as_bytes()[0]), _ => 0,
                };
                let key_name = if name.len() == 1 && !mods.shift { name.to_ascii_lowercase() } else { name.to_owned() };
                Command("Input.dispatchKeyEvent", json!({"type":if pressed {"rawKeyDown"} else {"keyUp"},
                    "key":key_name,"windowsVirtualKeyCode":virtual_key,"modifiers":modifiers(mods),"autoRepeat":repeat}))
            }
            _ => continue,
        };
        if command.1["type"] == "mouseMoved" && queue.back().is_some_and(|last| last.1["type"] == "mouseMoved") {
            queue.pop_back();
        }
        queue.push_back(command);
    }
}

const MAX_CAPTURE: usize = 16 * 1024 * 1024;
static HOST: Mutex<Weak<Browser>> = Mutex::new(Weak::new());

// The browser's hardware output remains muted. Tap the Web Audio graphs and eligible HTML
// media elements of the page and of its embedded frames before samples reach the output.
// Frames send their samples to the top page, which mixes every source together (several
// players at once are summed, not queued one after another).
const AUDIO_TAP: &str = r#"
(() => {
    const top = window === window.top;
    const sources = new Map();
    const frameId = Math.random().toString(36).slice(2);
    let next = 0;
    function store(id, samples) {
        let queue = sources.get(id);
        if (!queue) { queue = []; sources.set(id, queue); }
        if (queue.length >= 16) queue.shift();
        queue.push(samples);
    }
    if (top) addEventListener("message", event => {
        const data = event.data;
        if (!data || !Array.isArray(data.__liveMixAudio)) return;
        const [id, samples] = data.__liveMixAudio;
        if (typeof id === "string" && samples instanceof Int16Array && samples.length <= 65536) store(id, samples);
    });
    const taps = new WeakMap();
    const media = new WeakSet();
    const connect = AudioNode.prototype.connect;
    let mediaContext;
    function tap(context) {
        if (taps.has(context)) return taps.get(context);
        const processor = context.createScriptProcessor(1024, 2, 1);
        const id = frameId + ":" + (next++);
        let phase = 0;
        processor.onaudioprocess = event => {
            const input = event.inputBuffer;
            const left = input.getChannelData(0);
            const right = input.numberOfChannels > 1 ? input.getChannelData(1) : left;
            const step = input.sampleRate / 48000;
            const out = [];
            for (; phase < left.length; phase += step) {
                const i = Math.floor(phase);
                out.push(Math.round(Math.max(-1, Math.min(1, left[i])) * 32767),
                    Math.round(Math.max(-1, Math.min(1, right[i])) * 32767));
            }
            phase -= left.length;
            const samples = Int16Array.from(out);
            if (top) store(id, samples);
            else try { window.top.postMessage({__liveMixAudio: [id, samples]}, "*"); } catch (_) {}
        };
        connect.call(processor, context.destination);
        taps.set(context, processor);
        return processor;
    }
    AudioNode.prototype.connect = function(destination, ...args) {
        const result = connect.call(this, destination, ...args);
        if (destination === this.context.destination && this.context instanceof AudioContext) {
            try { connect.call(this, tap(this.context), args[0] || 0); } catch (_) {}
        }
        return result;
    };
    function attach(element) {
        if (!(element instanceof HTMLMediaElement) || media.has(element)) return;
        try {
            const stream = element.captureStream();
            if (!stream.getAudioTracks().length) return;
            mediaContext ||= new AudioContext({sampleRate: 48000});
            const source = mediaContext.createMediaStreamSource(stream);
            connect.call(source, tap(mediaContext));
            media.add(element);
        } catch (_) {}
    }
    addEventListener("playing", event => attach(event.target), true);
    if (!top) {
        // Frames resume their own media context; the top page collects what they send.
        setInterval(() => { if (mediaContext?.state === "suspended") mediaContext.resume().catch(() => {}); }, 1000);
        return;
    }
    Object.defineProperty(window, "__liveMixTakeAudio", {value: () => {
        if (mediaContext?.state === "suspended") mediaContext.resume().catch(() => {});
        // Sum every source sample by sample, clipped to 16 bits, then send 8 KiB pieces.
        let length = 0;
        const joined = [];
        for (const queue of sources.values()) {
            const total = queue.reduce((sum, part) => sum + part.length, 0);
            const all = new Int16Array(total);
            let at = 0;
            for (const part of queue) { all.set(part, at); at += part.length; }
            joined.push(all);
            length = Math.max(length, total);
        }
        sources.clear();
        if (!length) return [];
        const mixed = new Int16Array(length);
        for (let i = 0; i < length; i++) {
            let sum = 0;
            for (const all of joined) if (i < all.length) sum += all[i];
            mixed[i] = Math.max(-32768, Math.min(32767, sum));
        }
        const bytes = new Uint8Array(mixed.buffer);
        const chunks = [];
        for (let start = 0; start < bytes.length && chunks.length < 8; start += 8192) {
            let text = "";
            for (const byte of bytes.subarray(start, start + 8192)) text += String.fromCharCode(byte);
            chunks.push(btoa(text));
        }
        return chunks;
    }});
})();
"#;

pub fn valid_url(url: &str) -> bool {
    url.split_once("://").is_some_and(|(scheme, rest)| {
        matches!(scheme, "http" | "https")
            && !rest.split(['/', '?', '#']).next().unwrap_or("").is_empty()
            && !url.chars().any(|c| c.is_control() || c.is_whitespace())
            && url.len() <= 4096
    })
}

fn executable() -> Option<PathBuf> {
    let mut paths = Vec::new();
    #[cfg(windows)]
    for root in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(root) {
            let root = PathBuf::from(root);
            paths.push(root.join("Microsoft/Edge/Application/msedge.exe"));
            paths.push(root.join("Google/Chrome/Application/chrome.exe"));
        }
    }
    #[cfg(target_os = "macos")]
    for name in ["Google Chrome", "Microsoft Edge", "Chromium"] {
        paths.push(PathBuf::from(format!("/Applications/{name}.app/Contents/MacOS/{name}")));
    }
    paths.into_iter().find(|path| path.is_file())
}

struct Browser {
    process: Mutex<Child>,
    port: u16,
    devtools_path: String,
    profile: PathBuf,
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Ok(child) = self.process.get_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Browser {
    fn get() -> Result<Arc<Self>, String> {
        let mut host = HOST.lock().map_err(|error| error.to_string())?;
        if let Some(browser) = host.upgrade() {
            let running = browser.process.lock().is_ok_and(|mut child| matches!(child.try_wait(), Ok(None)));
            if running { return Ok(browser); }
        }
        let exe = executable().ok_or("Install Google Chrome or Microsoft Edge to use Web Browser inputs")?;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?.as_nanos();
        let profile = std::env::temp_dir().join(format!("livemix-browser-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&profile).map_err(|error| error.to_string())?;
        let mut command = ProcessCommand::new(exe);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let process = command.args(["--headless=new", "--remote-debugging-address=127.0.0.1",
            "--remote-debugging-port=0", "--no-first-run", "--disable-extensions",
            "--disable-background-networking", "--disable-sync", "--no-default-browser-check", "--hide-scrollbars",
            "--mute-audio", "--disable-background-timer-throttling", "--disable-renderer-backgrounding",
            // Keep embedded frames in the page's process so the audio tap also runs inside them.
            "--disable-site-isolation-trials", "--disable-features=IsolateOrigins,site-per-process", "about:blank"])
            .arg(format!("--user-data-dir={}", profile.display()))
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        let process = match process {
            Ok(process) => process,
            Err(error) => { let _ = std::fs::remove_dir(&profile); return Err(error.to_string()); }
        };
        let mut browser = Browser { process: Mutex::new(process), port: 0, devtools_path: String::new(), profile };
        let deadline = Instant::now() + Duration::from_secs(8);
        let marker = browser.profile.join("DevToolsActivePort");
        while Instant::now() < deadline {
            if let Ok(data) = std::fs::read_to_string(&marker) {
                let mut lines = data.lines();
                let port = lines.next().and_then(|line| line.parse::<u16>().ok()).filter(|p| *p != 0);
                let socket_path = lines.next().filter(|line| line.starts_with("/devtools/browser/")
                    && line.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-')));
                if let (Some(port), Some(path)) = (port, socket_path) {
                    browser.port = port;
                    browser.devtools_path = path.into();
                    let browser = Arc::new(browser);
                    *host = Arc::downgrade(&browser);
                    return Ok(browser);
                }
            }
            if browser.process.get_mut().ok().and_then(|child| child.try_wait().ok()).flatten().is_some() { break; }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err("The Chromium capture process could not start".into())
    }

    fn request(&self, method: &str, path: &str) -> Result<Value, String> {
        let mut socket = connect(self.port)?;
        write!(socket, "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n", self.port)
            .map_err(|error| error.to_string())?;
        let mut response = BufReader::new(socket.take(65_536));
        let mut line = String::new();
        response.read_line(&mut line).map_err(|error| format!("Browser HTTP {path}: {error}"))?;
        if !line.split_whitespace().nth(1).is_some_and(|code| code == "200") {
            return Err(format!("Chromium rejected {path}: {}", line.trim()));
        }
        let mut length = None;
        loop {
            line.clear();
            if response.read_line(&mut line).map_err(|error| error.to_string())? == 0 || line.trim().is_empty() { break; }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("Content-Length") { length = value.trim().parse::<usize>().ok(); }
            }
        }
        let length = length.filter(|size| *size <= 60_000).ok_or("Invalid browser response length")?;
        let mut body = vec![0; length];
        response.read_exact(&mut body).map_err(|error| error.to_string())?;
        serde_json::from_slice(&body).or(Ok(Value::Null))
    }
}

fn connect(port: u16) -> Result<TcpStream, String> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let timeout = Some(Duration::from_millis(750));
    let socket = TcpStream::connect_timeout(&address, timeout.unwrap()).map_err(|e| e.to_string())?;
    socket.set_read_timeout(timeout).and_then(|_| socket.set_write_timeout(timeout)).map_err(|e| e.to_string())?;
    Ok(socket)
}

struct Page {
    browser: Arc<Browser>,
    target: String,
    socket: WebSocket<TcpStream>,
    id: u64,
}

impl Drop for Page {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
        if !self.target.is_empty() {
            let _ = self.browser.request("GET", &format!("/json/close/{}", self.target));
        }
    }
}

impl Page {
    fn socket(port: u16, path: &str) -> Result<WebSocket<TcpStream>, String> {
        let socket = connect(port)?;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(MAX_CAPTURE)).max_frame_size(Some(MAX_CAPTURE));
        tungstenite::client::client_with_config(format!("ws://127.0.0.1:{port}{path}"), socket, Some(config))
            .map(|(socket, _)| socket).map_err(|error| error.to_string())
    }

    fn new(browser: Arc<Browser>, shared: &Shared) -> Result<Self, String> {
        let mut control = Self { browser: browser.clone(), target: String::new(),
            socket: Self::socket(browser.port, &browser.devtools_path)?, id: 0 };
        let description = control.command(shared, "Target.createTarget",
            json!({"url":"about:blank","newWindow":true,"width":1280,"height":720}))?;
        let target = description["targetId"].as_str().filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric()))
            .ok_or("Invalid browser page ID")?.to_owned();
        let socket = Self::socket(browser.port, &format!("/devtools/page/{target}"));
        match socket {
            Ok(socket) => Ok(Self { browser, target, socket, id: 0 }),
            Err(error) => {
                let _ = browser.request("GET", &format!("/json/close/{target}"));
                Err(format!("Could not attach to Chromium page: {error}"))
            }
        }
    }

    fn command(&mut self, shared: &Shared, method: &str, params: Value) -> Result<Value, String> {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            let response = self.command_once(shared, method, &params);
            if !response.as_ref().is_err_and(|error| error.contains("Not attached to an active page"))
                || shared.stop.load(Relaxed) || Instant::now() >= deadline
            {
                return response.map_err(|error| format!("{method}: {error}"));
            }
            std::thread::sleep(Duration::from_millis(60));
        }
    }

    fn command_once(&mut self, shared: &Shared, method: &str, params: &Value) -> Result<Value, String> {
        self.id += 1;
        let request = self.id;
        self.socket.send(Message::Text(json!({"id":request, "method":method, "params":params}).to_string().into()))
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline && !shared.stop.load(Relaxed) {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    let Ok(message) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    if message["method"] == "Page.javascriptDialogOpening" {
                        self.id += 1;
                        self.socket.send(Message::Text(json!({"id":self.id, "method":"Page.handleJavaScriptDialog",
                            "params":{"accept":false}}).to_string().into())).map_err(|e| e.to_string())?;
                    }
                    if message["id"].as_u64() != Some(request) { continue; }
                    if let Some(error) = message["error"]["message"].as_str() { return Err(error.into()); }
                    return Ok(message["result"].clone());
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(error)) if matches!(error.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Err("Browser capture stopped or timed out".into())
    }
}

pub fn run(shared: Arc<Shared>, url: String, transparent: bool) {
    if !valid_url(&url) { shared.set_status("Enter an HTTP or HTTPS page URL"); return; }
    let result = (|| -> Result<(), String> {
        shared.set_status("Starting browser…");
        let mut page = Page::new(Browser::get()?, &shared)?;
        page.command(&shared, "Page.enable", json!({}))?;
        page.command(&shared, "Page.addScriptToEvaluateOnNewDocument", json!({"source":AUDIO_TAP}))?;
        page.command(&shared, "Emulation.setDeviceMetricsOverride",
            json!({"width":1280,"height":720,"deviceScaleFactor":1,"mobile":false}))?;
        if transparent {
            page.command(&shared, "Emulation.setDefaultBackgroundColorOverride",
                json!({"color":{"r":0,"g":0,"b":0,"a":0}}))?;
        }
        let navigation = page.command(&shared, "Page.navigate", json!({"url":url}))?;
        if let Some(error) = navigation["errorText"].as_str() { return Err(error.into()); }
        let mut first = true;
        let mut next_picture = Instant::now();
        while !shared.stop.load(Relaxed) {
            let started = Instant::now();
            let commands: Vec<_> = shared.browser_input.lock().unwrap().drain(..).take(96).collect();
            for Command(method, params) in commands {
                let _ = page.command(&shared, method, params);
            }
            if let Ok(audio) = page.command(&shared, "Runtime.evaluate",
                json!({"expression":"window.__liveMixTakeAudio?.()","returnByValue":true}))
            {
                if let Some(chunks) = audio["result"]["value"].as_array() {
                    let samples: Vec<_> = chunks.iter().take(8).filter_map(Value::as_str)
                        .filter(|chunk| chunk.len() <= 16384)
                        .filter_map(|chunk| STANDARD.decode(chunk).ok())
                        .flat_map(|bytes| bytes.chunks_exact(2).map(|sample| i16::from_le_bytes([sample[0], sample[1]])).collect::<Vec<_>>())
                        .collect();
                    if !samples.is_empty() {
                        shared.audio_channels.store(2, Relaxed);
                        if let Ok(mut buffer) = shared.audio.lock() { buffer.capture_interleaved(&samples, 2, false); }
                    }
                }
            }
            if started < next_picture {
                std::thread::sleep((next_picture - started).min(Duration::from_millis(12)));
                continue;
            }
            let result = page.command(&shared, "Page.captureScreenshot", json!({"format":"png","fromSurface":true}))?;
            let data = result["data"].as_str().ok_or("Chromium did not return a picture")?;
            let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
            let rgba = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
                .map_err(|error| error.to_string())?.into_rgba8();
            let (w, h) = rgba.dimensions();
            if first {
                shared.set_status("");
                first = false;
            }
            crate::ffmpeg_media::publish(&shared, Frame { w: w as usize, h: h as usize, rgba: rgba.into_raw() });
            let rate = shared.browser_fps.load(Relaxed).clamp(2, 20);
            next_picture = started + Duration::from_millis(1000 / rate);
        }
        Ok(())
    })();
    if let Err(error) = result {
        if !shared.stop.load(Relaxed) { shared.set_status(format!("Web Browser: {error}")); }
    }
}


#[test]
fn web_input_restricts_top_level_navigation() {
    assert!(valid_url("https://example.test/graphics?channel=1"));
    assert!(!valid_url("file:///C:/Users/test"));
    assert!(!valid_url("javascript:alert(1)"));
    assert!(!valid_url("http:// bad"));
}

#[test]
#[ignore = "requires an installed Chrome or Edge browser"]
fn browser_renders_a_local_page_with_alpha() {
    use std::net::TcpListener;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let shared = Arc::new(Shared::default());
    let server_shared = shared.clone();
    let server = std::thread::spawn(move || {
        while !server_shared.stop.load(Relaxed) {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    socket.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                    let mut request = [0; 2048];
                    let _ = socket.read(&mut request);
                    let html = r##"<!doctype html><style>html,body{margin:0;background:transparent}div{width:400px;height:400px;background:#ff0000}</style><div></div><script>
                    document.querySelector("div").onclick = async e => {
                        e.target.style.background = "#00ff00";
                        const ctx = new AudioContext();
                        const tone = ctx.createOscillator();
                        tone.connect(ctx.destination);
                        tone.start();
                        await ctx.resume();
                    };
                    addEventListener("keydown", e => { if (e.key === "a") document.querySelector("div").style.background = "#0000ff"; });
                    </script>"##;
                    let _ = write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
    });
    let worker_shared = shared.clone();
    let worker = std::thread::spawn(move || run(worker_shared, format!("http://{address}/"), true));
    let deadline = Instant::now() + Duration::from_secs(18);
    let mut good = false;
    while !worker.is_finished() && Instant::now() < deadline {
        if let Some(frame) = &*shared.frame.lock().unwrap() {
            let red = (20 * frame.w + 20) * 4;
            let transparent = (500 * frame.w + 800) * 4 + 3;
            good = (frame.w, frame.h) == (1280, 720)
                && frame.rgba.get(red..red + 4) == Some(&[255, 0, 0, 255])
                && frame.rgba.get(transparent) == Some(&0);
        }
        if good { break; }
        std::thread::sleep(Duration::from_millis(30));
    }
    let command = |kind, button| Command("Input.dispatchMouseEvent",
        json!({"type":kind,"x":20,"y":20,"button":button,"clickCount":1}));
    shared.browser_input.lock().unwrap().extend([command("mousePressed", "left"), command("mouseReleased", "left"),
        Command("Input.dispatchKeyEvent", json!({"type":"rawKeyDown","key":"a","windowsVirtualKeyCode":65}))]);
    let deadline = Instant::now() + Duration::from_secs(8);
    let (mut blue, mut audible) = (false, false);
    while Instant::now() < deadline && !(blue && audible) {
        if let Some(frame) = &*shared.frame.lock().unwrap() {
            blue |= frame.rgba.get((20 * frame.w + 20) * 4..(20 * frame.w + 20) * 4 + 4) == Some(&[0,0,255,255]);
        }
        audible |= crate::audio::input_peak((&shared, false, 1.0), Instant::now()) > 0.1;
        std::thread::sleep(Duration::from_millis(30));
    }
    shared.stop.store(true, Relaxed);
    worker.join().unwrap();
    assert!(blue && audible, "browser events or audio missing: blue={blue}, audio={audible}");
    server.join().unwrap();
    assert!(good, "{}", shared.status.lock().unwrap());
}
