//! Recording, scopes, DeckLink and network outputs driven from the update loop.
use super::*;

impl App {
    pub(crate) fn composed_recordings(&self) -> impl Iterator<Item = &ComposedRecording> {
        self.recordings.iter().chain(&self.multi_outputs).filter_map(Option::as_ref)
            .chain(self.replay.current.iter().map(|track| &track.recording))
    }

    pub(crate) fn recording_error(&self, recording: &ComposedRecording) -> Option<String> {
        let error = recording.recorder.error.lock().unwrap();
        if !error.is_empty() {
            return Some(error.clone());
        }
        if self.program.lock().unwrap().error.is_some() && !self.can_capture_from_window(recording) {
            return Some(format!("Output {} requires offscreen capture", recording.channel + 1));
        }
        None
    }

    pub(crate) fn can_capture_from_window(&self, recording: &ComposedRecording) -> bool {
        recording.route(&self.s.cfg) == OutputRoute::default()
    }

    pub(crate) fn update_recording(&mut self, now: f64) {
        if self.composed_recordings().next().is_none() {
            let (w, h) = self.s.cfg.out_dims();
            let format = ([w, h], f64::from(self.s.cfg.frame_rate()));
            if self.record_format != format {
                self.record_format = format;
                self.next_record_frame = now;
            }
        }
        let ([w, h], fps) = self.record_format;
        let options = |mbps, format, hw, audio| media::RecOpts { mbps, size: Some((w, h)), fps: fps as f32, hw, format, audio };

        if self.s.rec.is_none() {
            if let Some(primary) = self.recordings[0].take() {
                self.s.notice(&format!("Recording stopped: {}", primary.recorder.path));
            }
            self.recordings[1] = None;
        } else {
            let c = &self.s.cfg;
            let split = f64::from(c.rec_split_min) * 60.0;
            if self.recordings[0].as_ref().is_some_and(|r| split > 0.0 && now - r.started >= split) {
                self.recordings = Default::default();
            }
            if self.recordings[0].is_none() {
                let name = safe_name(&c.rec_name);
                self.recordings[0] = Some(ComposedRecording::new(&c.rec_folder, &name, c.rec_source, now,
                    options(c.rec_mbps, c.rec_codec, c.hw_encoder, c.record_audio[0])));
                if c.rec_second {
                    self.recordings[1] = Some(ComposedRecording::new(&c.rec_folder, &format!("{name} - 2"), c.rec_second_source, now,
                        options(c.rec_second_mbps, c.rec_second_codec, c.hw_encoder, c.record_audio[1])));
                }
                let path = &self.recordings[0].as_ref().unwrap().recorder.path;
                self.s.notice(&format!("Recording to {path}"));
            }
            if let Some(error) = self.recordings[0].as_ref().and_then(|r| self.recording_error(r)) {
                self.s.rec = None;
                self.recordings = Default::default();
                self.s.notice(&error);
            } else if let Some(error) = self.recordings[1].as_ref().and_then(|r| self.recording_error(r)) {
                self.recordings[1] = None;
                self.s.notice(&format!("Recording 2: {error}"));
            }
        }

        let selected_camera = self.s.inputs.iter().any(|i| matches!(i.kind, Kind::Device { .. }) && !self.s.mc_off.contains(&i.id));
        if self.s.multicorder.is_some() && !selected_camera && !self.s.cfg.mc_outputs.contains(&true) {
            self.s.multicorder = None;
            self.s.notice("Select a camera or Output 1–4 in MultiCorder");
        }
        let split = f64::from(self.s.cfg.mc_split_min) * 60.0;
        for channel in 0..OUTPUT_COUNT {
            if self.s.multicorder.is_none() || !self.s.cfg.mc_outputs[channel] {
                self.multi_outputs[channel] = None;
                self.multi_output_failed[channel] = false;
                continue;
            }
            if self.multi_output_failed[channel] {
                continue;
            }
            if self.multi_outputs[channel].as_ref().is_some_and(|r| split > 0.0 && now - r.started >= split) {
                self.multi_outputs[channel] = None;
            }
            if self.multi_outputs[channel].is_none() {
                let c = &self.s.cfg;
                self.multi_outputs[channel] = Some(ComposedRecording::new(&c.mc_folder, &format!("MultiCorder Output {}", channel + 1), channel, now,
                    options(c.mc_mbps, c.mc_codec, c.hw_encoder, c.record_audio[2])));
            }
            if let Some(error) = self.multi_outputs[channel].as_ref().and_then(|r| self.recording_error(r)) {
                self.multi_outputs[channel] = None;
                self.multi_output_failed[channel] = true;
                self.s.notice(&format!("MultiCorder Output {}: {error}", channel + 1));
            }
        }
        for (id, recorder) in &self.multi {
            let error = std::mem::take(&mut *recorder.error.lock().unwrap());
            if !error.is_empty() {
                self.s.notice(&format!("MultiCorder Input {}: {error}", self.s.number_of(*id)));
            }
        }
    }

    pub(crate) fn advance_record_clock(&mut self, now: f64) {
        let fps = self.record_format.1;
        let elapsed = ((now - self.next_record_frame) * fps).floor() as u64;
        self.next_record_frame += (elapsed + 1) as f64 / fps;
        for recording in self.composed_recordings() {
            recording.recorder.note_dropped_frames(elapsed);
        }
    }

    pub(crate) fn update_scopes(&mut self, ctx: &egui::Context, now: f64) {
        let key = (self.s.preview, self.s.cfg.wave_type, self.s.cfg.vs_zoom);
        if key != self.scope_key {
            self.scope_key = key;
            self.next_wave = now;
            self.scopes.clear();
            self.s.scope_tex = model::scope_layout(key.1).1.into_iter().map(|(kind, _)| {
                let texture = ctx.load_texture(format!("scope-{kind}"),
                    egui::ColorImage::new(scopes::dimensions(kind), Color32::TRANSPARENT), egui::TextureOptions::LINEAR);
                let id = texture.id();
                self.scopes.insert(kind, texture);
                (kind, id)
            }).collect();
        }
        let frame = self.scope_frame.lock().unwrap().take();
        if let Some(frame) = frame.filter(|frame| frame.key == key && key.1 != 0) {
            let _ = self.scope_jobs.try_send(frame);
        }
        let images = self.scope_images.lock().unwrap().take();
        if let Some((_, images)) = images.filter(|(frame_key, _)| *frame_key == key && key.1 != 0) {
            let mut ids = Vec::new();
            for (kind, image) in images {
                let tex = self.scopes.entry(kind).or_insert_with(|| {
                    ctx.load_texture(format!("scope-{kind}"), egui::ColorImage::new([1, 1], Color32::TRANSPARENT), egui::TextureOptions::LINEAR)
                });
                tex.set(image, egui::TextureOptions::LINEAR);
                ids.push((kind, tex.id()));
            }
            self.s.scope_tex = ids;
        }
    }

    pub(crate) fn update_decklink_outputs(&mut self) {
        for channel in 0..OUTPUT_COUNT {
            let route = self.s.cfg.decklink_outputs[channel];
            let wanted = route.device.filter(|_| self.s.decklink_active[channel]).map(|device| decklink::OutputConfig {
                device, mode: route.mode, port: route.port,
                key_fill: route.key_fill,
                size: self.s.cfg.out_dims(), fps: self.s.cfg.frame_rate(), interlaced: self.s.cfg.interlaced,
            });
            let slot = &mut self.decklink_outputs[channel];
            if slot.as_ref().map(|(cfg, _)| *cfg) != wanted {
                if let Some((_, sink)) = slot {
                    sink.stop();
                    if !sink.finished() {
                        continue;
                    }
                }
                *slot = None;
                if let Some(cfg) = wanted {
                    *slot = Some((cfg, decklink::OutputSink::start(cfg)));
                }
            }
            if let Some((_, sink)) = slot {
                if let Some(error) = sink.error() {
                    self.s.decklink_status[channel] = error.clone();
                    self.s.decklink_active[channel] = false;
                    self.s.notice(&format!("DeckLink Output {}: {error}", channel + 1));
                } else {
                    self.s.decklink_status[channel] = if sink.frames() > 0 {
                        if wanted.is_some_and(|config| config.key_fill) { "Sending DeckLink Key + Fill".into() }
                        else { "Sending DeckLink output".into() }
                    } else {
                        "Starting DeckLink output…".into()
                    };
                }
            } else if self.s.decklink_active[channel] {
                self.s.decklink_active[channel] = false;
            }
        }
    }

    pub(crate) fn update_network_outputs(&mut self) {
        let offscreen_error = self.program.lock().unwrap().error.clone();
        for (channel, slot) in self.ndi_outputs.iter_mut().enumerate() {
            let active = &mut self.s.cfg.ndi_outputs[channel];
            let status = &mut self.s.ndi_status[channel];
            if let Some(error) = offscreen_error.as_ref().filter(|_| *active) {
                *status = format!("NDI requires offscreen video: {error}");
                *active = false;
            }
            let fps = self.record_format.1;
            if slot.as_ref().is_some_and(|(rate, _)| !*active || *rate != fps) {
                slot.take().unwrap().1.stop();
            }
            if *active {
                let (_, sender) = slot.get_or_insert_with(|| {
                    (fps, ndi::Sender::start(format!("LiveMix Output {}", channel + 1), fps))
                });
                let (message, failed) = sender.status();
                *status = message;
                if failed { *active = false; }
            } else if status.starts_with("Sending") || status.starts_with("Starting") || status.starts_with("Waiting") {
                status.clear();
            }
        }
        for (channel, slot) in self.omt_outputs.iter_mut().enumerate() {
            let active = &mut self.s.cfg.omt_outputs[channel];
            let status = &mut self.s.omt_status[channel];
            let config = (self.record_format.1, self.s.cfg.omt_quality[channel]);
            if let Some(error) = offscreen_error.as_ref().filter(|_| *active) {
                *status = format!("OMT requires offscreen video: {error}");
                *active = false;
            }
            if slot.as_ref().is_some_and(|(previous, _)| !*active || *previous != config) {
                slot.take().unwrap().1.stop();
            }
            if *active {
                let (_, sender) = slot.get_or_insert_with(|| {
                    (config, omt::Sender::start(format!("LiveMix Output {}", channel + 1), config.0, config.1))
                });
                let (message, failed) = sender.status();
                *status = message;
                if failed { *active = false; }
            }
        }
        for (channel, slot) in self.srt_outputs.iter_mut().enumerate() {
            let active = &mut self.s.srt_active[channel];
            let status = &mut self.s.srt_status[channel];
            if let Some(error) = offscreen_error.as_ref().filter(|_| *active) {
                *status = format!("SRT requires offscreen video: {error}");
                *active = false;
            }
            if slot.as_ref().is_some_and(|(format, _)| !*active || *format != self.record_format) {
                let (_, output) = slot.take().unwrap();
                self.s.srt_logs[channel] = output.diagnostics();
                output.stop();
            }
            if *active && slot.is_none() {
                let (size, fps) = self.record_format;
                self.s.srt_logs[channel].clear();
                match stream::Output::start_srt(&self.s.cfg.srt_outputs[channel], size, fps) {
                    Ok(output) => *slot = Some((self.record_format, output)),
                    Err(error) => { *status = error; *active = false; }
                }
            }
            if let Some((_, output)) = slot {
                let (message, failed) = output.status();
                *status = message;
                if self.s.srt_setup == Some(channel) || failed { self.s.srt_logs[channel] = output.diagnostics(); }
                if failed { *active = false; }
            }
        }
        let mut groups: [Vec<(usize, stream::Destination)>; stream::COUNT] = Default::default();
        for (index, destination) in self.s.cfg.streams.iter().enumerate() {
            let status = &mut self.s.stream_status[index];
            if !self.s.stream_active[index] {
                if status.starts_with("Live") || status.starts_with("Connecting") { status.clear(); }
                continue;
            }
            let error = offscreen_error.as_ref().map(|error| format!("Streaming requires offscreen video: {error}"))
                .or_else(|| stream::publish_url(destination).err());
            if let Some(error) = error {
                *status = error;
                self.s.stream_active[index] = false;
                continue;
            }
            let leader = groups.iter().position(|group| group.first().is_some_and(|(_, first)| {
                (first.source, first.direct, first.quality, first.bitrate, first.hardware, first.hevc, first.source_audio, first.audio_rate())
                    == (destination.source, destination.direct, destination.quality, destination.bitrate, destination.hardware,
                        destination.hevc, destination.source_audio, destination.audio_rate())
            })).unwrap_or(index);
            groups[leader].push((index, destination.clone()));
        }
        for (slot, group) in self.stream_outputs.iter_mut().zip(groups) {
            if slot.as_ref().is_some_and(|(format, previous, _)| *format != self.record_format || *previous != group) {
                let (_, previous, output) = slot.take().unwrap();
                let log = output.diagnostics();
                for (index, _) in previous { self.s.stream_logs[index].clone_from(&log); }
                output.stop();
            }
            if group.is_empty() { continue; }
            if slot.is_none() {
                let (size, fps) = self.record_format;
                let destinations: Vec<_> = group.iter().map(|(_, destination)| destination).collect();
                for (index, _) in &group { self.s.stream_logs[*index].clear(); }
                match stream::Output::start_group(&destinations, size, fps) {
                    Ok(output) => *slot = Some((self.record_format, group.clone(), output)),
                    Err(error) => {
                        for (index, _) in group {
                            self.s.stream_status[index] = error.clone();
                            self.s.stream_active[index] = false;
                        }
                        continue;
                    }
                }
            }
            if let Some((_, _, output)) = slot {
                let (message, failed) = output.status();
                let log = (self.s.stream_setup || failed).then(|| output.diagnostics());
                let group_size = group.len();
                for (index, _) in group {
                    self.s.stream_status[index] = if group_size > 1 && !failed {
                        format!("{message} · shared encoder ({group_size} destinations)")
                    } else { message.clone() };
                    if let Some(log) = &log { self.s.stream_logs[index].clone_from(log); }
                    if failed { self.s.stream_active[index] = false; }
                }
            }
        }
    }
}
