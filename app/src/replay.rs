//! Replay session metadata and event materialization.
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

const MANIFEST: &str = "livemix-replay.json";
pub const SEGMENT_MS: u64 = 4_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Setup {
    pub folder: String,
    pub size: Option<[u32; 2]>,
    pub fps: Option<f32>,
    pub mbps: u32,
    pub codec: String,
    pub audio: u8,
    pub buffer_seconds: u64,
    pub delete_old: bool,
    pub transition_ms: u32,
    pub match_speed: bool,
    pub show_live_sources: bool,
    pub advanced: bool,
    pub play_to_program: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self { folder: String::new(), size: None, fps: None, mbps: 25, codec: "h264".into(),
            audio: 0, buffer_seconds: 120, delete_old: true, transition_ms: 0, match_speed: false,
            show_live_sources: false, advanced: false, play_to_program: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Segment {
    pub start: u64,
    pub end: u64,
    pub files: [Option<String>; 8],
    #[serde(default)]
    pub frames: [Vec<FrameStamp>; 8],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameStamp {
    pub show_ms: u64,
    pub media_ms: u64,
}

impl Segment {
    pub fn seek_time(&self, angle: usize, time: u64) -> u64 {
        let show = time.saturating_sub(self.start);
        let Some(frames) = self.frames.get(angle).filter(|frames| !frames.is_empty()) else { return show; };
        frames[frames.partition_point(|frame| frame.show_ms <= show).saturating_sub(1)].media_ms
    }

    pub fn show_time(&self, angle: usize, time: u64) -> u64 {
        let position = self.frames.get(angle).filter(|frames| !frames.is_empty()).map_or(time, |frames| {
            let frame = &frames[frames.partition_point(|frame| frame.media_ms <= time).saturating_sub(1)];
            frame.show_ms.saturating_add(time.saturating_sub(frame.media_ms))
        });
        self.start.saturating_add(position).min(self.end)
    }

    fn cut_end(&self, angle: usize, time: u64) -> u64 {
        let show = time.saturating_sub(self.start);
        let Some(frames) = self.frames.get(angle).filter(|frames| !frames.is_empty()) else { return show; };
        let first = frames.partition_point(|frame| frame.show_ms < show);
        frames.get(first).map_or_else(
            || frames.last().unwrap().media_ms.saturating_add(show.saturating_sub(frames.last().unwrap().show_ms)),
            |frame| frame.media_ms,
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub id: u64,
    pub start: u64,
    pub end: u64,
    pub description: String,
    pub speed: u8,
    pub angles: [bool; 8],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Format {
    pub size: [u32; 2],
    pub fps: f32,
    pub codec: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Session {
    pub version: u8,
    pub next_id: u64,
    pub segments: Vec<Segment>,
    pub events: Vec<Event>,
    #[serde(default)]
    pub setup: Setup,
    #[serde(default)]
    pub camera_names: [String; 8],
    #[serde(default)]
    pub format: Option<Format>,
    #[serde(skip)]
    pub native: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self { version: 1, next_id: 1, segments: Vec::new(), events: Vec::new(),
            setup: Setup::default(), camera_names: Default::default(), format: None, native: false }
    }
}

impl Session {
    pub fn open(folder: &Path) -> Result<Self, String> {
        let path = folder.join(MANIFEST);
        if !path.is_file() { return native_session(folder); }
        let mut session: Self = serde_json::from_slice(&std::fs::read(path).map_err(|err| err.to_string())?)
            .map_err(|err| err.to_string())?;
        if session.version != 1 {
            return Err("Unsupported LiveMix replay session version".into());
        }
        session.setup.mbps = session.setup.mbps.clamp(1, 100);
        session.setup.buffer_seconds = session.setup.buffer_seconds.clamp(10, 3600);
        session.setup.transition_ms = session.setup.transition_ms.min(5000);
        session.setup.audio = session.setup.audio.min(9);
        session.setup.fps = session.setup.fps.filter(|fps| fps.is_finite() && (1.0..=120.0).contains(fps));
        session.setup.size = session.setup.size.filter(|[w, h]| (16..=7680).contains(w) && (16..=4320).contains(h));
        session.segments.retain(|segment| segment.start < segment.end);
        session.segments.sort_by_key(|segment| segment.start);
        for segment in &mut session.segments {
            for frames in &mut segment.frames {
                if !frames.windows(2).all(|pair| pair[0].show_ms <= pair[1].show_ms && pair[0].media_ms <= pair[1].media_ms) {
                    frames.clear();
                }
            }
        }
        session.events.retain(|event| event.start < event.end);
        for event in &mut session.events { event.speed = event.speed.clamp(1, 100); }
        session.next_id = session.next_id.max(session.events.iter().map(|event| event.id).max().unwrap_or(0).saturating_add(1));
        Ok(session)
    }

    pub fn save(&self, folder: &Path) -> Result<(), String> {
        if self.native { return Err("vMix replay sessions are read-only".into()); }
        let data = serde_json::to_vec_pretty(self).map_err(|err| err.to_string())?;
        let temp = folder.join("livemix-replay.pending");
        std::fs::write(&temp, data).and_then(|_| std::fs::rename(temp, folder.join(MANIFEST)))
            .map_err(|err| err.to_string())
    }

    pub fn new_event_with_angles(&mut self, start: u64, end: u64, provisional: [bool; 8]) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let angles = std::array::from_fn(|angle| provisional[angle] || self.segments.iter()
            .any(|segment| segment.end > start && segment.start < end && segment.files[angle].is_some()));
        self.events.insert(0, Event { id, start, end, description: format!("Event {id}"), speed: 100, angles });
        id
    }

    pub fn prune(&mut self, folder: &Path, before: u64, mark: Option<u64>) -> bool {
        let mut changed = false;
        self.segments.retain(|segment| {
            if segment.end >= before || mark.is_some_and(|start| segment.end > start)
                || self.events.iter().any(|event| event.start < segment.end && event.end > segment.start)
            {
                return true;
            }
            for file in segment.files.iter().flatten().filter_map(|file| media_path(folder, file)) {
                let _ = std::fs::remove_file(file);
            }
            changed = true;
            false
        });
        changed
    }
}

pub fn native_session(folder: &Path) -> Result<Session, String> {
    let xml = ["replay2.xml", "replay.xml"].into_iter().map(|name| folder.join(name))
        .find(|path| path.is_file()).ok_or("No LiveMix or vMix replay session found in this folder")?;
    let text = std::fs::read_to_string(xml).map_err(|error| error.to_string())?;
    let doc = roxmltree::Document::parse(&text).map_err(|error| error.to_string())?;
    let root = doc.root_element();
    if !root.has_tag_name("replay") { return Err("Invalid vMix replay manifest".into()); }
    let mut session = Session { native: true, ..Default::default() };
    session.setup.delete_old = false;
    if let Some(video) = root.children().find(|node| node.has_tag_name("video")) {
        let get = |key| video.attribute(key)?.parse::<u32>().ok();
        if let (Some(w), Some(h), Some(interval)) = (get("width"), get("height"), get("interval")) {
            if (83_333..=10_000_000).contains(&interval) && w > 0 && h > 0 {
                session.format = Some(Format { size: [w, h], fps: 10_000_000.0 / interval as f32, codec: "native".into() });
            }
        }
    }
    let ticks = |node: roxmltree::Node<'_, '_>, key| node.attribute(key).and_then(|value| value.parse::<u64>().ok()).map(|value| value / 10_000);
    let mut camera_media = Vec::new();
    for (angle, stream) in root.children().filter(|node| node.has_tag_name("stream")).take(8).enumerate() {
        session.camera_names[angle] = stream.attribute("cameraName").unwrap_or("").to_owned();
        for node in stream.children().filter(|node| node.has_tag_name("segment")) {
            let (Some(start), Some(file)) = (ticks(node, "videoStartTime"), node.attribute("filename")) else { continue };
            let name = Path::new(file);
            if name.components().count() != 1 || name.file_name().is_none() || file.contains(['\r', '\n', '\\', ':']) {
                continue;
            }
            let media = [PathBuf::from("data"), PathBuf::new()].into_iter().find_map(|parent| {
                let path = parent.join(name);
                if readable_media(&path) && folder.join(&path).is_file() { return Some(path); }
                if path.extension().is_some() { return None; }
                ["avi", "mpg", "mp4", "ts", "vdata"].into_iter().map(|ext| path.with_extension(ext))
                    .find(|candidate| folder.join(candidate).is_file())
            });
            camera_media.push((angle, start, media));
        }
    }
    for node in root.descendants().filter(|node| node.has_tag_name("event")) {
        let (Some(start), Some(end)) = (ticks(node, "inPoint"), ticks(node, "outPoint")) else { continue };
        if end <= start { continue; }
        let id = node.attribute("id").and_then(|value| value.parse::<u64>().ok()).unwrap_or(session.next_id);
        session.next_id = session.next_id.max(id.saturating_add(1));
        let angle = node.attribute("selectedAngle").and_then(|value| value.parse::<usize>().ok()).unwrap_or(0);
        let mut angles = [false; 8];
        for index in node.children().filter(|child| child.has_tag_name("selectedAngles"))
            .filter_map(|child| child.text()?.parse::<usize>().ok()).chain(std::iter::once(angle))
        {
            if let Some(selected) = angles.get_mut(index) { *selected = true; }
        }
        let speed = node.attribute("speed").and_then(|value| value.parse::<u8>().ok())
            .filter(|value| (1..=100).contains(value)).unwrap_or(100);
        let description = node.children().find(|child| child.has_tag_name("description"))
            .and_then(|child| child.text()).map(str::to_owned).unwrap_or_else(|| format!("Event {id}"));
        session.events.push(Event { id, start, end, description, speed, angles });
    }
    let origin = camera_media.iter().map(|item| item.1).chain(session.events.iter().map(|event| event.start)).min().unwrap_or(0);
    let last_event = session.events.iter().map(|event| event.end).max().unwrap_or(0);
    let mut starts = camera_media.iter().map(|entry| entry.1).collect::<Vec<_>>();
    starts.sort_unstable();
    starts.dedup();
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(last_event.max(start));
        if end <= start { continue; }
        let files = std::array::from_fn(|angle| camera_media.iter().find_map(|(camera, time, path)| {
            (*camera == angle && *time == start).then(|| path.as_ref().map(|path| path.to_string_lossy().into_owned())).flatten()
        }));
        if files.iter().any(Option::is_some) {
            session.segments.push(Segment { start: start - origin, end: end - origin, files, frames: Default::default() });
        }
    }
    for event in &mut session.events {
        event.start -= origin;
        event.end -= origin;
    }
    Ok(session)
}

fn readable_media(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
        ["mp4", "avi", "mkv", "mov", "ts", "mpg", "mpeg", "vdata"].iter().any(|format| ext.eq_ignore_ascii_case(format))
    })
}

pub fn indexed_path(folder: &Path, name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if !readable_media(path) || name.contains(['\r', '\n']) || path.components().any(|component| !matches!(component, std::path::Component::Normal(_))) {
        return None;
    }
    Some(folder.join(path))
}


pub fn media_path(folder: &Path, name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() != 1 || path.file_name().is_none() || path.extension()? != "mp4"
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
    {
        return None;
    }
    Some(folder.join(path))
}

struct NativeRemux {
    files: Vec<PathBuf>,
}

impl Drop for NativeRemux {
    fn drop(&mut self) {
        for file in &self.files { let _ = std::fs::remove_file(file); }
    }
}

impl NativeRemux {
    fn prepare(&mut self, video: PathBuf, output: &Path) -> Result<PathBuf, String> {
        if video.extension().and_then(|ext| ext.to_str()) != Some("vdata") {
            return Ok(video);
        }
        let filename = output.file_stem().unwrap_or_default().to_string_lossy();
        let target = output.with_file_name(format!("{filename}-native-{}.mp4", self.files.len()));
        self.files.push(target.clone());
        let audio = video.with_extension("adata");
        let mut ffmpeg = crate::stream::command();
        ffmpeg.args(["-y", "-i"]).arg(&video);
        if audio.is_file() {
            ffmpeg.arg("-i").arg(audio);
            ffmpeg.args(["-map", "0:v:0", "-map", "1:a:0"]);
        } else {
            ffmpeg.args(["-map", "0:v:0", "-map", "0:a:0?"]);
        }
        let result = ffmpeg.args(["-c:v", "copy", "-c:a", "aac"]).arg(&target).output()
            .map_err(|error| format!("vMix MPEG replay needs FFmpeg: {error}"))?;
        if !result.status.success() {
            return Err(format!("Could not decode vMix MPEG replay: {}",
                String::from_utf8_lossy(&result.stderr).lines().last().unwrap_or("unknown MPEG error")));
        }
        Ok(target)
    }
}

pub fn direct_path(folder: &Path, name: &str) -> Option<PathBuf> {
    indexed_path(folder, name).filter(|path| path.extension().and_then(|ext| ext.to_str()) != Some("vdata"))
}

pub fn event_media(folder: &Path, segments: &[Segment], event: &Event, angle: usize,
    speed: u8, output: &Path) -> Result<(), String>
{
    let mut cursor = event.start;
    let mut native = NativeRemux { files: Vec::new() };
    let mut script = String::from("ffconcat version 1.0\n");
    for segment in segments.iter().filter(|seg| seg.start < event.end && seg.end > event.start) {
        if segment.start > cursor.saturating_add(100) {
            return Err("Replay media has a gap in this range".into());
        }
        let path = segment.files.get(angle).and_then(Option::as_deref).and_then(|file| indexed_path(folder, file))
            .and_then(|path| path.canonicalize().ok()).ok_or("Camera media is missing for this range")?;
        let start = segment.seek_time(angle, cursor);
        let end = segment.cut_end(angle, event.end.min(segment.end)).max(start.saturating_add(1));
        let path = native.prepare(path, output)?.canonicalize().map_err(|error| error.to_string())?;
        let name = path.to_str().ok_or("Invalid replay filename")?;
        let name = name.strip_prefix(r"\\?\").unwrap_or(name).replace('\\', "/");
        if name.contains(['\r', '\n']) { return Err("Invalid replay path".into()); }
        let name = name.replace('\'', "'\\''");
        script.push_str(&format!("file '{name}'\ninpoint {:.3}\noutpoint {:.3}\nduration {:.3}\n",
            start as f64 / 1000.0, end as f64 / 1000.0, end.saturating_sub(start) as f64 / 1000.0));
        cursor = segment.end.min(event.end);
    }
    if cursor < event.end {
        return Err("Replay media is still being finalized".into());
    }
    let playlist = output.with_extension("ffconcat");
    std::fs::write(&playlist, script).map_err(|err| err.to_string())?;
    let speed = speed.clamp(1, 100) as f64 / 100.0;
    let mut atempo = Vec::new();
    let mut rate = speed;
    while rate < 0.5 {
        atempo.push("atempo=0.5".to_owned());
        rate *= 2.0;
    }
    atempo.push(format!("atempo={rate:.4}"));
    // concatdec_select removes preroll packets before the requested in-point after decoding.
    let video = format!("select=concatdec_select,setpts=(PTS-STARTPTS)/{speed:.4}");
    let audio = format!("aselect=concatdec_select,asetpts=PTS-STARTPTS,{}", atempo.join(","));
    let result = crate::stream::command().args(["-y", "-copyts", "-f", "concat", "-safe", "0",
        "-segment_time_metadata", "1", "-i"]).arg(&playlist)
        .args(["-map", "0:v:0", "-map", "0:a:0?", "-vf", &video, "-af", &audio,
            "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac",
            "-fps_mode", "vfr", "-movflags", "+faststart"])
        .arg(output).output();
    let _ = std::fs::remove_file(playlist);
    match result {
        Ok(result) if result.status.success() => Ok(()),
        Ok(result) => Err(String::from_utf8_lossy(&result.stderr).lines().last().unwrap_or("Replay export failed").into()),
        Err(error) => Err(format!("Replay needs FFmpeg: {error}")),
    }
}

pub enum Action {
    MarkIn,
    MarkOut,
    MarkLast(u64),
    CancelMark,
    SaveEvent(Event),
    DeleteEvent(u64),
    Play { event: u64, program: bool },
    Pause(usize),
    SetPlaying(usize, bool),
    Stop(usize),
    Live,
    Step(i8),
    Seek(u64),
    Jog(usize, i8),
    Cue(usize, u64),
    TakeCue(usize),
    Reverse,
    Export { event: u64, angle: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_marks_hold_the_previous_picture_across_asymmetric_drops() {
        let mut segment = Segment { start: 4000, end: 8000, files: Default::default(), frames: Default::default() };
        segment.frames[0] = vec![
            FrameStamp { show_ms: 10, media_ms: 0 },
            FrameStamp { show_ms: 40, media_ms: 30 },
            FrameStamp { show_ms: 200, media_ms: 190 },
        ];
        segment.frames[1] = vec![
            FrameStamp { show_ms: 40, media_ms: 0 },
            FrameStamp { show_ms: 120, media_ms: 80 },
            FrameStamp { show_ms: 200, media_ms: 160 },
        ];
        assert_eq!(segment.seek_time(0, 4140), 30);
        assert_eq!(segment.seek_time(1, 4140), 80);
        assert_eq!(segment.cut_end(0, 4140), 190);
        assert_eq!(segment.cut_end(1, 4140), 160);
        assert_eq!(segment.show_time(0, 190), segment.show_time(1, 160));
        assert_eq!(segment.seek_time(2, 4140), 140);
    }

    #[test]
    fn native_manifest_loads_markers_and_existing_media_read_only() {
        let root = std::env::temp_dir().join(format!("livemix-native-replay-{}", crate::replay_stamp()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/first.avi"), []).unwrap();
        std::fs::write(root.join("data/second.avi"), []).unwrap();
        let xml = r#"<replay><video width="1920" height="1080" interval="400000"/>
            <stream cameraName="Main"><segment filename="first.avi" videoStartTime="10000000"/>
                <segment filename="second" videoStartTime="20000000"/></stream>
            <events><event id="9" inPoint="17500000" outPoint="22500000" selectedAngle="0" speed="50">
                <description>Goal</description><selectedAngles>0</selectedAngles></event></events></replay>"#;
        std::fs::write(root.join("replay2.xml"), xml).unwrap();
        let session = Session::open(&root).unwrap();
        assert!(session.native);
        assert_eq!(session.format.as_ref().unwrap().fps, 25.0);
        assert_eq!(session.segments.len(), 2);
        assert_eq!(session.segments[0].end, 1000);
        let event = &session.events[0];
        assert_eq!((event.id, event.start, event.end, event.speed), (9, 750, 1250, 50));
        assert_eq!(event.description, "Goal");
        assert!(session.save(&root).is_err());
        assert_eq!(std::fs::read_to_string(root.join("replay2.xml")).unwrap(), xml);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires FFmpeg with MPEG and libx264"]
    fn native_mpeg_tracks_prepare_an_event_with_audio() {
        let root = std::env::temp_dir().join(format!("livemix-native-mpeg-{}", crate::replay_stamp()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let sources = [
            (["-f", "lavfi", "-i", "color=red:s=160x90:r=25", "-t", "1", "-c:v", "mpeg2video", "-f", "mpeg"], "sample.vdata"),
            (["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "1", "-c:a", "mp2", "-f", "mpeg"], "sample.adata"),
        ];
        for (args, name) in sources {
            let output = crate::stream::command().args(args).arg(root.join("data").join(name)).output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        }
        let xml = r#"<replay><video width="160" height="90" interval="400000"/>
            <stream cameraName="Main"><segment filename="sample" videoStartTime="10000000"/></stream>
            <events><event id="1" inPoint="10000000" outPoint="18000000" selectedAngle="0"/></events></replay>"#;
        std::fs::write(root.join("replay2.xml"), xml).unwrap();
        let session = Session::open(&root).unwrap();
        let target = root.join("prepared.mp4");
        event_media(&root, &session.segments, &session.events[0], 0, 100, &target).unwrap();
        let decoded = crate::stream::command().arg("-i").arg(&target)
            .args(["-map", "0:a:0", "-f", "s16le", "pipe:1"]).output().unwrap();
        assert!(decoded.status.success() && !decoded.stdout.is_empty());
        assert!(!root.join("livemix-replay.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[ignore = "requires FFmpeg with libx264"]
    fn replay_trims_across_segments_and_stretches_audio_with_video() {
        let root = std::env::temp_dir().join(format!("livemix-replay-trim-{}", crate::replay_stamp()));
        std::fs::create_dir_all(&root).unwrap();
        let segments = (0..2).map(|index| {
            let name = format!("camera-{index}.mp4");
            let result = crate::stream::command().args(["-f", "lavfi", "-i", "color=red:s=160x90:r=25",
                "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000", "-t", "1",
                "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"]).arg(root.join(&name)).output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            let mut files = std::array::from_fn(|_| None);
            files[0] = Some(name);
            Segment { start: index * 1000, end: (index + 1) * 1000, files, frames: Default::default() }
        }).collect::<Vec<_>>();
        let event = Event { id: 1, start: 760, end: 1240, description: String::new(), speed: 50, angles: [true; 8] };
        let target = root.join("result.mp4");
        event_media(&root, &segments, &event, 0, 50, &target).unwrap();
        let decoded = crate::stream::command().arg("-i").arg(&target).args(["-map", "0:a:0", "-ac", "2",
            "-ar", "48000", "-f", "s16le", "pipe:1"]).output().unwrap();
        assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
        let seconds = decoded.stdout.len() as f64 / (4.0 * 48_000.0);
        assert!((0.82..1.12).contains(&seconds), "Expected about 0.96 seconds of audio, got {seconds}");
        std::fs::remove_dir_all(root).unwrap();
    }
}
