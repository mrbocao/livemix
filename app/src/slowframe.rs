//! Optional slow-frame log for diagnosing hitches (for example when the window is maximised).
//!
//! Off by default. Start LiveMix with `LIVEMIX_SLOW_FRAMES=1` (or a threshold in milliseconds,
//! such as `LIVEMIX_SLOW_FRAMES=60`) and every update that takes longer than the threshold, or
//! that starts long after the previous one ended, appends one line to `slow-frames.log` in the
//! LiveMix storage folder. The line lists how long each phase of the update took.
use std::io::Write;
use std::time::{Duration, Instant};

const DEFAULT_THRESHOLD_MS: u64 = 100;
/// Time between two updates that is reported as a stall outside the update itself.
const GAP_THRESHOLD_MS: u64 = 250;
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub struct Probe {
    threshold: Option<Duration>,
    frame: u64,
    started: Instant,
    mark: Instant,
    previous_end: Option<Instant>,
    gap: Duration,
    size: [f32; 2],
    previous_size: [f32; 2],
    phases: Vec<(&'static str, Duration)>,
}

impl Default for Probe {
    fn default() -> Self {
        let threshold = std::env::var("LIVEMIX_SLOW_FRAMES").ok().and_then(|value| {
            let value = value.trim();
            if value.is_empty() || value == "0" { return None; }
            Some(Duration::from_millis(value.parse().unwrap_or(DEFAULT_THRESHOLD_MS)))
        });
        let now = Instant::now();
        Self { threshold, frame: 0, started: now, mark: now, previous_end: None, gap: Duration::ZERO,
            size: [0.0; 2], previous_size: [0.0; 2], phases: Vec::new() }
    }
}

impl Probe {
    fn on(&self) -> bool { self.threshold.is_some() }

    /// Call at the top of the update with the current window size in points.
    pub fn begin(&mut self, size: [f32; 2]) {
        if !self.on() { return; }
        let now = Instant::now();
        self.gap = self.previous_end.map_or(Duration::ZERO, |end| now.duration_since(end));
        self.started = now;
        self.mark = now;
        self.frame += 1;
        self.previous_size = self.size;
        self.size = size;
        self.phases.clear();
    }

    /// Records the time since the previous mark under `phase`.
    pub fn mark(&mut self, phase: &'static str) {
        if !self.on() { return; }
        let now = Instant::now();
        self.phases.push((phase, now.duration_since(self.mark)));
        self.mark = now;
    }

    /// Call at the end of the update; writes a line when the frame or the gap was slow.
    pub fn end(&mut self) {
        let Some(threshold) = self.threshold else { return };
        let now = Instant::now();
        self.previous_end = Some(now);
        let total = now.duration_since(self.started);
        if total < threshold && self.gap < Duration::from_millis(GAP_THRESHOLD_MS) { return; }
        let millis = |d: Duration| d.as_secs_f64() * 1000.0;
        let resized = if self.size != self.previous_size && self.frame > 1 { " RESIZED" } else { "" };
        let mut line = format!("{} frame={} size={:.0}x{:.0}{resized} gap={:.0}ms update={:.0}ms |",
            unix_millis(), self.frame, self.size[0], self.size[1], millis(self.gap), millis(total));
        for (phase, took) in &self.phases {
            if millis(*took) >= 1.0 { line.push_str(&format!(" {phase}={:.0}", millis(*took))); }
        }
        line.push('\n');
        let path = crate::media::storage_folder().join("slow-frames.log");
        if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES) { return; }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_is_off_without_the_environment_variable() {
        let mut probe = Probe { threshold: None, ..Probe::default() };
        probe.begin([100.0, 100.0]);
        probe.mark("x");
        probe.end();
        assert!(probe.phases.is_empty() && !probe.on());
    }

    #[test]
    fn slow_frames_record_their_phases() {
        let mut probe = Probe { threshold: Some(Duration::ZERO), ..Probe::default() };
        probe.begin([100.0, 100.0]);
        probe.mark("media");
        assert_eq!(probe.phases.len(), 1);
        assert_eq!(probe.phases[0].0, "media");
    }
}
