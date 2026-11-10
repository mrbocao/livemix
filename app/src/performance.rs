use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{w, GUID};
use windows::Win32::Devices::DeviceAndDriverInstallation::*;
use windows::Win32::Devices::Properties::{DEVPROPKEY, DEVPROPTYPE, DEVPROP_TYPE_UINT32};
use windows::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
use windows::Win32::System::Registry::{RegCloseKey, RegGetValueW, KEY_QUERY_VALUE, RRF_RT_REG_QWORD};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes, GetSystemTimes};

const MIB: u64 = 1024 * 1024;
const DEFAULT_TRANSFER_BUDGET: u64 = 250 * MIB;
const LARGE_BAR_FALLBACK: u64 = 1000 * MIB;

#[derive(Default)]
pub struct VideoClock {
    rate: f64,
    origin: f64,
    next_slot: u64,
    sample_slots: u64,
    sample_frames: u64,
    pub picture_time: f64,
    pub fps: Option<f64>,
}

impl VideoClock {
    pub fn advance(&mut self, now: f64, rate: f64) -> bool {
        if self.rate != rate {
            *self = Self { rate, origin: now, ..Self::default() };
        }
        let slot = (((now - self.origin) * rate + 1e-9).floor()) as u64;
        if slot < self.next_slot { return false; }

        // Missed slots lower the measured rate. They never trigger a burst of frames.
        self.sample_slots += slot - self.next_slot + 1;
        self.sample_frames += 1;
        self.next_slot = slot + 1;
        self.picture_time = now;
        if self.sample_slots as f64 >= rate {
            self.fps = Some(rate * (self.sample_frames as f64 / self.sample_slots as f64));
            self.sample_slots = 0;
            self.sample_frames = 0;
        }
        true
    }

    pub fn next_deadline(&self) -> f64 {
        self.origin + self.next_slot as f64 / self.rate
    }
}

#[cfg(test)]
mod video_clock_tests {
    use super::VideoClock;

    #[test]
    fn extra_repaints_do_not_create_video_frames() {
        for rate in [25.0, 29.97, 59.94, 60.0] {
            let mut clock = VideoClock::default();
            let frames = (0..2000).filter(|i| clock.advance(f64::from(*i) / 200.0, rate)).count();
            assert_eq!(frames, (10.0_f64 * rate).ceil() as usize);
            assert_eq!(clock.fps, Some(rate));
        }
    }

    #[test]
    fn missed_frames_are_measured_and_not_replayed() {
        let mut clock = VideoClock::default();
        for i in 0..50 {
            assert!(clock.advance(f64::from(i) / 25.0, 60.0));
        }
        assert!((clock.fps.unwrap() - 25.0).abs() < 1.0);
        assert!(!clock.advance(49.0 / 25.0, 60.0));
        assert!(clock.next_deadline() > 49.0 / 25.0);

        assert!(clock.advance(2.0, 30.0));
        assert_eq!(clock.fps, None);
        assert!(!clock.advance(2.001, 30.0));
    }
}

#[derive(Default)]
pub struct Status {
    pub process_cpu: Option<u32>,
    pub total_cpu: Option<u32>,
    pub render_ms: Option<u64>,
    pub transfer_percent: u32,
    pub transfer_bytes: u64,
    pub transfer_budget: u64,
}

pub struct Monitor {
    pub status: Status,
    pub render_peak: Arc<AtomicU64>,
    sampled_at: Instant,
    previous: CpuTimes,
    logical_processors: f64,
}

impl Monitor {
    pub fn new() -> Self {
        Self {
            status: Status { transfer_budget: DEFAULT_TRANSFER_BUDGET, ..Status::default() },
            render_peak: Arc::default(),
            sampled_at: Instant::now(),
            previous: CpuTimes::read(),
            logical_processors: std::thread::available_parallelism().map_or(1, |n| n.get()) as f64,
        }
    }

    pub fn set_adapter(&mut self, renderer: &str) {
        self.status.transfer_budget = unsafe { adapter_budget(renderer) }
            .unwrap_or(DEFAULT_TRANSFER_BUDGET);
    }

    pub fn update(&mut self, now: Instant, transfer_bytes: impl FnOnce() -> u64) {
        let elapsed = now.duration_since(self.sampled_at);
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        let current = CpuTimes::read();
        self.status.process_cpu = current.process.zip(self.previous.process)
            .and_then(|(next, prev)| next.checked_sub(prev))
            .map(|ms| (ms as f64 * 0.1 / seconds / self.logical_processors).round_ties_even() as u32);
        self.status.total_cpu = current.system.zip(self.previous.system)
            .and_then(|(next, prev)| system_percent(next, prev))
            // The vMix status formatter scales the rounded system sample by the timer interval.
            .map(|percent| (percent / seconds).round_ties_even() as u32);
        let peak = self.render_peak.swap(0, Relaxed);
        self.status.render_ms = (peak != 0).then_some(peak / 1000);
        self.status.transfer_bytes = transfer_bytes();
        self.status.transfer_percent = (100.0 * self.status.transfer_bytes as f64 / self.status.transfer_budget as f64)
            .round_ties_even().min(100.0) as u32;
        self.previous = current;
        self.sampled_at = now;
    }
}

struct CpuTimes {
    process: Option<u64>,
    system: Option<[u64; 3]>,
}

impl CpuTimes {
    fn read() -> Self {
        let (mut creation, mut exit, mut kernel, mut user, mut idle) = Default::default();
        unsafe {
            let process = GetProcessTimes(GetCurrentProcess(), &mut creation, &mut exit, &mut kernel, &mut user)
                .ok().map(|_| ((ticks(kernel) + ticks(user)) as f64 / 10_000.0).round_ties_even() as u64);
            let system = GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user))
                .ok().map(|_| [ticks(idle), ticks(kernel), ticks(user)]);
            Self { process, system }
        }
    }
}

fn ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn system_percent(next: [u64; 3], prev: [u64; 3]) -> Option<f64> {
    let idle = next[0].checked_sub(prev[0])?;
    let kernel = next[1].checked_sub(prev[1])?;
    let user = next[2].checked_sub(prev[2])?;
    let total = kernel.checked_add(user)?;
    let busy = total.checked_sub(idle)?;
    (total > 0).then(|| (100.0 * busy as f64 / total as f64).round_ties_even())
}

// Match the active GL adapter to a display device. As in vMix, use driver
// memory only when the PCI property reports a 64-bit prefetchable memory BAR.
unsafe fn adapter_budget(renderer: &str) -> Option<u64> {
    const BAR_TYPES: DEVPROPKEY = DEVPROPKEY {
        fmtid: GUID::from_u128(0x3ab22e31_8264_4b4e_9af5_a8d2d8e33e62),
        pid: 16,
    };
    let devices = SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_DISPLAY), None, None, DIGCF_PRESENT).ok()?;
    let mut budget = None;
    let renderer = renderer.to_ascii_lowercase();
    for index in 0.. {
        let mut device = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
        if SetupDiEnumDeviceInfo(devices, index, &mut device).is_err() {
            break;
        }
        let mut description = [0u8; 512];
        let mut len = 0;
        if SetupDiGetDeviceRegistryPropertyW(devices, &device, SPDRP_DEVICEDESC, None, Some(&mut description), Some(&mut len)).is_err() {
            continue;
        }
        let name: Vec<_> = description[..len as usize].chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|c| *c != 0).collect();
        let name = String::from_utf16_lossy(&name).to_ascii_lowercase();
        if name.is_empty() || !renderer.contains(&name) {
            continue;
        }
        let mut property_type = DEVPROPTYPE(0);
        let mut bar_types = [0u8; 4];
        if SetupDiGetDevicePropertyW(devices, &device, &BAR_TYPES, &mut property_type, Some(&mut bar_types), None, 0).is_ok()
            && property_type == DEVPROP_TYPE_UINT32 && bar_types[3] != 0
        {
            budget = Some(driver_memory(devices, &device).unwrap_or(LARGE_BAR_FALLBACK));
        }
        break;
    }
    let _ = SetupDiDestroyDeviceInfoList(devices);
    budget
}

unsafe fn driver_memory(devices: HDEVINFO, device: &SP_DEVINFO_DATA) -> Option<u64> {
    let key = SetupDiOpenDevRegKey(devices, device, DICS_FLAG_GLOBAL.0, 0, DIREG_DRV, KEY_QUERY_VALUE.0).ok()?;
    let mut bytes = 0u64;
    let mut size = std::mem::size_of::<u64>() as u32;
    let status = RegGetValueW(key, None, w!("HardwareInformation.qwMemorySize"), RRF_RT_REG_QWORD,
        None, Some((&mut bytes as *mut u64).cast()), Some(&mut size));
    let _ = RegCloseKey(key);
    (status == ERROR_SUCCESS && bytes > 0).then_some(bytes)
}
