//! End-to-end checks against a local MediaMTX instance.
use crate::{audio, program::NetworkFrame, srt, stream};
use std::{path::Path, process::{Child, Stdio}, sync::Arc, time::{Duration, Instant}};

struct Receiver(Child);
impl Drop for Receiver {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn verify_pulses(path: &Path) {
    let decode = |args: &[&str]| {
        let output = stream::command().args(["-v", "error", "-i"]).arg(path).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    };
    let picture = decode(&["-map", "0:v:0", "-vf", "fps=25,scale=1:1,format=gray", "-f", "rawvideo", "pipe:1"]);
    let sound = decode(&["-map", "0:a:0", "-ar", "48000", "-ac", "2", "-f", "s16le", "pipe:1"]);
    let video_edges: Vec<_> = picture.windows(2).enumerate().filter_map(|(n, p)| {
        (p[0] < 100 && p[1] > 150).then_some((n + 1) * 40)
    }).collect();
    let levels: Vec<_> = sound.chunks_exact(1920).map(|block| {
        (0..2).all(|ch| block.chunks_exact(4).any(|sample| {
            i16::from_le_bytes([sample[ch * 2], sample[ch * 2 + 1]]).unsigned_abs() > 1600
        }))
    }).collect();
    let audio_edges: Vec<_> = levels.windows(2).enumerate().filter_map(|(n, p)| {
        (!p[0] && p[1]).then_some((n + 1) * 10)
    }).collect();
    assert!(video_edges.len() >= 2 && audio_edges.len() >= 2, "video {video_edges:?}, audio {audio_edges:?}");
    for edge in &video_edges {
        if *edge < 150 || *edge > picture.len() * 40 - 150 { continue; }
        let gap = audio_edges.iter().map(|sound| sound.abs_diff(*edge)).min().unwrap();
        assert!(gap <= 120, "A/V flash offset {gap} ms; video {video_edges:?}, audio {audio_edges:?}");
    }
}

#[test]
#[ignore = "requires a local MediaMTX server, FFmpeg and FFprobe"]
fn mediamtx_delivers_synchronized_video_and_stereo() {
    let rtmp_port: u16 = std::env::var("LIVEMIX_MEDIAMTX_RTMP_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(1935);
    let srt_port: u16 = std::env::var("LIVEMIX_MEDIAMTX_SRT_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(8890);
    let name = format!("livemix-test-{}", std::process::id());
    let destinations = [0, 1].map(|n| stream::Destination {
        url: format!("rtmp://127.0.0.1:{rtmp_port}/{name}-{n}"), quality: 4,
        bitrate: Some(1), hardware: false, ..Default::default()
    });
    let rtmp = stream::Output::start_group(&destinations.iter().collect::<Vec<_>>(), [320, 180], 25.0).unwrap();
    let mut config = srt::Settings::default();
    config.connection.host = "127.0.0.1".into();
    config.connection.port = srt_port;
    config.connection.stream_id = format!("publish:{name}-srt");
    config.hardware = false;
    let srt = stream::Output::start_srt(&config, [320, 180], 25.0).unwrap();
    let targets = [
        destinations[0].url.clone(), destinations[1].url.clone(),
        format!("srt://127.0.0.1:{srt_port}?streamid=read:{name}-srt&latency=120000"),
    ];
    let paths = [0, 1, 2].map(|n| std::env::temp_dir().join(format!("{name}-{n}.mkv")));
    let frames = [16, 230].map(|value| NetworkFrame {
        width: 320, height: 180, pixels: Arc::new([value, value, value, 255].repeat(320 * 180)),
    });
    let pulse = audio::test_tone(1920);
    let silence = vec![[0; 2]; 1920].into();
    let mut receivers = Vec::new();
    let epoch = Instant::now();
    for slot in 0..425 {
        let when = epoch + Duration::from_millis(slot * 40);
        std::thread::sleep(when.saturating_duration_since(Instant::now()));
        let on = usize::from(slot % 25 < 6);
        let sound = if on == 1 { pulse.clone() } else { Arc::clone(&silence) };
        rtmp.submit_audio(frames[on].clone(), sound.clone(), when);
        srt.submit_audio(frames[on].clone(), sound, when);
        if slot == 85 {
            for (url, path) in targets.iter().zip(&paths) {
                let child = stream::command().args(["-v", "error", "-y", "-i", url, "-map", "0:v:0",
                    "-map", "0:a:0", "-c", "copy", "-t", "3.5"]).arg(path)
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
                receivers.push(Receiver(child));
            }
        }
        if receivers.len() == 3 && receivers.iter_mut().all(|child| child.0.try_wait().unwrap().is_some()) { break; }
        assert!(!rtmp.status().1 && !srt.status().1, "{:?}, {:?}", rtmp.status(), srt.status());
    }
    rtmp.stop();
    srt.stop();
    for (mut child, path) in receivers.into_iter().zip(&paths) {
        assert!(child.0.try_wait().unwrap().is_some_and(|status| status.success()), "receiver failed");
        verify_pulses(path);
        std::fs::remove_file(path).unwrap();
    }
}
