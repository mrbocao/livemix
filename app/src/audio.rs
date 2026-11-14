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
    /// Fractional milliseconds, so sub-millisecond corrections do not move the read position.
    pub ms: f64,
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
    live_resyncs: u64,
    extra_history: usize,
}

impl Buffer {
    pub fn diagnostics(&self) -> (usize, u64) {
        (self.data.len(), self.live_resyncs)
    }

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
        let excess = self.data.len().saturating_sub(HISTORY + self.extra_history);
        self.data.drain(..excess);
        self.first += excess as u64;
    }

    pub fn retain_for_delay(&mut self, seconds: f32) {
        self.extra_history = (seconds.clamp(0.0, 120.0) * RATE as f32) as usize;
        let excess = self.data.len().saturating_sub(HISTORY + self.extra_history);
        self.data.drain(..excess);
        self.first += excess as u64;
    }

    pub fn capture(&mut self, samples: &[Stereo]) {
        self.capture_at(samples, Instant::now());
    }

    fn capture_at(&mut self, samples: &[Stereo], now: Instant) {
        if samples.is_empty() { return; }
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
        if end.is_none() {
            if self.live_end.is_some() {
                self.live_resyncs += 1;
            }
            self.data.clear();
        }
        self.live_end = Some(end.unwrap_or(now));
        self.live_latency = (samples.len() as i64).clamp(1440, 5760);
        self.append(samples.iter().copied());
    }

    fn read(&self, shared: &crate::media::Shared, at: Instant, frames: usize, file: bool, delay: f32,
        mut add: impl FnMut(usize, Stereo))
    {
        let index = if file {
            // Waiting for the clock is inaudible; skipping it silenced the whole block.
            let Ok(transport) = shared.audio_position.lock() else { return };
            let Some(clock) = *transport else { return };
            if self.file_epoch != shared.audio_epoch.load(Relaxed)
                || shared.paused.load(Relaxed) || shared.ended.load(Relaxed) { return; }
            let offset = signed_frames(at, clock.stamp).clamp(-4800, 2400);
            (clock.ms * f64::from(RATE) / 1000.0).round() as i64 + offset - frames as i64 - self.first as i64
        } else {
            let Some(end) = self.live_end else { return };
            // Read slightly behind capture arrival to accommodate packet delivery jitter.
            let offset = signed_frames(at, end) - self.live_latency;
            self.data.len() as i64 + offset - frames as i64
        };
        let index = index - (delay * RATE as f32).round() as i64;
        let first = index.max(0);
        let end = (index + frames as i64).min(self.data.len() as i64);
        if first >= end { return; }
        let offset = (first - index) as usize;
        for (n, sample) in self.data.range(first as usize..end as usize).enumerate() {
            add(offset + n, *sample);
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

/// Marks where the file's audio is now, in fractional milliseconds. Integer milliseconds
/// quantise the position by up to 48 samples, which the clock then has to filter out.
pub fn mark_position(shared: &crate::media::Shared, ms: f64) {
    let mut position = shared.audio_position.lock().unwrap();
    *position = Some(smooth_clock(*position, ms, Instant::now()));
}

/// Advances the audio anchor to `ms`. Small deviations from the predicted
/// position (video delivery jitter) are corrected by at most one sample per
/// update, so the read position never jumps; a large one is a seek, a loop or
/// a new file and is followed at once.
fn smooth_clock(previous: Option<AudioClock>, ms: f64, now: Instant) -> AudioClock {
    let smoothed = previous.and_then(|clock| {
        let elapsed = now.saturating_duration_since(clock.stamp).as_secs_f64() * 1000.0;
        let predicted = clock.ms + elapsed;
        let error = ms - predicted;
        let sample = 1000.0 / f64::from(RATE);
        (error.abs() <= 120.0).then(|| (predicted + (error * 0.125).clamp(-sample, sample)).max(0.0))
    });
    AudioClock { ms: smoothed.unwrap_or(ms), stamp: now }
}

/// Contiguous audio for a consumer that mixes once per video frame. Each window starts
/// where the previous one ended, whatever the time between frames, so samples are neither
/// repeated nor skipped. A gap longer than half a second starts a new run.
#[derive(Default)]
pub struct AudioCursor {
    origin: Option<Instant>,
    emitted: u64,
}

impl AudioCursor {
    /// End time and length of the next window at `now`; `nominal` is one frame of samples.
    pub fn window(&mut self, now: Instant, nominal: usize) -> (Instant, usize) {
        let due = self.origin.and_then(|origin| now.checked_duration_since(origin))
            .map(|elapsed| (elapsed.as_secs_f64() * f64::from(RATE)).round() as u64)
            .filter(|&due| due >= self.emitted && due - self.emitted <= u64::from(RATE) / 2);
        if let (Some(origin), Some(due)) = (self.origin, due) {
            let count = (due - self.emitted) as usize;
            self.emitted = due;
            return (origin + sample_duration(due as usize), count);
        }
        self.origin = Some(now.checked_sub(sample_duration(nominal)).unwrap_or(now));
        self.emitted = nominal as u64;
        (now, nominal)
    }
}

/// Largest shortfall, in samples, that a consumer may cover by holding the last sample.
/// Mixer windows and output frames count samples in different ways, so they can differ by
/// a few samples. That is inaudible, unlike a cushion of delay between audio and picture.
const FIFO_HOLD: usize = 96;
/// A consumer that falls this far behind is resynchronised instead of delaying the sound.
const FIFO_LIMIT: usize = RATE as usize / 20;

/// Per-output queue between contiguous mixer windows and a consumer that takes a fixed
/// number of samples per video frame, so outputs that started at different times or repeat
/// a frame still write every sample exactly once. It adds no delay: audio leaves the queue
/// in the frame that carries its picture.
#[derive(Default)]
pub struct AudioFifo {
    data: VecDeque<Stereo>,
    last: Stereo,
}

impl AudioFifo {
    pub fn push(&mut self, samples: &[Stereo]) {
        self.data.extend(samples.iter().copied());
        if self.data.len() > FIFO_LIMIT {
            let excess = self.data.len() - FIFO_LIMIT;
            self.data.drain(..excess);
        }
    }

    pub fn pop(&mut self, count: usize) -> Vec<Stereo> {
        let take = count.min(self.data.len());
        let mut samples: Vec<Stereo> = self.data.drain(..take).collect();
        if let Some(&last) = samples.last() { self.last = last; }
        if take < count {
            // A few missing samples repeat the last one; a real underrun is silence.
            let fill = if count - take <= FIFO_HOLD { self.last } else { [0; 2] };
            samples.resize(count, fill);
        }
        samples
    }
}

pub fn reset_position(shared: &crate::media::Shared) {
    let mut position = shared.audio_position.lock().unwrap();
    *position = None;
    shared.audio_epoch.fetch_add(1, Relaxed);
}

pub fn mix<'a>(inputs: impl IntoIterator<Item = (&'a crate::media::Shared, bool, f32)>,
    at: Instant, frames: usize) -> Packet
{
    mix_delayed(inputs.into_iter().map(|(source, file, gain)| (source, file, gain, 0.0)), at, frames)
}

pub fn mix_delayed<'a>(inputs: impl IntoIterator<Item = (&'a crate::media::Shared, bool, f32, f32)>,
    at: Instant, frames: usize) -> Packet
{
    let mut mixed = vec![[0.0f32; 2]; frames];
    mix_into(inputs, at, &mut mixed);
    mixed.into_iter().map(|[l, r]| [l.round() as i16, r.round() as i16]).collect()
}

fn mix_into<'a>(inputs: impl IntoIterator<Item = (&'a crate::media::Shared, bool, f32, f32)>,
    at: Instant, mixed: &mut [[f32; 2]])
{
    mixed.fill([0.0; 2]);
    let frames = mixed.len();
    for (source, file, gain, delay) in inputs {
        if gain == 0.0 { continue; }
        // Writers hold the buffer for microseconds. Waiting is inaudible; skipping the
        // input (try_lock) silenced a whole block whenever the reader was appending.
        if let Ok(buffer) = source.audio.lock() {
            buffer.read(source, at, frames, file, delay, |i, sample| {
                mixed[i][0] += f32::from(sample[0]) * gain;
                mixed[i][1] += f32::from(sample[1]) * gain;
            });
        }
    }
}

pub fn stereo_peaks(packet: &[Stereo]) -> [f32; 2] {
    let mut peaks = [0u16; 2];
    for &[left, right] in packet {
        peaks[0] = peaks[0].max(left.unsigned_abs());
        peaks[1] = peaks[1].max(right.unsigned_abs());
    }
    peaks.map(|peak| f32::from(peak) / 32768.0)
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

#[cfg(test)]
pub fn input_peak(source: (&crate::media::Shared, bool, f32), at: Instant) -> f32 {
    let (shared, file, gain) = source;
    let [left, right] = delayed_peaks((shared, file, gain, 0.0), at);
    left.max(right)
}

pub fn delayed_peaks((shared, file, gain, delay): (&crate::media::Shared, bool, f32, f32), at: Instant) -> [f32; 2] {
    let mut peaks = [0u16; 2];
    if gain > 0.0 {
        if let Ok(buffer) = shared.audio.try_lock() {
            buffer.read(shared, at, 960, file, delay, |_, [left, right]| {
                peaks[0] = peaks[0].max(left.unsigned_abs());
                peaks[1] = peaks[1].max(right.unsigned_abs());
            });
        }
    }
    peaks.map(|peak| f32::from(peak) * gain / 32768.0)
}

/// Level where the soft knee starts, as a fraction of full scale.
const KNEE: f32 = 0.8;

/// Maps a decoded sample to 16 bits. Masters that peak above 0 dBFS (decoded AAC and MP3
/// overshoot by a dB or more) would be hard-clipped by the converter, which is harsh; above
/// the knee the curve rolls off smoothly towards full scale instead, without any latency.
pub(crate) fn soft_clip(sample: f32) -> i16 {
    let magnitude = sample.abs();
    let shaped = if magnitude <= KNEE { magnitude }
        else { KNEE + (1.0 - KNEE) * ((magnitude - KNEE) / (1.0 - KNEE)).tanh() };
    (shaped.copysign(sample) * 32767.0).round() as i16
}

/// Interleaved little-endian f32 stereo from FFmpeg.
pub(crate) fn pcm_f32(bytes: &[u8]) -> impl Iterator<Item = Stereo> + '_ {
    bytes.chunks_exact(8).map(|b| {
        let channel = |at: usize| f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        [soft_clip(channel(0)), soft_clip(channel(4))]
    })
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

type CapturedAudio = (Instant, Vec<Stereo>);

struct InputQueue {
    ready: std::sync::mpsc::SyncSender<CapturedAudio>,
    free: std::sync::mpsc::Receiver<Vec<Stereo>>,
}

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
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(8);
        let (free_tx, free_rx) = std::sync::mpsc::sync_channel(8);
        for _ in 0..8 {
            let _ = free_tx.try_send(Vec::with_capacity(4096));
        }
        let queue = InputQueue { ready: ready_tx, free: free_rx };
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => input_stream::<f32>(&device, config.config(), &shared, queue),
            cpal::SampleFormat::I16 => input_stream::<i16>(&device, config.config(), &shared, queue),
            cpal::SampleFormat::I32 => input_stream::<i32>(&device, config.config(), &shared, queue),
            cpal::SampleFormat::U16 => input_stream::<u16>(&device, config.config(), &shared, queue),
            _ => return Err("Unsupported audio input sample format".into()),
        }?;
        shared.set_status("");
        stream.play().map_err(|e| e.to_string())?;
        while !shared.stop.load(Relaxed) {
            match ready_rx.recv_timeout(Duration::from_millis(40)) {
                Ok((at, mut samples)) => {
                    shared.audio.lock().unwrap().capture_at(&samples, at);
                    samples.clear();
                    let _ = free_tx.try_send(samples);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(())
    })();
    if let Err(error) = result { shared.set_status(format!("Audio input: {error}")); }
}

fn input_stream<T>(device: &cpal::Device, config: cpal::StreamConfig,
    shared: &Arc<crate::media::Shared>, queue: InputQueue) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    i16: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    use cpal::Sample;
    let errors = shared.clone();
    let channels = usize::from(config.channels);
    let rate = config.sample_rate.0;
    let mut phase = 0;
    device.build_input_stream(&config, move |data: &[T], _| {
        let at = Instant::now();
        let Ok(mut samples) = queue.free.try_recv() else { return };
        for frame in data.chunks_exact(channels) {
            let left = i16::from_sample(frame[0]);
            let right = i16::from_sample(frame[usize::from(channels > 1)]);
            phase += RATE;
            while phase >= rate {
                samples.push([left, right]);
                phase -= rate;
            }
        }
        let _ = queue.ready.try_send((at, samples));
    }, move |error| errors.set_status(format!("Audio input: {error}")), None).map_err(|e| e.to_string())
}

type MonitorSource = (Arc<crate::media::Shared>, bool, f32, f32);

pub struct Monitor {
    _stream: cpal::Stream,
    pub sources: Arc<Mutex<Arc<[MonitorSource]>>>,
    pub error: Arc<Mutex<Option<String>>>,
}

pub fn output_devices() -> Vec<String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    cpal::default_host().output_devices().into_iter().flatten().filter_map(|device| device.name().ok()).collect()
}

impl Monitor {
    pub fn start(name: &str) -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let device = if name.is_empty() { host.default_output_device() } else {
            host.output_devices().map_err(|error| error.to_string())?
                .find(|device| device.name().is_ok_and(|current| current == name))
        }.ok_or("Headphone output device is unavailable")?;
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
    sources: &Arc<Mutex<Arc<[MonitorSource]>>>, error: &Arc<Mutex<Option<String>>>)
    -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<i16>,
{
    use cpal::traits::DeviceTrait;
    let sources = sources.clone();
    let error = error.clone();
    let channels = usize::from(config.channels);
    let rate = u64::from(config.sample_rate.0);
    let mut sources_snapshot = Arc::<[MonitorSource]>::default();
    let mut clock = MonitorClock::default();
    let mut mixed = Vec::new();
    let mut packet = Vec::new();
    device.build_output_stream(&config, move |data: &mut [T], _| {
        let count = data.len() / channels;
        if count == 0 { return; }
        let (at, frames, phase) = clock.block(Instant::now(), count, rate);
        if let Ok(latest) = sources.try_lock() {
            sources_snapshot = latest.clone();
        }
        mixed.resize(frames, [0.0; 2]);
        mix_into(sources_snapshot.iter().map(|(source, file, gain, delay)| (&**source, *file, *gain, *delay)), at, &mut mixed);
        packet.clear();
        packet.extend(mixed.iter().map(|[l, r]| [l.round() as i16, r.round() as i16]));
        for (i, frame) in data.chunks_exact_mut(channels).enumerate() {
            let position = phase + i as u64 * u64::from(RATE);
            let offset = (position / rate) as usize;
            let weight = (position % rate) as i32;
            let interpolate = |channel: usize| {
                let first = i32::from(packet[offset][channel]);
                let second = i32::from(packet[offset + 1][channel]);
                (first + ((second - first) as i64 * i64::from(weight) / rate as i64) as i32) as i16
            };
            let [l, r] = [interpolate(0), interpolate(1)];
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

#[derive(Default)]
struct MonitorClock {
    origin: Option<Instant>,
    output_frames: u64,
}

impl MonitorClock {
    fn block(&mut self, now: Instant, count: usize, rate: u64) -> (Instant, usize, u64) {
        let count = count as u64;
        let duration = |frames| Duration::from_secs_f64(frames as f64 / rate as f64);
        if self.origin.is_none_or(|origin| signed_frames(now, origin + duration(self.output_frames + count)).abs() > 5760) {
            self.origin = Some(now - duration(count));
            self.output_frames = 0;
        }
        let first = self.output_frames * u64::from(RATE);
        self.output_frames += count;
        let last = (self.output_frames - 1) * u64::from(RATE);
        let end = last / rate + 2;
        let at = self.origin.unwrap() + sample_duration(end as usize);
        (at, (end - first / rate) as usize, first % rate)
    }
}

pub(crate) fn ffprobe_command() -> std::process::Command {
    let decoder = crate::stream::command();
    let probe_name = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
    let sibling = std::path::Path::new(decoder.get_program()).with_file_name(probe_name);
    let probe = std::env::var_os("LIVEMIX_FFPROBE").map(std::path::PathBuf::from)
        .or_else(|| sibling.is_file().then(|| sibling.clone()))
        .or_else(|| std::fs::read_dir(sibling.parent()?).ok()?.flatten()
            .map(|entry| entry.path().join("bin").join(probe_name)).find(|path| path.is_file()))
        .unwrap_or_else(|| probe_name.into());
    let mut command = std::process::Command::new(probe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

pub fn play_audio_input(shared: Arc<crate::media::Shared>, path: String) {
    let mut command = ffprobe_command();
    let duration = command.args(["-v", "error", "-select_streams", "a:0", "-show_entries",
        "stream=codec_type:format=duration", "-of", "json", "-i"]).arg(&path).output().ok()
        .filter(|output| output.status.success()).and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .filter(|data| data["streams"][0]["codec_type"] == "audio")
        .and_then(|data| data["format"]["duration"].as_str()?.parse::<f64>().ok())
        .filter(|duration| duration.is_finite() && *duration > 0.0);
    let Some(duration) = duration else {
        shared.set_status("Cannot open audio. Check the file and FFprobe installation.");
        return;
    };
    let duration_ms = (duration * 1000.0) as u64;
    shared.dur_ms.store(duration_ms, Relaxed);
    shared.set_status("Audio file");
    // The first mark and the integration start share one instant; otherwise the clock
    // starts a few milliseconds off and spends seconds slewing back.
    let mut last = Instant::now();
    mark_position(&shared, 0.0);
    let source = shared.clone();
    std::thread::scope(|scope| {
        scope.spawn(move || play_file(source, path));
        let mut position = 0.0;
        let mut running = false;
        while !shared.stop.load(Relaxed) {
            let now = Instant::now();
            let seek = shared.seek_ms.swap(0, Relaxed).checked_sub(1);
            let restart = shared.restart.swap(false, Relaxed);
            if let Some(target) = seek.or(restart.then_some(0)) {
                position = target.min(duration_ms.saturating_sub(1)) as f64;
                shared.pos_ms.store(position as u64, Relaxed);
                shared.ended.store(false, Relaxed);
                reset_position(&shared);
                mark_position(&shared, position);
                running = true;
            } else if !shared.paused.load(Relaxed) && !shared.ended.load(Relaxed) {
                if !running {
                    // The clock kept time while paused. Starting again re-anchors it at once
                    // instead of spending seconds slewing back, which detunes the sound.
                    *shared.audio_position.lock().unwrap() = None;
                }
                position = (position + now.duration_since(last).as_secs_f64() * 1000.0).min(duration_ms as f64);
                shared.pos_ms.store(position as u64, Relaxed);
                mark_position(&shared, position);
                running = true;
                if position >= duration_ms as f64 {
                    if shared.looping.load(Relaxed) {
                        shared.restart.store(true, Relaxed);
                    } else {
                        shared.ended.store(true, Relaxed);
                    }
                }
            }
            if shared.paused.load(Relaxed) || shared.ended.load(Relaxed) { running = false; }
            last = now;
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

pub fn play_file(shared: Arc<crate::media::Shared>, path: String) {
    let mut last_epoch = None;
    while !shared.stop.load(Relaxed) {
        let (position, epoch) = {
            let position = shared.audio_position.lock().unwrap();
            (position.map(|clock| clock.ms as u64), shared.audio_epoch.load(Relaxed))
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
            buffer.append(pcm_f32(samples));
            cursor += (samples.len() / 8) as u64;
            true
        });
    }
}

fn read_pcm(shared: &crate::media::Shared, mut command: std::process::Command,
    mut consume: impl FnMut(&[u8]) -> bool) -> bool
{
    let Ok(mut child) = command.args(["-map", "0:a:0", "-vn", "-ac", "2", "-ar", "48000", "-f", "f32le", "pipe:1"])
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
        let mut bytes = [0; BLOCK * 8];
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
    stride: usize, channels: usize, rate: usize, scale: f32, phase: &mut usize, start: Option<Instant>)
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
    let mut buffer = shared.audio.lock().unwrap();
    if let Some(start) = start {
        let end = start + sample_duration(samples.len());
        if !samples.is_empty() {
            let gap = buffer.live_end.map(|previous| signed_frames(start, previous)).unwrap_or(0);
            if gap.abs() > i64::from(RATE) / 4 {
                buffer.data.clear();
                buffer.live_resyncs += 1;
            } else if gap > 0 {
                buffer.append(std::iter::repeat_n([0; 2], gap as usize));
            } else if gap < 0 {
                let skip = (-gap) as usize;
                if skip >= samples.len() { return; }
                buffer.append(samples[skip..].iter().copied());
                buffer.live_end = Some(end);
                buffer.live_latency = 0;
                return;
            }
            buffer.append(samples.iter().copied());
            buffer.live_end = Some(end);
        }
        buffer.live_latency = 0;
    } else {
        buffer.capture(&samples);
    }
}

/// Maps a sender's 100 ns timestamps to one local playout clock for audio and video.
#[derive(Default)]
pub(crate) struct NetworkClock {
    anchor: Option<(i64, Instant)>,
    video: VecDeque<(Instant, crate::media::Frame)>,
}

impl NetworkClock {
    pub fn stamp(&mut self, ticks: i64, now: Instant) -> Option<Instant> {
        if ticks <= 0 || ticks == i64::MAX { return None; }
        let lead = Duration::from_millis(60);
        if let Some((origin, local)) = self.anchor {
            let delta = ticks.abs_diff(origin).checked_mul(100).map(Duration::from_nanos);
            let mapped = delta.and_then(|delta| {
                if ticks >= origin { local.checked_add(delta) } else { local.checked_sub(delta) }
            });
            if let Some(mapped) = mapped.filter(|stamp| {
                stamp.saturating_duration_since(now).max(now.saturating_duration_since(*stamp))
                    <= Duration::from_millis(500)
            }) { return Some(mapped); }
            self.video.clear();
        }
        self.anchor = Some((ticks, now + lead));
        Some(now + lead)
    }

    pub fn queue_video(&mut self, frame: crate::media::Frame, stamp: Instant) {
        // Limit the queue by both frame count and decoded memory.
        let limit = (128 * 1024 * 1024 / frame.rgba.len().max(1)).clamp(2, 12);
        let index = self.video.partition_point(|(when, _)| *when <= stamp);
        self.video.insert(index, (stamp, frame));
        while self.video.len() > limit { self.video.pop_back(); }
    }

    pub fn present(&mut self, shared: &crate::media::Shared, now: Instant) {
        let mut latest = None;
        while self.video.front().is_some_and(|(when, _)| *when <= now) {
            latest = self.video.pop_front();
        }
        if let Some((_, frame)) = latest {
            *shared.size.lock().unwrap() = (frame.w, frame.h);
            *shared.frame.lock().unwrap() = Some(frame);
            shared.seq.fetch_add(1, Relaxed);
        }
    }
}

#[test]
fn network_timestamps_keep_sender_spacing_and_reset_after_discontinuity() {
    let now = Instant::now();
    let mut clock = NetworkClock::default();
    let origin = clock.stamp(1_000_000, now).unwrap();
    assert_eq!(origin, now + Duration::from_millis(60));
    assert_eq!(clock.stamp(1_200_000, now), Some(origin + Duration::from_millis(20)));
    assert_eq!(clock.stamp(i64::MAX, now), None);
    assert_eq!(clock.stamp(90_000_000, now), Some(origin));
}

#[test]
fn network_video_waits_for_its_playout_time() {
    let now = Instant::now();
    let shared = crate::media::Shared::default();
    let mut clock = NetworkClock::default();
    let when = clock.stamp(123, now).unwrap();
    clock.queue_video(crate::media::Frame { w: 1, h: 1, rgba: vec![1, 2, 3, 255] }, when);
    clock.present(&shared, now);
    assert_eq!(shared.seq.load(Relaxed), 0);
    clock.present(&shared, when);
    assert_eq!(shared.seq.load(Relaxed), 1);
}

#[test]
fn delayed_audio_reads_the_recorded_samples_at_the_requested_offset() {
    let shared = crate::media::Shared::default();
    let now = Instant::now();
    let mut buffer = shared.audio.lock().unwrap();
    buffer.retain_for_delay(3.0);
    buffer.live_end = Some(now);
    buffer.append(std::iter::repeat_n([1000, -1000], RATE as usize));
    buffer.append(std::iter::repeat_n([2000, -2000], RATE as usize * 3));
    drop(buffer);
    assert_eq!(mix_delayed([(&shared, false, 1.0, 3.0)], now, 1)[0], [1000, -1000]);
    assert_eq!(mix_delayed([(&shared, false, 1.0, 0.0)], now, 1)[0], [2000, -2000]);
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
    #[ignore = "requires FFmpeg and FFprobe"]
    fn audio_file_input_plays_and_seeks() {
        let path = std::env::temp_dir().join(format!("livemix-audio-test-{}.wav", std::process::id()));
        let created = crate::stream::command().args(["-y", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
            "-t", "2", "-c:a", "pcm_s16le"]).arg(&path).output().unwrap();
        assert!(created.status.success(), "{}", String::from_utf8_lossy(&created.stderr));
        let shared = crate::media::start_for_gl(crate::media::Source::AudioFile(path.to_string_lossy().into_owned()), true);
        let end = Instant::now() + Duration::from_secs(5);
        while shared.dur_ms.load(Relaxed) == 0 && Instant::now() < end { std::thread::sleep(Duration::from_millis(20)); }
        assert_eq!(shared.dur_ms.load(Relaxed), 2000, "{}", shared.status.lock().unwrap());
        shared.seek_ms.store(1001, Relaxed);
        let end = Instant::now() + Duration::from_secs(3);
        while shared.pos_ms.load(Relaxed) < 1000 && Instant::now() < end { std::thread::sleep(Duration::from_millis(20)); }
        assert_eq!(shared.pos_ms.load(Relaxed), 1000);
        shared.paused.store(false, Relaxed);
        let end = Instant::now() + Duration::from_millis(900);
        let mut peak = 0.0_f32;
        while Instant::now() < end && peak < 0.02 {
            peak = input_peak((&shared, true, 1.0), Instant::now());
            std::thread::sleep(Duration::from_millis(20));
        }
        shared.stop.store(true, Relaxed);
        assert!(peak > 0.02);
        for _ in 0..20 {
            if std::fs::remove_file(&path).is_ok() { break; }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn monitor_clock_preserves_phase_through_callback_jitter() {
        let mut clock = MonitorClock::default();
        let start = Instant::now();
        let (first, frames, phase) = clock.block(start, 127, 44_100);
        assert_eq!(phase, 0);
        assert_eq!(frames, 139);
        let (second, _, phase) = clock.block(start + Duration::from_millis(10), 127, 44_100);
        assert_eq!(phase, 127 * 48_000 % 44_100);
        assert_eq!(signed_frames(second, first), 138);
    }
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
            clock = Some(smooth_clock(clock, scheduled.round(), stamp));
            let at = base + Duration::from_secs_f64(scheduled / 1000.0);
            let clock = clock.unwrap();
            // Same mapping the mixer uses to find the sample to output.
            positions.push(clock.ms
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
    fn file_clock_without_jitter_never_moves_the_read_position() {
        // Marks that agree with the wall clock must leave the position exactly on it, not
        // wander by the sample-level rounding of the previous millisecond-based clock.
        let base = Instant::now();
        let mut clock = None;
        for update in 0..2000 {
            let at = base + Duration::from_micros(update * 5000);
            clock = Some(smooth_clock(clock, update as f64 * 5.0, at));
        }
        let clock = clock.unwrap();
        assert!((clock.ms - 1999.0 * 5.0).abs() < 1e-6, "clock drifted to {}", clock.ms);
    }

    #[test]
    fn file_clock_converges_without_jumping_more_than_a_sample() {
        let base = Instant::now();
        let mut clock = Some(AudioClock { ms: 10.0, stamp: base });
        let mut previous = 10.0;
        for update in 1..400u64 {
            let at = base + Duration::from_micros(update * 5000);
            // A true position 11 ms ahead of the clock's: it must be reached gradually.
            clock = Some(smooth_clock(clock, update as f64 * 5.0 + 21.0, at));
            let moved = clock.unwrap().ms - previous - 5.0;
            assert!(moved.abs() <= 1000.0 / f64::from(RATE) + 1e-9, "position stepped by {moved} ms");
            previous = clock.unwrap().ms;
        }
    }

    #[test]
    fn soft_clip_is_transparent_below_the_knee_and_rolls_off_above_it() {
        for level in [0.0f32, 0.1, 0.5, 0.79] {
            for sign in [1.0f32, -1.0] {
                let expected = (level * sign * 32767.0).round() as i16;
                assert_eq!(soft_clip(level * sign), expected);
            }
        }
        // Overshoot from a decoder is compressed, monotonic and never wraps.
        let mut last = soft_clip(0.8);
        for step in 1..=60 {
            let value = soft_clip(0.8 + step as f32 * 0.01);
            assert!(value >= last, "not monotonic at {step}");
            last = value;
        }
        assert!(last > 32000);
        assert_eq!(soft_clip(-4.0), -soft_clip(4.0));
        // The curve is smooth where the knee begins: no kink between neighbouring samples.
        let step = i32::from(soft_clip(0.805)) - i32::from(soft_clip(0.795));
        assert!((step - 328).abs() < 8, "slope changed at the knee: {step}");
    }

    #[test]
    fn mixing_never_drops_a_block_while_a_writer_holds_the_buffer() {
        let shared = Arc::new(crate::media::Shared::default());
        {
            let mut buffer = shared.audio.lock().unwrap();
            buffer.data.extend((0..RATE as usize * 4).map(|n| {
                let v = (4000.0 * (n as f32 * std::f32::consts::TAU / 48.0).sin()) as i16;
                [v, v]
            }));
        }
        *shared.audio_position.lock().unwrap() = Some(AudioClock { ms: 0.0, stamp: Instant::now() });
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let (shared, stop) = (shared.clone(), stop.clone());
            std::thread::spawn(move || while !stop.load(Relaxed) {
                // The reader appends while the monitor callback mixes.
                let guard = shared.audio.lock().unwrap();
                let until = Instant::now() + Duration::from_micros(300);
                while Instant::now() < until { std::hint::spin_loop(); }
                drop(guard);
                std::thread::sleep(Duration::from_micros(100));
            })
        };
        std::thread::sleep(Duration::from_millis(60));
        let mut silent = 0;
        for _ in 0..150 {
            let packet = mix([(&*shared, true, 1.0)], Instant::now(), 480);
            if packet.iter().all(|&[l, r]| l == 0 && r == 0) { silent += 1; }
            std::thread::sleep(Duration::from_millis(2));
        }
        stop.store(true, Relaxed);
        writer.join().unwrap();
        assert_eq!(silent, 0, "{silent} of 150 mixer blocks were silent");
    }

    #[test]
    fn audio_cursor_windows_are_contiguous_whatever_the_frame_timing() {
        let base = Instant::now();
        let mut cursor = AudioCursor::default();
        let mut previous_end: Option<Instant> = None;
        let mut total = 0usize;
        let mut first = None;
        for frame in 0..600u64 {
            // 59.94 Hz frames arriving up to 8 ms early or late.
            let jitter = ((frame * 2654435761) % 17) as f64 - 8.0;
            let at = base + Duration::from_secs_f64((frame as f64 * 1001.0 / 60.0 + 300.0 + jitter) / 1000.0);
            let (end, count) = cursor.window(at, 801);
            if let Some(previous) = previous_end {
                // Each window ends exactly `count` samples after the previous one.
                let gap = signed_frames(end, previous) - count as i64;
                assert_eq!(gap, 0, "window {frame} is not contiguous");
            } else {
                first = Some(end);
            }
            previous_end = Some(end);
            total += count;
        }
        let covered = signed_frames(previous_end.unwrap(), first.unwrap()) as usize + 801;
        assert_eq!(total, covered, "samples were repeated or skipped");
    }

    #[test]
    fn audio_fifo_adds_no_delay_and_covers_only_small_shortfalls() {
        let ramp: Vec<Stereo> = (1..=800).map(|n| [n, -n]).collect();
        let mut fifo = AudioFifo::default();
        fifo.push(&ramp);
        // The first frame carries its own audio: no cushion of silence in front of it.
        assert_eq!(fifo.pop(800), ramp);
        // A few missing samples hold the last one instead of dropping to silence.
        fifo.push(&ramp[..790]);
        let short = fifo.pop(800);
        assert_eq!(&short[..790], &ramp[..790]);
        assert!(short[790..].iter().all(|&sample| sample == [790, -790]));
        // A real underrun is silence, and a consumer that stops is resynchronised.
        assert!(fifo.pop(800).iter().all(|&sample| sample == [0, 0]));
        for _ in 0..40 { fifo.push(&ramp); }
        // 40 pushes of the 800-sample ramp keep only the newest FIFO_LIMIT samples.
        let first = (40 * 800 - FIFO_LIMIT) % 800 + 1;
        assert_eq!(fifo.pop(1)[0][0], first as i16);
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
    queue: AudioFifo,
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
        Ok(Self { output, slot: 0, n, d, queue: AudioFifo::default() })
    }

    /// Queues mixer audio ahead of the frames that will carry it, such as repeated pictures.
    pub fn queue_audio(&mut self, audio: &[Stereo]) {
        self.queue.push(audio);
    }

    /// Writes one picture with `audio` and one frame's worth of the queued samples.
    pub fn frame(&mut self, bgra: &[u8], audio: &[Stereo]) -> std::io::Result<()> {
        self.queue.push(audio);
        let count = frames_for_slot(self.slot, self.n, self.d);
        let audio = self.queue.pop(count);
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
        write_pcm(&mut self.output, &audio, count)?;
        self.slot += 1;
        Ok(())
    }
}
