use crate::model::{Effect, Switcher};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, atomic::{AtomicBool, Ordering}};

#[derive(Clone, Copy)]
enum Step {
    Preview(usize),
    Cut,
    Fade(u32),
    Wait(u32),
}

fn parse(script: &str) -> Result<VecDeque<Step>, String> {
    if script.len() > 8192 { return Err("Script exceeds 8 KiB".into()); }
    let mut steps = VecDeque::new();
    for (index, line) in script.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() { continue; }
        let fields: Vec<_> = line.split_whitespace().collect();
        let step = match fields.as_slice() {
            [op] if op.eq_ignore_ascii_case("cut") => Some(Step::Cut),
            [op, n] if op.eq_ignore_ascii_case("preview") => n.parse::<usize>().ok().filter(|n| *n > 0).map(Step::Preview),
            [op, ms] if op.eq_ignore_ascii_case("fade") => ms.parse::<u32>().ok().filter(|ms| (1..=60_000).contains(ms)).map(Step::Fade),
            [op, ms] if op.eq_ignore_ascii_case("wait") => ms.parse::<u32>().ok().filter(|ms| *ms <= 60_000).map(Step::Wait),
            _ => None,
        };
        steps.push_back(step.ok_or_else(|| format!("Invalid command on line {}", index + 1))?);
    }
    Ok(steps)
}

#[derive(Default)]
pub struct State {
    steps: VecDeque<Step>,
    until: Option<Instant>,
    waiting_for_transition: bool,
    previous_output: Option<Option<u32>>,
}

impl State {
    pub fn start(&mut self, script: &str) -> Result<(), String> {
        self.steps = parse(script)?;
        self.until = None;
        self.waiting_for_transition = false;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.steps.clear();
        self.until = None;
        self.waiting_for_transition = false;
    }

    pub fn active(&self) -> bool {
        self.until.is_some() || self.waiting_for_transition || !self.steps.is_empty()
    }

    pub fn tick(switcher: &mut Switcher, now: Instant) {
        let mut state = std::mem::take(&mut switcher.automation);
        let previous = state.previous_output.replace(switcher.output);
        if !state.active() && previous.is_some_and(|id| id != switcher.output) {
            if let Some(script) = switcher.output.and_then(|id| switcher.input(id))
                .map(|input| input.on_program.clone()).filter(|script| !script.trim().is_empty())
            {
                if let Err(error) = state.start(&script) { switcher.notice(&error); }
            }
        }
        if state.waiting_for_transition && switcher.trans.is_none() {
            state.waiting_for_transition = false;
        }
        if !state.waiting_for_transition && state.until.is_none_or(|until| now >= until) {
            state.until = None;
            if let Some(step) = state.steps.pop_front() {
                match step {
                    Step::Preview(index) => {
                        if let Some(input) = switcher.inputs.get(index - 1) {
                            switcher.preview = Some(input.id);
                        } else {
                            state.stop();
                            switcher.notice(&format!("Script input {index} is unavailable"));
                        }
                    }
                    Step::Cut => {
                        switcher.start_transition(Effect::Cut, 0);
                        state.waiting_for_transition = switcher.trans.is_some();
                    }
                    Step::Fade(ms) => {
                        switcher.start_transition(Effect::Fade, ms);
                        state.waiting_for_transition = switcher.trans.is_some();
                    }
                    Step::Wait(ms) => state.until = Some(now + Duration::from_millis(u64::from(ms))),
                }
            }
        }
        switcher.automation = state;
    }
}

pub const WEB_ADDRESS: &str = "127.0.0.1:8088";
const WEB_PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>LiveMix control</title>
<h1>LiveMix control</h1><label>Preview input <input id="input" type="number" min="1" value="1"></label>
<button onclick="send('Preview '+document.querySelector('#input').value)">Preview</button>
<button onclick="send('Cut')">Cut</button><button onclick="send('Fade 500')">Fade</button>
<p><a href="/telestrator/">Telestrator</a></p>
<p id="status" role="status"></p><script>
async function send(command) {
  const status = document.querySelector('#status');
  try {
    const response = await fetch('/command', {method:'POST', headers:{'X-LiveMix-Control':'1'}, body:command});
    status.textContent = await response.text();
  } catch { status.textContent = 'Connection failed'; }
}
</script>"#;

/// Telestrator page of the original (\telestrator\), drawing over the program picture.
const TELESTRATOR_PAGE: &str = r#"<!doctype html><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<title>LiveMix Telestrator</title>
<style>
  html,body{margin:0;height:100%;background:#111;color:#eee;font:13px system-ui,sans-serif;overflow:hidden}
  #bar{display:flex;gap:6px;align-items:center;padding:6px;flex-wrap:wrap}
  #bar button{background:#293038;color:#eee;border:1px solid #404b56;border-radius:3px;padding:5px 9px}
  .sw{width:26px;height:26px;border-radius:3px;border:2px solid #111;padding:0}
  .sw.on{border-color:#fff}
  #canvas{position:absolute;inset:44px 0 0 0;width:100%;height:calc(100% - 44px);touch-action:none;background:#000}
</style>
<div id="bar">
  <span>Colour</span>
  <button class="sw on" data-c="255,60,60" style="background:rgb(255,60,60)"></button>
  <button class="sw" data-c="60,220,60" style="background:rgb(60,220,60)"></button>
  <button class="sw" data-c="60,120,255" style="background:rgb(60,120,255)"></button>
  <button class="sw" data-c="255,220,60" style="background:rgb(255,220,60)"></button>
  <button class="sw" data-c="255,255,255" style="background:rgb(255,255,255)"></button>
  <span>Width</span><input id="width" type="range" min="1" max="24" value="7">
  <button id="undo">Undo</button><button id="clear">Clear</button>
  <span id="status" role="status"></span>
</div>
<canvas id="canvas"></canvas>
<script>
const canvas = document.querySelector('#canvas'), ctx = canvas.getContext('2d');
const status = document.querySelector('#status');
let colour = [255,60,60], drawing = false, points = [], local = [];
function resize() {
  canvas.width = canvas.clientWidth; canvas.height = canvas.clientHeight;
  redraw();
}
function redraw() {
  ctx.clearRect(0, 0, canvas.width, canvas.height);
  for (const s of local) line(s);
}
function line(s) {
  if (s.points.length < 1) return;
  ctx.strokeStyle = `rgb(${s.colour[0]},${s.colour[1]},${s.colour[2]})`;
  ctx.lineWidth = Math.max(1, s.width * canvas.clientHeight);
  ctx.lineCap = 'round'; ctx.lineJoin = 'round';
  ctx.beginPath();
  const at = p => [p[0] * canvas.width, p[1] * canvas.height];
  let [x, y] = at(s.points[0]); ctx.moveTo(x, y);
  for (const p of s.points.slice(1)) { [x, y] = at(p); ctx.lineTo(x, y); }
  if (s.points.length === 1) ctx.lineTo(x + 0.1, y + 0.1);
  ctx.stroke();
}
async function post(payload) {
  try {
    const response = await fetch('/telestrator/stroke', {method:'POST',
      headers:{'X-LiveMix-Control':'1','Content-Type':'application/json'}, body:JSON.stringify(payload)});
    status.textContent = response.ok ? '' : 'Not accepted';
  } catch { status.textContent = 'Connection failed'; }
}
function point(event) {
  const r = canvas.getBoundingClientRect();
  return [Math.min(1, Math.max(0, (event.clientX - r.left) / r.width)),
          Math.min(1, Math.max(0, (event.clientY - r.top) / r.height))];
}
function stroke() {
  return {points, colour, width: Number(document.querySelector('#width').value) / 1080 * 24};
}
canvas.addEventListener('pointerdown', event => {
  canvas.setPointerCapture(event.pointerId);
  drawing = true; points = [point(event)];
  const preview = stroke(); local.push(preview);
  redraw();
});
canvas.addEventListener('pointermove', event => {
  if (!drawing) return;
  points.push(point(event));
  local[local.length - 1] = stroke();
  redraw();
});
canvas.addEventListener('pointerup', async () => {
  if (!drawing) return;
  drawing = false;
  if (points.length === 0) return;
  await post(stroke());
});
document.querySelectorAll('.sw').forEach(button => button.addEventListener('click', () => {
  document.querySelectorAll('.sw').forEach(other => other.classList.remove('on'));
  button.classList.add('on');
  colour = button.dataset.c.split(',').map(Number);
}));
document.querySelector('#undo').addEventListener('click', async () => {
  local.pop(); redraw(); await post({undo:true});
});
document.querySelector('#clear').addEventListener('click', async () => {
  local.length = 0; redraw(); await post({clear:true});
});
addEventListener('resize', resize);
resize();
</script>"#;

/// One stroke received from the Telestrator web page.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct StrokeMessage {
    #[serde(default)]
    pub points: Vec<[f32; 2]>,
    #[serde(default)]
    pub colour: Option<[u8; 3]>,
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub undo: bool,
}

pub struct Remote {
    requests: mpsc::Receiver<String>,
    strokes: mpsc::Receiver<StrokeMessage>,
    stop: Arc<AtomicBool>,
}

impl Drop for Remote {
    fn drop(&mut self) { self.stop.store(true, Ordering::Release); }
}

impl Remote {
    pub fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(WEB_ADDRESS)?;
        listener.set_nonblocking(true)?;
        let (tx, requests) = mpsc::sync_channel(8);
        let (stroke_tx, strokes) = mpsc::sync_channel(64);
        let stop = Arc::new(AtomicBool::new(false));
        let running = stop.clone();
        std::thread::spawn(move || {
            while !running.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((socket, _)) => { let _ = request(socket, &tx, &stroke_tx); }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock =>
                        std::thread::sleep(Duration::from_millis(25)),
                    Err(_) => break,
                }
            }
        });
        Ok(Self { requests, strokes, stop })
    }

    pub fn receive(&self) -> Option<String> {
        self.requests.try_recv().ok()
    }

    /// Latest Telestrator stroke or action from the web page.
    pub fn receive_stroke(&self) -> Option<StrokeMessage> {
        self.strokes.try_recv().ok()
    }
}

fn request(mut socket: TcpStream, tx: &mpsc::SyncSender<String>,
    strokes: &mpsc::SyncSender<StrokeMessage>) -> std::io::Result<()> {
    let timeout = Some(Duration::from_millis(250));
    socket.set_read_timeout(timeout)?;
    socket.set_write_timeout(timeout)?;
    let mut input = BufReader::new((&mut socket).take(16_384));
    let mut line = String::new();
    input.read_line(&mut line)?;
    let request_line = line.trim().to_owned();
    let mut host = false;
    let mut authorized = false;
    let mut size = None;
    loop {
        line.clear();
        if input.read_line(&mut line)? == 0 { return Ok(()); }
        if line.trim().is_empty() { break; }
        if let Some((header, value)) = line.split_once(':') {
            if header.eq_ignore_ascii_case("Host") { host = value.trim() == WEB_ADDRESS; }
            if header.eq_ignore_ascii_case("X-LiveMix-Control") { authorized = value.trim() == "1"; }
            if header.eq_ignore_ascii_case("Content-Length") { size = value.trim().parse::<usize>().ok(); }
        }
    }
    let (status, mime, body) = if host && request_line.starts_with("GET / HTTP/1.") {
        ("200 OK", "text/html", WEB_PAGE)
    } else if host && (request_line.starts_with("GET /telestrator/ HTTP/1.")
        || request_line.starts_with("GET /telestrator HTTP/1."))
    {
        ("200 OK", "text/html", TELESTRATOR_PAGE)
    } else if host && authorized && request_line.starts_with("POST /command HTTP/1.") {
        let Some(size) = size.filter(|size| (1..=256).contains(size)) else {
            return response(&mut socket, "400 Bad Request", "text/plain", "Invalid command length");
        };
        let mut bytes = vec![0; size];
        input.read_exact(&mut bytes)?;
        match String::from_utf8(bytes) {
            Ok(command) if parse(&command).is_ok_and(|steps| steps.len() == 1) => {
                if tx.try_send(command).is_ok() { ("202 Accepted", "text/plain", "Command queued") }
                else { ("503 Service Unavailable", "text/plain", "Command queue is full") }
            }
            _ => ("400 Bad Request", "text/plain", "Invalid command"),
        }
    } else if host && authorized && request_line.starts_with("POST /telestrator/stroke HTTP/1.") {
        let Some(size) = size.filter(|size| (2..=65_536).contains(size)) else {
            return response(&mut socket, "400 Bad Request", "text/plain", "Invalid stroke length");
        };
        let mut bytes = vec![0; size];
        input.read_exact(&mut bytes)?;
        match serde_json::from_slice::<StrokeMessage>(&bytes) {
            Ok(stroke) if stroke.points.len() <= 4096 => {
                if strokes.try_send(stroke).is_ok() { ("202 Accepted", "text/plain", "Stroke queued") }
                else { ("503 Service Unavailable", "text/plain", "Stroke queue is full") }
            }
            _ => ("400 Bad Request", "text/plain", "Invalid stroke"),
        }
    } else {
        ("403 Forbidden", "text/plain", "Forbidden")
    };
    response(&mut socket, status, mime, body)
}

fn response(socket: &mut TcpStream, status: &str, mime: &str, body: &str) -> std::io::Result<()> {
    write!(socket, "HTTP/1.1 {status}\r\nContent-Type: {mime}; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

#[test]
fn program_trigger_waits_without_blocking_the_switcher() {
    use crate::model::{Input, Kind};
    let mut switcher = Switcher::empty();
    let before = switcher.add(Input::new(0, "Before", Kind::Blank));
    let after = switcher.add(Input::new(0, "After", Kind::Blank));
    switcher.input_mut(after).unwrap().on_program = "Wait 100\nPreview 1".into();
    let start = Instant::now();
    switcher.output = Some(before);
    State::tick(&mut switcher, start);
    switcher.output = Some(after);
    State::tick(&mut switcher, start);
    State::tick(&mut switcher, start + Duration::from_millis(99));
    assert_ne!(switcher.preview, Some(before));
    State::tick(&mut switcher, start + Duration::from_millis(100));
    assert_eq!(switcher.preview, Some(before));
}

#[test]
fn web_controller_accepts_a_local_command() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (send, receive) = mpsc::sync_channel(1);
    let (stroke_send, _stroke_receive) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || request(listener.accept().unwrap().0, &send, &stroke_send).unwrap());
    let mut client = TcpStream::connect(address).unwrap();
    client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    write!(client, "POST /command HTTP/1.1\r\nHost: {WEB_ADDRESS}\r\nX-LiveMix-Control: 1\r\nContent-Length: 3\r\n\r\nCut").unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 202"));
    assert_eq!(receive.recv_timeout(Duration::from_secs(1)).unwrap(), "Cut");
    worker.join().unwrap();
}
