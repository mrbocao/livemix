//! Convert static slides in a background media worker, then use the picture transport.
use crate::media_common::Shared;
use std::{path::{Path, PathBuf}, process::{Command, Stdio}, sync::{Arc, atomic::Ordering::Relaxed}, time::{Duration, Instant}};

pub fn accepted(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
        ["ppt", "pptx", "pps", "ppsx", "odp"].iter().any(|format| ext.eq_ignore_ascii_case(format))
    })
}

struct SlideFolder(PathBuf);
impl Drop for SlideFolder {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
}

static CONVERTER: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn run(shared: Arc<Shared>, path: String) {
    if let Err(error) = convert_and_play(&shared, Path::new(&path)) { shared.set_status(error); }
}

fn convert_and_play(shared: &Arc<Shared>, source: &Path) -> Result<(), String> {
    let source = source.canonicalize().map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let path = std::env::temp_dir().join(format!("livemix-slides-{}-{stamp}", std::process::id()));
    std::fs::create_dir(&path).map_err(|error| error.to_string())?;
    let folder = SlideFolder(path);
    shared.set_status("Converting presentation…");
    let guard = loop {
        if shared.stop.load(Relaxed) { return Ok(()); }
        if let Ok(lock) = CONVERTER.try_lock() { break lock; }
        std::thread::sleep(Duration::from_millis(50));
    };
    #[cfg(windows)]
    let office_result = {
        let mut powerpoint = Command::new("powershell.exe");
        powerpoint.args(["-NoProfile", "-NonInteractive", "-Command", POWERPOINT])
            .env("LIVEMIX_SLIDES_SOURCE", source.to_string_lossy().trim_start_matches(r"\\?\"))
            .env("LIVEMIX_SLIDES_OUTPUT", &folder.0);
        execute(shared, &mut powerpoint)
    };
    #[cfg(not(windows))]
    let office_result: Result<(), String> = Err("PowerPoint automation is available on Windows".into());

    if let Err(office_error) = office_result {
        if shared.stop.load(Relaxed) { return Ok(()); }
        for entry in std::fs::read_dir(&folder.0).map_err(|e| e.to_string())?.flatten() {
            if entry.path().extension().is_some_and(|ext| ext == "png") {
                std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
        let mut libreoffice = Command::new(libreoffice());
        let profile = folder.0.join("office-profile").to_string_lossy().replace('\\', "/");
        let profile = profile.trim_start_matches('/').replace('%', "%25").replace(' ', "%20").replace('#', "%23");
        libreoffice.arg(format!("-env:UserInstallation=file:///{profile}"))
            .args(["--headless", "--convert-to", "pdf", "--outdir"]).arg(&folder.0).arg(&source);
        execute(shared, &mut libreoffice).map_err(|error| format!("PowerPoint: {office_error}. LibreOffice: {error}"))?;
        if shared.stop.load(Relaxed) { return Ok(()); }
        let mut filename = source.file_stem().ok_or("Invalid presentation filename")?.to_os_string();
        filename.push(".pdf");
        let pdf = folder.0.join(filename);
        let mut raster = Command::new("pdftoppm");
        raster.args(["-png", "-scale-to", "1920"]).arg(pdf).arg(folder.0.join("slide"));
        execute(shared, &mut raster).map_err(|error| format!("Slide rasterization requires pdftoppm: {error}"))?;
        for entry in std::fs::read_dir(&folder.0).map_err(|error| error.to_string())?.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(number) = name.strip_prefix("slide-").and_then(|name| name.strip_suffix(".png"))
                .and_then(|name| name.parse::<u32>().ok())
            {
                std::fs::rename(entry.path(), folder.0.join(format!("{number:08}.png"))).map_err(|error| error.to_string())?;
            }
        }
    }
    drop(guard);
    if !shared.stop.load(Relaxed) {
        crate::media_common::run_pictures(shared.clone(), folder.0.to_string_lossy().into_owned(), true, 1.0, 0.0);
    }
    Ok(())
}

fn libreoffice() -> PathBuf {
    let paths = [
        PathBuf::from("/Applications/LibreOffice.app/Contents/MacOS/soffice"),
        std::env::var_os("ProgramFiles").map(PathBuf::from).unwrap_or_default().join("LibreOffice/program/soffice.exe"),
    ];
    paths.into_iter().find(|path| path.is_file()).unwrap_or_else(|| PathBuf::from("soffice"))
}

fn execute(shared: &Shared, command: &mut Command) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|error| format!("Could not launch converter ({error})"))?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return if status.success() { Ok(()) } else { Err(format!("Converter exited with {status}")) };
        }
        if shared.stop.load(Relaxed) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Presentation conversion stopped or timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(windows)]
const POWERPOINT: &str = r#"
$ErrorActionPreference = 'Stop'
$alreadyOpen = Get-Process POWERPNT -ErrorAction SilentlyContinue
$app = New-Object -ComObject PowerPoint.Application
$deck = $null
$security = $null
try {
    $security = $app.AutomationSecurity
    $app.AutomationSecurity = 3
    $deck = $app.Presentations.Open($env:LIVEMIX_SLIDES_SOURCE, -1, -1, 0)
    $scale = [Math]::Min(1920 / $deck.PageSetup.SlideWidth, 1080 / $deck.PageSetup.SlideHeight)
    $w = [int][Math]::Round($deck.PageSetup.SlideWidth * $scale)
    $h = [int][Math]::Round($deck.PageSetup.SlideHeight * $scale)
    $index = 0
    foreach ($slide in $deck.Slides) {
        if ($slide.SlideShowTransition.Hidden -ne 0) { continue }
        $index++
        $slide.Export((Join-Path $env:LIVEMIX_SLIDES_OUTPUT ('{0:D8}.png' -f $index)), 'PNG', $w, $h)
    }
    if ($index -eq 0) { throw 'No visible slides' }
} finally {
    if ($deck) { $deck.Close() }
    if ($null -ne $security) { $app.AutomationSecurity = $security }
    if (!$alreadyOpen) { $app.Quit() }
}
"#;


#[cfg(test)]
#[test]
#[ignore = "requires a presentation fixture and an installed slide converter"]
fn presentation_opens_paused_and_seeks_to_a_different_slide() {
    let path = std::env::var("LIVEMIX_TEST_PRESENTATION").expect("Set a deck with two distinct slides");
    let shared = Arc::new(Shared::default());
    shared.paused.store(true, Relaxed);
    let source = shared.clone();
    let worker = std::thread::spawn(move || run(source, path));
    let deadline = Instant::now() + Duration::from_secs(25);
    while shared.seq.load(Relaxed) < 1 && !worker.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(30));
    }
    let first = shared.frame.lock().unwrap().take();
    let mut second = None;
    if first.is_some() {
        shared.seek_ms.store(1001, Relaxed);
        while shared.seq.load(Relaxed) < 2 && !worker.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(30));
        }
        second = shared.frame.lock().unwrap().take();
    }
    shared.stop.store(true, Relaxed);
    worker.join().unwrap();
    let first = first.unwrap_or_else(|| panic!("{}", shared.status.lock().unwrap()));
    let second = second.expect("second slide");
    assert_eq!(shared.pos_ms.load(Relaxed), 1000);
    assert!(first.rgba != second.rgba, "Seeking must change the rendered slide");
}
