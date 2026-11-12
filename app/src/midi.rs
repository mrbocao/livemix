// MIDI input for shortcuts (the original's Shortcut.m_MIDIChannel / m_MIDINote /
// m_MIDIValue fields, driven by frmMidiDevices and frmMidiChannelMapping).
//
// The original opens every device in its CheckedListBox through winmm
// (midiInOpen / midiInStart) and matches incoming note messages against the
// shortcut table. This module does the same on Windows and uses CoreMIDI on
// macOS (including Apple Silicon), so both builds expose the same API.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// One incoming MIDI message, already normalised to the fields shortcuts use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    /// 0-based channel (the UI shows 1..16).
    pub channel: u8,
    /// Note number, or controller number for ControlChange.
    pub note: u8,
    /// Velocity, or controller value.
    pub value: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    #[default]
    NoteOn,
    NoteOff,
    ControlChange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable index for this enumeration; re-enumerate after a refresh.
    pub index: usize,
    pub name: String,
}

/// A source of MIDI events. Create one and keep it for the life of the app.
pub struct Input {
    event_queue: Arc<Mutex<VecDeque<Event>>>,
    backend: Backend,
}

impl Input {
    pub fn new() -> Result<Self, String> {
        let event_queue = Arc::new(Mutex::new(VecDeque::new()));
        let backend = Backend::new(event_queue.clone())?;
        Ok(Self { event_queue, backend })
    }

    /// Opens exactly the selected devices and closes the others.
    pub fn set_devices(&mut self, devices: &[usize]) {
        self.backend.set_devices(devices);
    }

    /// Devices currently visible to the system. Call after `set_devices` so stale
    /// selections can be ignored.
    /// Drains the events received since the last call.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut queue = self.event_queue.lock().unwrap_or_else(|error| error.into_inner());
        queue.drain(..).collect()
    }
}

pub fn output_devices() -> Vec<DeviceInfo> {
    platform::Output::devices()
}

pub use platform::Output;

pub fn message(kind: Kind, channel: u8, note: u8, value: u8) -> [u8; 3] {
    let status = match kind { Kind::NoteOn => 0x90, Kind::NoteOff => 0x80, Kind::ControlChange => 0xB0 };
    [status | channel.min(15), note.min(127), value.min(127)]
}

/// Splits a short-message stream into note/controller events. Exposed for tests; also
/// used by the CoreMIDI read callback, whose packets may hold several messages.
fn parse_bytes(bytes: &[u8], mut push: impl FnMut(Event)) {
    let mut i = 0;
    while i < bytes.len() {
        let status = bytes[i];
        if status < 0x80 {
            // Data byte without a status (or running status, which CoreMIDI disallows).
            i += 1;
            continue;
        }
        let (kind, length) = match status & 0xF0 {
            0x80 => (Kind::NoteOff, 3),
            0x90 => (Kind::NoteOn, 3),
            0xB0 => (Kind::ControlChange, 3),
            // Other channel messages (program change, aftertouch) and system messages.
            _ => {
                i += 1;
                continue;
            }
        };
        if i + length > bytes.len() {
            break;
        }
        let channel = status & 0x0F;
        let note = bytes[i + 1] & 0x7F;
        let value = bytes[i + 2] & 0x7F;
        // A Note On with velocity 0 is the usual Note Off shorthand.
        let kind = match (kind, value) {
            (Kind::NoteOn, 0) => Kind::NoteOff,
            (kind, _) => kind,
        };
        push(Event { kind, channel, note, value });
        i += length;
    }
}

#[cfg(windows)]
mod platform {
    use super::*;

    const CALLBACK_FUNCTION: u32 = 0x0003_0000;
    const MIM_DATA: u32 = 0x3C3;

    #[repr(C)]
    struct MidiInCaps {
        mid: u16,
        pid: u16,
        version: u32,
        name: [u16; 32],
        support: u32,
    }

    #[repr(C)]
    struct MidiOutCaps {
        mid: u16,
        pid: u16,
        version: u32,
        name: [u16; 32],
        technology: u16,
        voices: u16,
        notes: u16,
        channel_mask: u16,
        support: u32,
    }

    #[link(name = "winmm")]
    extern "system" {
        fn midiOutGetNumDevs() -> u32;
        fn midiOutGetDevCapsW(device: usize, caps: *mut MidiOutCaps, size: u32) -> u32;
        fn midiOutOpen(handle: *mut usize, device: u32, callback: usize, instance: usize, flags: u32) -> u32;
        fn midiOutShortMsg(handle: usize, message: u32) -> u32;
        fn midiOutClose(handle: usize) -> u32;
        fn midiInGetNumDevs() -> u32;
        fn midiInGetDevCapsW(device: usize, caps: *mut MidiInCaps, size: u32) -> u32;
        fn midiInOpen(handle: *mut usize, device: u32, callback: usize, instance: usize,
            flags: u32) -> u32;
        fn midiInStart(handle: usize) -> u32;
        fn midiInStop(handle: usize) -> u32;
        fn midiInReset(handle: usize) -> u32;
        fn midiInClose(handle: usize) -> u32;
    }

    pub struct Output(usize);

    impl Output {
        pub fn devices() -> Vec<DeviceInfo> {
            (0..unsafe { midiOutGetNumDevs() } as usize).filter_map(|index| unsafe {
                let mut caps: MidiOutCaps = std::mem::zeroed();
                if midiOutGetDevCapsW(index, &mut caps, std::mem::size_of::<MidiOutCaps>() as u32) != 0 {
                    return None;
                }
                let length = caps.name.iter().position(|&c| c == 0).unwrap_or(caps.name.len());
                Some(DeviceInfo { index, name: String::from_utf16_lossy(&caps.name[..length]) })
            }).collect()
        }

        pub fn open(index: usize) -> Result<Self, String> {
            let mut handle = 0;
            let code = unsafe { midiOutOpen(&mut handle, index as u32, 0, 0, 0) };
            if code == 0 { Ok(Self(handle)) } else { Err(format!("WinMM MIDI output error {code}")) }
        }

        pub fn send(&self, [status, data1, data2]: [u8; 3]) -> Result<(), String> {
            let code = unsafe { midiOutShortMsg(self.0, u32::from_le_bytes([status, data1, data2, 0])) };
            if code == 0 { Ok(()) } else { Err(format!("WinMM MIDI send error {code}")) }
        }
    }

    impl Drop for Output {
        fn drop(&mut self) {
            unsafe { midiOutClose(self.0); }
        }
    }

    struct Context {
        events: Arc<Mutex<VecDeque<Event>>>,
    }

    /// winmm calls back on its own thread; the pointer is the Box kept in `WinInput`.
    unsafe extern "system" fn callback(_handle: usize, message: u32, instance: usize,
        param1: usize, _param2: usize)
    {
        if message != MIM_DATA || instance == 0 {
            return;
        }
        let context = &*(instance as *const Context);
        let packet = param1 as u32;
        let status = (packet & 0xFF) as u8;
        let note = ((packet >> 8) & 0x7F) as u8;
        let value = ((packet >> 16) & 0x7F) as u8;
        let kind = match status & 0xF0 {
            0x80 => Kind::NoteOff,
            0x90 if value == 0 => Kind::NoteOff,
            0x90 => Kind::NoteOn,
            0xB0 => Kind::ControlChange,
            _ => return,
        };
        let mut queue = context.events.lock().unwrap_or_else(|error| error.into_inner());
        if queue.len() < 512 {
            queue.push_back(Event { kind, channel: status & 0x0F, note, value });
        }
    }

    struct OpenDevice {
        index: usize,
        handle: usize,
        context: *mut Context,
    }

    impl Drop for OpenDevice {
        fn drop(&mut self) {
            unsafe {
                midiInStop(self.handle);
                midiInReset(self.handle);
                midiInClose(self.handle);
                if !self.context.is_null() {
                    drop(Box::from_raw(self.context));
                }
            }
        }
    }

    pub struct Backend {
        events: Arc<Mutex<VecDeque<Event>>>,
        open: Vec<OpenDevice>,
    }

    impl Backend {
        pub fn new(events: Arc<Mutex<VecDeque<Event>>>) -> Result<Self, String> {
            Ok(Self { events, open: Vec::new() })
        }

        pub fn devices() -> Vec<DeviceInfo> {
            let count = unsafe { midiInGetNumDevs() };
            (0..count as usize).filter_map(|index| unsafe {
                let mut caps: MidiInCaps = std::mem::zeroed();
                let result = midiInGetDevCapsW(index, &mut caps,
                    std::mem::size_of::<MidiInCaps>() as u32);
                if result != 0 {
                    return None;
                }
                let length = caps.name.iter().position(|c| *c == 0).unwrap_or(32);
                Some(DeviceInfo { index, name: String::from_utf16_lossy(&caps.name[..length]) })
            }).collect()
        }

        pub fn set_devices(&mut self, devices: &[usize]) {
            self.open.retain(|device| devices.contains(&device.index));
            for &index in devices {
                if self.open.iter().any(|device| device.index == index) {
                    continue;
                }
                let context = Box::into_raw(Box::new(Context { events: self.events.clone() }));
                let mut handle = 0usize;
                let result = unsafe {
                    midiInOpen(&mut handle, index as u32, callback as *const () as usize, context as usize,
                        CALLBACK_FUNCTION)
                };
                if result != 0 {
                    unsafe { drop(Box::from_raw(context)); }
                    continue;
                }
                if unsafe { midiInStart(handle) } != 0 {
                    unsafe {
                        midiInClose(handle);
                        drop(Box::from_raw(context));
                    }
                    continue;
                }
                self.open.push(OpenDevice { index, handle, context });
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::ffi::c_void;

    type OsStatus = i32;
    type CfStringRef = *const c_void;
    type MidiClientRef = u32;
    type MidiPortRef = u32;
    type MidiEndpointRef = u32;

    const UTF8: u32 = 0x0800_0100;

    #[link(name = "CoreMIDI", kind = "framework")]
    extern "C" {
        fn MIDIClientCreate(name: CfStringRef, notify: *const c_void, ref_con: *mut c_void,
            out_client: *mut MidiClientRef) -> OsStatus;
        fn MIDIClientDispose(client: MidiClientRef) -> OsStatus;
        fn MIDIInputPortCreate(client: MidiClientRef, name: CfStringRef,
            read_proc: unsafe extern "C" fn(*const u8, *mut c_void, *mut c_void),
            ref_con: *mut c_void, out_port: *mut MidiPortRef) -> OsStatus;
        fn MIDIPortDispose(port: MidiPortRef) -> OsStatus;
        fn MIDIPortConnectSource(port: MidiPortRef, source: MidiEndpointRef,
            connection: *mut c_void) -> OsStatus;
        fn MIDIPortDisconnectSource(port: MidiPortRef, source: MidiEndpointRef) -> OsStatus;
        fn MIDIOutputPortCreate(client: MidiClientRef, name: CfStringRef, port: *mut MidiPortRef) -> OsStatus;
        fn MIDIGetNumberOfDestinations() -> usize;
        fn MIDIGetDestination(index: usize) -> MidiEndpointRef;
        fn MIDIPacketListInit(list: *mut u8) -> *mut u8;
        fn MIDIPacketListAdd(list: *mut u8, size: usize, current: *mut u8, time: u64,
            length: usize, data: *const u8) -> *mut u8;
        fn MIDISend(port: MidiPortRef, destination: MidiEndpointRef, list: *const u8) -> OsStatus;
        fn MIDIGetNumberOfSources() -> usize;
        fn MIDIGetSource(index: usize) -> MidiEndpointRef;
        fn MIDIObjectGetStringProperty(object: u32, property: CfStringRef,
            out: *mut CfStringRef) -> OsStatus;
        static kMIDIPropertyName: CfStringRef;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(allocator: *const c_void, c_str: *const i8,
            encoding: u32) -> CfStringRef;
        fn CFStringGetCString(string: CfStringRef, buffer: *mut i8, size: isize,
            encoding: u32) -> u8;
        fn CFRelease(value: *const c_void);
    }

    fn cf_string(text: &str) -> CfStringRef {
        let Ok(c_text) = std::ffi::CString::new(text) else { return std::ptr::null() };
        unsafe { CFStringCreateWithCString(std::ptr::null(), c_text.as_ptr(), UTF8) }
    }

    pub struct Output {
        client: MidiClientRef,
        port: MidiPortRef,
        endpoint: MidiEndpointRef,
    }

    impl Output {
        pub fn devices() -> Vec<DeviceInfo> {
            (0..unsafe { MIDIGetNumberOfDestinations() }).filter_map(|index| {
                let endpoint = unsafe { MIDIGetDestination(index) };
                (endpoint != 0).then(|| DeviceInfo { index, name: device_name(endpoint) })
            }).collect()
        }

        pub fn open(index: usize) -> Result<Self, String> {
            let endpoint = unsafe { MIDIGetDestination(index) };
            if endpoint == 0 { return Err("MIDI destination not available".into()); }
            let name = cf_string("LiveMix Output");
            let mut client = 0;
            let mut port = 0;
            let code = unsafe {
                let mut code = MIDIClientCreate(name, std::ptr::null(), std::ptr::null_mut(), &mut client);
                if code == 0 { code = MIDIOutputPortCreate(client, name, &mut port); }
                CFRelease(name);
                code
            };
            if code == 0 { return Ok(Self { client, port, endpoint }); }
            if client != 0 { unsafe { MIDIClientDispose(client); } }
            Err(format!("CoreMIDI output error {code}"))
        }

        pub fn send(&self, message: [u8; 3]) -> Result<(), String> {
            let mut storage = [0_u64; 4];
            let list = storage.as_mut_ptr().cast::<u8>();
            let code = unsafe {
                let packet = MIDIPacketListInit(list);
                if MIDIPacketListAdd(list, std::mem::size_of_val(&storage), packet, 0,
                    message.len(), message.as_ptr()).is_null()
                {
                    return Err("Could not create MIDI packet".into());
                }
                MIDISend(self.port, self.endpoint, list)
            };
            if code == 0 { Ok(()) } else { Err(format!("CoreMIDI send error {code}")) }
        }
    }

    impl Drop for Output {
        fn drop(&mut self) {
            unsafe {
                MIDIPortDispose(self.port);
                MIDIClientDispose(self.client);
            }
        }
    }

    fn device_name(endpoint: MidiEndpointRef) -> String {
        unsafe {
            let mut value: CfStringRef = std::ptr::null();
            if MIDIObjectGetStringProperty(endpoint, kMIDIPropertyName, &mut value) != 0
                || value.is_null()
            {
                return String::new();
            }
            let mut buffer = [0i8; 256];
            let read = CFStringGetCString(value, buffer.as_mut_ptr(), buffer.len() as isize, UTF8);
            CFRelease(value);
            if read == 0 {
                return String::new();
            }
            std::ffi::CStr::from_ptr(buffer.as_ptr()).to_string_lossy().into_owned()
        }
    }

    struct Context {
        events: Arc<Mutex<VecDeque<Event>>>,
    }

    /// CoreMIDI receives packed MIDI 1.0 packets (see MIDIServices.h). The header is
    /// packed to 4 bytes: UInt32 count then the first MIDIPacket at offset 4. The next
    /// packet starts after data[length], 4-byte aligned on ARM64.
    unsafe extern "C" fn read_proc(packet_list: *const u8, ref_con: *mut c_void,
        _source: *mut c_void)
    {
        if packet_list.is_null() || ref_con.is_null() {
            return;
        }
        let context = &*(ref_con as *const Context);
        let count = *(packet_list as *const u32);
        let mut packet = packet_list.add(4);
        for _ in 0..count.min(512) {
            let length = (packet.add(8) as *const u16).read_unaligned() as usize;
            let data = packet.add(10);
            let bytes = std::slice::from_raw_parts(data, length.min(4096));
            let mut events = context.events.lock().unwrap_or_else(|error| error.into_inner());
            parse_bytes(bytes, |event| {
                if events.len() < 512 {
                    events.push_back(event);
                }
            });
            drop(events);
            packet = ((packet.add(10 + length) as usize + 3) & !3) as *const u8;
        }
    }

    pub struct Backend {
        client: MidiClientRef,
        port: MidiPortRef,
        context: *mut Context,
        connected: Vec<MidiEndpointRef>,
    }

    impl Drop for Backend {
        fn drop(&mut self) {
            unsafe {
                for source in self.connected.drain(..) {
                    MIDIPortDisconnectSource(self.port, source);
                }
                if self.port != 0 {
                    MIDIPortDispose(self.port);
                }
                if self.client != 0 {
                    MIDIClientDispose(self.client);
                }
                if !self.context.is_null() {
                    drop(Box::from_raw(self.context));
                }
            }
        }
    }

    impl Backend {
        pub fn new(events: Arc<Mutex<VecDeque<Event>>>) -> Result<Self, String> {
            let context = Box::into_raw(Box::new(Context { events }));
            let name = cf_string("LiveMix");
            let mut client = 0;
            let status = unsafe {
                MIDIClientCreate(name, std::ptr::null(), std::ptr::null_mut(), &mut client)
            };
            if !name.is_null() {
                unsafe { CFRelease(name); }
            }
            if status != 0 {
                unsafe { drop(Box::from_raw(context)); }
                return Err(format!("CoreMIDI client error {status}"));
            }
            let port_name = cf_string("LiveMix Input");
            let mut port = 0;
            let status = unsafe {
                MIDIInputPortCreate(client, port_name, read_proc, context as *mut c_void, &mut port)
            };
            if !port_name.is_null() {
                unsafe { CFRelease(port_name); }
            }
            if status != 0 {
                unsafe {
                    MIDIClientDispose(client);
                    drop(Box::from_raw(context));
                }
                return Err(format!("CoreMIDI input port error {status}"));
            }
            Ok(Self { client, port, context, connected: Vec::new() })
        }

        pub fn devices() -> Vec<DeviceInfo> {
            let count = unsafe { MIDIGetNumberOfSources() };
            (0..count).filter_map(|index| {
                let source = unsafe { MIDIGetSource(index) };
                if source == 0 {
                    return None;
                }
                let name = device_name(source);
                Some(DeviceInfo { index, name: if name.is_empty() { format!("MIDI Source {}", index + 1) } else { name } })
            }).collect()
        }

        pub fn set_devices(&mut self, devices: &[usize]) {
            let count = unsafe { MIDIGetNumberOfSources() };
            let wanted: Vec<MidiEndpointRef> = devices.iter().filter(|index| **index < count)
                .map(|index| unsafe { MIDIGetSource(*index) }).filter(|source| *source != 0)
                .collect();
            for source in self.connected.drain(..) {
                unsafe { MIDIPortDisconnectSource(self.port, source); }
            }
            self.connected = wanted.into_iter().filter(|source| {
                unsafe { MIDIPortConnectSource(self.port, *source, std::ptr::null_mut()) == 0 }
            }).collect();
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;

    pub struct Output;

    impl Output {
        pub fn devices() -> Vec<DeviceInfo> { Vec::new() }
        pub fn open(_index: usize) -> Result<Self, String> { Err("MIDI is unavailable".into()) }
        pub fn send(&self, _message: [u8; 3]) -> Result<(), String> { Err("MIDI is unavailable".into()) }
    }

    pub struct Backend;

    impl Backend {
        pub fn new(_events: Arc<Mutex<VecDeque<Event>>>) -> Result<Self, String> {
            Ok(Self)
        }
        pub fn devices() -> Vec<DeviceInfo> { Vec::new() }
        pub fn set_devices(&mut self, _devices: &[usize]) {}
    }
}

use platform::Backend;

/// Enumerates MIDI input devices without opening them.
pub fn devices() -> Vec<DeviceInfo> {
    Backend::devices()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(bytes: &[u8]) -> Vec<Event> {
        let mut out = Vec::new();
        parse_bytes(bytes, |event| out.push(event));
        out
    }

    #[test]
    fn note_on_and_off_are_normalised() {
        assert_eq!(events(&[0x90, 60, 100]), [Event {
            kind: Kind::NoteOn, channel: 0, note: 60, value: 100,
        }]);
        assert_eq!(events(&[0x93, 61, 0])[0].kind, Kind::NoteOff);
        assert_eq!(events(&[0x82, 61, 22])[0].channel, 2);
    }

    #[test]
    fn several_messages_in_one_packet() {
        let parsed = events(&[0x90, 36, 100, 0xB0, 7, 64]);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[1], Event { kind: Kind::ControlChange, channel: 0, note: 7, value: 64 });
    }
}
