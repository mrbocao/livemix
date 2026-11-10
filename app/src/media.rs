// Real media through Windows Media Foundation:
// - video capture devices (webcams, HDMI/SDI capture cards, Blackmagic devices through the
//   Blackmagic Desktop Video WDM driver, ATEM Mini / Web Presenter as USB cameras)
// - video files (mp4, mov, wmv, avi, mkv … whatever Windows can decode)
// - MP4 (H.264) recording
// - the Windows "Open file" picker
// Each source runs on its own thread and hands RGBA frames to the UI.

pub use crate::media_common::*;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID, HSTRING, PROPVARIANT, PWSTR};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
    COINIT_MULTITHREADED,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};

type Res<T> = windows::core::Result<T>;

/// Name of a video subtype: its FOURCC (MJPG, YUY2, NV12, H264…) or the RGB number.
fn subtype_name(g: &GUID) -> String {
    match g.data1 {
        20 => "RGB24".into(),
        21 => "ARGB32".into(),
        22 => "RGB32".into(),
        23 => "RGB565".into(),
        24 => "RGB555".into(),
        v => {
            let b = v.to_le_bytes();
            if b.iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
                String::from_utf8_lossy(&b).trim().to_string()
            } else {
                format!("0x{v:08X}")
            }
        }
    }
}

unsafe fn mode_of(t: &IMFMediaType) -> Option<CamMode> {
    let (w, h) = unpack(t.GetUINT64(&MF_MT_FRAME_SIZE).ok()?);
    let (n, d) = t.GetUINT64(&MF_MT_FRAME_RATE).map(unpack).unwrap_or((0, 1));
    let format = t.GetGUID(&MF_MT_SUBTYPE).map(|g| subtype_name(&g)).unwrap_or_default();
    Some(CamMode { w: w as u32, h: h as u32, fps_n: n as u32, fps_d: d.max(1) as u32, format })
}

fn mf_init() -> bool {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_FULL).is_ok()
    }
}

unsafe fn enum_activates() -> Res<Vec<IMFActivate>> {
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs = attrs.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    attrs.SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)?;
    let mut pp: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFEnumDeviceSources(&attrs, &mut pp, &mut count)?;
    let mut v = Vec::new();
    if !pp.is_null() {
        for i in 0..count as usize {
            if let Some(a) = (*pp.add(i)).take() {
                v.push(a);
            }
        }
        CoTaskMemFree(Some(pp as *const _));
    }
    Ok(v)
}

unsafe fn get_string(a: &IMFActivate, key: &GUID) -> String {
    let mut p = PWSTR::null();
    let mut len = 0u32;
    if a.GetAllocatedString(key, &mut p, &mut len).is_ok() && !p.is_null() {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    } else {
        String::new()
    }
}

/// Every video capture device Windows knows about.
pub fn list_devices() -> Vec<DeviceInfo> {
    std::thread::spawn(|| {
        if !mf_init() {
            return Vec::new();
        }
        unsafe {
            enum_activates()
                .map(|v| {
                    v.iter()
                        .map(|a| {
                            let name = get_string(a, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME);
                            let link = get_string(a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK);
                            DeviceInfo { blackmagic: is_blackmagic(&name), name, link }
                        })
                        .collect()
                })
                .unwrap_or_default()
        }
    })
    .join()
    .unwrap_or_default()
}

/// Is the Blackmagic Desktop Video driver installed? (its API DLL lives in System32)
pub fn blackmagic_driver_installed() -> bool {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    std::path::Path::new(&root).join("System32").join("DeckLinkAPI64.dll").exists()
        || std::path::Path::new(&root).join("System32").join("DeckLinkAPI.dll").exists()
}

pub fn start(src: Source) -> Arc<Shared> {
    let sh = Arc::new(Shared::default());
    sh.set_status("Starting…");
    let s2 = sh.clone();
    std::thread::spawn(move || match src {
        Source::DeckLink(config) => crate::decklink::run_capture(s2, config),
        Source::Ndi(name) => crate::ndi::run_capture(s2, &name),
        Source::Omt(address) => crate::omt::run_capture(s2, &address),
        Source::Srt(config) => crate::srt::run_capture(s2, config),
        Source::Desktop => run_desktop(s2),
        Source::Sequence(folder, fps) => run_pictures(s2, folder, true, fps, 0.0),
        Source::Photos(folder, secs) => run_pictures(s2, folder, false, 0.0, secs),
        other => run(s2, other),
    });
    sh
}

unsafe fn video_device_manager() -> Res<IMFDXGIDeviceManager> {
    let mut device = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        None,
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        None,
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        None,
    )?;
    let device = device.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    let _ = device.cast::<ID3D10Multithread>()?.SetMultithreadProtected(true);

    let mut token = 0;
    let mut manager = None;
    MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
    let manager = manager.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    manager.ResetDevice(&device, token)?;
    Ok(manager)
}

unsafe fn reader_attrs(manager: Option<&IMFDXGIDeviceManager>) -> Res<IMFAttributes> {
    let mut a: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut a, 3)?;
    let a = a.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    if let Some(manager) = manager {
        a.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, manager)?;
        a.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)?;
        a.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
    } else {
        a.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
    }
    Ok(a)
}

unsafe fn open(src: &Source, manager: Option<&IMFDXGIDeviceManager>, sh: &Shared) -> Res<(IMFSourceReader, Dims)> {
    let attrs = reader_attrs(manager)?;
    let reader = open_reader(src, &attrs)?;
    let dev = match src {
        Source::Device(_, want) => Some((want.as_ref(), sh)),
        _ => None,
    };
    let dims = configure(&reader, dev)?;
    Ok((reader, dims))
}

unsafe fn open_reader(src: &Source, attrs: &IMFAttributes) -> Res<IMFSourceReader> {
    match src {
        Source::Device(link, _) => {
            for a in enum_activates()? {
                if get_string(&a, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_SYMBOLIC_LINK) == *link {
                    let source: IMFMediaSource = a.ActivateObject()?;
                    return MFCreateSourceReaderFromMediaSource(&source, attrs);
                }
            }
            Err(windows::core::Error::from(E_FAIL))
        }
        Source::File(path) => MFCreateSourceReaderFromURL(&HSTRING::from(path.as_str()), attrs),
        _ => Err(windows::core::Error::from(E_FAIL)),
    }
}

fn unpack(v: u64) -> (u64, u64) {
    (v >> 32, v & 0xffff_ffff)
}

/// Camera: use the chosen mode, or (Auto) the best one up to 1920x1080; list every mode in
/// `sh.modes`. Then ask for RGB32 frames.
unsafe fn configure(r: &IMFSourceReader, dev: Option<(Option<&CamMode>, &Shared)>) -> Res<Dims> {
    let vs = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let _ = r.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
    r.SetStreamSelection(vs, true)?;
    if let Some((want, sh)) = dev {
        let mut best: Option<(u64, IMFMediaType)> = None;
        let mut chosen: Option<IMFMediaType> = None;
        let mut modes: Vec<CamMode> = Vec::new();
        let mut i = 0;
        while let Ok(t) = r.GetNativeMediaType(vs, i) {
            i += 1;
            if let Some(m) = mode_of(&t) {
                if want == Some(&m) && chosen.is_none() {
                    chosen = Some(t.clone());
                }
                if !modes.contains(&m) {
                    modes.push(m);
                }
            }
            if let Ok(fs) = t.GetUINT64(&MF_MT_FRAME_SIZE) {
                let (w, h) = unpack(fs);
                let max_h = AUTO_CAM_MAX_H.load(Ordering::Relaxed) as u64;
                if h <= max_h && w <= max_h * 16 / 9 {
                    let fr = t.GetUINT64(&MF_MT_FRAME_RATE).map(|v| { let (n, d) = unpack(v); n as f64 / d.max(1) as f64 }).unwrap_or(0.0);
                    let score = w * h * 1000 + (fr.min(60.0) * 10.0) as u64;
                    if best.as_ref().map_or(true, |(s, _)| score > *s) {
                        best = Some((score, t));
                    }
                }
            }
            if i > 500 {
                break;
            }
        }
        let pick = chosen.or(best.map(|(_, t)| t));
        if let Some(t) = &pick {
            let _ = r.SetCurrentMediaType(vs, None, t);
        }
        modes.sort_by(|a, b| (b.w * b.h, b.fps_n * 1000 / b.fps_d.max(1), a.format.clone()).cmp(&(a.w * a.h, a.fps_n * 1000 / a.fps_d.max(1), b.format.clone())));
        *sh.modes.lock().unwrap() = modes;
        *sh.mode.lock().unwrap() = pick.as_ref().and_then(|t| mode_of(t));
    }
    let out = MFCreateMediaType()?;
    out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    out.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
    r.SetCurrentMediaType(vs, None, &out)?;
    current_dims(r)
}

unsafe fn current_dims(r: &IMFSourceReader) -> Res<Dims> {
    let vs = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let cur = r.GetCurrentMediaType(vs)?;
    let (fw, fh) = unpack(cur.GetUINT64(&MF_MT_FRAME_SIZE)?);
    let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(fw as i32 * 4);
    let mut d = Dims { full_w: fw as usize, full_h: fh as usize, stride, w: fw as usize, h: fh as usize, ox: 0, oy: 0 };
    // Decoders pad the picture (e.g. 1080 -> 1088); the display aperture is the real picture.
    let mut blob = [0u8; 16];
    if cur.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, &mut blob, None).is_ok() {
        let ox = i16::from_le_bytes([blob[2], blob[3]]).max(0) as usize;
        let oy = i16::from_le_bytes([blob[6], blob[7]]).max(0) as usize;
        let cx = i32::from_le_bytes([blob[8], blob[9], blob[10], blob[11]]).max(0) as usize;
        let cy = i32::from_le_bytes([blob[12], blob[13], blob[14], blob[15]]).max(0) as usize;
        if cx > 0 && cy > 0 && ox + cx <= d.full_w && oy + cy <= d.full_h {
            d.w = cx;
            d.h = cy;
            d.ox = ox;
            d.oy = oy;
        }
    }
    Ok(d)
}

/// Frame layout: full buffer size and the visible picture inside it.
#[derive(Clone, Copy, Default, Debug)]
struct Dims {
    full_w: usize,
    full_h: usize,
    stride: i32,
    w: usize,
    h: usize,
    ox: usize,
    oy: usize,
}

unsafe fn copy_rows(p0: *const u8, pitch: isize, w: usize, h: usize, out: &mut [u8], key: u64) {
    let (kr, kg, kb, tol) = ((key >> 16) & 255, (key >> 8) & 255, key & 255, (key >> 24) & 255);
    let keyed = key != 0;
    for y in 0..h {
        let row = p0.offset(y as isize * pitch);
        for x in 0..w {
            let s = row.add(x * 4);
            let d = (y * w + x) * 4;
            let (b, g, r) = (*s, *s.add(1), *s.add(2));
            out[d] = r;
            out[d + 1] = g;
            out[d + 2] = b;
            out[d + 3] = 255;
            if keyed {
                // Simple colour-distance chroma key with a soft edge.
                let dr = r as i32 - kr as i32;
                let dg = g as i32 - kg as i32;
                let db = b as i32 - kb as i32;
                let d2 = (dr * dr + dg * dg + db * db) as f32;
                let t = tol as f32 * 1.7;
                if d2 < t * t {
                    out[d + 3] = 0;
                } else if d2 < (t + 40.0) * (t + 40.0) {
                    let dist = d2.sqrt();
                    out[d + 3] = (((dist - t) / 40.0) * 255.0) as u8;
                }
            }
        }
    }
}

unsafe fn copy_sample(s: &IMFSample, d: &Dims, key: u64) -> Res<Vec<u8>> {
    let (w, h) = (d.w, d.h);
    let buf = s.ConvertToContiguousBuffer()?;
    let mut out = vec![0u8; w * h * 4];
    if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut pitch = 0i32;
        b2.Lock2D(&mut p, &mut pitch)?;
        if !p.is_null() {
            let start = p.offset(d.oy as isize * pitch as isize + d.ox as isize * 4);
            copy_rows(start, pitch as isize, w, h, &mut out, key);
        }
        b2.Unlock2D()?;
    } else {
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut p, None, Some(&mut len))?;
        let pitch_abs = (d.stride.unsigned_abs() as usize).max(d.full_w * 4);
        if !p.is_null() && len as usize >= pitch_abs * d.full_h {
            let (top, pitch) = if d.stride < 0 {
                (p.add((d.full_h - 1) * pitch_abs), -(pitch_abs as isize))
            } else {
                (p, pitch_abs as isize)
            };
            let start = top.offset(d.oy as isize * pitch + d.ox as isize * 4);
            copy_rows(start, pitch, w, h, &mut out, key);
        }
        buf.Unlock()?;
    }
    Ok(out)
}

fn run(sh: Arc<Shared>, src: Source) {
    if !mf_init() {
        sh.set_status("Windows Media Foundation is not available");
        return;
    }
    let is_file = matches!(src, Source::File(_));
    // Capture devices keep their existing driver path. File readers can use DXVA.
    let mut manager = if is_file {
        unsafe { video_device_manager().ok() }
    } else {
        None
    };
    let vs = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
    let mut reader: Option<IMFSourceReader> = None;
    let mut dims = Dims::default();
    let mut t0 = Instant::now();
    let mut base_ts: i64 = -1;
    let mut need_open = true;
    let mut last_frame_ts = None;
    let mut resume_at = None;
    let mut seek_to = None;
    while !sh.stop.load(Ordering::Relaxed) {
        if sh.restart.swap(false, Ordering::Relaxed) {
            need_open = true;
            resume_at = None;
            seek_to = None;
            sh.pos_ms.store(0, Ordering::Relaxed);
        }
        if need_open {
            reader = None;
            let r = unsafe {
                open(&src, manager.as_ref(), &sh).or_else(|e| {
                    if manager.take().is_some() { open(&src, None, &sh) } else { Err(e) }
                })
            };
            match r {
                Ok((r, d)) => {
                    if let Some(ts) = resume_at {
                        // Seeking may land on an earlier keyframe. Skip it below.
                        let _ = unsafe { r.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(ts)) };
                    }
                    reader = Some(r);
                    dims = d;
                    *sh.size.lock().unwrap() = (d.w, d.h);
                    sh.set_status("");
                    sh.ended.store(false, Ordering::Relaxed);
                }
                Err(e) => {
                    sh.set_status(format!("No signal ({})", e.message()));
                    std::thread::sleep(Duration::from_millis(1500));
                    continue;
                }
            }
            need_open = false;
            t0 = Instant::now();
            base_ts = -1;
            last_frame_ts = None;
        }
        if is_file {
            if let Some(ms) = sh.seek_ms.swap(0, Ordering::Relaxed).checked_sub(1) {
                let duration = sh.dur_ms.load(Ordering::Relaxed);
                let ms = if duration > 0 { ms.min(duration.saturating_sub(50)) } else { ms };
                let ts = (ms.min(i64::MAX as u64 / 10_000) * 10_000) as i64;
                if let Some(r) = &reader {
                    match unsafe { r.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(ts)) } {
                        Ok(()) => {
                            seek_to = Some(ts);
                            resume_at = None;
                            base_ts = -1;
                            last_frame_ts = None;
                            sh.ended.store(false, Ordering::Relaxed);
                            sh.set_status("");
                        }
                        Err(e) => sh.set_status(format!("Unable to seek ({})", e.message())),
                    }
                }
            }
        }
        if is_file {
            // Pause / end of file: hold the last frame.
            while seek_to.is_none() && (sh.paused.load(Ordering::Relaxed) || sh.ended.load(Ordering::Relaxed))
                && !sh.stop.load(Ordering::Relaxed)
                && !sh.restart.load(Ordering::Relaxed)
                && sh.seek_ms.load(Ordering::Relaxed) == 0
            {
                if sh.ended.load(Ordering::Relaxed) && sh.looping.load(Ordering::Relaxed) {
                    break;
                }
                let a = Instant::now();
                std::thread::sleep(Duration::from_millis(15));
                t0 += a.elapsed();
            }
            if sh.restart.load(Ordering::Relaxed) || sh.seek_ms.load(Ordering::Relaxed) != 0 {
                continue;
            }
            if sh.ended.load(Ordering::Relaxed) {
                if sh.looping.load(Ordering::Relaxed) {
                    need_open = true;
                }
                continue;
            }
        }
        let Some(r) = reader.as_ref() else { continue };
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample: Option<IMFSample> = None;
        let result = unsafe { r.ReadSample(vs, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)) };
        if result.is_err() || flags & (MF_SOURCE_READERF_ERROR.0 as u32) != 0 {
            need_open = true;
            if manager.take().is_some() {
                resume_at = resume_at.or(last_frame_ts);
                continue;
            }
            resume_at = None;
            let e = result.err().unwrap_or_else(|| windows::core::Error::from(E_FAIL));
            sh.set_status(format!("Lost signal ({})", e.message()));
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            resume_at = None;
            seek_to = None;
            if is_file {
                let pos = sh.pos_ms.load(Ordering::Relaxed);
                sh.dur_ms.fetch_max(pos, Ordering::Relaxed);
                if sh.looping.load(Ordering::Relaxed) {
                    need_open = true;
                } else {
                    sh.ended.store(true, Ordering::Relaxed);
                }
            } else {
                need_open = true;
            }
            continue;
        }
        if flags & (MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32) != 0 {
            if let Ok(d) = unsafe { current_dims(r) } {
                dims = d;
                *sh.size.lock().unwrap() = (d.w, d.h);
            }
        }
        let Some(s) = sample else { continue };
        if is_file {
            if seek_to.is_some_and(|target| ts < target) {
                continue;
            }
            if resume_at.is_some_and(|last| ts <= last) {
                continue;
            }
            resume_at = None;
            if base_ts < 0 {
                base_ts = ts;
                t0 = Instant::now();
            }
            let target = Duration::from_nanos(((ts - base_ts).max(0) as u64) * 100);
            let el = t0.elapsed();
            if target > el {
                std::thread::sleep(target - el);
            }
            sh.pos_ms.store((ts / 10_000).max(0) as u64, Ordering::Relaxed);
        }
        let (w, h) = (dims.w, dims.h);
        if w == 0 || h == 0 {
            continue;
        }
        let key = sh.key.load(Ordering::Relaxed);
        if let Ok(rgba) = unsafe { copy_sample(&s, &dims, key) } {
            *sh.frame.lock().unwrap() = Some(Frame { w, h, rgba });
            sh.seq.fetch_add(1, Ordering::Relaxed);
            last_frame_ts = Some(ts);
            seek_to = None;
        } else if manager.take().is_some() {
            resume_at = seek_to.map(|target| target - 1).or(last_frame_ts);
            need_open = true;
        }
    }
}

// ---------------------------------------------------------------- recording

impl RecordFormat {
    fn subtype(self) -> GUID {
        match self {
            Self::H264 => MFVideoFormat_H264,
            Self::Hevc => MFVideoFormat_HEVC,
            Self::Wmv => MFVideoFormat_WMV3,
        }
    }
}

unsafe fn make_writer(path: &str, w: u32, h: u32, opts: &RecOpts) -> Res<(IMFSinkWriter, u32)> {
    let bitrate = opts.mbps.clamp(1, 100) * 1_000_000;
    let mut a: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut a, 1)?;
    let a = a.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    a.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, opts.hw as u32)?;
    let wr = MFCreateSinkWriterFromURL(&HSTRING::from(path), None::<&IMFByteStream>, &a)?;
    let size = ((w as u64) << 32) | h as u64;
    let (rn, rd) = rate_fraction(opts.fps);
    let rate = (u64::from(rn) << 32) | u64::from(rd);
    let par = (1u64 << 32) | 1;
    let out = MFCreateMediaType()?;
    out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    out.SetGUID(&MF_MT_SUBTYPE, &opts.format.subtype())?;
    out.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
    out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    out.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
    out.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
    out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, par)?;
    let idx = wr.AddStream(&out)?;
    let inp = MFCreateMediaType()?;
    inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    inp.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
    inp.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    inp.SetUINT64(&MF_MT_FRAME_SIZE, size)?;
    inp.SetUINT64(&MF_MT_FRAME_RATE, rate)?;
    inp.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, par)?;
    inp.SetUINT32(&MF_MT_DEFAULT_STRIDE, w * 4)?;
    wr.SetInputMediaType(idx, &inp, None)?;
    wr.BeginWriting()?;
    Ok((wr, idx))
}

unsafe fn encoder_name(writer: &IMFSinkWriter, stream: u32, format: RecordFormat) -> String {
    let codec = format.codec();
    let unknown = format!("{codec} encoder not reported by Windows");
    let Ok(writer) = writer.cast::<IMFSinkWriterEx>() else { return unknown };
    for index in 0.. {
        let mut category = GUID::zeroed();
        let mut transform = None;
        if writer.GetTransformForStream(stream, index, Some(&mut category), &mut transform).is_err() {
            break;
        }
        if category != MFT_CATEGORY_VIDEO_ENCODER {
            continue;
        }
        let Some(attrs) = transform.and_then(|mft| mft.GetAttributes().ok()) else { break };
        let read = |key| {
            let len = attrs.GetStringLength(key).ok()?;
            let mut value = vec![0; len as usize + 1];
            attrs.GetString(key, &mut value, None).ok()?;
            Some(String::from_utf16_lossy(&value[..len as usize]))
        };
        let name = read(&MFT_FRIENDLY_NAME_Attribute);
        if attrs.GetStringLength(&MFT_ENUM_HARDWARE_URL_Attribute).is_ok() {
            let vendor = read(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).unwrap_or_default().to_ascii_uppercase();
            return match vendor.as_str() {
                "VEN_10DE" => format!("NVIDIA {codec} (hardware)"),
                "VEN_8086" => format!("Intel {codec} (hardware)"),
                "VEN_1002" => format!("AMD {codec} (hardware)"),
                _ => format!("{} (hardware)", name.as_deref().unwrap_or(codec)),
            };
        }
        return name.unwrap_or_else(|| format!("{codec} (software)"));
    }
    unknown
}

pub(crate) fn record_thread(path: String, rx: Receiver<RecFrame>, err: Arc<Mutex<String>>, encoder: Arc<Mutex<String>>, opts: RecOpts) {
    if !mf_init() {
        *err.lock().unwrap() = "Media Foundation not available".into();
        return;
    }
    let mut writer: Option<(IMFSinkWriter, u32, usize, usize)> = None;
    let mut first_frame = None;
    let mut last_t: i64 = -1;
    let (rn, rd) = rate_fraction(opts.fps);
    let frame_dur = (10_000_000u64 * u64::from(rd) / u64::from(rn.max(1))) as i64;
    while let Ok(RecFrame { mut pixels, w: mut fw, h: mut fh, captured_at, bgra }) = rx.recv() {
        // Scale the picture to the project's Output Size.
        if let Some((tw, th)) = opts.size {
            let (tw, th) = ((tw as usize & !1).max(2), (th as usize & !1).max(2));
            if (fw, fh) != (tw, th) && fw > 0 && fh > 0 {
                let Some(img) = image::RgbaImage::from_raw(fw as u32, fh as u32, Arc::unwrap_or_clone(pixels)) else { continue };
                pixels = Arc::new(image::imageops::resize(&img, tw as u32, th as u32, image::imageops::FilterType::Triangle).into_raw());
                fw = tw;
                fh = th;
            }
        }
        if writer.is_none() {
            let w = (fw & !1).max(2);
            let h = (fh & !1).max(2);
            match unsafe { make_writer(&path, w as u32, h as u32, &opts) } {
                Ok((wr, idx)) => {
                    *encoder.lock().unwrap() = unsafe { encoder_name(&wr, idx, opts.format) };
                    writer = Some((wr, idx, w, h));
                }
                Err(e) => {
                    *err.lock().unwrap() = format!("Could not start {} recording: {}", opts.format.codec(), e.message());
                    return;
                }
            }
        }
        let Some((wr, idx, w, h)) = writer.as_ref() else { continue };
        let (w, h) = (*w, *h);
        let len = w * h * 4;
        let start = *first_frame.get_or_insert(captured_at);
        let mut t = (captured_at.duration_since(start).as_nanos() / 100) as i64;
        if t <= last_t {
            t = last_t + 1;
        }
        last_t = t;
        let r = unsafe {
            (|| -> Res<()> {
                let buf = MFCreateMemoryBuffer(len as u32)?;
                let mut p: *mut u8 = std::ptr::null_mut();
                buf.Lock(&mut p, None, None)?;
                let dst = std::slice::from_raw_parts_mut(p, len);
                if bgra && (fw, fh) == (w, h) {
                    dst.copy_from_slice(&pixels);
                } else {
                    for y in 0..h {
                        for x in 0..w {
                            let d = (y * w + x) * 4;
                            if x < fw && y < fh {
                                let s = (y * fw + x) * 4;
                                let (b, r) = if bgra { (s, s + 2) } else { (s + 2, s) };
                                dst[d..d + 4].copy_from_slice(&[pixels[b], pixels[s + 1], pixels[r], 255]);
                            } else {
                                dst[d..d + 4].copy_from_slice(&[0, 0, 0, 255]);
                            }
                        }
                    }
                }
                buf.Unlock()?;
                buf.SetCurrentLength(len as u32)?;
                let s = MFCreateSample()?;
                s.AddBuffer(&buf)?;
                s.SetSampleTime(t)?;
                s.SetSampleDuration(frame_dur)?;
                wr.WriteSample(*idx, &s)?;
                Ok(())
            })()
        };
        if let Err(e) = r {
            *err.lock().unwrap() = format!("Recording error: {}", e.message());
        }
    }
    if let Some((wr, ..)) = writer {
        if let Err(error) = unsafe { wr.Finalize() } {
            let mut status = err.lock().unwrap();
            if status.is_empty() {
                *status = format!("Could not finalize recording: {}", error.message());
            }
        }
    }
}

// ---------------------------------------------------------------- file picker

/// Opens the Windows file picker on its own thread; the answer arrives on the channel.
pub fn browse(kind: PickKind) -> Receiver<Option<String>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let r = (|| -> Res<String> {
            use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, FOS_ALLOWMULTISELECT, FOS_PICKFOLDERS};
            let name_of = |it: &windows::Win32::UI::Shell::IShellItem| -> Res<String> {
                let p = it.GetDisplayName(SIGDN_FILESYSPATH)?;
                let s = p.to_string().unwrap_or_default();
                CoTaskMemFree(Some(p.0 as *const _));
                Ok(s)
            };
            let preset = COMDLG_FILTERSPEC { pszName: windows::core::w!("Project preset (*.vmix)"), pszSpec: windows::core::w!("*.vmix") };
            if kind == PickKind::PresetSave {
                let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
                d.SetFileTypes(&[preset])?;
                d.SetDefaultExtension(windows::core::w!("vmix"))?;
                d.Show(HWND::default())?;
                return name_of(&d.GetResult()?);
            }
            let d: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let video = COMDLG_FILTERSPEC {
                pszName: windows::core::w!("Video files"),
                pszSpec: windows::core::w!("*.mp4;*.mov;*.m4v;*.wmv;*.avi;*.mkv;*.mpg;*.mpeg;*.ts;*.mts"),
            };
            let image = COMDLG_FILTERSPEC { pszName: windows::core::w!("Pictures"), pszSpec: windows::core::w!("*.png;*.jpg;*.jpeg;*.bmp;*.tif;*.tiff;*.gif") };
            let all = COMDLG_FILTERSPEC { pszName: windows::core::w!("All files"), pszSpec: windows::core::w!("*.*") };
            match kind {
                PickKind::PresetOpen => d.SetFileTypes(&[preset, all])?,
                PickKind::PresetSave => unreachable!(),
                PickKind::Video => d.SetFileTypes(&[video, all])?,
                PickKind::Image => d.SetFileTypes(&[image, all])?,
                PickKind::Title => d.SetFileTypes(&[
                    COMDLG_FILTERSPEC { pszName: windows::core::w!("GT Designer titles"), pszSpec: windows::core::w!("*.gtzip") },
                    all,
                ])?,
                PickKind::VideoMany => {
                    d.SetFileTypes(&[video, all])?;
                    d.SetOptions(d.GetOptions()? | FOS_ALLOWMULTISELECT)?;
                }
                PickKind::Folder => d.SetOptions(d.GetOptions()? | FOS_PICKFOLDERS)?,
            }
            d.Show(HWND(std::ptr::null_mut()))?;
            if kind == PickKind::VideoMany {
                let items = d.GetResults()?;
                let mut out = Vec::new();
                for i in 0..items.GetCount()? {
                    out.push(name_of(&items.GetItemAt(i)?)?);
                }
                return Ok(out.join("\n"));
            }
            name_of(&d.GetResult()?)
        })();
        let _ = tx.send(r.ok().filter(|s| !s.is_empty()));
    });
    rx
}

// ---------------------------------------------------------------- desktop and pictures

/// Desktop capture of the main screen (GDI BitBlt), ~20 frames a second (estimated rate).
fn run_desktop(sh: Arc<Shared>) {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe {
        let w = GetSystemMetrics(SM_CXSCREEN).max(1);
        let h = GetSystemMetrics(SM_CYSCREEN).max(1);
        let screen = GetDC(HWND::default());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bmp);
        let mut bi = BITMAPINFO::default();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // top-down rows
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;
        let (wu, hu) = (w as usize, h as usize);
        let mut buf = vec![0u8; wu * hu * 4];
        *sh.size.lock().unwrap() = (wu, hu);
        sh.set_status("");
        while !sh.stop.load(Ordering::Relaxed) {
            let t0 = Instant::now();
            if BitBlt(mem, 0, 0, w, h, screen, 0, 0, SRCCOPY | CAPTUREBLT).is_ok()
                && GetDIBits(mem, bmp, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS) > 0
            {
                let mut rgba = vec![0u8; wu * hu * 4];
                copy_rows(buf.as_ptr(), (wu * 4) as isize, wu, hu, &mut rgba, sh.key.load(Ordering::Relaxed));
                *sh.frame.lock().unwrap() = Some(Frame { w: wu, h: hu, rgba });
                sh.seq.fetch_add(1, Ordering::Relaxed);
            } else {
                sh.set_status("Desktop capture failed");
            }
            let el = t0.elapsed();
            if el < Duration::from_millis(50) {
                std::thread::sleep(Duration::from_millis(50) - el);
            }
        }
        SelectObject(mem, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(HWND::default(), screen);
    }
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

fn local_time() -> windows::Win32::Foundation::SYSTEMTIME {
    unsafe { windows::Win32::System::SystemInformation::GetLocalTime() }
}

pub fn local_clock_ms() -> u32 {
    let t = local_time();
    ((u32::from(t.wHour) * 60 + u32::from(t.wMinute)) * 60 + u32::from(t.wSecond)) * 1000
        + u32::from(t.wMilliseconds)
}

/// The original's RecordingFilenameFormat "{0} - {1:dd MMMM yyyy - hh-mm-ss tt}"
/// (config default) — e.g. "capture - 06 November 2026 - 01-23-45 AM".
pub fn recording_name(base: &str) -> String {
    let t = local_time();
    let h12 = match t.wHour % 12 {
        0 => 12,
        h => h,
    };
    format!(
        "{} - {:02} {} {:04} - {:02}-{:02}-{:02} {}",
        base,
        t.wDay,
        MONTHS[(t.wMonth.clamp(1, 12) - 1) as usize],
        t.wYear,
        h12,
        t.wMinute,
        t.wSecond,
        if t.wHour < 12 { "AM" } else { "PM" }
    )
}

/// Snapshot default name: "<prefix>" + "dd MMMM yyyy HH-mm-ss" (kb::c @0xe3850).
pub fn snapshot_name(prefix: &str) -> String {
    let t = local_time();
    format!("{}{:02} {} {:04} {:02}-{:02}-{:02}", prefix, t.wDay, MONTHS[(t.wMonth.clamp(1, 12) - 1) as usize], t.wYear, t.wHour, t.wMinute, t.wSecond)
}

/// The original's storage folder: StoragePath, or Documents\<app>Storage when empty
/// (rzzzzzc::zd @0x1b9700 / rzzzzzc::za @0x1b9668, SpecialFolder 5 = Documents).
/// Screens attached to this computer as [left, top, width, height] in pixels, the main
/// screen first (for the Fullscreen display's "Display:" choice).
pub fn monitors() -> Vec<[i32; 4]> {
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    unsafe extern "system" fn each(m: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<([i32; 4], bool)>);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(m, &mut info).as_bool() {
            let r = info.rcMonitor;
            list.push(([r.left, r.top, r.right - r.left, r.bottom - r.top], info.dwFlags & 1 != 0));
        }
        BOOL(1)
    }
    let mut list: Vec<([i32; 4], bool)> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    // MONITORINFOF_PRIMARY = 1: the main screen first, the others left to right.
    list.sort_by_key(|(r, primary)| (!*primary, r[0], r[1]));
    list.into_iter().map(|(r, _)| r).collect()
}

pub fn storage_folder() -> std::path::PathBuf {
    let docs = unsafe {
        windows::Win32::UI::Shell::SHGetKnownFolderPath(
            &windows::Win32::UI::Shell::FOLDERID_Documents,
            windows::Win32::UI::Shell::KNOWN_FOLDER_FLAG(0),
            windows::Win32::Foundation::HANDLE::default(),
        )
        .ok()
        .map(|p| {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        })
    }
    .filter(|s| !s.is_empty())
    .map(std::path::PathBuf::from)
    .unwrap_or_else(|| std::env::var("USERPROFILE").map(|u| std::path::PathBuf::from(u).join("Documents")).unwrap_or_else(|_| ".".into()));
    let dir = docs.join("LiveMixStorage");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Snapshot "Save As" dialog like the original (kb::c @0xe3850): filter
/// "PNG (*.png)|*.png|JPEG (*.jpg)|*.jpg|BMP|*.bmp", remembered filter index.
/// Answer: (path, 0-based filter index).
pub fn save_picture_dialog(default_name: String, filter_index: u32) -> Receiver<Option<(String, u32)>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let r = (|| -> Res<(String, u32)> {
            let d: windows::Win32::UI::Shell::IFileSaveDialog =
                CoCreateInstance(&windows::Win32::UI::Shell::FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
            let specs = [
                COMDLG_FILTERSPEC { pszName: windows::core::w!("PNG (*.png)"), pszSpec: windows::core::w!("*.png") },
                COMDLG_FILTERSPEC { pszName: windows::core::w!("JPEG (*.jpg)"), pszSpec: windows::core::w!("*.jpg") },
                COMDLG_FILTERSPEC { pszName: windows::core::w!("BMP"), pszSpec: windows::core::w!("*.bmp") },
            ];
            d.SetFileTypes(&specs)?;
            d.SetFileTypeIndex(filter_index.min(2) + 1)?;
            d.SetDefaultExtension(windows::core::w!("png"))?;
            d.SetFileName(&HSTRING::from(default_name.as_str()))?;
            d.Show(HWND(std::ptr::null_mut()))?;
            let it = d.GetResult()?;
            let p = it.GetDisplayName(SIGDN_FILESYSPATH)?;
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            let idx = d.GetFileTypeIndex().unwrap_or(1).max(1) - 1;
            Ok((s, idx))
        })();
        let _ = tx.send(r.ok().filter(|(s, _)| !s.is_empty()));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_devices_without_crashing() {
        let d = list_devices();
        println!("video devices: {:?}", d);
    }

    #[test]
    fn records_mp4_and_plays_it_back() {
        let dir = std::env::current_dir().unwrap().join("target").join("media-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("test-{}.mp4", std::process::id())).to_string_lossy().to_string();
        let (rec, worker) = Recorder::spawn_writer(path.clone(), RecOpts { hw: false, ..RecOpts::default() });
        let error = Arc::clone(&rec.error);
        let first_capture = Instant::now();
        {
            for f in 0..45u32 {
                let (w, h) = (320usize, 180usize);
                let mut px = vec![0u8; w * h * 4];
                for y in 0..h {
                    for x in 0..w {
                        let d = (y * w + x) * 4;
                        px[d] = (x as u32 * 255 / w as u32) as u8;
                        px[d + 1] = (f * 5) as u8;
                        px[d + 2] = (y as u32 * 255 / h as u32) as u8;
                        px[d + 3] = 255;
                    }
                }
                let mut frame = RecFrame {
                    pixels: Arc::new(px), w, h, bgra: false,
                    captured_at: first_capture + Duration::from_millis(33 * u64::from(f)),
                };
                loop {
                    match rec.tx.try_send(frame) {
                        Ok(()) => break,
                        Err(std::sync::mpsc::TrySendError::Full(pending)) => {
                            assert!(first_capture.elapsed() < Duration::from_secs(30), "The encoder stopped consuming frames");
                            frame = pending;
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                            panic!("Recorder stopped: {}", error.lock().unwrap());
                        }
                    }
                }
            }
        }
        drop(rec);
        let wait = Instant::now();
        while !worker.is_finished() && wait.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(worker.is_finished(), "The MP4 writer did not finalize");
        worker.join().unwrap();
        assert_eq!(error.lock().unwrap().as_str(), "");
        let meta = std::fs::metadata(&path).expect("mp4 written");
        println!("mp4 size {}", meta.len());
        assert!(meta.len() > 1000);
        let sh = start(Source::File(path.clone()));
        let t = Instant::now();
        while sh.seq.load(Ordering::Relaxed) < 5 && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(50));
        }
        println!("status: {:?}", sh.status.lock().unwrap());
        let frames = sh.seq.load(Ordering::Relaxed);
        let size = *sh.size.lock().unwrap();
        if let Some(f) = sh.frame.lock().unwrap().as_ref() {
            let top = f.rgba[2];
            let bottom = f.rgba[(f.h - 1) * f.w * 4 + 2];
            println!("blue at top {top}, at bottom {bottom}");
            assert!(top < 80 && bottom > 170, "picture must not be upside down");
        }
        println!("frames read {frames}, size {:?}", size);
        assert!(frames >= 5);
        assert_eq!(size, (320, 180));

        sh.paused.store(true, Ordering::Relaxed);
        let seek = |ms| {
            let previous = sh.seq.load(Ordering::Relaxed);
            sh.seek_ms.store(ms + 1, Ordering::Relaxed);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if sh.seq.load(Ordering::Relaxed) > previous {
                    let pos = sh.pos_ms.load(Ordering::Relaxed);
                    if (ms..=ms + 100).contains(&pos) {
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        let seeks_while_paused = seek(850);
        let restarts_while_paused = seek(0);
        sh.stop.store(true, Ordering::Relaxed);
        assert!(seeks_while_paused, "seek must decode a new frame while paused");
        assert!(restarts_while_paused, "restart must return to the beginning while paused");
    }
}

#[cfg(test)]
mod camera_test {
    use super::*;

    /// Opens the first real camera for a moment and checks frames arrive.
    #[test]
    fn first_camera_delivers_frames() {
        let Some(dev) = list_devices().into_iter().next() else { return };
        let sh = start(Source::Device(dev.link.clone(), None));
        let t = Instant::now();
        while sh.seq.load(Ordering::Relaxed) < 3 && t.elapsed() < Duration::from_secs(12) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let frames = sh.seq.load(Ordering::Relaxed);
        println!("{}: frames {frames}, size {:?}, status {:?}", dev.name, *sh.size.lock().unwrap(), sh.status.lock().unwrap());
        sh.stop.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(300));
        assert!(frames >= 3);
    }
}

#[cfg(test)]
mod desktop_test {
    use super::*;

    #[test]
    fn desktop_capture_delivers_frames() {
        let sh = start(Source::Desktop);
        let t = Instant::now();
        while sh.seq.load(Ordering::Relaxed) < 3 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let frames = sh.seq.load(Ordering::Relaxed);
        println!("desktop frames {frames}, size {:?}", *sh.size.lock().unwrap());
        sh.stop.store(true, Ordering::Relaxed);
        assert!(frames >= 3);
    }

    #[test]
    fn photos_reads_a_folder() {
        let dir = std::env::current_dir().unwrap().join("target").join("photos-test");
        let _ = std::fs::create_dir_all(&dir);
        for (n, c) in [[255u8, 0, 0, 255], [0, 255, 0, 255]].iter().enumerate() {
            let img = image::RgbaImage::from_pixel(64, 36, image::Rgba(*c));
            img.save(dir.join(format!("p{n}.png"))).unwrap();
        }
        let sh = start(Source::Sequence(dir.to_string_lossy().to_string(), 30.0));
        let t = Instant::now();
        while sh.seq.load(Ordering::Relaxed) < 2 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        sh.stop.store(true, Ordering::Relaxed);
        assert!(sh.seq.load(Ordering::Relaxed) >= 2);
        assert_eq!(*sh.size.lock().unwrap(), (64, 36));
    }
}
