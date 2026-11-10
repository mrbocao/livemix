//! Blackmagic DeckLink capture and playback, through the DeckLink COM API directly.
//!
//! LiveMix uses the DeckLink SDK interfaces (as current Desktop Video installs them): `IDeckLinkIterator`,
//! `IDeckLinkInput` for capture and `IDeckLinkOutput` for playback. The interface GUIDs and
//! vtable orders below were read from the type library of `DeckLinkAPI64.dll`.
//!
//! Capture runs one thread per input: it enables format detection on the selected connector,
//! starts the streams and converts every BGRA (or 2vuy) frame into the RGBA picture the UI
//! expects. Playback runs one thread per External output: it takes the newest composed picture
//! and schedules it at the chosen display mode's rate.

#![allow(non_snake_case)] // Use the names of the COM methods in the SDK.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{implement, interface, Interface, IUnknown, IUnknown_Vtbl, BSTR, GUID, HRESULT, HSTRING};
use windows::Win32::Foundation::S_OK;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, IClassFactory, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::media::{CamMode, Frame, Shared};

// ---------------------------------------------------------------- constants

/// CLSID of the DeckLink device iterator (CDeckLinkIterator in DeckLinkAPI.h).
const CLSID_DECKLINK_ITERATOR: GUID = GUID::from_u128(0xBA6C6F44_6DA5_4DCE_94AA_EE2D1372A676);

/// Pixel formats.
pub const PIXEL_BGRA: u32 = 0x4247_5241; // 'BGRA'
pub const PIXEL_YUV: u32 = 0x3276_7579; // '2vuy'

/// DeckLink configuration keys (BMDDeckLinkConfigurationID).
const CONFIG_VIDEO_INPUT_CONNECTION: u32 = 0x7669_636E; // bmdDeckLinkConfigVideoInputConnection
const CONFIG_VIDEO_OUTPUT_CONNECTION: u32 = 0x766F_636E; // bmdDeckLinkConfigVideoOutputConnection

/// `bmdVideoInputEnableFormatDetection`.
const VIDEO_INPUT_ENABLE_FORMAT_DETECTION: u32 = 1;

/// Connectors, as the original's Input/Output Settings list them.
pub const CONNECTIONS: [(&str, u32); 7] = [
    ("Auto", 0),
    ("SDI", 1),
    ("HDMI", 2),
    ("Optical SDI", 4),
    ("Component", 8),
    ("Composite", 16),
    ("S-Video", 32),
];

pub fn connection_name(value: u32) -> &'static str {
    CONNECTIONS.iter().find(|(_, v)| *v == value).map(|(n, _)| *n).unwrap_or("Auto")
}

pub fn device_index(link: &str) -> Option<usize> {
    link.strip_prefix("decklink:")?.parse().ok()
}

fn hr(what: &str, value: HRESULT) -> String {
    format!("{what} failed (0x{:08X})", value.0)
}

// ---------------------------------------------------------------- interfaces

#[interface("50FB36CD-3063-4B73-BDBB-958087F2D8BA")]
unsafe trait IDeckLinkIterator: IUnknown {
    unsafe fn Next(&self, deck_link: *mut *mut c_void) -> HRESULT;
}

#[interface("C418FBDD-0587-48ED-8FE5-640F0A14AF91")]
unsafe trait IDeckLink: IUnknown {
    unsafe fn GetModelName(&self, name: *mut BSTR) -> HRESULT;
    unsafe fn GetDisplayName(&self, name: *mut BSTR) -> HRESULT;
}

#[interface("4095DB82-E294-4B8C-AAA8-3B9E80C49336")]
unsafe trait IDeckLinkInput: IUnknown {
    unsafe fn DoesSupportVideoMode(
        &self,
        connection: u32,
        requested_mode: u32,
        requested_pixel_format: u32,
        conversion_mode: u32,
        flags: u32,
        actual_mode: *mut u32,
        supported: *mut i32,
    ) -> HRESULT;
    unsafe fn GetDisplayMode(&self, display_mode: u32, result_display_mode: *mut *mut c_void) -> HRESULT;
    unsafe fn GetDisplayModeIterator(&self, iterator: *mut *mut c_void) -> HRESULT;
    unsafe fn SetScreenPreviewCallback(&self, preview_callback: *mut c_void) -> HRESULT;
    unsafe fn EnableVideoInput(&self, display_mode: u32, pixel_format: u32, flags: u32) -> HRESULT;
    unsafe fn EnableVideoInputWithAllocatorProvider(
        &self,
        display_mode: u32,
        pixel_format: u32,
        flags: u32,
        allocator_provider: *mut c_void,
    ) -> HRESULT;
    unsafe fn DisableVideoInput(&self) -> HRESULT;
    unsafe fn GetAvailableVideoFrameCount(&self, available_frame_count: *mut u32) -> HRESULT;
    unsafe fn EnableAudioInput(&self, sample_rate: u32, sample_type: u32, channel_count: u32) -> HRESULT;
    unsafe fn DisableAudioInput(&self) -> HRESULT;
    unsafe fn GetAvailableAudioSampleFrameCount(&self, available_sample_frame_count: *mut u32) -> HRESULT;
    unsafe fn StartStreams(&self) -> HRESULT;
    unsafe fn StopStreams(&self) -> HRESULT;
    unsafe fn PauseStreams(&self) -> HRESULT;
    unsafe fn FlushStreams(&self) -> HRESULT;
    unsafe fn SetCallback(&self, the_callback: *mut c_void) -> HRESULT;
    unsafe fn GetHardwareReferenceClock(
        &self,
        desired_time_scale: i64,
        hardware_time: *mut i64,
        time_in_frame: *mut i64,
        ticks_per_frame: *mut i64,
    ) -> HRESULT;
}

#[interface("1A8077F1-9FE2-4533-8147-2294305E253F")]
unsafe trait IDeckLinkOutput: IUnknown {
    unsafe fn DoesSupportVideoMode(
        &self,
        connection: u32,
        requested_mode: u32,
        requested_pixel_format: u32,
        conversion_mode: u32,
        flags: u32,
        actual_mode: *mut u32,
        supported: *mut i32,
    ) -> HRESULT;
    unsafe fn GetDisplayMode(&self, display_mode: u32, result_display_mode: *mut *mut c_void) -> HRESULT;
    unsafe fn GetDisplayModeIterator(&self, iterator: *mut *mut c_void) -> HRESULT;
    unsafe fn SetScreenPreviewCallback(&self, preview_callback: *mut c_void) -> HRESULT;
    unsafe fn EnableVideoOutput(&self, display_mode: u32, flags: u32) -> HRESULT;
    unsafe fn DisableVideoOutput(&self) -> HRESULT;
    unsafe fn CreateVideoFrame(
        &self,
        width: i32,
        height: i32,
        row_bytes: i32,
        pixel_format: u32,
        flags: u32,
        out_frame: *mut *mut c_void,
    ) -> HRESULT;
    unsafe fn CreateVideoFrameWithBuffer(
        &self,
        width: i32,
        height: i32,
        row_bytes: i32,
        pixel_format: u32,
        flags: u32,
        buffer: *mut c_void,
        out_frame: *mut *mut c_void,
    ) -> HRESULT;
    unsafe fn RowBytesForPixelFormat(&self, pixel_format: u32, width: i32, row_bytes: *mut i32) -> HRESULT;
    unsafe fn CreateAncillaryData(&self, pixel_format: u32, out_buffer: *mut *mut c_void) -> HRESULT;
    unsafe fn DisplayVideoFrameSync(&self, the_frame: *mut c_void) -> HRESULT;
    unsafe fn ScheduleVideoFrame(
        &self,
        the_frame: *mut c_void,
        display_time: i64,
        display_duration: i64,
        time_scale: i64,
    ) -> HRESULT;
    unsafe fn SetScheduledFrameCompletionCallback(&self, the_callback: *mut c_void) -> HRESULT;
    unsafe fn GetBufferedVideoFrameCount(&self, buffered_frame_count: *mut u32) -> HRESULT;
    unsafe fn EnableAudioOutput(&self, sample_rate: u32, sample_type: u32, channel_count: u32, stream_type: u32) -> HRESULT;
    unsafe fn DisableAudioOutput(&self) -> HRESULT;
    unsafe fn WriteAudioSamplesSync(&self, buffer: *mut c_void, sample_frame_count: u32, sample_frames_written: *mut u32) -> HRESULT;
    unsafe fn BeginAudioPreroll(&self) -> HRESULT;
    unsafe fn EndAudioPreroll(&self) -> HRESULT;
    unsafe fn ScheduleAudioSamples(
        &self,
        buffer: *mut c_void,
        sample_frame_count: u32,
        stream_time: i64,
        time_scale: i64,
        sample_frames_written: *mut u32,
    ) -> HRESULT;
    unsafe fn GetBufferedAudioSampleFrameCount(&self, buffered_sample_frame_count: *mut u32) -> HRESULT;
    unsafe fn FlushBufferedAudioSamples(&self) -> HRESULT;
    unsafe fn SetAudioCallback(&self, the_callback: *mut c_void) -> HRESULT;
    unsafe fn StartScheduledPlayback(&self, playback_start_time: i64, time_scale: i64, playback_speed: f64) -> HRESULT;
    unsafe fn StopScheduledPlayback(&self, stop_playback_at_time: i64, actual_stop_time: *mut i64, time_scale: i64) -> HRESULT;
    unsafe fn IsScheduledPlaybackRunning(&self, active: *mut i32) -> HRESULT;
    unsafe fn GetScheduledStreamTime(&self, desired_time_scale: i64, stream_time: *mut i64, playback_speed: *mut f64) -> HRESULT;
    unsafe fn GetReferenceStatus(&self, reference_status: *mut u32) -> HRESULT;
    unsafe fn GetHardwareReferenceClock(
        &self,
        desired_time_scale: i64,
        hardware_time: *mut i64,
        time_in_frame: *mut i64,
        ticks_per_frame: *mut i64,
    ) -> HRESULT;
    unsafe fn GetFrameCompletionReferenceTimestamp(
        &self,
        the_frame: *mut c_void,
        desired_time_scale: i64,
        frame_completion_timestamp: *mut i64,
    ) -> HRESULT;
}

#[interface("6502091C-615F-4F51-BAF6-45C4256DD5B0")]
unsafe trait IDeckLinkVideoFrame: IUnknown {
    unsafe fn GetWidth(&self) -> i32;
    unsafe fn GetHeight(&self) -> i32;
    unsafe fn GetRowBytes(&self) -> i32;
    unsafe fn GetPixelFormat(&self) -> u32;
    unsafe fn GetFlags(&self) -> u32;
    unsafe fn GetTimecode(&self, format: u32, timecode: *mut *mut c_void) -> HRESULT;
    unsafe fn GetAncillaryData(&self, ancillary: *mut *mut c_void) -> HRESULT;
}

#[interface("CCB4B64A-5C86-4E02-B778-885D352709FE")]
unsafe trait IDeckLinkVideoBuffer: IUnknown {
    unsafe fn GetBytes(&self, buffer: *mut *mut c_void) -> HRESULT;
    unsafe fn StartAccess(&self, flags: u32) -> HRESULT;
    unsafe fn EndAccess(&self, flags: u32) -> HRESULT;
}

#[interface("CF9EB134-0374-4C5B-95FA-1EC14819FF62")]
unsafe trait IDeckLinkMutableVideoFrame: IDeckLinkVideoFrame {
    unsafe fn SetFlags(&self, new_flags: u32) -> HRESULT;
    unsafe fn SetTimecode(&self, format: u32, timecode: *mut c_void) -> HRESULT;
    unsafe fn SetTimecodeFromComponents(
        &self,
        format: u32,
        hours: u8,
        minutes: u8,
        seconds: u8,
        frames: u8,
        flags: u32,
    ) -> HRESULT;
    unsafe fn SetAncillaryData(&self, ancillary: *mut c_void) -> HRESULT;
    unsafe fn SetTimecodeUserBits(&self, format: u32, user_bits: u32) -> HRESULT;
    unsafe fn SetInterfaceProvider(&self, iid: *const GUID, iface: *mut c_void) -> HRESULT;
}

#[interface("3A94F075-C37D-4BA8-BCC0-1D778C8F881B")]
unsafe trait IDeckLinkInputCallback: IUnknown {
    unsafe fn VideoInputFormatChanged(
        &self,
        notification_events: u32,
        new_display_mode: *mut c_void,
        detected_signal_flags: u32,
    ) -> HRESULT;
    unsafe fn VideoInputFrameArrived(&self, video_frame: *mut c_void, audio_packet: *mut c_void) -> HRESULT;
}

#[interface("3EB2C1AB-0A3D-4523-A3AD-F40D7FB14E78")]
unsafe trait IDeckLinkDisplayMode: IUnknown {
    unsafe fn GetName(&self, name: *mut BSTR) -> HRESULT;
    unsafe fn GetDisplayMode(&self) -> u32;
    unsafe fn GetWidth(&self) -> i32;
    unsafe fn GetHeight(&self) -> i32;
    unsafe fn GetFrameRate(&self, frame_duration: *mut i64, time_scale: *mut i64) -> HRESULT;
    unsafe fn GetFieldDominance(&self) -> u32;
    unsafe fn GetFlags(&self) -> u32;
}

#[interface("9C88499F-F601-4021-B80B-032E4EB41C35")]
unsafe trait IDeckLinkDisplayModeIterator: IUnknown {
    unsafe fn Next(&self, deck_link_display_mode: *mut *mut c_void) -> HRESULT;
}

#[interface("912F634B-2D4E-40A4-8AAB-8D80B73F1289")]
unsafe trait IDeckLinkConfiguration: IUnknown {
    unsafe fn SetFlag(&self, cfg_id: u32, value: i32) -> HRESULT;
    unsafe fn GetFlag(&self, cfg_id: u32, value: *mut i32) -> HRESULT;
    unsafe fn SetInt(&self, cfg_id: u32, value: i64) -> HRESULT;
    unsafe fn GetInt(&self, cfg_id: u32, value: *mut i64) -> HRESULT;
    unsafe fn SetFloat(&self, cfg_id: u32, value: f64) -> HRESULT;
    unsafe fn GetFloat(&self, cfg_id: u32, value: *mut f64) -> HRESULT;
    unsafe fn SetString(&self, cfg_id: u32, value: *mut c_void) -> HRESULT;
    unsafe fn GetString(&self, cfg_id: u32, value: *mut *mut c_void) -> HRESULT;
    unsafe fn WriteConfigurationToPreferences(&self) -> HRESULT;
}

// ---------------------------------------------------------------- device list

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// Position in the enumeration order.
    pub index: usize,
    /// Name shown in the UI (the display name, or the model name when that is empty).
    pub name: String,
    /// Model name ("DeckLink Mini Recorder", "UltraStudio 4K Mini", …).
    pub model: String,
    pub capture: bool,
    pub playback: bool,
}

static DEVICES: Mutex<Option<Vec<Device>>> = Mutex::new(None);

fn com_init() -> bool {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() }
}

/// Loads `DeckLinkAPI64.dll` and obtains `DllGetClassObject`. Used when the COM class is not
/// registered (a normal Desktop Video install registers it, but this costs little).
unsafe fn class_factory() -> Result<IClassFactory, String> {
    let mut module = None;
    if let Ok(dll) = LoadLibraryW(&HSTRING::from("DeckLinkAPI64.dll")) {
        module = Some(dll);
    }
    if module.is_none() {
        for path in [
            "C:\\Program Files\\Blackmagic Design\\Blackmagic Desktop Video\\DeckLinkAPI64.dll",
            "C:\\Program Files (x86)\\Blackmagic Design\\Blackmagic Desktop Video\\DeckLinkAPI64.dll",
        ] {
            if let Ok(dll) = LoadLibraryW(&HSTRING::from(path)) {
                module = Some(dll);
                break;
            }
        }
    }
    let module = module.ok_or_else(|| "DeckLinkAPI64.dll not found".to_string())?;
    let proc = GetProcAddress(module, windows::core::s!("DllGetClassObject"))
        .ok_or_else(|| "DeckLinkAPI64.dll has no DllGetClassObject".to_string())?;
    let get_class_object: unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT =
        std::mem::transmute(proc);
    let mut raw: *mut c_void = std::ptr::null_mut();
    let result = get_class_object(
        &CLSID_DECKLINK_ITERATOR,
        &IClassFactory::IID,
        &mut raw as *mut *mut c_void,
    );
    if result.is_err() || raw.is_null() {
        return Err(hr("DllGetClassObject", result));
    }
    Ok(IClassFactory::from_raw(raw))
}

unsafe fn create_iterator() -> Result<IDeckLinkIterator, String> {
    if let Ok(iterator) = CoCreateInstance(&CLSID_DECKLINK_ITERATOR, None::<&IUnknown>, CLSCTX_INPROC_SERVER) {
        return Ok(iterator);
    }
    let factory = class_factory()?;
    factory.CreateInstance(None::<&IUnknown>)
        .map_err(|e| format!("Could not create the DeckLink iterator: {e}"))
}

fn take_string(call: impl FnOnce(*mut BSTR) -> HRESULT) -> String {
    let mut value = BSTR::new();
    if call(&mut value).is_ok() { value.to_string() } else { String::new() }
}

unsafe fn device_name(device: &IDeckLink) -> (String, String) {
    let display = take_string(|p| device.GetDisplayName(p));
    let model = take_string(|p| device.GetModelName(p));
    let name = if display.is_empty() { model.clone() } else { display };
    (name, model)
}

fn device_list() -> Vec<Device> {
    std::thread::spawn(|| unsafe {
        if !com_init() {
            return Vec::new();
        }
        let Ok(iterator) = create_iterator() else { return Vec::new() };
        let mut devices = Vec::new();
        loop {
            let mut raw: *mut c_void = std::ptr::null_mut();
            if iterator.Next(&mut raw).is_err() || raw.is_null() {
                break;
            }
            let deck_link = IDeckLink::from_raw(raw);
            let (name, model) = device_name(&deck_link);
            let capture = deck_link.cast::<IDeckLinkInput>().is_ok();
            let playback = deck_link.cast::<IDeckLinkOutput>().is_ok();
            devices.push(Device { index: devices.len(), name, model, capture, playback });
        }
        devices
    })
    .join()
    .unwrap_or_default()
}

pub fn devices_cached() -> Vec<Device> {
    if let Some(list) = DEVICES.lock().unwrap().as_ref() {
        return list.clone();
    }
    let list = device_list();
    *DEVICES.lock().unwrap() = Some(list.clone());
    list
}

pub fn refresh_devices() -> Vec<Device> {
    let list = device_list();
    *DEVICES.lock().unwrap() = Some(list.clone());
    list
}

unsafe fn device_at(index: usize) -> Result<IDeckLink, String> {
    let iterator = create_iterator()?;
    for position in 0..=index {
        let mut raw: *mut c_void = std::ptr::null_mut();
        if iterator.Next(&mut raw).is_err() || raw.is_null() {
            return Err(format!("DeckLink device {} is not present", index + 1));
        }
        if position == index {
            return Ok(IDeckLink::from_raw(raw));
        }
        // Release the devices we skip.
        drop(IDeckLink::from_raw(raw));
    }
    Err(format!("DeckLink device {} is not present", index + 1))
}

// ---------------------------------------------------------------- display modes

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mode {
    /// BMDDisplayMode four-character code.
    pub mode: u32,
    pub w: u32,
    pub h: u32,
    /// Frame rate as a fraction: time scale / time value (30000/1001 for 29.97).
    pub fps_n: u32,
    pub fps_d: u32,
    pub name: String,
    pub interlaced: bool,
}

impl Mode {
    pub fn fps(&self) -> f64 {
        self.fps_n as f64 / self.fps_d.max(1) as f64
    }

    pub fn label(&self) -> String {
        let fps = self.fps();
        let rate = if (fps - fps.round()).abs() < 0.01 {
            format!("{}", fps.round() as i64)
        } else {
            format!("{fps:.2}")
        };
        format!("{}x{}{} {}", self.w, self.h, if self.interlaced { "i" } else { "p" }, rate)
    }

    pub fn cam_mode(&self) -> CamMode {
        CamMode {
            w: self.w,
            h: self.h,
            fps_n: self.fps_n,
            fps_d: self.fps_d.max(1),
            format: "DeckLink".into(),
        }
    }
}

unsafe fn read_modes(device: &IDeckLink, playback: bool) -> Vec<Mode> {
    let mut raw_iterator: *mut c_void = std::ptr::null_mut();
    let available = if playback {
        device.cast::<IDeckLinkOutput>().is_ok_and(|output| output.GetDisplayModeIterator(&mut raw_iterator).is_ok())
    } else {
        device.cast::<IDeckLinkInput>().is_ok_and(|input| input.GetDisplayModeIterator(&mut raw_iterator).is_ok())
    };
    if !available || raw_iterator.is_null() {
        return Vec::new();
    }
    let iterator = IDeckLinkDisplayModeIterator::from_raw(raw_iterator);
    let mut modes = Vec::new();
    loop {
        let mut raw: *mut c_void = std::ptr::null_mut();
        if iterator.Next(&mut raw).is_err() || raw.is_null() {
            break;
        }
        let display_mode = IDeckLinkDisplayMode::from_raw(raw);
        let name = take_string(|p| display_mode.GetName(p));
        let (mut time_scale, mut time_value) = (0i64, 1i64);
        let _ = display_mode.GetFrameRate(&mut time_value, &mut time_scale);
        if time_scale <= 0 || time_value <= 0 {
            time_scale = 0;
            time_value = 1;
        }
        let dominance = display_mode.GetFieldDominance();
        let interlaced = dominance == 0x7570_7072 /* upper field first */ || dominance == 0x6C6F_7772 /* lower field first */;
        modes.push(Mode {
            mode: display_mode.GetDisplayMode(),
            w: display_mode.GetWidth().max(0) as u32,
            h: display_mode.GetHeight().max(0) as u32,
            fps_n: time_scale as u32,
            fps_d: time_value.max(1) as u32,
            name,
            interlaced,
        });
    }
    modes.sort_by(|a, b| (a.w * a.h, a.interlaced, a.fps_n).cmp(&(b.w * b.h, b.interlaced, b.fps_n)));
    modes
}

/// Display modes offered by a device (for Settings → External Output).
pub fn display_modes(device_index: usize, playback: bool) -> Vec<Mode> {
    std::thread::spawn(move || unsafe {
        if !com_init() {
            return Vec::new();
        }
        let Ok(device) = device_at(device_index) else { return Vec::new() };
        read_modes(&device, playback)
    })
    .join()
    .unwrap_or_default()
}

fn pick_mode(modes: &[Mode], wanted: u32, size: (u32, u32), fps: f32, interlaced: bool) -> Option<Mode> {
    if wanted != 0 {
        return modes.iter().find(|m| m.mode == wanted && m.fps_n > 0).cloned();
    }
    let score = |m: &Mode| -> f64 {
        let mut value = 0.0;
        if (m.w, m.h) == size {
            value += 1000.0;
        }
        if m.interlaced == interlaced {
            value += 100.0;
        }
        value - (m.fps() - fps as f64).abs()
    };
    modes
        .iter()
        .filter(|m| m.w > 0 && m.h > 0 && m.fps_n > 0)
        .max_by(|a, b| score(a).partial_cmp(&score(b)).unwrap_or(std::cmp::Ordering::Equal))
        .cloned()
}

// ---------------------------------------------------------------- capture

/// Settings of one DeckLink input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CaptureConfig {
    pub device: usize,
    /// BMDVideoConnection (0 = the device's default).
    pub port: u32,
    pub mode: u32,
}

#[implement(IDeckLinkInputCallback)]
struct CaptureCallback {
    shared: Arc<Shared>,
    detected_mode: Arc<AtomicU64>,
}

impl CaptureCallback {
    fn note_mode(&self, display_mode: &IDeckLinkDisplayMode) {
        unsafe {
            let (mut time_scale, mut time_value) = (0i64, 1i64);
            let _ = display_mode.GetFrameRate(&mut time_value, &mut time_scale);
            if time_scale <= 0 || time_value <= 0 {
                time_scale = 0;
                time_value = 1;
            }
            let mode = CamMode {
                w: display_mode.GetWidth().max(0) as u32,
                h: display_mode.GetHeight().max(0) as u32,
                fps_n: time_scale as u32,
                fps_d: time_value.max(1) as u32,
                format: "DeckLink".into(),
            };
            if mode.w > 0 && mode.h > 0 {
                *self.shared.size.lock().unwrap() = (mode.w as usize, mode.h as usize);
                *self.shared.mode.lock().unwrap() = Some(mode);
            }
        }
    }
}

impl IDeckLinkInputCallback_Impl for CaptureCallback_Impl {
    unsafe fn VideoInputFormatChanged(
        &self,
        _notification_events: u32,
        new_display_mode: *mut c_void,
        _detected_signal_flags: u32,
    ) -> HRESULT {
        if !new_display_mode.is_null() {
            let display_mode = std::mem::ManuallyDrop::new(IDeckLinkDisplayMode::from_raw(new_display_mode));
            self.note_mode(&display_mode);
            self.detected_mode.store(display_mode.GetDisplayMode() as u64, Ordering::Relaxed);
        }
        S_OK
    }

    unsafe fn VideoInputFrameArrived(
        &self,
        video_frame: *mut c_void,
        _audio_packet: *mut c_void,
    ) -> HRESULT {
        if video_frame.is_null() || self.shared.stop.load(Ordering::Relaxed) {
            return S_OK;
        }
        // The frame is only lent to us for the duration of the callback.
        let frame = std::mem::ManuallyDrop::new(IDeckLinkVideoFrame::from_raw(video_frame));
        let width = frame.GetWidth().max(0) as usize;
        let height = frame.GetHeight().max(0) as usize;
        let row_bytes = frame.GetRowBytes().max(0) as usize;
        let pixel_format = frame.GetPixelFormat();
        let flags = frame.GetFlags();
        if flags & 0x8000_0000 != 0 {
            *self.shared.status.lock().unwrap() = "DeckLink: no input signal".into();
            return S_OK;
        }
        let min_row = if pixel_format == PIXEL_BGRA { width * 4 } else { width * 2 };
        if width == 0 || height == 0 || width > 8192 || height > 4320 || row_bytes < min_row {
            return S_OK;
        }
        let Ok(buffer) = frame.cast::<IDeckLinkVideoBuffer>() else {
            return S_OK;
        };
        let mut raw: *mut c_void = std::ptr::null_mut();
        let accessed = buffer.StartAccess(1 /* bmdBufferAccessRead */).is_ok();
        if buffer.GetBytes(&mut raw).is_err() || raw.is_null() {
            if accessed {
                let _ = buffer.EndAccess(1);
            }
            return S_OK;
        }
        let source = raw as *const u8;
        let mut rgba = vec![0u8; width * height * 4];
        match pixel_format {
            PIXEL_BGRA => {
                for y in 0..height {
                    let line = if flags & 1 != 0 { height - 1 - y } else { y };
                    let row = source.add(line * row_bytes);
                    let dst = &mut rgba[y * width * 4..(y + 1) * width * 4];
                    for x in 0..width {
                        let p = row.add(x * 4);
                        dst[x * 4] = *p.add(2);
                        dst[x * 4 + 1] = *p.add(1);
                        dst[x * 4 + 2] = *p;
                        dst[x * 4 + 3] = 255;
                    }
                }
            }
            PIXEL_YUV => {
                // '2vuy': Cb Y0 Cr Y1, 4 bytes per two pixels.
                let (rv, gu, gv, bu) = if width >= 1280 { (459, 55, 136, 541) } else { (409, 100, 208, 516) };
                for y in 0..height {
                    let line = if flags & 1 != 0 { height - 1 - y } else { y };
                    let row = source.add(line * row_bytes);
                    for pair in 0..width / 2 {
                        let s = row.add(pair * 4);
                        let (cb, y0, cr, y1) = (*s as i32, *s.add(1) as i32, *s.add(2) as i32, *s.add(3) as i32);
                        let d = cb - 128;
                        let e = cr - 128;
                        let mut put = |px: usize, luma: i32| {
                            let c = (luma - 16).max(0);
                            let r = (298 * c + rv * e + 128) >> 8;
                            let g = (298 * c - gu * d - gv * e + 128) >> 8;
                            let b = (298 * c + bu * d + 128) >> 8;
                            let base = (y * width + px) * 4;
                            rgba[base] = r.clamp(0, 255) as u8;
                            rgba[base + 1] = g.clamp(0, 255) as u8;
                            rgba[base + 2] = b.clamp(0, 255) as u8;
                            rgba[base + 3] = 255;
                        };
                        put(pair * 2, y0);
                        put(pair * 2 + 1, y1);
                    }
                }
            }
            _ => {
                if accessed {
                    let _ = buffer.EndAccess(1);
                }
                *self.shared.status.lock().unwrap() = format!("DeckLink: unsupported pixel format 0x{pixel_format:08X}");
                return S_OK;
            }
        }
        if accessed {
            let _ = buffer.EndAccess(1);
        }
        *self.shared.size.lock().unwrap() = (width, height);
        *self.shared.frame.lock().unwrap() = Some(Frame { w: width, h: height, rgba });
        self.shared.seq.fetch_add(1, Ordering::Relaxed);
        self.shared.status.lock().unwrap().clear();
        S_OK
    }
}

/// Runs a DeckLink input until `shared.stop` is set.
pub fn run_capture(shared: Arc<Shared>, config: CaptureConfig) {
    unsafe {
        if !com_init() {
            *shared.status.lock().unwrap() = "COM could not be started for DeckLink".into();
            return;
        }
        let result = (|| -> Result<(), String> {
            let device = device_at(config.device)?;
            if let Ok(device_config) = device.cast::<IDeckLinkConfiguration>() {
                if config.port != 0 {
                    device_config.SetInt(CONFIG_VIDEO_INPUT_CONNECTION, config.port as i64).ok()
                        .map_err(|e| format!("Input connector: {e}"))?;
                }
            }
            let input: IDeckLinkInput = device
                .cast()
                .map_err(|e| format!("This DeckLink device has no capture input: {e}"))?;
            let modes = read_modes(&device, false);
            let mut mode = pick_mode(&modes, config.mode, (1920, 1080), 29.97, true)
                .ok_or("No DeckLink capture modes are available")?.mode;
            let detected_mode = Arc::new(AtomicU64::new(0));
            let callback: IDeckLinkInputCallback = CaptureCallback { shared: shared.clone(), detected_mode: detected_mode.clone() }.into();
            input
                .SetCallback(callback.as_raw())
                .ok()
                .map_err(|e| format!("DeckLink SetCallback: {e}"))?;
            let run = (|| -> Result<(), String> {
                let mut flags = if config.mode == 0 { VIDEO_INPUT_ENABLE_FORMAT_DETECTION } else { 0 };
                if input.EnableVideoInput(mode, PIXEL_YUV, flags).is_err() {
                    flags = 0;
                    input.EnableVideoInput(mode, PIXEL_YUV, flags).ok()
                        .map_err(|e| format!("EnableVideoInput: {e}"))?;
                }
                if let Some(mode) = modes.iter().find(|m| m.mode == mode) {
                    *shared.mode.lock().unwrap() = Some(mode.cam_mode());
                }
                input.StartStreams().ok().map_err(|e| format!("StartStreams: {e}"))?;
                *shared.status.lock().unwrap() = "DeckLink: waiting for signal".into();
                while !shared.stop.load(Ordering::Relaxed) {
                        let detected = detected_mode.swap(0, Ordering::Relaxed) as u32;
                        if detected != 0 && detected != mode {
                            input.StopStreams().ok().map_err(|e| format!("StopStreams: {e}"))?;
                            let _ = input.FlushStreams();
                            let _ = input.DisableVideoInput();
                            input.EnableVideoInput(detected, PIXEL_YUV, flags).ok()
                                .map_err(|e| format!("Could not switch input mode: {e}"))?;
                            input.StartStreams().ok().map_err(|e| format!("StartStreams: {e}"))?;
                            mode = detected;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                }
                Ok(())
            })();
            let _ = input.StopStreams();
            let _ = input.DisableVideoInput();
            let _ = input.SetCallback(std::ptr::null_mut());
            drop(callback);
            run
        })();
        if let Err(error) = result {
            *shared.status.lock().unwrap() = format!("DeckLink: {error}");
        }
    }
}

// ---------------------------------------------------------------- playback

/// Settings of one DeckLink output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputConfig {
    pub device: usize,
    /// Wanted display mode (0 = pick one matching the project).
    pub mode: u32,
    pub port: u32,
    /// Project picture size and frame rate, used when picking a mode.
    pub size: (u32, u32),
    pub fps: f32,
    pub interlaced: bool,
}

struct Latest {
    pixels: Arc<Vec<u8>>,
    w: usize,
    h: usize,
}

struct OutputShared {
    latest: Mutex<Option<Latest>>,
    stop: AtomicBool,
    finished: AtomicBool,
    error: Mutex<String>,
    frames: AtomicU64,
}

/// A running DeckLink output. Cloning it shares the same playback thread.
#[derive(Clone)]
pub struct OutputSink {
    shared: Arc<OutputShared>,
}

impl OutputSink {
    /// Starts playback on a background thread. Device errors appear in [`OutputSink::error`].
    pub fn start(config: OutputConfig) -> OutputSink {
        let shared = Arc::new(OutputShared {
            latest: Mutex::new(None),
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            error: Mutex::new(String::new()),
            frames: AtomicU64::new(0),
        });
        let sink = OutputSink { shared: shared.clone() };
        std::thread::spawn(move || {
            output_thread(Arc::clone(&shared), config);
            shared.finished.store(true, Ordering::Release);
        });
        sink
    }

    /// Hands over the newest composed picture (BGRA rows, as OpenGL read them).
    pub fn submit(&self, pixels: Arc<Vec<u8>>, w: usize, h: usize) {
        if self.shared.stop.load(Ordering::Relaxed) {
            return;
        }
        *self.shared.latest.lock().unwrap() = Some(Latest { pixels, w, h });
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }

    pub fn error(&self) -> Option<String> {
        let error = self.shared.error.lock().unwrap();
        (!error.is_empty()).then(|| error.clone())
    }

    pub fn finished(&self) -> bool {
        self.shared.finished.load(Ordering::Acquire)
    }

    pub fn frames(&self) -> u64 {
        self.shared.frames.load(Ordering::Relaxed)
    }
}

fn output_thread(shared: Arc<OutputShared>, config: OutputConfig) {
    unsafe {
        let result = (|| -> Result<(), String> {
            if !com_init() {
                return Err("COM could not be started".into());
            }
            let device = device_at(config.device)?;
            if let Ok(device_config) = device.cast::<IDeckLinkConfiguration>() {
                if config.port != 0 {
                    device_config.SetInt(CONFIG_VIDEO_OUTPUT_CONNECTION, config.port as i64).ok()
                        .map_err(|e| format!("Output connector: {e}"))?;
                }
            }
            let output: IDeckLinkOutput = device
                .cast()
                .map_err(|e| format!("This DeckLink device has no playback output: {e}"))?;
            let modes = read_modes(&device, true);
            let mode = pick_mode(&modes, config.mode, config.size, config.fps, config.interlaced)
                .ok_or_else(|| "The selected DeckLink output mode is unavailable".to_string())?;
            let (width, height) = (mode.w as i32, mode.h as i32);
            if width <= 0 || height <= 0 {
                return Err("Invalid DeckLink output mode".into());
            }
            output
                .EnableVideoOutput(mode.mode, 0)
                .ok()
                .map_err(|e| format!("DeckLink EnableVideoOutput ({}): {e}", mode.label()))?;
            let time_scale = mode.fps_n.max(1) as i64;
            let time_value = mode.fps_d.max(1) as i64;
            shared.error.lock().unwrap().clear();
            let playback = (|| -> Result<(), String> {
                let mut display_time: i64 = 0;
                let mut latest: Option<Latest> = None;
                let mut started = false;
                while !shared.stop.load(Ordering::Relaxed) {
                if let Some(frame) = shared.latest.lock().unwrap().take() {
                    latest = Some(frame);
                }
                let Some(frame) = latest.as_ref() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                if started {
                    let mut buffered = 0;
                    output.GetBufferedVideoFrameCount(&mut buffered).ok()
                        .map_err(|e| format!("GetBufferedVideoFrameCount: {e}"))?;
                    if buffered >= 3 {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                }
                let mut raw_frame: *mut c_void = std::ptr::null_mut();
                let created = output.CreateVideoFrame(
                    width,
                    height,
                    width * 4,
                    PIXEL_BGRA,
                    0,
                    &mut raw_frame as *mut *mut c_void,
                );
                if created.is_err() || raw_frame.is_null() {
                    return Err(hr("DeckLink CreateVideoFrame", created));
                }
                let video_frame = IDeckLinkMutableVideoFrame::from_raw(raw_frame);
                let buffer: IDeckLinkVideoBuffer = video_frame
                    .cast()
                    .map_err(|e| format!("DeckLink frame has no video buffer: {e}"))?;
                let mut bytes: *mut c_void = std::ptr::null_mut();
                buffer
                    .StartAccess(2 /* bmdBufferAccessWrite */)
                    .ok()
                    .map_err(|e| format!("DeckLink frame write access: {e}"))?;
                let bytes_result = buffer.GetBytes(&mut bytes).ok();
                if bytes_result.is_ok() && !bytes.is_null() {
                    blit_bgra(bytes as *mut u8, width as usize, height as usize, &frame.pixels, frame.w, frame.h);
                }
                let _ = buffer.EndAccess(2);
                bytes_result.map_err(|e| format!("DeckLink frame bytes: {e}"))?;
                if bytes.is_null() {
                    return Err("DeckLink returned an empty output frame".into());
                }
                drop(buffer);
                let scheduled = output.ScheduleVideoFrame(video_frame.as_raw(), display_time, time_value, time_scale);
                drop(video_frame);
                scheduled
                    .ok()
                    .map_err(|e| format!("DeckLink ScheduleVideoFrame: {e}"))?;
                display_time = display_time.wrapping_add(time_value);
                if !started && display_time >= 3 * time_value {
                    output.StartScheduledPlayback(0, time_scale, 1.0).ok()
                        .map_err(|e| format!("StartScheduledPlayback: {e}"))?;
                    started = true;
                }
                if started {
                    shared.frames.fetch_add(1, Ordering::Relaxed);
                }
                }
                Ok(())
            })();
            let _ = output.StopScheduledPlayback(0, std::ptr::null_mut(), time_scale);
            let _ = output.DisableVideoOutput();
            playback
        })();
        if let Err(error) = result {
            *shared.error.lock().unwrap() = error;
        }
    }
}

/// Copies BGRA pixels into a DeckLink frame, scaling (letterboxed) when sizes differ.
unsafe fn blit_bgra(dst: *mut u8, dst_w: usize, dst_h: usize, src: &[u8], src_w: usize, src_h: usize) {
    if src_w == 0 || src_h == 0 || src.len() < src_w * src_h * 4 {
        return;
    }
    if (src_w, src_h) == (dst_w, dst_h) {
        std::ptr::copy_nonoverlapping(src.as_ptr(), dst, dst_w * dst_h * 4);
        return;
    }
    // Black, opaque.
    let words = dst as *mut u32;
    for i in 0..dst_w * dst_h {
        words.add(i).write_unaligned(0xFF00_0000);
    }
    let scale = (dst_w as f64 / src_w as f64).min(dst_h as f64 / src_h as f64);
    let draw_w = ((src_w as f64 * scale) as usize).clamp(1, dst_w);
    let draw_h = ((src_h as f64 * scale) as usize).clamp(1, dst_h);
    let ox = (dst_w - draw_w) / 2;
    let oy = (dst_h - draw_h) / 2;
    for y in 0..draw_h {
        let sy = y * src_h / draw_h;
        let src_row = src.as_ptr().add(sy * src_w * 4);
        let dst_row = dst.add(((oy + y) * dst_w + ox) * 4);
        for x in 0..draw_w {
            let sx = x * src_w / draw_w;
            let s = src_row.add(sx * 4);
            let d = dst_row.add(x * 4);
            *d = *s;
            *d.add(1) = *s.add(1);
            *d.add(2) = *s.add(2);
            *d.add(3) = *s.add(3);
        }
    }
}
