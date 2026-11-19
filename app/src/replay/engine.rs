//! Replay recording, event marking and A/B playback driven from the update loop.
use crate::*;

fn replay_omt_source(s: &Switcher, source: u32) -> Option<&str> {
    if !s.replay_setup.compressed_omt || s.replay_setup.audio != 0 || source == SRC_OUTPUT || source == MV_AUTO {
        return None;
    }
    match &s.input(source.checked_sub(1)?)?.kind {
        Kind::Device { link, .. } => link.strip_prefix("omt:"),
        _ => None,
    }
}

/// Input whose capture thread records this replay angle (Replay setup: Record cameras from
/// capture). Program and inputs other than cameras are composed and read back.
fn replay_capture_input(s: &Switcher, source: u32, failed: &std::collections::HashSet<u32>) -> Option<u32> {
    if !s.replay_setup.direct_capture || matches!(source, SRC_OUTPUT | MV_AUTO) { return None; }
    let id = source.checked_sub(1)?;
    let input = s.input(id)?;
    (matches!(&input.kind, Kind::Device { link, .. } if !link.starts_with("audio:")) && !failed.contains(&id)).then_some(id)
}

impl App {
    pub(crate) fn indexed_command(&mut self, action: replay::Action) {
        if self.replay.folder.is_none() { return; }
        let inputs = self.s.ensure_replay_inputs();
        let frame = self.replay.frame_ms(self.s.cfg.frame_rate());
        let explicit = match action {
            replay::Action::Jog(channel, _) | replay::Action::Cue(channel, _) | replay::Action::CueOut(channel, _) => Some(channel),
            _ => None,
        };
        self.replay.queue = None;
        for channel in 0..2 {
            if explicit.map_or(self.s.replay_channel < 2 && self.s.replay_channel != channel, |target| target != channel)
                || self.replay.playback[channel].is_some() {
                continue;
            }
            let angle = self.s.replay_cameras[channel];
            let event_id = match action {
                replay::Action::Cue(_, event) | replay::Action::CueOut(_, event) => Some(event),
                _ => self.s.replay_event,
            };
            let event = event_id.and_then(|id| self.replay.session.find(id));
            let event = event.as_ref();
            let previous = self.replay.indexed[channel].as_ref().filter(|cursor| cursor.angle == angle);
            let mut range = previous.map_or_else(|| event.map(|event| (event.start, event.end)), |cursor| cursor.range);
            let folder = self.replay.folder.as_deref().unwrap();
            let tail = self.replay.session.segments.iter().rev().find(|segment| segment.direct_path(folder, angle).is_some());
            let position = previous.map(|cursor| {
                if cursor.direction == ReplayMotion::Forward {
                    if let Some(segment) = self.replay.session.segments.iter().find(|segment| {
                        cursor.file.is_some() && segment.files[angle] == cursor.file
                    }) {
                        let position = self.s.input(inputs[channel]).map_or(0.0, |input| input.pos);
                        return segment.show_time(angle, (f64::from(position) * 1000.0) as u64) as f64;
                    }
                }
                cursor.position
            }).or_else(|| {
                let input = self.s.inputs.iter().find(|input| matches!(input.kind, Kind::Replay { channel: c, .. } if c == channel))?;
                event.map(|event| event.start as f64 + f64::from(input.pos) * f64::from(self.s.replay_speed[channel]) * 10.0)
            });
            let (point, direction) = match action {
                replay::Action::Live => {
                    range = None;
                    (tail.map(|segment| segment.start as f64), ReplayMotion::Forward)
                }
                replay::Action::Seek(position) => {
                    range = None;
                    (Some(position as f64), ReplayMotion::Still)
                }
                replay::Action::Step(step) | replay::Action::Jog(_, step) => {
                    (position.map(|point| point + f64::from(step) * frame), ReplayMotion::Still)
                }
                replay::Action::Cue(_, _) | replay::Action::CueOut(_, _) => {
                    let Some(event) = event.filter(|event| event.angles[angle]) else {
                        self.s.replay_status = "Selected event has no recording for this angle".into();
                        continue;
                    };
                    range = Some((event.start, event.end));
                    let out = matches!(action, replay::Action::CueOut(_, _));
                    (Some(if out { event.end as f64 - frame } else { event.start as f64 }), ReplayMotion::Still)
                }
                replay::Action::Reverse if previous.is_some_and(|cursor| cursor.direction == ReplayMotion::Reverse) => (position, ReplayMotion::Still),
                replay::Action::Reverse => {
                    let point = position.filter(|point| range.is_none_or(|(start, _)| *point > start as f64 + frame))
                        .or_else(|| range.map(|(_, end)| end as f64 - frame));
                    (point, ReplayMotion::Reverse)
                }
                _ => return,
            };
            if let Some(position) = point.or_else(|| tail.map(|segment| segment.end as f64 - frame)) {
                let position = range.map_or(position, |(start, end)| position.clamp(start as f64, (end as f64 - frame).max(start as f64)));
                self.replay.indexed[channel] = Some(IndexedReplay {
                    angle, event: range.and_then(|_| match action {
                        replay::Action::Cue(_, _) | replay::Action::CueOut(_, _) => event.map(|event| event.id),
                        _ => previous.and_then(|cursor| cursor.event).or_else(|| event.map(|event| event.id)),
                    }), position, range, direction, paused: false, file: None, seek: None, due: 0.0, take: None, mix: None,
                });
                if explicit.is_none() && (self.s.replay_channel < 2 || channel == 0) && self.s.output != Some(inputs[channel]) {
                    self.s.select_preview(inputs[channel]);
                }
            } else {
                self.s.replay_status = if self.replay.session.native {
                    "No directly seekable camera file. Use Play to prepare a native MPEG event.".into()
                } else { "No finalized video for this camera".into() };
            }
        }
    }

    pub(crate) fn update_indexed_replay(&mut self, now: f64) {
        use std::sync::atomic::Ordering::Relaxed;
        let Some(folder) = self.replay.folder.as_deref() else { return };
        let frame = self.replay.frame_ms(self.s.cfg.frame_rate());
        let mut first_frames = Vec::new();
        for (channel, transport) in self.replay.indexed.iter_mut().enumerate() {
            let Some(cursor) = transport else { continue };
            if cursor.paused { continue; }
            let Some(input) = self.s.inputs.iter_mut().find(|input| matches!(input.kind,
                Kind::Replay { channel: current, .. } if current == channel)) else { continue };
            let source = self.media.get(&input.id).and_then(|feed| feed.shared.as_ref());
            if let Some((baseline, target, sent)) = cursor.seek {
                let done = source.is_some_and(|source| source.seq.load(Relaxed) != baseline
                    && source.seek_ms.load(Relaxed) == 0 && source.pos_ms.load(Relaxed).abs_diff(target) <= frame.ceil() as u64);
                if !done {
                    if now - sent > 3.0 {
                        cursor.seek = None;
                        cursor.direction = ReplayMotion::Still;
                        cursor.take = None;
                        self.s.replay_status = "Replay decoder could not seek to the requested frame".into();
                    }
                    continue;
                }
                cursor.seek = None;
                let stopped = self.s.replay_speed[channel] == 0;
                if let Some(program) = cursor.take.take() {
                    if !program || !stopped { first_frames.push((input.id, program, cursor.mix)); }
                }
                if cursor.direction != ReplayMotion::Reverse {
                    input.playing = cursor.direction == ReplayMotion::Forward && !stopped;
                    cursor.paused = cursor.direction == ReplayMotion::Forward && stopped;
                    continue;
                }
            } else if cursor.direction == ReplayMotion::Still && cursor.file.is_some() {
                continue;
            }
            if cursor.direction == ReplayMotion::Forward && cursor.file.is_some() {
                if let Some(segment) = self.replay.session.segments.iter().find(|segment| {
                    segment.files[cursor.angle].as_ref() == cursor.file.as_ref()
                }) {
                    if let Some(source) = source {
                        cursor.position = segment.show_time(cursor.angle, source.pos_ms.load(Relaxed)) as f64;
                        if cursor.range.is_some_and(|(_, end)| cursor.position >= end as f64) {
                            cursor.direction = ReplayMotion::Still;
                            input.playing = false;
                            continue;
                        }
                        if !source.ended.load(Relaxed) && cursor.position < segment.end as f64 { continue; }
                    }
                    cursor.position = segment.end as f64;
                }
            } else if cursor.direction == ReplayMotion::Reverse && cursor.file.is_some() {
                if now < cursor.due || self.s.replay_speed[channel] == 0 { continue; }
                let next = cursor.position - frame;
                if next < cursor.range.map_or(0.0, |(start, _)| start as f64) - 0.01 {
                    cursor.direction = ReplayMotion::Still;
                    continue;
                }
                cursor.position = next.max(0.0);
            }
            if cursor.direction == ReplayMotion::Forward && cursor.range.is_some_and(|(_, end)| cursor.position >= end as f64) {
                cursor.direction = ReplayMotion::Still;
                input.playing = false;
                continue;
            }
            let segments = &self.replay.session.segments;
            let has_angle = |segment: &&replay::Segment| segment.direct_path(folder, cursor.angle).is_some();
            let segment = segments.iter().filter(has_angle).find(|segment| {
                (segment.start as f64..segment.end as f64).contains(&cursor.position)
            }).or_else(|| {
                if cursor.direction == ReplayMotion::Forward {
                    segments.iter().filter(has_angle).find(|segment| segment.start as f64 >= cursor.position)
                } else {
                    segments.iter().filter(has_angle).rev().find(|segment| segment.end as f64 <= cursor.position + frame)
                }
            });
            let Some(segment) = segment else {
                if cursor.direction != ReplayMotion::Forward { cursor.direction = ReplayMotion::Still; }
                continue;
            };
            if cursor.direction == ReplayMotion::Forward && cursor.range.is_some_and(|(_, end)| segment.start >= end) {
                cursor.direction = ReplayMotion::Still;
                input.playing = false;
                continue;
            }
            let file = segment.files[cursor.angle].as_ref().unwrap();
            let Some(path) = segment.direct_path(folder, cursor.angle) else { continue };
            if cursor.direction == ReplayMotion::Forward && cursor.file.as_ref() == Some(file) { continue; }
            cursor.position = cursor.position.clamp(segment.start as f64, (segment.end as f64 - frame).max(segment.start as f64));
            let offset = segment.seek_time(cursor.angle, cursor.position as u64);
            let path = path.to_string_lossy().into_owned();
            let same_file = matches!(&input.kind, Kind::Replay { path: current, .. } if *current == path);
            let baseline = if same_file { source.map_or(0, |source| source.seq.load(Relaxed)) } else { 0 };
            input.kind = Kind::Replay { path, channel };
            input.playing = false;
            input.seek_ms = Some(offset);
            cursor.file = Some(file.clone());
            cursor.seek = Some((baseline, offset, now));
            cursor.due = now + frame / (10.0 * f64::from(self.s.replay_speed[channel].max(1)));
        }
        for (id, program, mix) in first_frames {
            if program {
                let duration = mix.unwrap_or(self.s.replay_setup.transition_ms);
                self.s.transition_input(id, if duration == 0 { Effect::Cut } else { Effect::Fade }, duration);
            } else {
                self.s.select_preview(id);
            }
            self.s.replay_status = "Replay playing directly from the recording".into();
        }
    }

    /// Cues a quick mark on its In Point as soon as the recording it needs is readable.
    /// While recording live the newest segment is still open, so this waits for it to close.
    fn cue_pending_mark(&mut self, now: f64) {
        let Some((channel, id, since)) = self.replay.pending_cue else { return };
        let Some(event) = self.replay.session.find(id) else {
            self.replay.pending_cue = None;
            return;
        };
        let ready_until = self.replay.session.segments.last().map_or(0, |segment| segment.end);
        if event.end <= ready_until {
            self.replay.pending_cue = None;
            // The player's camera may not exist in this event: use the first one that does.
            if !event.angles[self.s.replay_cameras[channel]] {
                if let Some(angle) = event.angles.iter().position(|&available| available) {
                    self.s.replay_cameras[channel] = angle;
                }
            }
            self.indexed_command(replay::Action::Cue(channel, id));
            self.s.replay_live = false;
        } else if now - since > 15.0 || (self.replay.current.is_empty() && self.replay.finalizing.is_empty()) {
            self.replay.pending_cue = None;
        }
    }

    /// Starts the next queued highlight once the queue's channel is idle.
    fn step_replay_queue(&mut self, now: f64) {
        use std::sync::atomic::Ordering::Relaxed;
        let Some(queue) = &mut self.replay.queue else { return };
        let channel = queue.channel;
        if self.replay.requested[channel].is_some() || self.replay.playback[channel].is_some() { return; }
        let input = self.s.inputs.iter().find(|input| matches!(input.kind, Kind::Replay { channel: c, .. } if c == channel));
        let ended = input.and_then(|input| self.media.get(&input.id)).and_then(|feed| feed.shared.as_ref())
            .is_some_and(|source| source.ended.load(Relaxed));
        let active = match &self.replay.indexed[channel] {
            Some(cursor) => cursor.direction == ReplayMotion::Forward || cursor.seek.is_some(),
            None => !ended && input.is_some_and(|input| {
                input.playing || !matches!(&input.kind, Kind::Replay { path, .. } if path.is_empty())
            }),
        };
        if queue.next > 0 && !queue.running {
            if active { queue.running = true; }
            else if now - queue.issued > 1.0 { self.replay.queue = None; }
            return;
        }
        if queue.next > 0 && active { return; }
        if queue.next >= queue.items.len() {
            if !queue.looped {
                self.replay.queue = None;
                return;
            }
            queue.next = 0;
        }
        let item = queue.items[queue.next];
        // A mix needs both players: the incoming clip starts on the other channel.
        let target = if queue.program && queue.next > 0 && item.mix_ms > 0 { 1 - channel } else { channel };
        let program = queue.program;
        queue.next += 1;
        queue.channel = target;
        queue.running = false;
        queue.issued = now;
        self.s.replay_cameras[target] = item.angle;
        self.s.replay_speed[target] = item.speed.clamp(1, 100);
        self.replay.requested[target] = Some(ReplayRequest { event: item.id, angle: item.angle,
            speed: item.speed.clamp(1, 100), program, route: true, mix: Some(item.mix_ms) });
        self.replay.playback[target] = None;
        self.replay.indexed[target] = None;
        self.replay.playing[target] = Some(item.id);
    }

    /// Copies transport state into the controller: positions, cues, tally and the live clock.
    fn publish_replay_state(&mut self, now: f64) {
        let inputs: [Option<&Input>; 2] = std::array::from_fn(|channel| self.s.inputs.iter()
            .find(|input| matches!(input.kind, Kind::Replay { channel: c, .. } if c == channel)));
        let frame = self.replay.frame_ms(self.s.cfg.frame_rate());
        let mut position = [None; 2];
        let mut cued = [false; 2];
        let mut active = [false; 2];
        let mut paused = [false; 2];
        let mut on_air = None;
        for channel in 0..2 {
            let cursor = self.replay.indexed[channel].as_ref();
            position[channel] = cursor.map(|cursor| cursor.position.max(0.0) as u64);
            // A player parked on the last frame of its highlight has nothing left to take.
            cued[channel] = cursor.is_some_and(|cursor| cursor.direction == ReplayMotion::Still && cursor.event.is_some()
                && cursor.range.is_some_and(|(_, end)| cursor.position + 2.0 * frame < end as f64));
            paused[channel] = cursor.is_some_and(|cursor| cursor.direction != ReplayMotion::Still && cursor.paused);
            active[channel] = inputs[channel].is_some_and(|input| input.playing)
                || cursor.is_some_and(|cursor| cursor.direction != ReplayMotion::Still && !cursor.paused);
            if inputs[channel].is_some_and(|input| self.s.output == Some(input.id)) && on_air.is_none() {
                on_air = self.replay.playing[channel];
            }
        }
        let panel = &mut self.s.replay_ui;
        (panel.position, panel.cued, panel.active, panel.on_air) = (position, cued, active, on_air);
        panel.paused = paused;
        panel.queue = self.replay.queue.is_some();
        panel.clock = (!self.replay.current.is_empty()).then(|| self.replay.position(now));
        panel.frame_ms = frame;
    }

    pub(crate) fn update_replay(&mut self, now: f64) {
        if let Some(new_session) = self.s.replay_load.take() {
            if self.s.replay_arm || !self.replay.current.is_empty() || !self.replay.finalizing.is_empty()
                || self.replay.playback.iter().any(Option::is_some) || self.replay.requested.iter().any(Option::is_some)
                || self.replay.export.is_some()
            {
                self.s.replay_status = "Stop recording and wait for replay jobs before changing sessions".into();
            } else {
                let folder = std::path::PathBuf::from(self.s.replay_setup.folder.trim());
                let opened = (|| -> Result<replay::Session, String> {
                    if folder.as_os_str().is_empty() { return Err("Select a Session Folder".into()); }
                    if new_session {
                        std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
                        if std::fs::read_dir(&folder).map_err(|error| error.to_string())?.next().is_some() {
                            return Err("Choose an empty folder for a new replay session".into());
                        }
                        let camera_names = self.s.replay_angles.map(|source| {
                            if source == SRC_OUTPUT { "Program".into() }
                            else { source.checked_sub(1).and_then(|id| self.s.input(id)).map(|input| input.title.clone()).unwrap_or_default() }
                        });
                        let session = replay::Session { setup: self.s.replay_setup.clone(), camera_names, ..Default::default() };
                        session.save(&folder)?;
                        Ok(session)
                    } else {
                        replay::Session::open(&folder)
                    }
                })();
                match opened {
                    Ok(session) => {
                        let selected_folder = folder.to_string_lossy().into_owned();
                        self.s.replay_setup = session.setup.clone();
                        self.s.replay_setup.folder = selected_folder.clone();
                        self.s.replay_session = selected_folder;
                        self.s.replay_read_only = session.native;
                        self.s.replay_live = new_session;
                        for (index, name) in session.camera_names.iter().enumerate() {
                            if name.is_empty() && !new_session { self.s.replay_angles[index] = MV_AUTO; continue; }
                            if name.is_empty() { continue; }
                            self.s.replay_angles[index] = if name == "Program" { SRC_OUTPUT } else {
                                self.s.inputs.iter().find(|input| input.title == *name).map_or(MV_AUTO, |input| input.id + 1)
                            };
                        }
                        self.s.replay_event = session.events.first().map(|event| event.id);
                        self.s.replay_events = session.events.clone();
                        let panel = &mut self.s.replay_ui;
                        *panel = replay::UiState { undocked: panel.undocked, monitors: panel.monitors,
                            playlists: session.playlists.clone(), tags: session.tags.clone(), ..Default::default() };
                        panel.tags.resize(replay::TAGS, String::new());
                        self.s.replay_status = if session.native && session.segments.is_empty() {
                            "Legacy event list loaded read-only. No compatible recording files found.".into()
                        } else if session.native {
                            "Legacy replay opened read-only".into()
                        } else { "Replay session ready".into() };
                        self.s.replay_setup_open = false;
                        self.s.ensure_replay_inputs();
                        for input in &mut self.s.inputs {
                            if let Kind::Replay { path, .. } = &mut input.kind {
                                path.clear();
                                input.playing = false;
                            }
                        }
                        self.replay = ReplayCapture { folder: Some(folder), session, ..Default::default() };
                    }
                    Err(error) => self.s.replay_status = error,
                }
            }
        }

        if matches!(self.s.replay_command, Some(replay::Action::Live | replay::Action::Step(_) | replay::Action::Reverse
            | replay::Action::Seek(_) | replay::Action::Jog(_, _) | replay::Action::Cue(_, _) | replay::Action::CueOut(_, _))) {
            let action = self.s.replay_command.take().unwrap();
            self.s.replay_live = false;
            self.indexed_command(action);
        }
        let replay = &mut self.replay;
        let Some(folder) = replay.folder.clone() else {
            self.s.replay_arm = false;
            self.s.replay_timeline = None;
            return;
        };
        if self.s.replay_session.is_empty() && !self.s.replay_arm && replay.finalizing.is_empty() && replay.current.is_empty() {
            self.replay = Default::default();
            return;
        }
        if replay.session.native { self.s.replay_arm = false; }
        let mut changed = false;
        if std::mem::take(&mut self.s.replay_ui.dirty) {
            let panel = &mut self.s.replay_ui;
            replay.session.merge(&self.s.replay_events, &mut panel.playlists, &panel.tags);
            changed = !replay.session.native;
        }
        let options = &mut replay.session.setup;
        let configured = &self.s.replay_setup;
        let options_changed = !replay.session.native
            && ((options.show_live_sources, options.advanced, options.play_to_program)
                != (configured.show_live_sources, configured.advanced, configured.play_to_program)
                || options.camera_folders != configured.camera_folders || options.compressed_omt != configured.compressed_omt
                || options.fragmented_mp4 != configured.fragmented_mp4 || options.resume_on_speed != configured.resume_on_speed
                || options.hardware_encoding != configured.hardware_encoding);
        if options_changed {
            options.show_live_sources = configured.show_live_sources;
            options.advanced = configured.advanced;
            options.play_to_program = configured.play_to_program;
            options.camera_folders.clone_from(&configured.camera_folders);
            options.compressed_omt = configured.compressed_omt;
            options.fragmented_mp4 = configured.fragmented_mp4;
            options.hardware_encoding = configured.hardware_encoding;
            options.resume_on_speed = configured.resume_on_speed;
        }
        if replay.finalizing.first().is_some_and(|segment| segment.tracks.iter().all(|track| track.worker.is_finished())) {
            let segment = replay.finalizing.remove(0);
            let mut retry_in_software = false;
            let mut files = std::array::from_fn(|_| None);
            let mut frames = std::array::from_fn(|_| Vec::new());
            let mut storage = std::array::from_fn(|_| None);
            for track in segment.tracks {
                let joined = track.worker.join().is_ok();
                let file = track.storage.as_deref().unwrap_or(&folder).join(&track.file);
                if joined && track.error.lock().unwrap().is_empty() && std::fs::metadata(&file).is_ok_and(|entry| entry.len() > 0) {
                    files[track.angle] = Some(track.file);
                    storage[track.angle] = track.storage;
                    if let Some(index) = track.index {
                        frames[track.angle] = index.lock().unwrap().drain(..).map(|(time, media_ms)| {
                            let offset = time.saturating_duration_since(segment.origin).as_millis() as u64;
                            replay::FrameStamp { show_ms: offset, media_ms }
                        }).collect();
                    }
                } else {
                    let _ = std::fs::remove_file(file);
                    let error = track.error.lock().unwrap();
                    retry_in_software |= segment.hardware_encoding && track.encoded && !error.is_empty();
                    self.s.replay_status = if error.is_empty() { format!("Camera {} failed to record", track.angle + 1) }
                        else { format!("Camera {}: {error}", track.angle + 1) };
                }
            }
            if files.iter().any(Option::is_some) && segment.end > segment.start {
                replay.session.segments.push(replay::Segment { start: segment.start, end: segment.end, files, frames, storage });
                replay.session.segments.sort_by_key(|segment| segment.start);
                changed = true;
            } else if !retry_in_software {
                self.s.replay_arm = false;
            }
            if retry_in_software && self.s.replay_arm && !replay.software_fallback {
                replay.software_fallback = true;
                self.s.replay_status.push_str(" · Replay switching to software encoding");
            }
        }

        let position = replay.position(now);
        if let Some(command) = self.s.replay_command.take() {
            let mark_time = if replay.session.native { None }
                else if self.s.replay_live { (!replay.current.is_empty()).then_some(position) }
                else {
                    let channel = if self.s.replay_channel == 1 { 1 } else { 0 };
                    replay.indexed[channel].as_ref().filter(|cursor| cursor.seek.is_none() && cursor.file.is_some())
                        .map(|cursor| cursor.position as u64)
                };
            match command {
                replay::Action::MarkIn | replay::Action::MarkOut | replay::Action::MarkLast(_) => {
                    if let Some(end) = mark_time {
                        let start = match command {
                            replay::Action::MarkIn => { replay.mark_in = Some(end); None }
                            replay::Action::MarkOut => replay.mark_in.take(),
                            replay::Action::MarkLast(ms) => {
                                let limit = if self.s.replay_live { replay.clock.map_or(end, |(_, start)| start) }
                                    else { replay.session.segments.first().map_or(end, |segment| segment.start) };
                                Some(end.saturating_sub(ms).max(limit))
                            }
                            _ => unreachable!(),
                        };
                        if let Some(start) = start.filter(|&start| start < end) {
                            let id = replay.mark_event(start, end, position);
                            self.s.replay_event = Some(id);
                            self.s.replay_status = format!("Event {id} marked");
                            if matches!(command, replay::Action::MarkLast(_)) {
                                // Quick replay: park the selected player on the new In Point.
                                replay.pending_cue = Some((self.s.replay_channel.min(1), id, now));
                            }
                            self.s.replay_events.clone_from(&replay.session.events);
                            changed |= !replay.provisional.contains(&id);
                        }
                    } else {
                        self.s.replay_status = "Record live or cue a recorded frame before marking".into();
                    }
                }
                replay::Action::CancelMark => replay.mark_in = None,
                replay::Action::SaveEvent(event) if !replay.session.native => {
                    if let Some(stored) = replay.session.events.iter_mut().find(|stored| stored.id == event.id) {
                        *stored = event;
                        changed = true;
                    }
                }
                replay::Action::DeleteEvent(id) if !replay.session.native => {
                    replay.session.events.retain(|event| event.id != id);
                    self.s.replay_event = replay.session.events.first().map(|event| event.id);
                    changed = true;
                }
                replay::Action::Play { event, program } => {
                    self.s.replay_live = false;
                    replay.queue = None;
                    replay.sync_playback = self.s.replay_channel == 2;
                    for channel in 0..2 {
                        if self.s.replay_channel < 2 && self.s.replay_channel != channel { continue; }
                        let speed = self.s.replay_speed[channel];
                        if speed == 0 { continue; }
                        let route = self.s.replay_channel < 2 || channel == 0;
                        replay.requested[channel] = Some(ReplayRequest {
                            event, angle: self.s.replay_cameras[channel], speed, program: program && route, route, mix: None,
                        });
                        replay.playing[channel] = Some(event);
                        replay.playback[channel] = None;
                        replay.indexed[channel] = None;
                    }
                }
                replay::Action::PlayQueue { items, channel, program, looped } if !items.is_empty() => {
                    self.s.replay_live = false;
                    replay.sync_playback = false;
                    replay.queue = Some(ReplayQueue { items, next: 0, channel: channel.min(1), program, looped,
                        running: true, issued: now });
                }
                replay::Action::TakeCue(channel, program) if channel < 2 && replay.playback[channel].is_none() => {
                    replay.queue = None;
                    let speed = self.s.replay_speed[channel];
                    let cue = replay.indexed[channel].as_ref().filter(|cursor| {
                        cursor.direction == ReplayMotion::Still && cursor.seek.is_none() && cursor.file.is_some()
                            && cursor.angle == self.s.replay_cameras[channel]
                    });
                    let result = cue.and_then(|cursor| cursor.event.map(|event| (event, cursor.position as u64)))
                        .ok_or_else(|| "Cue and preview the replay angle first".to_owned())
                        .and_then(|(event, start)| {
                            if speed == 0 { return Err("Set a replay speed above zero".into()); }
                            if speed == 100 && !replay.session.native { return Ok((event, None)); }
                            replay.job(event, self.s.replay_cameras[channel], speed, false, Some(start))
                                .map(|job| (event, Some(job)))
                        });
                    match result {
                        Ok((event, job)) => {
                            replay.sync_playback = false;
                            replay.playing[channel] = Some(event);
                            self.s.replay_live = false;
                            if let Some(job) = job {
                                replay.indexed[channel] = None;
                                replay.playback[channel] = Some(ReplayPlayback { job, ready: None, program, speed, mix: None });
                                self.s.replay_status = "Preparing cued replay…".into();
                            } else if let Some(cursor) = &mut replay.indexed[channel] {
                                // Normal speed plays the recording from the cued frame.
                                cursor.direction = ReplayMotion::Forward;
                                cursor.paused = false;
                                cursor.file = None;
                                cursor.take = Some(program);
                            }
                        }
                        Err(error) => self.s.replay_status = error,
                    }
                }
                replay::Action::Stop(channel) => {
                    let selected = |target: usize| channel == 2 || target == channel;
                    replay.queue = None;
                    for target in (0..2).filter(|&target| selected(target)) {
                        replay.playing[target] = None;
                        replay.playback[target] = None;
                        replay.requested[target] = None;
                        replay.indexed[target] = None;
                    }
                    for input in &mut self.s.inputs {
                        if matches!(input.kind, Kind::Replay { channel, .. } if selected(channel)) {
                            input.playing = false;
                        }
                    }
                    self.s.replay_status = "Replay stopped".into();
                }
                replay::Action::Pause(channel) | replay::Action::SetPlaying(channel, _) => {
                    let selected = |target: usize| channel == 2 || target == channel;
                    let resume = match command {
                        replay::Action::SetPlaying(_, playing) => playing,
                        _ => !replay.indexed.iter().enumerate().any(|(index, cursor)| selected(index)
                            && cursor.as_ref().is_some_and(|cursor| cursor.direction != ReplayMotion::Still && !cursor.paused))
                            && !self.s.inputs.iter().any(|input| matches!(input.kind, Kind::Replay { channel, .. }
                                if selected(channel)) && input.playing),
                    };
                    for (index, cursor) in replay.indexed.iter_mut().enumerate() {
                        if selected(index) {
                            replay.paused_by_speed[index] = false;
                            if let Some(cursor) = cursor { cursor.paused = !resume; }
                        }
                    }
                    for input in &mut self.s.inputs {
                        if let Kind::Replay { channel, .. } = input.kind {
                            if selected(channel) {
                                input.playing = resume && replay.indexed[channel].as_ref()
                                    .is_none_or(|cursor| cursor.direction == ReplayMotion::Forward && cursor.seek.is_none());
                            }
                        }
                    }
                }
                replay::Action::Export { event, angle } if replay.export.is_none() => match replay.job(event, angle, 100, true, None) {
                    Ok(job) => { replay.export = Some(job); self.s.replay_status = "Exporting replay…".into(); }
                    Err(error) => self.s.replay_status = error,
                },
                _ => {}
            }
        }

        self.step_replay_queue(now);
        let replay = &mut self.replay;
        let ready_until = replay.session.segments.last().map_or(0, |segment| segment.end);
        let mut rotate_for_play = false;
        for channel in 0..2 {
            let Some(request) = replay.requested[channel] else { continue };
            let event = replay.session.find(request.event)
                .filter(|event| event.angles.get(request.angle) == Some(&true));
            let Some(event) = event else {
                replay.requested[channel] = None;
                self.s.replay_status = "Select an angle available in this event".into();
                continue;
            };
            let range = (event.start, event.end);
            let opening = replay.session.segments.iter().find(|segment| {
                segment.start <= event.start && event.start < segment.end
                    && segment.direct_path(&folder, request.angle).is_some()
            });
            let active_segment = replay.active_readable_segment(request.angle, range.1).is_some();
            let waiting_for_active = replay.current.iter().any(|track| track.angle == request.angle && track.readable_while_recording()
                && replay.position(track.started) < range.1
                && track.error.try_lock().is_ok_and(|error| error.is_empty()));
            if request.speed == 100 && !replay.sync_playback && !replay.session.native && opening.is_some()
                && (range.1 <= ready_until || !waiting_for_active)
            {
                self.s.ensure_replay_inputs();
                replay.indexed[channel] = Some(IndexedReplay {
                    angle: request.angle, event: Some(request.event), position: range.0 as f64,
                    range: Some(range), direction: ReplayMotion::Forward, paused: false,
                    file: None, seek: None, due: 0.0, take: request.route.then_some(request.program),
                    mix: request.mix,
                });
                replay.requested[channel] = None;
                rotate_for_play |= range.1 > ready_until && replay.current.first()
                    .is_some_and(|track| replay.position(track.started) < range.1);
            } else if range.1 <= ready_until || active_segment {
                replay.requested[channel] = None;
                match replay.job(request.event, request.angle, request.speed, false, None) {
                    Ok(job) => {
                        replay.playback[channel] = Some(ReplayPlayback {
                            job, ready: None, program: request.program, speed: request.speed, mix: request.mix,
                        });
                    }
                    Err(error) => self.s.replay_status = error,
                }
            } else if waiting_for_active {
                self.s.replay_status = "Waiting for recorded frames after the Out Point…".into();
            } else if !replay.current.is_empty() || !replay.finalizing.is_empty() {
                rotate_for_play |= replay.current.first().is_some_and(|track| replay.position(track.started) < range.1);
                self.s.replay_status = "Opening the recorded camera segment…".into();
            } else {
                replay.requested[channel] = None;
                self.s.replay_status = "Recording for this event is unavailable".into();
            }
        }

        for job in replay.playback.iter_mut().flatten() {
            if job.ready.is_none() {
                job.ready = poll_replay_job(&job.job);
            }
        }
        let apply_playback = !replay.sync_playback || replay.playback.iter().flatten().all(|job| job.ready.is_some());
        for channel in 0..2 {
            let result = replay.playback[channel].as_mut().and_then(|job| {
                if !apply_playback { return None; }
                job.ready.take().map(|result| (result, job.program, job.speed, job.mix))
            });
            if let Some((result, program, speed, mix)) = result {
                replay.playback[channel] = None;
                match result {
                    Ok(path) => {
                        let id = self.s.ensure_replay_inputs()[channel];
                        let playing = self.s.replay_speed[channel] != 0;
                        if let Some(input) = self.s.input_mut(id) {
                            input.kind = Kind::Replay { path: path.to_string_lossy().into_owned(), channel };
                            input.playing = playing;
                            input.pos = 0.0;
                        }
                        if program && self.s.replay_speed[channel] != 0 {
                            let mut transition = mix.unwrap_or(self.s.replay_setup.transition_ms);
                            if mix.is_none() && self.s.replay_setup.match_speed {
                                transition = transition.saturating_mul(100) / u32::from(speed.max(1));
                            }
                            self.s.transition_input(id, if transition == 0 { Effect::Cut } else { Effect::Fade }, transition);
                        } else if !replay.sync_playback || channel == 0 {
                            self.s.select_preview(id);
                        }
                        self.s.replay_status = format!("Replay {} playing", ["A", "B"][channel]);
                    }
                    Err(error) => self.s.replay_status = error,
                }
            }
        }
        if let Some(result) = replay.export.as_ref().and_then(poll_replay_job) {
            replay.export = None;
            self.s.replay_status = match result {
                Ok(path) => format!("Exported to {}", path.display()),
                Err(error) => error,
            };
        }

        if self.s.replay_arm && self.program.lock().unwrap().error.is_some()
            && self.s.replay_angles.iter().any(|&source| !matches!(source, SRC_OUTPUT | MV_AUTO)
                && replay_omt_source(&self.s, source).is_none())
        {
            self.s.replay_arm = false;
            self.s.replay_status = "Multicamera replay requires offscreen capture".into();
        }
        if !replay.current.is_empty() && (!self.s.replay_arm || (
            replay.finalizing.len() < 2
                && (rotate_for_play || (replay.current_uses_hardware && replay.software_fallback)
                    || (replay.current_uses_hardware && replay.current.iter().any(|track|
                        track.recording.is_some() && track.worker.is_finished()))
                    || now - replay.current[0].started >= replay::SEGMENT_MS as f64 / 1000.0)))
        {
            let start = replay.position(replay.current[0].started);
            let origin = replay.current[0].origin;
            let tracks = replay.current.drain(..).map(|track| {
                drop(track.direct);
                FinalizingTrack { angle: track.angle, encoded: track.recording.is_some(), file: track.file, storage: track.storage,
                    error: track.error, index: track.index, worker: track.worker }
            }).collect();
            replay.finalizing.push(FinalizingReplay { origin, start, end: position, tracks,
                hardware_encoding: replay.current_uses_hardware });
        }

        if self.s.replay_arm && replay.clock.is_none() {
            let setup = &self.s.replay_setup;
            let format = replay::Format {
                size: setup.size.unwrap_or(self.record_format.0),
                fps: setup.fps.unwrap_or(self.record_format.1 as f32),
                codec: media::RecordFormat::parse(&setup.codec).key().to_owned(),
            };
            if replay.session.format.as_ref().is_some_and(|old| *old != format) {
                self.s.replay_arm = false;
                self.s.replay_status = "Recording format differs from this session. Restore it or create a new session.".into();
            } else {
                replay.session.format = Some(format);
            }
        }
        if self.s.replay_arm && replay.current.is_empty() {
            if replay.clock.is_none() {
                let last = replay.session.segments.last().map_or(0, |segment| segment.end);
                replay.clock = Some((now, last));
                self.s.replay_live = true;
                replay.session.setup = self.s.replay_setup.clone();
                replay.session.camera_names = self.s.replay_angles.map(|source| {
                    if source == SRC_OUTPUT { "Program".into() }
                    else { source.checked_sub(1).and_then(|id| self.s.input(id)).map(|input| input.title.clone()).unwrap_or_default() }
                });
                changed = true;
            }
            let stamp = replay_stamp();
            let setup = &self.s.replay_setup;
            let [w, h] = setup.size.unwrap_or(self.record_format.0);
            let fps = setup.fps.unwrap_or(self.record_format.1 as f32);
            replay.current_uses_hardware = setup.hardware_encoding && !replay.software_fallback;
            let options = media::RecOpts { mbps: setup.mbps.clamp(1, 100), size: Some((w, h)),
                fps, hw: replay.current_uses_hardware, format: media::RecordFormat::parse(&setup.codec), audio: true };
            let origin = Instant::now();
            let mut seen = std::collections::HashSet::new();
            let mut unavailable = Vec::new();
            for (angle, source) in self.s.replay_angles.into_iter().enumerate() {
                if source == MV_AUTO || !seen.insert(source) { continue; }
                if source != SRC_OUTPUT && !self.s.input(source - 1).is_some_and(|input| !matches!(input.kind, Kind::Replay { .. })) { continue; }
                let omt_source = replay_omt_source(&self.s, source);
                let extension = if omt_source.is_some() { "mkv" }
                    else if setup.fragmented_mp4 && matches!(options.format, media::RecordFormat::H264 | media::RecordFormat::Hevc) { "fmp4.mp4" }
                    else { "mp4" };
                let file = format!("camera-{}-{stamp}.{extension}", angle + 1);
                let storage = replay::camera_storage(&setup.camera_folders[angle]).unwrap_or_else(|error| {
                    unavailable.push(format!("{} ({error})", angle + 1));
                    None
                });
                if let Some(address) = omt_source {
                    let path = storage.as_deref().unwrap_or(&folder).join(&file);
                    let (direct, error, index, worker) = omt::record_compressed(address.to_owned(), path, origin, true, true);
                    replay.current.push(ReplayTrack { recording: None, direct: Some(direct), started: now,
                        error, index, origin, angle, file, storage, worker });
                    continue;
                }
                let route = OutputRoute { source, overlays: source == SRC_OUTPUT };
                let (recorder, worker) = media::Recorder::spawn_writer_indexed(
                    storage.as_deref().unwrap_or(&folder).join(&file).to_string_lossy().into_owned(), options, true);
                let recording = ComposedRecording { recorder: Arc::new(recorder), channel: 0,
                    route_override: Some(route), audio_override: match setup.audio {
                        0 => None,
                        1 => Some(SRC_OUTPUT),
                        n => self.s.replay_angles.get(usize::from(n - 2)).copied().filter(|&source| source != MV_AUTO),
                    }, capture: replay_capture_input(&self.s, source, &replay.capture_failed), started: now };
                let error = recording.recorder.error.clone();
                let index = recording.recorder.replay_index.clone();
                replay.current.push(ReplayTrack { recording: Some(recording), direct: None, started: now,
                    error, index, origin, angle, file, storage, worker });
            }
            if replay.current.is_empty() {
                self.s.replay_arm = false;
                self.s.replay_status = "Select at least one available camera".into();
            } else if !unavailable.is_empty() {
                self.s.replay_status = format!("Using session drive for camera(s) {}", unavailable.join(", "));
            } else if self.s.replay_status == "Replay session ready" || self.s.replay_status.starts_with("Using session drive") {
                self.s.replay_status = format!("Recording {} camera(s)", replay.current.len());
            }
        }
        // Cameras recorded from their capture threads: follow the input's current reader (a
        // camera that reopens gets a new one) and compose an angle whose source sends nothing.
        for track in &mut replay.current {
            let Some(recording) = &mut track.recording else { continue };
            let Some(input) = recording.capture else { continue };
            let shared = self.media.get(&input).and_then(|feed| feed.shared.as_ref());
            let receiving = track.index.as_ref().is_some_and(|index| !index.lock().unwrap().is_empty());
            if !receiving && now - track.started > 2.0 {
                if let Some(shared) = shared { shared.remove_replay_tap(&recording.recorder); }
                recording.capture = None;
                replay.capture_failed.insert(input);
                self.s.replay_status = format!("Camera {} is recorded from the Program renderer: its source sent no pictures",
                    track.angle + 1);
                continue;
            }
            if let Some(shared) = shared { shared.add_replay_tap(&recording.recorder); }
        }
        if !self.s.replay_arm {
            if replay.current.is_empty() { replay.capture_failed.clear(); }
            if let Some(start) = self.s.replay_live.then(|| replay.mark_in.take()).flatten().filter(|start| *start < position) {
                if !replay.session.native {
                    let id = replay.mark_event(start, position, position);
                    self.s.replay_event = Some(id);
                    changed = true;
                }
            }
            if replay.finalizing.is_empty() { replay.clock = None; }
        }

        let ready_until = replay.session.segments.last().map_or(0, |segment| segment.end);
        replay.provisional.retain(|&id| {
            let Some(event) = replay.session.events.iter_mut().find(|event| event.id == id) else { return false };
            if event.end > ready_until && (!replay.current.is_empty() || !replay.finalizing.is_empty()) {
                return true;
            }
            for (angle, enabled) in event.angles.iter_mut().enumerate() {
                *enabled &= replay.session.segments.iter().any(|segment| {
                    segment.end > event.start && segment.start < event.end && segment.files[angle].is_some()
                });
            }
            changed = true;
            false
        });
        if changed && self.s.replay_setup.delete_old && replay.export.is_none() && replay.playback.iter().all(Option::is_none) {
            let mark = replay.mark_in.into_iter()
                .chain(replay.indexed.iter().flatten().map(|cursor| cursor.position as u64)).min();
            replay.session.prune(&folder, ready_until.saturating_sub(self.s.replay_setup.buffer_seconds * 1000), mark);
        }
        if changed { self.s.replay_events = replay.session.events.clone(); }
        if changed || options_changed {
            if let Err(error) = replay.save() {
                self.s.replay_status = format!("Could not save replay session: {error}");
            }
        }
        self.s.replay_marking = replay.mark_in.is_some();
        for channel in 0..2 {
            if self.s.replay_speed[channel] != 0 {
                if std::mem::take(&mut replay.paused_by_speed[channel]) && self.s.replay_setup.resume_on_speed {
                    if let Some(cursor) = &mut replay.indexed[channel] {
                        if cursor.direction != ReplayMotion::Still && cursor.take.is_none() {
                            cursor.paused = false;
                            for input in &mut self.s.inputs {
                                if matches!(input.kind, Kind::Replay { channel: target, .. } if target == channel) {
                                    input.playing = cursor.direction == ReplayMotion::Forward && cursor.seek.is_none();
                                }
                            }
                        }
                    }
                }
                continue;
            }
            if let Some(cursor) = &mut replay.indexed[channel] {
                if cursor.direction != ReplayMotion::Still && cursor.take.is_none() && !cursor.paused {
                    cursor.paused = true;
                    replay.paused_by_speed[channel] = true;
                }
            }
            for input in &mut self.s.inputs {
                if matches!(input.kind, Kind::Replay { channel: target, .. } if target == channel) {
                    input.playing = false;
                }
            }
        }
        self.cue_pending_mark(now);
        self.update_indexed_replay(now);
        self.publish_replay_state(now);
        let channel = usize::from(self.s.replay_channel == 1);
        let angle = self.s.replay_cameras[channel];
        let mut segments = self.replay.session.segments.iter().filter(|segment| segment.files[angle].is_some());
        self.s.replay_timeline = segments.next().map(|first| {
            let end = segments.next_back().map_or(first.end, |last| last.end).saturating_sub(1).max(first.start);
            let position = self.replay.indexed[channel].as_ref().filter(|cursor| cursor.angle == angle)
                .map_or(end, |cursor| cursor.position as u64).clamp(first.start, end);
            (first.start, end, position)
        });
    }
}

#[test]
fn replay_keeps_recording_after_hardware_segment_fails() {
    let mut app = App::new(None);
    let folder = std::env::temp_dir().join(format!("livemix-replay-recovery-{}", replay_stamp()));
    app.s.replay_setup.folder = folder.to_string_lossy().into_owned();
    app.s.replay_setup.hardware_encoding = true;
    app.s.replay_load = Some(true);
    app.update_replay(0.0);
    app.s.replay_arm = true;
    let worker = std::thread::spawn(|| {});
    while !worker.is_finished() {
        std::thread::yield_now();
    }
    app.replay.finalizing.push(FinalizingReplay {
        origin: Instant::now(), start: 0, end: 100, hardware_encoding: true,
        tracks: vec![FinalizingTrack {
            angle: 0, encoded: true, file: "failed.mp4".into(), storage: None,
            error: Arc::new(Mutex::new("Recording GPU failure (0x887A0005)".into())),
            index: None, worker,
        }],
    });
    app.update_replay(0.1);
    assert!(app.s.replay_arm);
    assert!(app.replay.software_fallback);
    assert!(!app.replay.current_uses_hardware);
    assert_eq!(app.replay.current.len(), 1);
    assert!(app.s.replay_status.contains("switching to software"));
    drop(app);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
#[ignore = "requires FFmpeg/FFprobe with libx264 and libx265"]
fn replay_reads_fragmented_mp4_while_writer_is_open() {
    for codec in ["h264", "hevc"] {
        let mut app = App::new(None);
        app.s.cfg = Settings::default();
        app.s.cfg.hw_encoder = false;
        app.s.replay_setup.fragmented_mp4 = true;
        app.s.replay_setup.codec = codec.into();
        let folder = std::env::temp_dir().join(format!("livemix-active-mp4-test-{}", replay_stamp()));
        app.s.replay_setup.folder = folder.to_string_lossy().into_owned();
        app.s.replay_load = Some(true);
        app.record_format = ([320, 180], 25.0);
        app.update_replay(0.0);
        app.s.replay_arm = true;
        app.update_replay(0.0);
        assert_eq!(app.replay.current.len(), 1);
        let track = &app.replay.current[0];
        let name = track.file.clone();
        let index = track.index.clone().unwrap();
        let origin = track.origin;
        let rec = track.recording.as_ref().unwrap().recorder.clone();
        assert!(replay::fragmented_mp4(&name));
        for frame in 0..40 {
            rec.push_audio([100, 25, 230, 255].repeat(320 * 180), 320, 180,
                origin + std::time::Duration::from_millis(frame * 40), audio::test_tone(1920));
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        drop(rec);
        let deadline = Instant::now() + std::time::Duration::from_secs(3);
        while index.lock().unwrap().last().is_none_or(|entry| entry.1 < 1200) && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let event = app.replay.mark_event(200, 880, 1560);
        app.s.replay_command = Some(replay::Action::Play { event, program: false });
        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        while Instant::now() < deadline {
            app.update_replay(1.56);
            if app.s.inputs.iter().any(|input| matches!(&input.kind, Kind::Replay { path, .. } if path.ends_with(".mp4") && !path.ends_with(&name))) { break; }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(app.s.inputs.iter().any(|input| matches!(&input.kind, Kind::Replay { path, .. } if path.ends_with(".mp4") && !path.ends_with(&name))),
            "{codec}: {}", app.s.replay_status);
        assert_eq!(app.replay.current[0].file, name, "Play rotated the live segment");
        assert!(!app.replay.current[0].worker.is_finished());
        let output = app.s.inputs.iter().find_map(|input| match &input.kind {
            Kind::Replay { path, .. } if path.ends_with(".mp4") => Some(path),
            _ => None,
        }).unwrap();
        let decoded = crate::stream::command().args(["-v", "error", "-xerror", "-i", output, "-map", "0:v:0",
            "-map", "0:a:0", "-f", "null", "-"]).output().unwrap();
        assert!(decoded.status.success(), "{}", String::from_utf8_lossy(&decoded.stderr));
        app.s.replay_arm = false;
        app.update_replay(2.0);
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while !app.replay.finalizing.is_empty() && Instant::now() < deadline {
            app.update_replay(2.0);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(app.replay.finalizing.is_empty());
        std::fs::remove_dir_all(folder).unwrap();
    }
}

#[cfg(all(test, windows))]
#[test]
fn replay_marks_without_rotating_records_and_reopens_both_channels() {
    let mut app = App::new(None);
    app.s.cfg = Settings::default();
    app.s.cfg.hw_encoder = false;
    let folder = std::env::temp_dir().join(format!("livemix-replay-test-{}", replay_stamp()));
    let second_disk = std::env::temp_dir().join(format!("livemix-replay-storage-test-{}", replay_stamp()));
    std::fs::create_dir(&second_disk).unwrap();
    app.s.replay_setup.folder = folder.to_string_lossy().into_owned();
    app.s.replay_setup.camera_folders[1] = second_disk.to_string_lossy().into_owned();
    app.s.replay_load = Some(true);
    app.record_format = ([320, 180], 25.0);
    app.s.replay_angles[1] = app.s.inputs[0].id + 1;
    app.update_replay(0.0);
    app.s.replay_arm = true;
    app.update_replay(0.0);
    assert_eq!(app.replay.current.len(), 2);
    let original = app.replay.current[0].file.clone();
    let start = app.replay.current[0].origin;
    for index in 0..20 {
        for track in &app.replay.current {
            if track.angle == 1 && (5..8).contains(&index) { continue; }
            track.recording.as_ref().unwrap().recorder.push_audio([220, 60, 30, 255].repeat(320 * 180), 320, 180,
                start + std::time::Duration::from_millis(index * 40), audio::test_tone(1920));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    app.s.replay_command = Some(replay::Action::MarkIn);
    app.update_replay(0.08);
    app.s.replay_command = Some(replay::Action::MarkOut);
    app.update_replay(0.68);
    assert_eq!(app.replay.current[0].file, original);
    assert_eq!(app.s.replay_events.len(), 1, "Mark Out must publish the event before rotation");
    let id = app.s.replay_events[0].id;
    app.s.replay_command = Some(replay::Action::Play { event: id, program: false });
    app.update_replay(0.76);
    assert_ne!(app.replay.current[0].file, original, "Play must close the active tail on demand");
    assert!(app.s.replay_arm, "Replay recording must continue after Play");
    let deadline = Instant::now() + std::time::Duration::from_secs(8);
    while app.replay.indexed[0].is_none() && Instant::now() < deadline {
        app.update_replay(0.76);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(app.replay.indexed[0].is_some(), "{}", app.s.replay_status);
    assert!(app.replay.playback[0].is_none(), "Normal-speed playback must skip the render job");
    let next_recording = app.replay.current[0].origin;
    for index in 0..3 {
        for track in &app.replay.current {
            track.recording.as_ref().unwrap().recorder.push_audio([220, 60, 30, 255].repeat(320 * 180), 320, 180,
                next_recording + std::time::Duration::from_millis(index * 40), audio::test_tone(1920));
        }
    }
    app.s.replay_arm = false;
    app.update_replay(0.9);
    let deadline = Instant::now() + std::time::Duration::from_secs(8);
    while !app.replay.finalizing.is_empty() && Instant::now() < deadline {
        app.update_replay(0.8);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let segment = app.replay.session.segments[0].clone();
    assert!(!segment.frames[0].is_empty() && !segment.frames[1].is_empty());
    assert!(segment.storage[0].is_none());
    let external = segment.path(&folder, 1).unwrap();
    assert_eq!(external.parent(), replay::camera_storage(&second_disk.to_string_lossy()).unwrap().as_deref());
    assert!(external.is_file());
    app.s.replay_setup.camera_folders[1].clear();
    app.update_replay(0.9);
    let reopened = replay::Session::open(&folder).unwrap();
    assert!(reopened.setup.camera_folders[1].is_empty());
    assert_eq!(reopened.segments[0].path(&folder, 1).unwrap(), external);
    assert!(!segment.frames[1].iter().any(|frame| (200..320).contains(&frame.show_ms)));
    assert!(segment.seek_time(1, segment.start + 280) < segment.seek_time(1, segment.start + 400));
    let event = &app.s.replay_events[0];
    assert_eq!((event.start, event.end), (80, 680));
    assert_eq!(event.angles[..2], [true, true]);
    let id = event.id;
    app.s.replay_load = Some(false);
    app.update_replay(0.8);
    assert_eq!(app.s.replay_event, Some(id));
    if crate::stream::command().arg("-version").output().is_ok_and(|output| output.status.success()) {
        app.s.replay_channel = 2;
        app.s.replay_cameras = [0, 1];
        app.s.replay_speed = [100, 50];
        app.s.replay_command = Some(replay::Action::Play { event: id, program: false });
        app.update_replay(0.8);
        let deadline = Instant::now() + std::time::Duration::from_secs(12);
        while app.replay.playback.iter().any(Option::is_some) && Instant::now() < deadline {
            app.update_replay(0.8);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        for channel in 0..2 {
            assert!(app.s.inputs.iter().any(|input| {
                matches!(&input.kind, Kind::Replay { channel: c, path } if *c == channel
                    && std::fs::metadata(path).is_ok_and(|entry| entry.len() > 1000))
            }), "{}", app.s.replay_status);
        }
    }
    let tail = app.replay.session.segments.last().unwrap().files[0].clone().unwrap();
    app.s.replay_channel = 0;
    app.s.replay_command = Some(replay::Action::Live);
    app.update_replay(0.8);
    let id = app.s.preview.unwrap();
    assert!(matches!(&app.s.input(id).unwrap().kind, Kind::Replay { channel: 0, path } if path.ends_with(&tail)));
    assert_eq!(app.s.input(id).unwrap().seek_ms, Some(0));
    app.s.replay_command = Some(replay::Action::Step(1));
    app.update_replay(0.81);
    assert_eq!(app.s.input(id).unwrap().seek_ms, Some(40));
    app.s.replay_command = Some(replay::Action::Reverse);
    app.update_replay(0.82);
    assert!(matches!(app.replay.indexed[0].as_ref().unwrap().direction, ReplayMotion::Reverse));
    // A quick mark parks the selected player on its In Point once its recording is readable.
    let marked = app.s.replay_events[0].clone();
    app.s.replay_cameras[0] = 5; // an angle this event does not have
    app.replay.pending_cue = Some((0, marked.id, 0.82));
    app.update_replay(0.83);
    assert!(app.replay.pending_cue.is_none(), "{}", app.s.replay_status);
    let cursor = app.replay.indexed[0].as_ref().unwrap();
    assert_eq!(cursor.event, Some(marked.id));
    assert!(cursor.direction == ReplayMotion::Still, "the cue must be parked, not playing");
    assert_eq!(cursor.position, marked.start as f64);
    assert!(marked.angles[app.s.replay_cameras[0]], "the player must move to an angle the event has");
    // A mark whose event disappeared, or that never becomes readable, stops waiting.
    app.replay.pending_cue = Some((0, u64::MAX, 0.83));
    app.update_replay(0.84);
    assert!(app.replay.pending_cue.is_none());
    std::fs::remove_dir_all(folder).unwrap();
    std::fs::remove_dir_all(second_disk).unwrap();
}

#[test]
fn replay_camera_from_capture_falls_back_to_the_renderer_without_pictures() {
    let mut app = App::new(None);
    app.s.cfg = Settings::default();
    app.s.cfg.hw_encoder = false;
    let folder = std::env::temp_dir().join(format!("livemix-capture-fallback-{}", replay_stamp()));
    app.s.replay_setup.folder = folder.to_string_lossy().into_owned();
    app.s.replay_setup.direct_capture = true;
    app.s.replay_load = Some(true);
    app.record_format = ([320, 180], 25.0);
    let camera = app.s.add(Input::new(0, "Camera", Kind::Device { link: "test:camera".into(), blackmagic: false }));
    let mic = app.s.add(Input::new(0, "Mic", Kind::Device { link: "audio:test".into(), blackmagic: false }));
    app.update_replay(0.0);
    app.s.replay_angles = [MV_AUTO; 8];
    app.s.replay_angles[0] = camera + 1;
    app.s.replay_angles[1] = mic + 1;
    app.s.replay_angles[2] = SRC_OUTPUT;
    app.s.replay_arm = true;
    app.update_replay(0.0);
    let capture = |app: &App| app.replay.current.iter()
        .map(|track| track.recording.as_ref().and_then(|recording| recording.capture)).collect::<Vec<_>>();
    // Only the camera is recorded from its capture thread; audio devices and Program are composed.
    assert_eq!(capture(&app), [Some(camera), None, None]);
    app.update_replay(1.0);
    assert_eq!(capture(&app)[0], Some(camera));
    // No picture arrived: the angle is composed for the rest of the session.
    app.update_replay(2.5);
    assert_eq!(capture(&app)[0], None);
    assert!(app.replay.capture_failed.contains(&camera));
    assert!(app.s.replay_status.contains("Camera 1"), "{}", app.s.replay_status);
    app.s.replay_arm = false;
    app.update_replay(3.0);
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while !app.replay.finalizing.is_empty() && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        app.update_replay(3.0);
    }
    let _ = std::fs::remove_dir_all(folder);
}
