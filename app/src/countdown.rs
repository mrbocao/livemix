//! Title countdowns, as in the original's `vMix.Countdown` and `vMix.CountdownSettings`
//! (vMix64.exe): one per title text field, driven by the StartCountdown, StopCountdown,
//! PauseCountdown, SuspendCountdown, SetCountdown, ChangeCountdown and AdjustCountdown shortcuts.
//! Times are milliseconds; `now` is the app clock in seconds.

/// One field's countdown. Settings default like `CountdownSettings::.ctor` (0x06004ef0):
/// Duration 0, Start 0, Format "HH:mm:ss", Reverse and ReverseDisplay off.
#[derive(Clone, Debug)]
pub struct Countdown {
    pub duration_ms: i64,
    /// Added to the shown time of day (CountdownSettings.Start).
    pub start_ms: i64,
    pub format: String,
    /// Counts up from 0 to Duration instead of down.
    pub reverse: bool,
    /// When stopped by hand, shows the other end (Duration for a countdown).
    pub reverse_display: bool,
    offset_ms: i64,
    /// The original's timer exists (started, maybe paused).
    running: bool,
    paused: bool,
    /// The original's stopwatch: time counted so far and when it last resumed.
    watch: Option<(f64, Option<f64>)>,
    /// AdjustCountdown asks the next tick to redraw even while paused.
    redraw: bool,
    /// The text the field shows, once the countdown has drawn.
    pub value: Option<String>,
}

impl Default for Countdown {
    fn default() -> Self {
        Self { duration_ms: 0, start_ms: 0, format: "HH:mm:ss".into(), reverse: false, reverse_display: false,
            offset_ms: 0, running: false, paused: false, watch: None, redraw: false, value: None }
    }
}

impl Countdown {
    /// `CountdownRunning`: started and not paused.
    #[cfg(test)]
    pub fn is_running(&self) -> bool { self.running && !self.paused }

    fn watch_ms(&self, now: f64) -> f64 {
        self.watch.map_or(0.0, |(counted, since)| counted + since.map_or(0.0, |since| (now - since).max(0.0) * 1000.0))
    }

    /// `Countdown::b` (0x06004edc): Duration − elapsed + 999 ms (or elapsed when counting up),
    /// plus the adjustment, kept within 0…Duration.
    pub fn current_ms(&self, now: f64) -> i64 {
        let elapsed = self.watch_ms(now) as i64;
        let value = if self.reverse { elapsed } else { self.duration_ms - elapsed + 999 } + self.offset_ms;
        value.clamp(0, self.duration_ms.max(0))
    }

    /// `Countdown::c` (0x06004ede): the value as a time of day (+ Start) in Format. A format that
    /// starts with minutes ("mm:" or "mm") shows hours as extra minutes (75:00, not 15:00).
    pub fn text(&self, ms: i64) -> String {
        let day = (ms + self.start_ms).rem_euclid(86_400_000) as u32;
        let mut text = crate::gt::clock_text(&format!("{{0:{}}}", self.format), day)
            .unwrap_or_else(|| crate::gt::clock_text("{0:HH:mm:ss}", day).unwrap_or_default());
        let hours = day / 3_600_000;
        if hours > 0 && (self.format.starts_with("mm:") || self.format == "mm") {
            let mut parts: Vec<String> = text.split(':').map(str::to_owned).collect();
            if let Some(minutes) = parts.first().and_then(|first| first.parse::<u32>().ok()) {
                parts[0] = format!("{:02}", minutes + hours * 60);
                text = parts.join(":");
            }
        }
        text
    }

    fn show(&mut self, ms: i64) { self.value = Some(self.text(ms)); }

    /// `Countdown::d` (0x06004ee1): stop the timer and stopwatch; the adjustment is cleared with the
    /// stopwatch. With `update` the field shows 0 (or Duration when counting up); a stop by hand
    /// with ReverseDisplay shows the other end.
    fn finish(&mut self, completed: bool, update: bool) {
        self.running = false;
        self.paused = false;
        if self.watch.take().is_some() { self.offset_ms = 0; }
        let mut full = self.reverse;
        if !completed && self.reverse_display { full = !full; }
        if update { self.show(if full { self.duration_ms } else { 0 }); }
    }

    /// StartCountdown (0x06004ee2): resumes a paused countdown, otherwise restarts it from the top.
    pub fn start(&mut self, now: f64) {
        if self.running {
            if self.paused {
                self.paused = false;
                if let Some((_, since)) = &mut self.watch { *since = Some(now); }
            }
            return;
        }
        self.finish(false, true);
        self.running = true;
        // The original's 250 ms timer fires at once and starts its stopwatch on that first tick.
        self.watch = Some((0.0, Some(now)));
        self.tick(now);
    }

    /// StopCountdown (0x06004edf): stop and reset what the field shows.
    pub fn stop(&mut self) { self.finish(false, true); }

    /// PauseCountdown (0x06004ee4): pause a running countdown, otherwise start or resume it.
    pub fn pause(&mut self, now: f64) {
        if self.running && !self.paused { self.hold(now); } else { self.start(now); }
    }

    /// SuspendCountdown (0x06004ee5): pause only — never starts.
    pub fn suspend(&mut self, now: f64) {
        if self.watch.is_some() && !self.paused { self.hold(now); }
    }

    fn hold(&mut self, now: f64) {
        self.paused = true;
        let counted = self.watch_ms(now);
        if self.watch.is_some() { self.watch = Some((counted, None)); }
    }

    /// AdjustCountdown (0x06004ee3): add `ms` (the shortcut's Value is whole seconds × 1000).
    pub fn adjust(&mut self, ms: i64, now: f64) {
        self.offset_ms += ms;
        self.redraw = true;
        if !self.running { let value = self.current_ms(now); self.show(value); }
    }

    /// ChangeCountdown (0x06004edd): move the current value to `target`, kept within 0…Duration.
    pub fn change(&mut self, target_ms: i64, now: f64) {
        let current = self.current_ms(now);
        let mut delta = target_ms - current;
        let over = current + delta - self.duration_ms;
        if over > 0 { delta -= over; } else if current + delta < 0 { delta -= current + delta; }
        self.adjust(delta, now);
    }

    /// SetCountdown: a new Duration (`ozzzzzi::zz` 0x06003913 sets CountdownSettings.Duration).
    pub fn set_duration(&mut self, ms: i64) { self.duration_ms = ms.max(0); }

    /// The original's 250 ms timer callback (`Countdown::a` 0x06004edb). Returns true when the
    /// countdown reaches 0 (or Duration when counting up) and stops.
    pub fn tick(&mut self, now: f64) -> bool {
        if !self.running || (self.paused && !self.redraw) { return false; }
        self.redraw = false;
        let value = self.current_ms(now);
        self.show(value);
        if (value <= 0 && !self.reverse) || (value >= self.duration_ms && self.reverse) {
            self.finish(true, true);
            return true;
        }
        false
    }
}

/// Reads a shortcut time such as `00:10:00`, `10:00`, `90` (seconds) or `1:30.5`.
pub fn parse_time(text: &str) -> Option<i64> {
    let mut total = 0.0_f64;
    let parts: Vec<&str> = text.trim().split(':').collect();
    if parts.is_empty() || parts.len() > 3 { return None; }
    for part in &parts {
        let value: f64 = part.trim().parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        total = total * 60.0 + value;
    }
    Some((total * 1000.0).round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ten_minutes() -> Countdown { Countdown { duration_ms: 600_000, ..Countdown::default() } }

    #[test]
    fn counts_down_shows_the_ceiling_second_and_stops_at_zero() {
        let mut countdown = ten_minutes();
        countdown.start(100.0);
        assert_eq!(countdown.value.as_deref(), Some("00:10:00"));
        countdown.tick(100.5);
        assert_eq!(countdown.value.as_deref(), Some("00:10:00"), "999 ms rounds the shown second up");
        countdown.tick(101.0);
        assert_eq!(countdown.value.as_deref(), Some("00:09:59"));
        assert!(!countdown.tick(700.5), "the shown second is rounded up, so 0 comes 999 ms after Duration");
        assert!(countdown.tick(701.0), "it completes at zero");
        assert_eq!(countdown.value.as_deref(), Some("00:00:00"));
        assert!(!countdown.is_running());
    }

    #[test]
    fn pause_toggles_suspend_only_pauses_and_start_resumes() {
        let mut countdown = ten_minutes();
        countdown.start(0.0);
        countdown.tick(10.0);
        countdown.pause(10.0);
        countdown.tick(50.0);
        assert_eq!(countdown.value.as_deref(), Some("00:09:50"), "paused at 10 s");
        countdown.pause(50.0);
        countdown.tick(55.0);
        assert_eq!(countdown.value.as_deref(), Some("00:09:45"));
        countdown.suspend(55.0);
        countdown.suspend(60.0);
        assert!(!countdown.is_running(), "suspend never resumes");
        countdown.start(70.0);
        countdown.tick(70.0);
        assert_eq!(countdown.value.as_deref(), Some("00:09:45"));
    }

    #[test]
    fn adjust_change_and_stop_follow_the_original() {
        let mut countdown = ten_minutes();
        countdown.start(0.0);
        countdown.adjust(-60_000, 0.0);
        countdown.tick(0.0);
        assert_eq!(countdown.value.as_deref(), Some("00:09:00"));
        countdown.change(30_000, 0.0);
        countdown.tick(0.0);
        assert_eq!(countdown.value.as_deref(), Some("00:00:30"));
        countdown.change(3_600_000, 0.0);
        countdown.tick(0.0);
        assert_eq!(countdown.value.as_deref(), Some("00:10:00"), "never above Duration");
        countdown.stop();
        assert_eq!(countdown.value.as_deref(), Some("00:00:00"));
        countdown.reverse_display = true;
        countdown.stop();
        assert_eq!(countdown.value.as_deref(), Some("00:10:00"));
    }

    #[test]
    fn counting_up_and_minute_formats() {
        let mut countdown = Countdown { duration_ms: 5_400_000, reverse: true, format: "mm:ss".into(), ..Countdown::default() };
        countdown.start(0.0);
        countdown.tick(4_500.0);
        assert_eq!(countdown.value.as_deref(), Some("75:00"), "minutes keep counting past an hour");
        assert!(countdown.tick(5_400.0));
        assert_eq!(countdown.value.as_deref(), Some("90:00"));
        assert_eq!(parse_time("00:10:00"), Some(600_000));
        assert_eq!(parse_time("1:30"), Some(90_000));
        assert_eq!(parse_time("45"), Some(45_000));
        assert_eq!(parse_time("x"), None);
    }
}
