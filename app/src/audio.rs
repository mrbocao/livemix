//! Stereo PCM shared by media readers, the mixer and the output workers.
use std::collections::VecDeque;
use std::io::{BufReader, Read, Write};
use std::process::Stdio;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const RATE: u32 = 48_000;
pub type Stereo = [i16; 2];
pub type Packet = Arc<[Stereo]>;
const HISTORY: usize = RATE as usize * 2;
const BLOCK: usize = 480;

/// Where the file's audio is at a given instant. The media position advances in
/// real time from `stamp`, and `ms` is smoothed so video delivery jitter does not
/// move it backwards and forwards (which skipped or repeated samples).
#[derive(Clone, Copy, Debug)]
pub struct AudioClock {
    pub ms: u64,
    pub stamp: Instant,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Effects {
    pub channel: u8,
    pub gate_threshold: f32,
    pub comp_threshold: f32,
    pub comp_ratio: f32,
    pub eq: [f32; 10],
    pub gain_db: f32,
    pub delay_ms: f32,
    /// Stereo balance, -1 fully left, 0 centre, +1 fully right (original's Pan knob).
    #[serde(default)]
    pub pan: f32,
}

impl Default for Effects {
    fn default() -> Self {
        Self { channel: 0, gate_threshold: -90.0, comp_threshold: -18.0, comp_ratio: 1.0,
            eq: [0.0; 10], gain_db: 0.0, delay_ms: 0.0, pan: 0.0 }
    }
}

impl Effects {
    pub fn normalized(mut self) -> Self {
        self.channel = self.channel.min(2);
        self.gate_threshold = self.gate_threshold.clamp(-90.0, 0.0);
        self.comp_threshold = self.comp_threshold.clamp(-60.0, 0.0);
        self.comp_ratio = self.comp_ratio.clamp(1.0, 20.0);
        self.eq = self.eq.map(|gain| gain.clamp(-18.0, 18.0));
        self.gain_db = self.gain_db.clamp(-24.0, 24.0);
        self.delay_ms = self.delay_ms.clamp(0.0, 1000.0);
        self.pan = self.pan.clamp(-1.0, 1.0);
        self
    }
}

#[derive(Default)]
pub struct Buffer {
    effects: Effects,
    chain: crate::dsp::Chain,
    scratch: Vec<f32>,
    first: u64,
    data: VecDeque<Stereo>,
    live_end: Option<Instant>,
    live_latency: i64,
    file_epoch: u64,
}

impl Buffer {
    pub fn set_effects(&mut self, effects: &Effects) {
        if self.effects != *effects {
            self.effects = effects.clone();
            self.chain = Default::default();
        }
    }

    fn append(&mut self, samples: impl IntoIterator<Item = Stereo>) {
        if self.effects == Effects::default() {
            self.data.extend(samples);
        } else {
            self.scratch.clear();
            self.scratch.extend(samples.into_iter().flatten().map(|sample| f32::from(sample) / 32768.0));
            self.chain.process(&mut self.scratch, &self.effects);
            self.data.extend(self.scratch.chunks_exact(2).map(|pair| {
                [(pair[0] * 32768.0).round() as i16, (pair[1] * 32768.0).round() as i16]
            }));
        }
        let excess = self.data.len().saturating_sub(HISTORY);
        self.data.drain(..excess);
        self.first += excess as u64;
    }

    pub fn capture(&mut self, samples: &[Stereo]) {
        if samples.is_empty() { return; }
        let now = Instant::now();
        let duration = sample_duration(samples.len());
        let end = self.live_end.and_then(|previous| {
            let predicted = previous + duration;
            let error = signed_frames(now, predicted);
            if error.abs() > 5760 { return None; }
            // Follow slow drift between the device and system clocks while
            // filtering out callback and network delivery jitter.
            let correction = (error / 64).clamp(-2, 2);
            let step = sample_duration(correction.unsigned_abs() as usize);
            Some(if correction < 0 { predicted - step } else { predicted + step })
        });
        if end.is_none() { self.data.clear(); }
        self.live_end = Some(end.unwrap_or(now));
        self.live_latency = (samples.len() as i64).clamp(1440, 5760);
        self.append(samples.iter().copied());
    }

    fn read(&self, shared: &crate::media::Shared, at: Instant, frames: usize, file: bool,
        mut add: impl FnMut(usize, Stereo))
    {
        let index = if file {
            let Ok(transport) = shared.audio_position.try_lock() else { return };
            let Some(clock) = *transport else { return };
            if self.file_epoch != shared.audio_epoch.load(Relaxed)
                || shared.paused.load(Relaxed) || shared.ended.load(Relaxed) { return; }
            let offset = signed_frames(at, clock.stamp).clamp(-4800, 2400);
            (clock.ms * u64::from(RATE) / 1000) as i64 + offset - frames as i64 - self.first as i64
        } else {
            let Some(end) = self.live_end else { return };
            // Read slightly behind capture arrival to accommodate packet delivery jitter.
            let offset = signed_frames(at, end) - self.live_latency;
            self.data.len() as i64 + offset - frames as i64
        };
        for n in 0..frames {
            if let Some(sample) = usize::try_from(index + n as i64).ok().and_then(|i| self.data.get(i)) {
                add(n, *sample);
            }
        }
    }
}

fn signed_frames(a: Instant, b: Instant) -> i64 {
    let secs = a.saturating_duration_since(b).as_secs_f64() - b.saturating_duration_since(a).as_secs_f64();
    (secs * f64::from(RATE)).round() as i64
}

pub fn sample_duration(frames: usize) -> Duration {
    Duration::from_secs_f64(frames as f64 / f64::from(RATE))
}

pub fn mark_position(shared: &crate::media::Shared, ms: u64) {
    let mut position = shared.audio_position.lock().unwrap();
    *position = Some(smooth_clock(*position, ms, Instant::now()));
}

/// Advances the audio anchor to `ms`. Small deviations from the predicted
/// position (video delivery jitter) are corrected gradually; a large one is a
/// seek, a loop or a new file and is followed at once.
fn smooth_clock(previous: Option<AudioClock>, ms: u64, now: Instant) -> AudioClock {
    let smoothed = previous.and_then(|clock| {
        let elapsed = now.saturating_duration_since(clock.stamp).as_secs_f64() * 1000.0;
        let predicted = clock.ms as f64 + elapsed;
        let error = ms as f64 - predicted;
        (error.abs() <= 120.0).then(|| (predicted + error * 0.125).max(0.0) as u64)
    });
    AudioClock { ms: smoothed.unwrap_or(ms), stamp: now }
}

pub fn reset_position(shared: &crate::media::Shared) {
    let mut position = shared.audio_position.lock().unwrap();
    *position = None;
    shared.audio_epoch.fetch_add(1, Relaxed);
}

pub fn mix<'a>(inputs: impl IntoIterator<Item = (&'a crate::media::Shared, bool, f32)>,
    at: Instant, frames: usize) -> Packet
{
    let mut mixed = vec![[0.0f32; 2]; frames];
    for (source, file, gain) in inputs {
        if gain == 0.0 { continue; }
        if let Ok(buffer) = source.audio.try_lock() {
            buffer.read(source, at, frames, file, |i, sample| {
                mixed[i][0] += f32::from(sample[0]) * gain;
                mixed[i][1] += f32::from(sample[1]) * gain;
            });
        }
    }
    mixed.into_iter().map(|[l, r]| [l.round() as i16, r.round() as i16]).collect()
}

pub fn peak(packet: &[Stereo]) -> f32 {
    packet.iter().flatten().map(|value| f32::from(value.unsigned_abs()) / 32768.0)
        .fold(0.0, f32::max)
}

#[cfg(test)]
pub fn assert_audio_file(path: &std::path::Path) {
    let output = crate::stream::command().args(["-v", "error", "-i"]).arg(path)
        .args(["-map", "0:a:0", "-ac", "2", "-ar", "48000", "-f", "s16le", "pipe:1"])
        .stdin(Stdio::null()).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let samples: Vec<_> = pcm(&output.stdout).collect();
    for channel in 0..2 {
        assert!(samples.iter().any(|frame| frame[channel].unsigned_abs() > 1600),
            "Output audio channel {} is silent", channel + 1);
    }
}

#[cfg(test)]
pub fn test_tone(frames: usize) -> Packet {
    (0..frames).map(|i| {
        let phase = i as f32 * std::f32::consts::TAU / 48.0;
        [(4800.0 * phase.sin()) as i16, (4000.0 * phase.cos()) as i16]
    }).collect()
}

pub fn input_peak((shared, file, gain): (&crate::media::Shared, bool, f32), at: Instant) -> f32 {
    let mut peak = 0u16;
    if gain > 0.0 {
        if let Ok(buffer) = shared.audio.lock() {
            buffer.read(shared, at, 960, file, |_, [l, r]| {
                peak = peak.max(l.unsigned_abs()).max(r.unsigned_abs());
            });
        }
    }
    f32::from(peak) * gain / 32768.0
}

pub(crate) fn pcm(bytes: &[u8]) -> impl Iterator<Item = Stereo> + '_ {
    bytes.chunks_exact(4).map(|b| [i16::from_le_bytes([b[0], b[1]]), i16::from_le_bytes([b[2], b[3]])])
}

pub fn devices() -> Vec<crate::media::DeviceInfo> {
    use cpal::traits::HostTrait;
    cpal::default_host().input_devices().into_iter().flatten().filter_map(|device| {
        let (name, id) = audio_device_identity(&device)?;
        Some(crate::media::DeviceInfo { name, link: format!("audio:{id}"), blackmagic: false })
    }).collect()
}

fn audio_device_identity(device: &cpal::Device) -> Option<(String, String)> {
    use cpal::traits::DeviceTrait;
    let name = device.name().ok()?;
    Some((name.clone(), name))
}

const NATIVE_RATE: cpal::SampleRate = cpal::SampleRate(RATE);

pub fn capture_device(shared: Arc<crate::media::Shared>, id: &str) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let result = (|| -> Result<(), String> {
        let device = cpal::default_host().input_devices().map_err(|e| e.to_string())?
            .find(|device| audio_device_identity(device).is_some_and(|(_, key)| key == id))
            .ok_or("Audio input disconnected")?;
        let default = device.default_input_config().map_err(|e| e.to_string())?;
        let config = device.supported_input_configs().ok().and_then(|ranges| {
            ranges.filter(|range| range.channels() == default.channels() && range.sample_format() == default.sample_format())
                .find_map(|range| range.try_with_sample_rate(NATIVE_RATE))
        }).unwrap_or(default);
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => input_stream::<f32>(&device, config.config(), &shared),
            cpal::SampleFormat::I16 => input_stream::<i16>(&device, config.config(), &shared),
            cpal::SampleFormat::I32 => input_stream::<i32>(&device, config.config(), &shared),
            cpal::SampleFormat::U16 => input_stream::<u16>(&device, config.config(), &shared),
            _ => return Err("Unsupported audio input sample format".into()),
        }?;
        shared.set_status("");
        stream.play().map_err(|e| e.to_string())?;
        while !shared.stop.load(Relaxed) { std::thread::sleep(Duration::from_millis(40)); }
        Ok(())
    })();
    if let Err(error) = result { shared.set_status(format!("Audio input: {error}")); }
}

fn input_stream<T>(device: &cpal::Device, config: cpal::StreamConfig,
    shared: &Arc<crate::media::Shared>) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    i16: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    use cpal::Sample;
    let source = shared.clone();
    let errors = shared.clone();
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;
    let mut phase = 0;
    let mut samples = Vec::new();
    device.build_input_stream(&config, move |data: &[T], _| {
        samples.clear();
        for frame in data.chunks_exact(channels) {
            let left = i16::from_sample(frame[0]);
            let right = i16::from_sample(frame[usize::from(channels > 1)]);
            phase += RATE;
            while phase >= rate {
                samples.push([left, right]);
                phase -= rate;
            }
        }
        if let Ok(mut buffer) = source.audio.try_lock() { buffer.capture(&samples); }
    }, move |error| errors.set_status(format!("Audio input: {error}")), None).map_err(|e| e.to_string())
}

type MonitorSource = (Arc<crate::media::Shared>, bool, f32);

pub struct Monitor {
    _stream: cpal::Stream,
    pub sources: Arc<Mutex<Vec<MonitorSource>>>,
    pub error: Arc<Mutex<Option<String>>>,
}

impl Monitor {
    pub fn start() -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host().default_output_device().ok_or("No system audio output")?;
        let default = device.default_output_config().map_err(|e| e.to_string())?;
        let config = device.supported_output_configs().ok().and_then(|ranges| {
            ranges.filter(|range| range.channels() == default.channels() && range.sample_format() == default.sample_format())
                .find_map(|range| range.try_with_sample_rate(NATIVE_RATE))
        }).unwrap_or(default);
        let sources = Arc::default();
        let error = Arc::default();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => monitor_stream::<f32>(&device, config.config(), &sources, &error),
            cpal::SampleFormat::I16 => monitor_stream::<i16>(&device, config.config(), &sources, &error),
            cpal::SampleFormat::I32 => monitor_stream::<i32>(&device, config.config(), &sources, &error),
            cpal::SampleFormat::U16 => monitor_stream::<u16>(&device, config.config(), &sources, &error),
            _ => return Err("Unsupported audio output sample format".into()),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self { _stream: stream, sources, error })
    }
}

fn monitor_stream<T>(device: &cpal::Device, config: cpal::StreamConfig,
    sources: &Arc<Mutex<Vec<MonitorSource>>>, error: &Arc<Mutex<Option<String>>>)
    -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<i16>,
{
    use cpal::traits::DeviceTrait;
    let sources = sources.clone();
    let error = error.clone();
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0 as usize;
    // Kept so a rare lock miss repeats the previous block instead of dropping to
    // silence, which is heard as a click.
    let mut last: Option<Packet> = None;
    device.build_output_stream(&config, move |data: &mut [T], _| {
        let frames = (data.len() / channels * RATE as usize).div_ceil(rate);
        let packet = match sources.try_lock() {
            Ok(sources) => {
                let packet = mix(sources.iter().map(|(source, file, gain)| (&**source, *file, *gain)),
                    Instant::now(), frames);
                last = Some(packet.clone());
                Some(packet)
            }
            Err(_) => last.clone(),
        };
        for (i, frame) in data.chunks_exact_mut(channels).enumerate() {
            let [l, r] = packet.as_ref().and_then(|p| p.get(i * RATE as usize / rate)).copied().unwrap_or_default();
            frame.fill(T::from_sample(0));
            if channels == 1 {
                frame[0] = T::from_sample(((i32::from(l) + i32::from(r)) / 2) as i16);
            } else {
                frame[0] = T::from_sample(l);
                frame[1] = T::from_sample(r);
            }
        }
    }, move |err| *error.lock().unwrap() = Some(err.to_string()), None).map_err(|e| e.to_string())
}

pub fn play_file(shared: Arc<crate::media::Shared>, path: String) {
    let mut last_epoch = None;
    while !shared.stop.load(Relaxed) {
        let (position, epoch) = {
            let position = shared.audio_position.lock().unwrap();
            (position.map(|clock| clock.ms), shared.audio_epoch.load(Relaxed))
        };
        let Some(position) = position.filter(|_| Some(epoch) != last_epoch) else {
            std::thread::sleep(Duration::from_millis(30));
            continue;
        };
        last_epoch = Some(epoch);
        let mut command = crate::stream::command();
        command.args(["-ss", &format!("{:.3}", position as f64 / 1000.0), "-i", &path]);
        let mut cursor = position * u64::from(RATE) / 1000;
        {
            let mut buffer = shared.audio.lock().unwrap();
            buffer.data.clear();
            buffer.first = cursor;
            buffer.file_epoch = epoch;
            buffer.chain = Default::default();
        }
        read_pcm(&shared, command, |samples| {
            while cursor > (shared.pos_ms.load(Relaxed) + 650) * u64::from(RATE) / 1000 {
                if shared.stop.load(Relaxed) || shared.audio_epoch.load(Relaxed) != epoch { return false; }
                std::thread::sleep(Duration::from_millis(10));
            }
            if shared.audio_epoch.load(Relaxed) != epoch { return false; }
            let mut buffer = shared.audio.lock().unwrap();
            buffer.append(pcm(samples));
            cursor += (samples.len() / 4) as u64;
            true
        });
    }
}

fn read_pcm(shared: &crate::media::Shared, mut command: std::process::Command,
    mut consume: impl FnMut(&[u8]) -> bool) -> bool
{
    let Ok(mut child) = command.args(["-map", "0:a:0", "-vn", "-ac", "2", "-ar", "48000", "-f", "s16le", "pipe:1"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn() else { return false };
    let mut pipe = BufReader::new(child.stdout.take().unwrap());
    let child = Mutex::new(child);
    let done = std::sync::atomic::AtomicBool::new(false);
    let epoch = shared.audio_epoch.load(Relaxed);
    let mut read = false;
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Relaxed) && !shared.stop.load(Relaxed) && shared.audio_epoch.load(Relaxed) == epoch {
                std::thread::sleep(Duration::from_millis(20));
            }
            let mut child = child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        });
        let mut bytes = [0; BLOCK * 4];
        while !shared.stop.load(Relaxed) {
            let mut len = 0;
            while len < bytes.len() {
                match pipe.read(&mut bytes[len..]) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => len += n,
                }
            }
            if len < 4 || !consume(&bytes[..len]) { break; }
            read = true;
            if len < bytes.len() { break; }
        }
        done.store(true, Relaxed);
    });
    read
}

pub fn frames_for_slot(slot: u64, n: u32, d: u32) -> usize {
    let scale = u64::from(RATE) * u64::from(d);
    ((slot + 1) * scale / u64::from(n) - slot * scale / u64::from(n)) as usize
}

pub fn planar(packet: &[Stereo], scale: f32) -> Vec<f32> {
    (0..2).flat_map(|channel| packet.iter().map(move |sample| f32::from(sample[channel]) * scale))
        .collect()
}

/// Read a borrowed SDK buffer, before returning it to the receiver.
pub unsafe fn capture_planar(shared: &crate::media::Shared, data: *const u8, frames: usize,
    stride: usize, channels: usize, rate: usize, scale: f32, phase: &mut usize)
{
    if data.is_null() || !(1..=192_000).contains(&rate) || !(1..=rate / 2).contains(&frames)
        || !(1..=64).contains(&channels) || !(frames * 4..=1_048_576).contains(&stride)
    {
        return;
    }
    let mut samples = Vec::with_capacity((frames * RATE as usize).div_ceil(rate));
    for index in 0..frames {
        let sample = |channel| {
            let value = data.add(index * 4 + channel * stride).cast::<f32>().read_unaligned();
            (value * scale).round() as i16
        };
        *phase += RATE as usize;
        while *phase >= rate {
            samples.push([sample(0), sample(usize::from(channels > 1))]);
            *phase -= rate;
        }
    }
    shared.audio.lock().unwrap().capture(&samples);
}

pub fn write_pcm(mut output: impl Write, samples: &[Stereo], count: usize) -> std::io::Result<()> {
    let valid = samples.len().min(count);
    output.write_all(bytemuck::cast_slice(&samples[..valid]))?;
    let zeros = [0; 4096];
    let mut remaining = (count - valid) * 4;
    while remaining > 0 {
        let size = remaining.min(zeros.len());
        output.write_all(&zeros[..size])?;
        remaining -= size;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_clock_keeps_fractional_video_rates() {
        let counts: usize = (0..60_000).map(|slot| frames_for_slot(slot, 60_000, 1001)).sum();
        assert_eq!(counts, 48_048_000);
    }

    #[test]
    fn audio_clock_filters_video_delivery_jitter() {
        // A 59.94 Hz file whose frames are delivered a few milliseconds early or
        // late: the audio position must still advance smoothly, never backwards.
        let base = Instant::now();
        let frame = 1000.0 / 59.94;
        let jitter = [3.0, -2.0, 5.0, -4.0, 0.0, 2.0, -5.0, 1.0];
        let mut clock = None;
        let mut positions = Vec::new();
        for n in 0..240 {
            let scheduled = n as f64 * frame;
            let stamp = base + Duration::from_secs_f64((scheduled + jitter[n % jitter.len()]) / 1000.0);
            clock = Some(smooth_clock(clock, scheduled.round() as u64, stamp));
            let at = base + Duration::from_secs_f64(scheduled / 1000.0);
            let clock = clock.unwrap();
            // Same mapping the mixer uses to find the sample to output.
            positions.push(clock.ms as f64
                + signed_frames(at, clock.stamp) as f64 * 1000.0 / f64::from(RATE));
        }
        for (i, pair) in positions[120..].windows(2).enumerate() {
            let step = pair[1] - pair[0];
            assert!((step - frame).abs() < 1.5,
                "position step {step:.3} ms is not {frame:.3} ms at {i} ({:.3} -> {:.3})",
                pair[0], pair[1]);
        }
    }

    #[test]
    #[ignore = "requires FFmpeg"]
    fn ffmpeg_pipe_decodes_stereo_and_preserves_picture_orientation() {
        let mut container = AvPipe::new(Vec::new(), 4, 2, 30_000, 1001).unwrap();
        let pixels = [[0, 0, 200, 255].repeat(4), [200, 0, 0, 255].repeat(4)].concat();
        let audio = [[1800, -3000]; 1602];
        container.frame(&pixels, &audio).unwrap();
        let decode = |args: &[&str]| {
            let mut child = crate::stream::command().args(["-v", "error", "-f", "matroska", "-i", "pipe:0"])
                .args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            child.stdin.take().unwrap().write_all(&container.output).unwrap();
            let result = child.wait_with_output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            result.stdout
        };
        let sound = decode(&["-map", "0:a:0", "-f", "s16le", "pipe:1"]);
        assert_eq!(pcm(&sound).collect::<Vec<_>>(), &audio[..1601]);
        let picture = decode(&["-map", "0:v:0", "-pix_fmt", "bgr0", "-f", "rawvideo", "pipe:1"]);
        assert_eq!(&picture[..3], &[0, 0, 200]);
        assert_eq!(&picture[16..19], &[200, 0, 0]);
    }
}

// An unknown-length Matroska segment transports both tracks through one pipe.
pub struct AvPipe<W> {
    output: W,
    slot: u64,
    n: u32,
    d: u32,
}

fn element_header(mut out: impl Write, id: &[u8], len: usize) -> std::io::Result<()> {
    out.write_all(id)?;
    out.write_all(&(len as u64 | (1 << 56)).to_be_bytes())
}

fn element(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
    element_header(&mut *out, id, data.len()).unwrap();
    out.extend(data);
}

fn uint(out: &mut Vec<u8>, id: &[u8], value: u64) {
    let bytes = value.to_be_bytes();
    element(out, id, &bytes[(value.leading_zeros().min(56) / 8) as usize..]);
}

fn master(out: &mut Vec<u8>, id: &[u8], build: impl FnOnce(&mut Vec<u8>)) {
    let mut children = Vec::new();
    build(&mut children);
    element(out, id, &children);
}

impl<W: Write> AvPipe<W> {
    pub fn new(mut output: W, w: u32, h: u32, n: u32, d: u32) -> std::io::Result<Self> {
        let mut header = Vec::new();
        master(&mut header, &[0x1a, 0x45, 0xdf, 0xa3], |ebml| {
            uint(ebml, &[0x42, 0x86], 1); // EBMLVersion
            uint(ebml, &[0x42, 0xf7], 1); // EBMLReadVersion
            uint(ebml, &[0x42, 0xf2], 4); // EBMLMaxIDLength
            uint(ebml, &[0x42, 0xf3], 8); // EBMLMaxSizeLength
            element(ebml, &[0x42, 0x82], b"matroska");
            uint(ebml, &[0x42, 0x87], 4); // DocTypeVersion
            uint(ebml, &[0x42, 0x85], 2); // DocTypeReadVersion
        });
        header.extend([0x18, 0x53, 0x80, 0x67, 0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
        master(&mut header, &[0x15, 0x49, 0xa9, 0x66], |info| {
            uint(info, &[0x2a, 0xd7, 0xb1], 1000); // Nanoseconds per timestamp unit
            element(info, &[0x4d, 0x80], b"LiveMix");
            element(info, &[0x57, 0x41], b"LiveMix");
        });
        master(&mut header, &[0x16, 0x54, 0xae, 0x6b], |tracks| {
            master(tracks, &[0xae], |video| {
                uint(video, &[0xd7], 1); // TrackNumber
                uint(video, &[0x73, 0xc5], 1); // TrackUID
                uint(video, &[0x83], 1); // TrackType
                uint(video, &[0x23, 0xe3, 0x83], 1_000_000_000 * u64::from(d) / u64::from(n));
                element(video, &[0x86], b"V_MS/VFW/FOURCC");
                // BITMAPINFOHEADER for a top-down, uncompressed 32-bit bitmap.
                let bitmap: Vec<_> = [40, w, (-(h as i32)) as u32, 1 | (32 << 16), 0, w * h * 4, 0, 0, 0, 0]
                    .into_iter().flat_map(u32::to_le_bytes).collect();
                element(video, &[0x63, 0xa2], &bitmap);
                master(video, &[0xe0], |picture| {
                    uint(picture, &[0xb0], u64::from(w));
                    uint(picture, &[0xba], u64::from(h));
                });
            });
            master(tracks, &[0xae], |audio| {
                uint(audio, &[0xd7], 2);
                uint(audio, &[0x73, 0xc5], 2);
                uint(audio, &[0x83], 2);
                element(audio, &[0x86], b"A_PCM/INT/LIT");
                master(audio, &[0xe1], |format| {
                    element(format, &[0xb5], &f64::from(RATE).to_be_bytes());
                    uint(format, &[0x9f], 2);
                    uint(format, &[0x62, 0x64], 16);
                });
            });
        });
        output.write_all(&header)?;
        Ok(Self { output, slot: 0, n, d })
    }

    pub fn frame(&mut self, bgra: &[u8], audio: &[Stereo]) -> std::io::Result<()> {
        let count = frames_for_slot(self.slot, self.n, self.d);
        let time = self.slot * 1_000_000 * u64::from(self.d) / u64::from(self.n);
        let mut stamp = Vec::new();
        uint(&mut stamp, &[0xe7], time);
        let overhead = 1 + 8 + 4; // SimpleBlock ID, size, and block header
        element_header(&mut self.output, &[0x1f, 0x43, 0xb6, 0x75],
            stamp.len() + 2 * overhead + bgra.len() + count * 4)?;
        self.output.write_all(&stamp)?;
        element_header(&mut self.output, &[0xa3], 4 + bgra.len())?;
        self.output.write_all(&[0x81, 0, 0, 0x80])?;
        self.output.write_all(bgra)?;
        element_header(&mut self.output, &[0xa3], 4 + count * 4)?;
        self.output.write_all(&[0x82, 0, 0, 0x80])?;
        write_pcm(&mut self.output, audio, count)?;
        self.slot += 1;
        Ok(())
    }
}
