use eframe::egui;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const CHUNK: usize = 4 * 1024 * 1024;
const CHUNKS: usize = 128;
const TOTAL: usize = CHUNK * CHUNKS;

#[derive(Default)]
struct Run {
    written: AtomicUsize,
    cancel: AtomicBool,
    result: Mutex<Option<Result<f64, String>>>,
}

struct TestFile {
    file: Option<File>,
    path: PathBuf,
}

impl Drop for TestFile {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

fn measure(folder: &Path, state: &Run, mut update: impl FnMut()) -> io::Result<f64> {
    fs::create_dir_all(folder)?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let path = folder.join(format!(".livemix-speed-{}-{stamp}.tmp", std::process::id()));
    // Only remove a file this run successfully created.
    let file = OpenOptions::new().write(true).create_new(true).open(&path)?;
    let mut temporary = TestFile { file: Some(file), path };
    let file = temporary.file.as_mut().unwrap();
    let mut bytes = vec![0u8; CHUNK * 4];
    let mut x = 0x94d0_49bb_1331_11ebu64;
    for word in bytes.chunks_exact_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        word.copy_from_slice(&x.to_le_bytes());
    }
    let started = Instant::now();
    for chunk in 0..CHUNKS {
        if state.cancel.load(Relaxed) { return Err(io::ErrorKind::Interrupted.into()); }
        let offset = chunk % 4 * CHUNK;
        file.write_all(&bytes[offset..offset + CHUNK])?;
        state.written.store((chunk + 1) * CHUNK, Relaxed);
        if chunk % 8 == 7 { update(); }
    }
    file.sync_all()?;
    if state.cancel.load(Relaxed) { return Err(io::ErrorKind::Interrupted.into()); }
    Ok(TOTAL as f64 / started.elapsed().as_secs_f64() / 1_000_000.0)
}

pub(crate) fn show(ui: &mut egui::Ui, folder: PathBuf, busy: bool, bitrate: Option<u32>) {
    let id = ui.id().with("recording-disk-speed");
    let current = ui.data(|data| data.get_temp::<(PathBuf, Arc<Run>)>(id));
    let result = current.as_ref().and_then(|(_, run)| run.result.lock().ok()?.clone());
    let running = current.is_some() && result.is_none();
    if busy && running {
        if let Some((_, run)) = &current { run.cancel.store(true, Relaxed); }
    }
    let same_folder = current.as_ref().is_some_and(|(path, _)| *path == folder);
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(!busy && !running, egui::Button::new(crate::tr(ui, "Test Write Speed"))).clicked() {
            let run = Arc::new(Run::default());
            ui.data_mut(|data| data.insert_temp(id, (folder.clone(), Arc::clone(&run))));
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let result = measure(&folder, &run, || ctx.request_repaint()).map_err(|err| {
                    if err.kind() == io::ErrorKind::Interrupted { String::new() } else { err.to_string() }
                });
                *run.result.lock().unwrap() = Some(result);
                ctx.request_repaint();
            });
        }
        if running {
            let (path, run) = current.as_ref().unwrap();
            let written = run.written.load(Relaxed);
            let label = if written == TOTAL { crate::tr(ui, "Flushing to disk…").to_owned() }
                else { format!("{} / 512 MiB", written / (1024 * 1024)) };
            ui.add(egui::ProgressBar::new(written as f32 / TOTAL as f32).desired_width(145.0).text(label))
                .on_hover_text(path.display().to_string());
            if ui.button(crate::tr(ui, "Cancel")).clicked() { run.cancel.store(true, Relaxed); }
        } else if same_folder {
            match result {
                Some(Ok(speed)) => {
                    ui.strong(format!("{speed:.1} MB/s"));
                    if let Some(mbps) = bitrate {
                        let required = f64::from(mbps) / 8.0;
                        let text = format!("{} {required:.1} MB/s", crate::tr(ui, "Configured video:"));
                        if speed < required {
                            ui.colored_label(egui::Color32::LIGHT_RED, text);
                        } else {
                            ui.label(text);
                        }
                    }
                }
                Some(Err(error)) if error.is_empty() => { ui.label(crate::tr(ui, "Test canceled")); }
                Some(Err(error)) => {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("{} {error}", crate::tr(ui, "Write test failed:")));
                }
                None => {}
            }
        }
    });
}
