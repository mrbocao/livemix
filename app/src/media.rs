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

use windows::core::{AgileReference, Interface, GUID, HSTRING, PROPVARIANT, PWSTR};
use windows::Win32::Foundation::{E_FAIL, HWND};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, IDXGIFactory6,
    DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE};
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

#[cfg(test)]
pub fn start(src: Source) -> Arc<Shared> {
    start_with_pause(src, false)
}

#[cfg(test)]
pub fn start_with_pause(src: Source, paused: bool) -> Arc<Shared> {
    start_reader(src, paused, false)
}

pub fn start_for_gl(src: Source, paused: bool) -> Arc<Shared> {
    start_reader(src, paused, true)
}

fn start_reader(src: Source, paused: bool, gpu_playback: bool) -> Arc<Shared> {
    let sh = Arc::new(Shared::default());
    sh.gpu_playback.store(gpu_playback, Ordering::Relaxed);
    sh.paused.store(paused, Ordering::Relaxed);
    sh.set_status("Starting…");
    if let Source::File(path) | Source::AutoFile(path, _, _) | Source::FfmpegFile(path, _, _) | Source::DirectShowFile(path, _, _, _) = &src {
        let (source, path) = (sh.clone(), path.clone());
        std::thread::spawn(move || crate::audio::play_file(source, path));
    }
    let s2 = sh.clone();
    std::thread::spawn(move || match src {
        Source::DeckLink(config) => crate::decklink::run_capture(s2, config),
        Source::Ndi(name) => crate::ndi::run_capture(s2, &name),
        Source::Omt(address) => crate::omt::run_capture(s2, &address),
        Source::Srt(config) => crate::srt::run_capture(s2, config),
        Source::AudioDevice(device) => crate::audio::capture_device(s2, &device),
        Source::AudioFile(path) => crate::audio::play_audio_input(s2, path),
        Source::Presentation(path) => crate::slides::run(s2, path),
        Source::Web(url, transparent) => crate::web_input::run(s2, url, transparent),
        Source::FfmpegFile(path, deinterlace, hardware) => crate::ffmpeg_media::play_configured(&s2, &path, deinterlace, hardware, None),
        Source::Desktop => run_desktop(s2),
        Source::Sequence(folder, fps) => run_pictures(s2, folder, true, fps, 0.0),
        Source::Photos(folder, secs) => run_pictures(s2, folder, false, 0.0, secs),
        Source::DirectShowFile(path, config, deinterlace, hardware) => {
            if let Err(error) = crate::directshow::play(&s2, &path, &config) {
                if s2.seq.load(Ordering::Relaxed) == 0 && !s2.stop.load(Ordering::Relaxed) {
                    let reason = format!("DirectShow: {error}");
                    crate::ffmpeg_media::play_configured(&s2, &path, deinterlace, hardware, Some(&reason));
                } else {
                    s2.set_status(format!("DirectShow: {error}"));
                }
            }
        }
        Source::AutoFile(path, deinterlace, hardware) => run(s2, Source::File(path), Some((deinterlace, hardware))),
        other => run(s2, other, None),
    });
    sh
}

unsafe fn gpu_picture(sample: &IMFSample, dims: &Dims) -> Res<crate::dx_interop::Picture> {
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8X8_UNORM};
    let buffer = sample.GetBufferByIndex(0)?.cast::<IMFDXGIBuffer>()?;
    let mut resource = std::ptr::null_mut();
    buffer.GetResource(&ID3D11Texture2D::IID, &mut resource)?;
    if resource.is_null() { return Err(windows::core::Error::from(E_FAIL)); }
    let texture = ID3D11Texture2D::from_raw(resource);
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    texture.GetDesc(&mut desc);
    if !matches!(desc.Format, DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8X8_UNORM)
        || dims.ox + dims.w > desc.Width as usize || dims.oy + dims.h > desc.Height as usize
        || desc.SampleDesc.Count != 1
    {
        return Err(windows::core::Error::from(E_FAIL));
    }
    let device = texture.GetDevice()?;
    let subresource = buffer.GetSubresourceIndex()?;
    if desc.MipLevels != 1 { return Err(windows::core::Error::from(E_FAIL)); }
    let crop = D3D11_BOX {
        left: dims.ox as u32, top: dims.oy as u32, front: 0,
        right: (dims.ox + dims.w) as u32, bottom: (dims.oy + dims.h) as u32, back: 1,
    };
    desc.Width = dims.w as u32;
    desc.Height = dims.h as u32;
    desc.ArraySize = 1;
    desc.Usage = D3D11_USAGE_DEFAULT;
    desc.BindFlags = (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32;
    desc.CPUAccessFlags = 0;
    desc.MiscFlags = 0;
    let mut copy = None;
    device.CreateTexture2D(&desc, None, Some(&mut copy))?;
    let copy = copy.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    // Make an owned GPU copy so the decoder can recycle its sample immediately.
    device.GetImmediateContext()?.CopySubresourceRegion(&copy, 0, 0, 0, 0, &texture, subresource, Some(&crop));
    Ok(crate::dx_interop::Picture { surface: crate::dx_interop::Surface(copy), size: [dims.w, dims.h] })
}

#[derive(Default)]
struct VideoDeviceSelection {
    preference: String,
    device: Option<(AgileReference<IMFDXGIDeviceManager>, String)>,
}

static VIDEO_DEVICE: std::sync::LazyLock<Mutex<VideoDeviceSelection>> =
    std::sync::LazyLock::new(|| Mutex::new(VideoDeviceSelection::default()));

pub fn select_video_adapter(preference: &str) {
    let mut selection = VIDEO_DEVICE.lock().unwrap();
    if selection.preference != preference {
        selection.preference = preference.to_owned();
        selection.device = None;
    }
}

fn video_adapters() -> Vec<(IDXGIAdapter1, String)> {
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else { return Vec::new() };
    let preferred = factory.cast::<IDXGIFactory6>().ok();
    (0..).map_while(|index| unsafe {
        match &preferred {
            Some(factory) => factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(index, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE).ok(),
            None => factory.EnumAdapters1(index).ok(),
        }
    }).filter_map(|adapter| {
        let desc = unsafe { adapter.GetDesc1().ok()? };
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 { return None; }
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_owned();
        Some((adapter, name))
    }).collect()
}

pub fn video_adapter_names() -> Vec<String> {
    video_adapters().into_iter().map(|(_, name)| name).collect()
}

pub(crate) fn renderer_matches_adapter(renderer: &str, adapter: &str) -> bool {
    !adapter.is_empty() && renderer.to_ascii_lowercase().contains(&adapter.to_ascii_lowercase())
}

unsafe fn video_device_manager() -> Res<(IMFDXGIDeviceManager, String)> {
    let mut selection = VIDEO_DEVICE.lock().unwrap();
    if let Some((manager, adapter)) = &selection.device {
        return Ok((manager.resolve()?, adapter.clone()));
    }
    let (manager, adapter) = create_video_device_manager(&selection.preference)?;
    selection.device = Some((AgileReference::new(&manager)?, adapter.clone()));
    Ok((manager, adapter))
}

pub(crate) unsafe fn shared_d3d11_device() -> Res<windows::Win32::Graphics::Direct3D11::ID3D11Device> {
    use windows::Win32::Graphics::Direct3D11::ID3D11Device;
    if !mf_init() { return Err(windows::core::Error::from(E_FAIL)); }
    let (manager, _) = video_device_manager()?;
    let handle = manager.OpenDeviceHandle()?;
    let mut device = std::ptr::null_mut();
    let result = manager.GetVideoService(handle, &ID3D11Device::IID, &mut device);
    let _ = manager.CloseDeviceHandle(handle);
    result?;
    if device.is_null() { return Err(windows::core::Error::from(E_FAIL)); }
    Ok(ID3D11Device::from_raw(device))
}

unsafe fn create_video_device_manager(preference: &str) -> Res<(IMFDXGIDeviceManager, String)> {
    let adapter = if preference.is_empty() { None } else {
        video_adapters().into_iter().find(|(_, name)| preference == "@high" || preference == name)
            .map(|(adapter, _)| adapter)
    };
    let mut device = None;
    D3D11CreateDevice(
        adapter.as_deref(),
        if adapter.is_some() { D3D_DRIVER_TYPE_UNKNOWN } else { D3D_DRIVER_TYPE_HARDWARE },
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
    let desc = device.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?;
    let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_owned();
    Ok((manager, name))
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
    if matches!(src, Source::Device(..)) && CAPTURE_LOW_LATENCY.load(Ordering::Relaxed) {
        attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
    }
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
    if key == 0 {
        for (y, dst) in out.chunks_exact_mut(w * 4).take(h).enumerate() {
            let src = std::slice::from_raw_parts(p0.offset(y as isize * pitch), w * 4);
            for (pixel, bgra) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                pixel.copy_from_slice(&[bgra[2], bgra[1], bgra[0], 255]);
            }
        }
        return;
    }
    let (kr, kg, kb, tol) = ((key >> 16) & 255, (key >> 8) & 255, key & 255, (key >> 24) & 255);
    let t = tol as f32 * 1.7;
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
            {
                // Simple colour-distance chroma key with a soft edge.
                let dr = r as i32 - kr as i32;
                let dg = g as i32 - kg as i32;
                let db = b as i32 - kb as i32;
                let d2 = (dr * dr + dg * dg + db * db) as f32;
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
    let buf = if s.GetBufferCount()? == 1 {
        s.GetBufferByIndex(0)?
    } else {
        s.ConvertToContiguousBuffer()?
    };
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

fn run(sh: Arc<Shared>, src: Source, fallback: Option<(u8, bool)>) {
    let try_fallback = |reason: &str| {
        if let (Source::File(path), Some((deinterlace, hardware))) = (&src, fallback) {
            if !sh.stop.load(Ordering::Relaxed) && sh.seq.load(Ordering::Relaxed) == 0 {
                crate::ffmpeg_media::play_configured(&sh, path, deinterlace, hardware, Some(reason));
                return true;
            }
        }
        false
    };
    if !mf_init() {
        if try_fallback("Media Foundation unavailable") { return; }
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
    let mut needs_picture = true;
    let mut last_decoder_gpu = None;
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
                open(&src, manager.as_ref().map(|(manager, _)| manager), &sh).or_else(|e| {
                    if manager.take().is_some() { open(&src, None, &sh) } else { Err(e) }
                })
            };
            match r {
                Ok((r, d)) => {
                    *sh.video_adapter.lock().unwrap() = if is_file {
                        Some(manager.as_ref().map_or_else(|| "System-memory reader".into(), |(_, name)| name.clone()))
                    } else { None };
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
                    if try_fallback(&format!("Media Foundation could not open the file ({})", e.message())) { return; }
                    sh.set_status(format!("No signal ({})", e.message()));
                    std::thread::sleep(Duration::from_millis(1500));
                    continue;
                }
            }
            need_open = false;
            needs_picture = true;
            if is_file { crate::audio::reset_position(&sh); }
            t0 = Instant::now();
            base_ts = -1;
            last_frame_ts = None;
        }
        if is_file {
            if let Some(ms) = sh.seek_ms.swap(0, Ordering::Relaxed).checked_sub(1) {
                let duration = sh.dur_ms.load(Ordering::Relaxed);
                let margin = if sh.precise_seek.load(Ordering::Relaxed) { 1 } else { 50 };
                let ms = if duration > 0 { ms.min(duration.saturating_sub(margin)) } else { ms };
                let ts = (ms.min(i64::MAX as u64 / 10_000) * 10_000) as i64;
                if let Some(r) = &reader {
                    match unsafe { r.SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(ts)) } {
                        Ok(()) => {
                            crate::audio::reset_position(&sh);
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
            while seek_to.is_none() && ((!needs_picture && sh.paused.load(Ordering::Relaxed)) || sh.ended.load(Ordering::Relaxed))
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
            if try_fallback(&format!("Media Foundation failed before the first frame ({})", e.message())) { return; }
            sh.set_status(format!("Lost signal ({})", e.message()));
            std::thread::sleep(Duration::from_millis(500));
            continue;
        }
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            if try_fallback("Media Foundation reached the end before the first frame") { return; }
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
            crate::audio::mark_position(&sh, (ts / 10_000).max(0) as u64);
        }
        let (w, h) = (dims.w, dims.h);
        if w == 0 || h == 0 {
            continue;
        }
        let key = sh.key.load(Ordering::Relaxed);
        if is_file && sh.gpu_playback.load(Ordering::Acquire) && !sh.cpu_frames_required.load(Ordering::Acquire) {
            if let Ok(picture) = unsafe { gpu_picture(&s, &dims) } {
                if last_decoder_gpu != Some(true) {
                    *sh.decoder.lock().unwrap() = "Media Foundation · D3D11 texture".into();
                    last_decoder_gpu = Some(true);
                }
                *sh.frame.lock().unwrap() = None;
                *sh.gpu_frame.lock().unwrap() = Some(picture);
                sh.seq.fetch_add(1, Ordering::Release);
                needs_picture = false;
                last_frame_ts = Some(ts);
                seek_to = None;
                continue;
            }
        }
        if let Ok(rgba) = unsafe { copy_sample(&s, &dims, key) } {
            if is_file && last_decoder_gpu != Some(false) {
                *sh.decoder.lock().unwrap() = "Media Foundation · system memory".into();
                last_decoder_gpu = Some(false);
            }
            *sh.gpu_frame.lock().unwrap() = None;
            needs_picture = false;
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
            Self::Wmv9 => MFVideoFormat_WMV3,
            _ => unreachable!("FFmpeg profiles bypass the Media Foundation writer"),
        }
    }
}

unsafe fn make_writer(path: &str, w: u32, h: u32, opts: &RecOpts, manager: Option<&IMFDXGIDeviceManager>) -> Res<(IMFSinkWriter, u32, Option<u32>)> {
    let bitrate = opts.mbps.clamp(1, 100) * 1_000_000;
    let mut a: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut a, 2)?;
    let a = a.ok_or_else(|| windows::core::Error::from(E_FAIL))?;
    a.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, opts.hw as u32)?;
    if let Some(manager) = manager {
        a.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, manager)?;
    }
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
    let audio = if opts.audio {
        let make_audio_type = |subtype, bytes_per_second, alignment| -> Res<IMFMediaType> {
            let ty = MFCreateMediaType()?;
            ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            ty.SetGUID(&MF_MT_SUBTYPE, subtype)?;
            ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
            ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, crate::audio::RATE)?;
            ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
            ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, bytes_per_second)?;
            ty.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, alignment)?;
            Ok(ty)
        };
        let target = if opts.format == RecordFormat::Wmv9 {
            let types = MFTranscodeGetAudioOutputAvailableTypes(&MFAudioFormat_WMAudioV8,
                MFT_ENUM_FLAG_ALL.0 as u32, None::<&IMFAttributes>)?;
            let mut chosen = None;
            let mut best = 0;
            for i in 0..types.GetElementCount()? {
                let ty: IMFMediaType = types.GetElement(i)?.cast()?;
                let bytes = ty.GetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND).unwrap_or(0);
                if ty.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).ok() == Some(crate::audio::RATE)
                    && ty.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).ok() == Some(2)
                    && bytes > best && bytes <= 24_000
                {
                    best = bytes;
                    chosen = Some(ty);
                }
            }
            chosen.ok_or_else(|| windows::core::Error::from(E_FAIL))?
        } else {
            make_audio_type(&MFAudioFormat_AAC, 16_000, 1)?
        };
        let pcm = make_audio_type(&MFAudioFormat_PCM, crate::audio::RATE * 4, 4)?;
        let index = wr.AddStream(&target)?;
        wr.SetInputMediaType(index, &pcm, None)?;
        Some(index)
    } else {
        None
    };
    wr.BeginWriting()?;
    Ok((wr, idx, audio))
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

pub(crate) fn record_thread(path: String, rx: Receiver<RecFrame>, err: Arc<Mutex<String>>, encoder: Arc<Mutex<String>>, opts: RecOpts,
    index: Option<ReplayIndex>, gpu: Arc<std::sync::atomic::AtomicBool>)
{
    if !opts.format.native_windows() {
        if let Err(message) = crate::ffmpeg_media::record(&path, rx, &encoder, opts, index.as_ref()) {
            *err.lock().unwrap() = message;
        }
        return;
    }
    if !mf_init() {
        *err.lock().unwrap() = "Media Foundation not available".into();
        return;
    }
    let mut writer: Option<(IMFSinkWriter, u32, Option<u32>, usize, usize)> = None;
    let mut first_frame = None;
    let mut last_t: i64 = -1;
    let mut next_audio_slot = 0;
    let mut wrote_video = false;
    let (rn, rd) = rate_fraction(opts.fps);
    let frame_dur = (10_000_000u64 * u64::from(rd) / u64::from(rn.max(1))) as i64;
    while let Ok(RecFrame { mut pixels, w: mut fw, h: mut fh, captured_at, timeline_origin, bgra, audio, surface }) = rx.recv() {
        if surface.is_some() && !gpu.load(Ordering::Acquire) { continue; }
        // Scale the picture to the project's Output Size.
        if let Some((tw, th)) = opts.size {
            let (tw, th) = ((tw as usize & !1).max(2), (th as usize & !1).max(2));
            if (fw, fh) != (tw, th) && fw > 0 && fh > 0 {
                if surface.is_some() {
                    gpu.store(false, Ordering::Release);
                    continue;
                }
                let Some(img) = image::RgbaImage::from_raw(fw as u32, fh as u32, Arc::unwrap_or_clone(pixels)) else { continue };
                pixels = Arc::new(image::imageops::resize(&img, tw as u32, th as u32, image::imageops::FilterType::Triangle).into_raw());
                fw = tw;
                fh = th;
            }
        }
        if writer.is_none() {
            let w = (fw & !1).max(2);
            let h = (fh & !1).max(2);
            let manager = if opts.hw && opts.format != RecordFormat::Wmv9 {
                unsafe { video_device_manager().ok() }
            } else {
                None
            };
            let opened = unsafe { make_writer(&path, w as u32, h as u32, &opts, manager.as_ref().map(|(manager, _)| manager)) };
            let opened = if manager.is_some() && opened.is_err() {
                unsafe { make_writer(&path, w as u32, h as u32, &opts, None) }
            } else { opened };
            match opened {
                Ok((wr, idx, audio_idx)) => {
                    let name = unsafe { encoder_name(&wr, idx, opts.format) };
                    if !name.ends_with("(hardware)") {
                        gpu.store(false, Ordering::Release);
                        if surface.is_some() {
                            let _ = unsafe { wr.Finalize() };
                            continue;
                        }
                    }
                    *encoder.lock().unwrap() = name;
                    writer = Some((wr, idx, audio_idx, w, h));
                }
                Err(e) => {
                    if surface.is_some() {
                        gpu.store(false, Ordering::Release);
                        continue;
                    }
                    *err.lock().unwrap() = format!("Could not start {} recording: {}", opts.format.codec(), e.message());
                    return;
                }
            }
        }
        let Some((wr, idx, audio_idx, w, h)) = writer.as_ref() else { continue };
        let (w, h) = (*w, *h);
        let len = w * h * 4;
        let start = *first_frame.get_or_insert(timeline_origin.unwrap_or(captured_at));
        let mut t = (captured_at.saturating_duration_since(start).as_nanos() / 100) as i64;
        if t <= last_t {
            t = last_t + 1;
        }
        last_t = t;
        let mut wrote_this_frame = false;
        let r = unsafe {
            (|| -> Res<()> {
                let buf = if let Some(surface) = &surface {
                    use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
                    if (fw, fh) != (w, h) { return Err(windows::core::Error::from(E_FAIL)); }
                    let buffer = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &surface.0, 0, false)?;
                    buffer.SetCurrentLength(len as u32)?;
                    buffer
                } else {
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
                buf
                };
                let s = MFCreateSample()?;
                s.AddBuffer(&buf)?;
                s.SetSampleTime(t)?;
                s.SetSampleDuration(frame_dur)?;
                wr.WriteSample(*idx, &s)?;
                if let Some(index) = &index { index.lock().unwrap().push((captured_at, (t / 10_000) as u64)); }
                wrote_this_frame = true;
                wrote_video = true;
                if surface.is_some() {
                    let mut status = encoder.lock().unwrap();
                    if !status.ends_with(" · D3D11 input") { status.push_str(" · D3D11 input"); }
                } else {
                    let mut status = encoder.lock().unwrap();
                    if let Some(name) = status.strip_suffix(" · D3D11 input") { *status = name.to_owned(); }
                }
                if let Some(index) = *audio_idx {
                    let slot = ((t + frame_dur / 2) / frame_dur) as u64;
                    let slot = slot.max(next_audio_slot);
                    next_audio_slot = slot + 1;
                    let frame_count = crate::audio::frames_for_slot(slot, rn, rd);
                    let audio_offset = slot * u64::from(crate::audio::RATE) * u64::from(rd) / u64::from(rn);
                    let to_hns = |samples| (samples * 10_000_000 / u64::from(crate::audio::RATE)) as i64;
                    let len = (frame_count * 4) as u32;
                    let buffer = MFCreateMemoryBuffer(len)?;
                    let mut ptr = std::ptr::null_mut();
                    buffer.Lock(&mut ptr, None, None)?;
                    let bytes = std::slice::from_raw_parts_mut(ptr, len as usize);
                    crate::audio::write_pcm(bytes, &audio, frame_count)
                        .map_err(|_| windows::core::Error::from(E_FAIL))?;
                    buffer.Unlock()?;
                    buffer.SetCurrentLength(len)?;
                    let sample = MFCreateSample()?;
                    sample.AddBuffer(&buffer)?;
                    sample.SetSampleTime(to_hns(audio_offset))?;
                    sample.SetSampleDuration(to_hns(audio_offset + frame_count as u64) - to_hns(audio_offset))?;
                    wr.WriteSample(index, &sample)?;
                }
                Ok(())
            })()
        };
        if let Err(e) = r {
            if surface.is_some() && !wrote_this_frame {
                gpu.store(false, Ordering::Release);
                if !wrote_video {
                    if let Some((failed, ..)) = writer.take() {
                        let _ = unsafe { failed.Finalize() };
                    }
                }
                continue;
            }
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
            let preset = COMDLG_FILTERSPEC { pszName: windows::core::w!("Project preset"), pszSpec: windows::core::w!("*.livemix") };
            let config = COMDLG_FILTERSPEC { pszName: windows::core::w!("LiveMix settings (*.json)"), pszSpec: windows::core::w!("*.json") };
            let legacy_config = COMDLG_FILTERSPEC { pszName: windows::core::w!("Legacy XML settings (*.config)"), pszSpec: windows::core::w!("*.config") };
            if matches!(kind, PickKind::PresetSave | PickKind::SettingsSave) {
                let d: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
                if kind == PickKind::SettingsSave {
                    d.SetFileTypes(&[config, legacy_config])?;
                    d.SetFileName(windows::core::w!("LiveMix-settings"))?;
                } else {
                    d.SetFileTypes(&[preset])?;
                    d.SetDefaultExtension(windows::core::w!("livemix"))?;
                }
                d.Show(HWND::default())?;
                let path = name_of(&d.GetResult()?)?;
                if kind == PickKind::SettingsSave
                    && std::path::Path::new(&path).extension().is_none()
                {
                    let extension = if d.GetFileTypeIndex()? == 2 { "config" } else { "json" };
                    let path = format!("{path}.{extension}");
                    if std::path::Path::new(&path).exists() {
                        return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x80070050u32 as i32)));
                    }
                    return Ok(path);
                }
                return Ok(path);
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
                PickKind::SettingsOpen => d.SetFileTypes(&[config, legacy_config, all])?,
                PickKind::PresetSave | PickKind::SettingsSave => unreachable!(),
                PickKind::Video => d.SetFileTypes(&[video, all])?,
                PickKind::Lut(_) => d.SetFileTypes(&[
                    COMDLG_FILTERSPEC { pszName: windows::core::w!("3D LUT files"), pszSpec: windows::core::w!("*.cube;*.png") }, all
                ])?,
                PickKind::Presentation => d.SetFileTypes(&[
                    COMDLG_FILTERSPEC { pszName: windows::core::w!("Presentations"), pszSpec: windows::core::w!("*.ppt;*.pptx;*.pps;*.ppsx;*.odp") }, all
                ])?,
                PickKind::Audio => d.SetFileTypes(&[
                    COMDLG_FILTERSPEC { pszName: windows::core::w!("Audio files"), pszSpec: windows::core::w!("*.wav;*.mp3;*.flac;*.m4a;*.aac;*.ogg;*.opus;*.aif;*.aiff;*.wma") },
                    all,
                ])?,
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
    #[ignore = "requires a hardware D3D11 adapter"]
    fn decoder_and_recorder_workers_resolve_the_same_video_manager() {
        let resolve = || {
            assert!(mf_init());
            let (manager, adapter) = unsafe { video_device_manager() }.unwrap();
            let identity = manager.cast::<windows::core::IUnknown>().unwrap();
            (identity.as_raw() as usize, adapter)
        };
        let decoder = std::thread::spawn(resolve).join().unwrap();
        let recorder = std::thread::spawn(resolve).join().unwrap();
        assert_eq!(decoder, recorder);
    }

    #[test]
    #[ignore = "requires a D3D11 hardware encoder"]
    fn hardware_recorder_accepts_d3d11_surfaces() {
        use windows::Win32::Graphics::Direct3D11::{
            D3D11_BIND_RENDER_TARGET, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
        select_video_adapter("@high");
        let device = unsafe { shared_d3d11_device() }.unwrap();
        let path = std::env::temp_dir().join(format!("livemix-d3d11-{}.mp4", std::process::id()));
        let (recorder, worker) = Recorder::spawn_writer(path.to_string_lossy().into_owned(), RecOpts { hw: true, ..Default::default() });
        let recorder = Arc::new(recorder);
        let descriptor = D3D11_TEXTURE2D_DESC {
            Width: 320, Height: 180, MipLevels: 1, ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            ..Default::default()
        };
        let start = Instant::now();
        for frame in 0..12 {
            let mut surface = None;
            unsafe { device.CreateTexture2D(&descriptor, None, Some(&mut surface)) }.unwrap();
            Recorder::push_surface_group(std::slice::from_ref(&recorder), crate::dx_interop::Surface(surface.unwrap()),
                [320, 180], start + Duration::from_millis(frame * 33), Default::default());
            std::thread::sleep(Duration::from_millis(34));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while recorder.diagnostics().starts_with("Waiting") && recorder.accepts_surface() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let diagnostics = recorder.diagnostics();
        let error = recorder.error.clone();
        drop(recorder);
        worker.join().unwrap();
        let size = std::fs::metadata(&path).map_or(0, |file| file.len());
        let decoded = start_for_gl(Source::File(path.to_string_lossy().into_owned()), true);
        let deadline = Instant::now() + Duration::from_secs(5);
        while decoded.seq.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let picture = decoded.gpu_frame.lock().unwrap().take();
        decoded.stop.store(true, Ordering::Release);
        let _ = std::fs::remove_file(path);
        let error = error.lock().unwrap().clone();
        println!("{diagnostics}");
        assert!(diagnostics.contains("D3D11 input") && size > 1000 && error.is_empty(),
            "{diagnostics}, {size} bytes, {error}");
        assert_eq!(picture.map(|picture| picture.size), Some([320, 180]),
            "Hardware-decoded MP4 should publish a GPU texture: {}", decoded.status.lock().unwrap());
    }

    #[test]
    #[ignore = "requires FFmpeg and FFprobe"]
    fn auto_decoder_opens_ffv1_and_reports_the_backend() {
        let path = std::env::temp_dir().join(format!("livemix-auto-decoder-{}.mkv", std::process::id()));
        let output = crate::stream::command().args(["-y", "-f", "lavfi", "-i", "color=size=64x48:rate=25",
            "-t", "1", "-c:v", "ffv1"]).arg(&path).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let reader = start_for_gl(Source::AutoFile(path.to_string_lossy().into_owned(), 0, false), true);
        let deadline = Instant::now() + Duration::from_secs(5);
        while reader.seq.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        reader.stop.store(true, Ordering::Relaxed);
        let decoder = reader.decoder.lock().unwrap().clone();
        let _ = std::fs::remove_file(path);
        assert!(reader.seq.load(Ordering::Relaxed) > 0, "{}", reader.status.lock().unwrap());
        assert!(decoder.starts_with("Media Foundation") || decoder.starts_with("FFmpeg · software · fallback:"),
            "{decoder}");
        println!("{decoder}");
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
                    timeline_origin: None,
                    surface: None,
                    audio: crate::audio::test_tone(1600),
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
        if crate::stream::command().arg("-version").output().is_ok_and(|output| output.status.success()) {
            crate::audio::assert_audio_file(std::path::Path::new(&path));
        }
        let sh = start_with_pause(Source::File(path.clone()), true);
        let first_deadline = Instant::now() + Duration::from_secs(5);
        while sh.seq.load(Ordering::Relaxed) == 0 && Instant::now() < first_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(sh.seq.load(Ordering::Relaxed), 1, "paused clip must show and hold its first frame");
        sh.paused.store(false, Ordering::Relaxed);
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
