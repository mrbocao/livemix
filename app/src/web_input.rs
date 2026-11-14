//! Chromium pages rendered in an isolated, shared browser process.
use crate::media_common::{Frame, Shared};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{atomic::Ordering::Relaxed, Arc, Mutex, Weak},
    time::{Duration, Instant},
};
use tungstenite::{Message, WebSocket};

const MAX_CAPTURE: usize = 16 * 1024 * 1024;
static HOST: Mutex<Weak<Browser>> = Mutex::new(Weak::new());

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
        let mut command = Command::new(exe);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let process = command.args(["--headless=new", "--remote-debugging-address=127.0.0.1",
            "--remote-debugging-port=0", "--no-first-run", "--disable-extensions",
            "--disable-background-networking", "--disable-sync", "--no-default-browser-check", "--hide-scrollbars",
            "--mute-audio", "--disable-background-timer-throttling", "--disable-renderer-backgrounding", "about:blank"])
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
        page.command(&shared, "Emulation.setDeviceMetricsOverride",
            json!({"width":1280,"height":720,"deviceScaleFactor":1,"mobile":false}))?;
        if transparent {
            page.command(&shared, "Emulation.setDefaultBackgroundColorOverride",
                json!({"color":{"r":0,"g":0,"b":0,"a":0}}))?;
        }
        let navigation = page.command(&shared, "Page.navigate", json!({"url":url}))?;
        if let Some(error) = navigation["errorText"].as_str() { return Err(error.into()); }
        let mut first = true;
        while !shared.stop.load(Relaxed) {
            let started = Instant::now();
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
            let delay = Duration::from_millis(1000 / rate);
            while !shared.stop.load(Relaxed) && started.elapsed() < delay {
                std::thread::sleep(Duration::from_millis(20));
            }
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
                    let html = "<!doctype html><style>html,body{margin:0;background:transparent}div{width:400px;height:400px;background:#ff0000}</style><div></div>";
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
    shared.stop.store(true, Relaxed);
    worker.join().unwrap();
    server.join().unwrap();
    assert!(good, "{}", shared.status.lock().unwrap());
}