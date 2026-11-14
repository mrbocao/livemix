//! Optional Windows DirectShow file decoder. The graph stays on its worker.
use crate::media_common::{Frame, Shared};
use crate::settings_xml::DirectShow;
use std::sync::atomic::Ordering::Relaxed;
use std::time::{Duration, Instant};
use windows::core::{w, GUID, HRESULT, HSTRING, Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS};
use windows::Win32::Media::DirectShow::*;
use windows::Win32::Media::MediaFoundation::{
    AM_MEDIA_TYPE, CLSID_FilterGraph, FORMAT_VideoInfo, MEDIASUBTYPE_RGB32, MEDIATYPE_Video, VIDEOINFOHEADER,
};
use windows::Win32::System::{Com::*, Registry::*};

const SAMPLE_GRABBER: GUID = GUID::from_u128(0xc1f400a0_3f08_11d3_9f0b_006008039e37);
const NULL_RENDERER: GUID = GUID::from_u128(0xc1f400a4_3f08_11d3_9f0b_006008039e37);

windows_core::imp::define_interface!(ISampleGrabber, SampleGrabberVtable, 0x6b652fff_11fe_4fce_92ad_0266b5d7c78f);

#[repr(C)]
pub struct SampleGrabberVtable {
    base: windows_core::IUnknown_Vtbl,
    set_one_shot: unsafe extern "system" fn(*mut core::ffi::c_void, i32) -> HRESULT,
    set_media_type: unsafe extern "system" fn(*mut core::ffi::c_void, *const AM_MEDIA_TYPE) -> HRESULT,
    get_connected_media_type: unsafe extern "system" fn(*mut core::ffi::c_void, *mut AM_MEDIA_TYPE) -> HRESULT,
    set_buffer_samples: unsafe extern "system" fn(*mut core::ffi::c_void, i32) -> HRESULT,
    get_current_buffer: unsafe extern "system" fn(*mut core::ffi::c_void, *mut i32, *mut i32) -> HRESULT,
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) { unsafe { let _ = RegCloseKey(self.0); } }
}

fn read_string(key: HKEY, subkey: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut text = [0u16; 512];
    let mut bytes = size_of_val(&text) as u32;
    let status = unsafe { RegGetValueW(key, subkey, name, RRF_RT_REG_SZ, None,
        Some(text.as_mut_ptr().cast()), Some(&mut bytes)) };
    (status == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&text[..(bytes as usize / 2).saturating_sub(1)]))
}

fn registered() -> Vec<(String, GUID)> {
    let mut raw = HKEY::default();
    let category = w!(r"CLSID\{083863F1-70DE-11D0-BD40-00A0C911CE86}\Instance");
    if unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, category, 0, KEY_READ, &mut raw) } != ERROR_SUCCESS {
        return Vec::new();
    }
    let key = RegistryKey(raw);
    let mut result = Vec::new();
    for index in 0..4096 {
        let mut name = [0u16; 256];
        let mut count = name.len() as u32;
        let status = unsafe { RegEnumKeyExW(key.0, index, PWSTR(name.as_mut_ptr()), &mut count,
            None, PWSTR::null(), None, None) };
        if status == ERROR_NO_MORE_ITEMS { break; }
        if status != ERROR_SUCCESS { continue; }
        let subkey = PCWSTR(name.as_ptr());
        if let (Some(friendly), Some(class)) = (read_string(key.0, subkey, w!("FriendlyName")),
            read_string(key.0, subkey, w!("CLSID")))
        {
            let digits = class.trim_matches(['{', '}']).replace('-', "");
            if digits.len() == 32 {
                if let Ok(id) = u128::from_str_radix(&digits, 16) {
                    result.push((friendly, GUID::from_u128(id)));
                }
            }
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result.dedup();
    result
}

pub fn filter_names() -> Vec<String> {
    registered().into_iter().map(|(name, _)| name).collect()
}

struct Com;

impl Com {
    fn init() -> windows::core::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()?; }
        Ok(Self)
    }
}

impl Drop for Com {
    fn drop(&mut self) { unsafe { CoUninitialize(); } }
}

fn first_pin(filter: &IBaseFilter, direction: PIN_DIRECTION) -> windows::core::Result<IPin> {
    let pins = unsafe { filter.EnumPins()? };
    let mut slot = [None];
    while unsafe { pins.Next(&mut slot, None) } == HRESULT(0) {
        if let Some(pin) = slot[0].take() {
            if unsafe { pin.QueryDirection()? } == direction { return Ok(pin); }
        }
    }
    Err(windows::core::Error::from_hresult(HRESULT(0x80040216u32 as i32)))
}

fn video_codec(path: &str) -> Option<String> {
    let result = crate::audio::ffprobe_command().args(["-v", "error", "-select_streams", "v:0",
        "-show_entries", "stream=codec_name", "-of", "default=nw=1:nk=1", "-i", path]).output().ok()?;
    result.status.success().then(|| String::from_utf8_lossy(&result.stdout).lines().next().unwrap_or_default().trim().to_owned())
}

fn active_filters(graph: &IGraphBuilder) -> windows::core::Result<Vec<(String, GUID)>> {
    let filters = unsafe { graph.EnumFilters()? };
    let mut found = Vec::new();
    let mut slot = [None];
    while unsafe { filters.Next(&mut slot, None) } == HRESULT(0) {
        let Some(filter) = slot[0].take() else { continue; };
        let pins = unsafe { filter.EnumPins()? };
        let mut pin = [None];
        let mut connected = false;
        while unsafe { pins.Next(&mut pin, None) } == HRESULT(0) {
            connected |= pin[0].take().is_some_and(|pin| unsafe { pin.ConnectedTo().is_ok() });
        }
        if !connected { continue; }
        let mut info = FILTER_INFO::default();
        unsafe { filter.QueryFilterInfo(&mut info)?; core::mem::ManuallyDrop::drop(&mut info.pGraph); }
        let len = info.achName.iter().position(|&ch| ch == 0).unwrap_or(info.achName.len());
        found.push((String::from_utf16_lossy(&info.achName[..len]), unsafe { filter.GetClassID()? }));
    }
    Ok(found)
}

struct Running(IMediaControl);

impl Drop for Running {
    fn drop(&mut self) { unsafe { let _ = self.0.Stop(); } }
}

struct PictureFormat {
    width: usize,
    height: usize,
    bottom_up: bool,
    period: i64,
}

unsafe fn picture_format(grabber: &ISampleGrabber) -> Result<PictureFormat, String> {
    let mut format = AM_MEDIA_TYPE::default();
    (grabber.vtable().get_connected_media_type)(grabber.as_raw(), &mut format).ok().map_err(|e| e.to_string())?;
    let header = (format.formattype == FORMAT_VideoInfo && !format.pbFormat.is_null()
        && format.cbFormat as usize >= size_of::<VIDEOINFOHEADER>())
        .then(|| format.pbFormat.cast::<VIDEOINFOHEADER>().read_unaligned());
    CoTaskMemFree(Some(format.pbFormat.cast()));
    core::mem::ManuallyDrop::drop(&mut format.pUnk);
    let header = header.ok_or("Unsupported DirectShow picture layout")?;
    let width = header.bmiHeader.biWidth;
    let height = header.bmiHeader.biHeight;
    if !(1..=8192).contains(&width) || !(1..=8192).contains(&height.unsigned_abs())
        || header.bmiHeader.biBitCount != 32
    {
        return Err("Invalid DirectShow picture dimensions or pixel format".into());
    }
    Ok(PictureFormat { width: width as usize, height: height.unsigned_abs() as usize,
        bottom_up: height > 0, period: if header.AvgTimePerFrame > 0 { header.AvgTimePerFrame } else { 333_667 } })
}

pub fn play(shared: &Shared, path: &str, config: &DirectShow) -> Result<(), String> {
    let _com = Com::init().map_err(|e| e.to_string())?;
    let registry = registered();
    let lookup = |name: &str| registry.iter().find(|(candidate, _)| candidate.eq_ignore_ascii_case(name)).map(|(_, id)| *id);
    let blocked: Vec<_> = config.blocked.iter().filter_map(|name| lookup(name)).collect();
    let codec = if config.mpeg_video.is_empty() && config.h264_video.is_empty() { None } else { video_codec(path) };
    let preferred = match codec.as_deref() {
        Some("mpeg2video") => &config.mpeg_video,
        Some("h264") => &config.h264_video,
        _ => "",
    };
    let explicit = if preferred.is_empty() { None } else {
        let clsid = lookup(preferred).ok_or_else(|| format!("DirectShow filter unavailable: {preferred}"))?;
        if blocked.contains(&clsid) { return Err(format!("DirectShow filter blocked: {preferred}")); }
        Some(clsid)
    };
    unsafe { play_graph(shared, path, config, &blocked, preferred, explicit) }
}

unsafe fn play_graph(shared: &Shared, path: &str, config: &DirectShow, blocked: &[GUID],
    preferred: &str, explicit: Option<GUID>) -> Result<(), String>
{
    let err = |e: windows::core::Error| e.to_string();
    let graph: IGraphBuilder = CoCreateInstance(&CLSID_FilterGraph, None, CLSCTX_INPROC_SERVER).map_err(err)?;
    let sample_filter: IBaseFilter = CoCreateInstance(&SAMPLE_GRABBER, None, CLSCTX_INPROC_SERVER).map_err(err)?;
    let grabber: ISampleGrabber = sample_filter.cast().map_err(err)?;
    let renderer: IBaseFilter = CoCreateInstance(&NULL_RENDERER, None, CLSCTX_INPROC_SERVER).map_err(err)?;
    let requested = AM_MEDIA_TYPE { majortype: MEDIATYPE_Video, subtype: MEDIASUBTYPE_RGB32,
        formattype: FORMAT_VideoInfo, ..Default::default() };
    (grabber.vtable().set_media_type)(grabber.as_raw(), &requested).ok().map_err(err)?;
    (grabber.vtable().set_buffer_samples)(grabber.as_raw(), 1).ok().map_err(err)?;
    graph.AddFilter(&sample_filter, w!("LiveMix Frame Capture")).map_err(err)?;
    graph.AddFilter(&renderer, w!("LiveMix Null Renderer")).map_err(err)?;
    let capture_input = first_pin(&sample_filter, PINDIR_INPUT).map_err(err)?;
    let source = graph.AddSourceFilter(&HSTRING::from(path), w!("File Source")).map_err(err)?;
    if let Some(clsid) = explicit {
        let filter: IBaseFilter = CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER).map_err(err)?;
        graph.AddFilter(&filter, &HSTRING::from(preferred)).map_err(err)?;
    }
    let pins = source.EnumPins().map_err(err)?;
    let mut slot = [None];
    let mut connected = false;
    while pins.Next(&mut slot, None) == HRESULT(0) {
        let Some(pin) = slot[0].take() else { continue; };
        if pin.QueryDirection().map_err(err)? == PINDIR_OUTPUT && graph.Connect(&pin, &capture_input).is_ok() {
            connected = true;
            break;
        }
    }
    if !connected { return Err("DirectShow could not connect the video graph".into()); }
    graph.ConnectDirect(&first_pin(&sample_filter, PINDIR_OUTPUT).map_err(err)?,
        &first_pin(&renderer, PINDIR_INPUT).map_err(err)?, None).map_err(err)?;
    let filters = active_filters(&graph).map_err(err)?;
    if let Some((name, _)) = filters.iter().find(|(name, id)| blocked.contains(id)
        || config.blocked.iter().any(|blocked| name.eq_ignore_ascii_case(blocked)))
    {
        return Err(format!("DirectShow graph rejected because it uses blocked filter {name}"));
    }
    if explicit.is_some_and(|id| !filters.iter().any(|(_, active)| *active == id)) {
        return Err(format!("DirectShow could not use the selected decoder: {preferred}"));
    }
    let format = picture_format(&grabber)?;
    let seeking: IMediaSeeking = graph.cast().map_err(err)?;
    let duration = seeking.GetDuration().map_err(err)?.max(0);
    shared.dur_ms.store((duration / 10_000) as u64, Relaxed);
    let events: IMediaEvent = graph.cast().map_err(err)?;
    let control = Running(graph.cast().map_err(err)?);
    control.0.Pause().map_err(err)?;
    let mut pixels = vec![0i32; format.width * format.height];
    let mut frame_at = None;
    let mut running = false;
    let mut deadline = Instant::now() + Duration::from_secs(3);
    let mut target_position = 0;
    let path_description = filters.iter().filter(|(name, _)| !name.starts_with("LiveMix ") && name != "File Source")
        .map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(" → ");
    while !shared.stop.load(Relaxed) {
        let seek = shared.seek_ms.swap(0, Relaxed);
        let restart = shared.restart.swap(false, Relaxed);
        let replay = shared.ended.load(Relaxed) && shared.looping.load(Relaxed) && !shared.paused.load(Relaxed);
        if seek > 0 || restart || replay {
            let mut position = seek.saturating_sub(1).saturating_mul(10_000).min(duration.max(0) as u64) as i64;
            control.0.Stop().map_err(err)?;
            (grabber.vtable().set_buffer_samples)(grabber.as_raw(), 0).ok().map_err(err)?;
            (grabber.vtable().set_buffer_samples)(grabber.as_raw(), 1).ok().map_err(err)?;
            seeking.SetPositions(Some(&mut position), AM_SEEKING_AbsolutePositioning.0 as u32, None, 0).map_err(err)?;
            control.0.Pause().map_err(err)?;
            target_position = position;
            running = false;
            frame_at = None;
            deadline = Instant::now() + Duration::from_secs(3);
            shared.ended.store(false, Relaxed);
            crate::audio::reset_position(shared);
        }
        let (mut code, mut first, mut second) = (0, 0, 0);
        while events.GetEvent(&mut code, &mut first, &mut second, 0).is_ok() {
            events.FreeEventParams(code, first, second).map_err(err)?;
            match code as u32 {
                EC_COMPLETE if running && frame_at.is_some() => {
                    shared.pos_ms.store((duration / 10_000) as u64, Relaxed);
                    shared.ended.store(true, Relaxed);
                }
                EC_ERRORABORT => return Err(format!("DirectShow graph failed ({first:#x})")),
                _ => {}
            }
        }
        let paused = shared.paused.load(Relaxed);
        let play = !paused && !shared.ended.load(Relaxed) && frame_at.is_some();
        if play != running {
            if play { control.0.Run().map_err(err)?; } else { control.0.Pause().map_err(err)?; }
            running = play;
        }
        if paused && frame_at.is_some() || shared.ended.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }
        let position = if frame_at.is_none() { target_position } else { seeking.GetCurrentPosition().map_err(err)? };
        if duration > 0 && position >= duration && frame_at.is_some() {
            shared.ended.store(true, Relaxed);
            continue;
        }
        if frame_at.is_none_or(|last| position >= last + format.period) {
            let mut bytes = (pixels.len() * size_of::<i32>()) as i32;
            let status = (grabber.vtable().get_current_buffer)(grabber.as_raw(), &mut bytes, pixels.as_mut_ptr());
            if status.is_ok() && bytes as usize == pixels.len() * 4 {
                let mut rgba = Vec::with_capacity(pixels.len() * 4);
                for row in 0..format.height {
                    let y = if format.bottom_up { format.height - 1 - row } else { row };
                    for &pixel in &pixels[y * format.width..(y + 1) * format.width] {
                        let [b, g, r, _] = pixel.to_le_bytes();
                        rgba.extend_from_slice(&[r, g, b, 255]);
                    }
                }
                if shared.seq.load(Relaxed) == 0 {
                    *shared.decoder.lock().unwrap() = format!("DirectShow · system memory · {path_description}");
                    shared.set_status("");
                }
                crate::ffmpeg_media::publish(shared, Frame { w: format.width, h: format.height, rgba });
                let ms = (position.max(0) / 10_000) as u64;
                shared.pos_ms.store(ms, Relaxed);
                crate::audio::mark_position(shared, ms);
                frame_at = Some(target_position + (position - target_position).max(0) / format.period * format.period);
            } else if frame_at.is_none() && Instant::now() >= deadline {
                return Err("DirectShow timed out waiting for the first picture".into());
            }
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    Ok(())
}


#[cfg(test)]
#[test]
#[ignore = "requires the Windows DirectShow Sample Grabber and FFmpeg"]
fn windows_graph_delivers_a_paused_frame_and_rejects_a_blocked_filter() {
    let path = std::env::temp_dir().join(format!("livemix-directshow-{}.avi", std::process::id()));
    let result = crate::stream::command().args(["-y", "-f", "lavfi", "-i", "color=red:size=64x48:rate=25:d=1",
        "-f", "lavfi", "-i", "color=blue:size=64x48:rate=25:d=1",
        "-filter_complex", "concat=n=2:v=1:a=0", "-c:v", "rawvideo", "-pix_fmt", "bgr24"])
        .arg(&path).output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let shared = std::sync::Arc::new(Shared::default());
    shared.paused.store(true, Relaxed);
    let worker_source = shared.clone();
    let video_path = path.to_string_lossy().into_owned();
    let worker_path = video_path.clone();
    let worker = std::thread::spawn(move || play(&worker_source, &worker_path, &DirectShow::default()));
    let until = Instant::now() + Duration::from_secs(5);
    while shared.seq.load(Relaxed) == 0 && !worker.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    let frame = shared.frame.lock().unwrap().take();
    if frame.is_some() {
        shared.seek_ms.store(1001, Relaxed);
        let until = Instant::now() + Duration::from_secs(4);
        while shared.pos_ms.load(Relaxed) != 1000 && !worker.is_finished() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    shared.stop.store(true, Relaxed);
    worker.join().unwrap().unwrap();
    assert_eq!(shared.pos_ms.load(Relaxed), 1000);
    let sought = shared.frame.lock().unwrap().take().expect("picture after seek");
    assert!(sought.rgba[0] < 30 && sought.rgba[1] < 30 && sought.rgba[2] > 200, "{:?}", &sought.rgba[..4]);
    let frame = frame.expect("paused DirectShow picture");
    assert_eq!((frame.w, frame.h), (64, 48));
    assert!(frame.rgba[0] > 200 && frame.rgba[1] < 30 && frame.rgba[2] < 30);
    let chain = shared.decoder.lock().unwrap().clone();
    println!("{chain}");
    let name = chain.split(" · ").last().unwrap().split(" → ").next().unwrap();
    assert!(!name.is_empty());
    let blocked = DirectShow { blocked: vec![name.into()], ..Default::default() };
    let probe = Shared::default();
    let error = play(&probe, &video_path, &blocked).unwrap_err();
    let _ = std::fs::remove_file(path);
    assert!(error.contains("blocked filter"), "{error}");
    assert_eq!(probe.seq.load(Relaxed), 0);
}
