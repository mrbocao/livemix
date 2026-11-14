//! Native DeckLink Key + Fill through the Desktop Video framework on macOS.
use std::ffi::{c_char, c_void};
use std::ptr::{null_mut, NonNull};

type Raw = *mut c_void;
type ResultCode = i32;
type CfString = *const c_void;
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const UTF8: u32 = 0x0800_0100;

#[repr(C)]
struct InterfaceId([u8; 16]);

const OUTPUT: InterfaceId = InterfaceId(0xBE2D9020461E442F84B7E949CB953B9Du128.to_be_bytes());
const ATTRIBUTES: InterfaceId = InterfaceId(0x17D4BF8E4911473A80A0731CF6FF345Bu128.to_be_bytes());
const KEYER: InterfaceId = InterfaceId(0x89AFCAF565F8421E98F796FE5F5BFBA3u128.to_be_bytes());

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringGetCString(value: CfString, buffer: *mut c_char, capacity: isize, encoding: u32) -> u8;
    fn CFRelease(value: CfString);
}

// These slots use the macOS DeckLink SDK interfaces, including IUnknown's first three entries.
struct Interface(NonNull<c_void>);

impl Interface {
    fn new(ptr: Raw) -> Option<Self> { NonNull::new(ptr).map(Self) }
    fn raw(&self) -> Raw { self.0.as_ptr() }

    unsafe fn method<T: Copy>(&self, slot: usize) -> T {
        let table = *self.raw().cast::<*const *const c_void>();
        std::mem::transmute_copy(&*table.add(slot))
    }

    unsafe fn query(&self, id: InterfaceId) -> Option<Self> {
        let query: unsafe extern "C" fn(Raw, InterfaceId, *mut Raw) -> ResultCode = self.method(0);
        let mut result = null_mut();
        (query(self.raw(), id, &mut result) == 0).then(|| Self::new(result)).flatten()
    }

    unsafe fn command(&self, slot: usize) -> ResultCode {
        let call: unsafe extern "C" fn(Raw) -> ResultCode = self.method(slot);
        call(self.raw())
    }
}

impl Drop for Interface {
    fn drop(&mut self) {
        unsafe {
            let release: unsafe extern "C" fn(Raw) -> u32 = self.method(2);
            release(self.raw());
        }
    }
}

fn check(result: ResultCode, context: &str) -> Result<(), String> {
    if result == 0 { Ok(()) } else { Err(format!("{context} (0x{:08x})", result as u32)) }
}

unsafe fn display_name(device: &Interface) -> Option<String> {
    let get_name: unsafe extern "C" fn(Raw, *mut CfString) -> ResultCode = device.method(4);
    let mut value = std::ptr::null();
    if get_name(device.raw(), &mut value) != 0 || value.is_null() { return None; }
    let mut bytes = [0u8; 1024];
    let ok = CFStringGetCString(value, bytes.as_mut_ptr().cast(), bytes.len() as isize, UTF8) != 0;
    CFRelease(value);
    ok.then(|| std::ffi::CStr::from_ptr(bytes.as_ptr().cast()).to_string_lossy().into_owned())
}

pub(crate) struct Keyer {
    frame: Interface,
    keyer: Interface,
    output: Interface,
    width: usize,
    height: usize,
    _library: libloading::Library,
}

impl Keyer {
    pub fn open(name: &str, address: &str, mode: u32, width: u32, height: u32) -> Result<Self, String> {
        unsafe {
            let library = libloading::Library::new("/Library/Frameworks/DeckLinkAPI.framework/DeckLinkAPI")
                .map_err(|error| format!("Could not load Desktop Video for Key + Fill: {error}"))?;
            let factory = library.get::<unsafe extern "C" fn() -> Raw>(b"CreateDeckLinkIteratorInstance_0004\0")
                .map_err(|_| "Desktop Video does not export the DeckLink device iterator")?;
            let iterator = Interface::new(factory()).ok_or("No native DeckLink iterator is available")?;
            let next: unsafe extern "C" fn(Raw, *mut Raw) -> ResultCode = iterator.method(3);
            let mut device = None;
            loop {
                let mut raw = null_mut();
                if next(iterator.raw(), &mut raw) != 0 { break; }
                let Some(candidate) = Interface::new(raw) else { break };
                if display_name(&candidate).is_some_and(|label| label == name || label == address) {
                    if device.is_some() { return Err("More than one DeckLink device has this display name".into()); }
                    device = Some(candidate);
                }
            }
            let device = device.ok_or("DeckLink playback device was not found in the native driver")?;
            let attributes = device.query(ATTRIBUTES).ok_or("DeckLink profile attributes are unavailable")?;
            let flag: unsafe extern "C" fn(Raw, u32, *mut bool) -> ResultCode = attributes.method(3);
            let mut support = false;
            if flag(attributes.raw(), u32::from_be_bytes(*b"keye"), &mut support) != 0 || !support {
                return Err("External keying is unavailable in the selected DeckLink profile".into());
            }
            let output = device.query(OUTPUT).ok_or("The native DeckLink output interface is unavailable")?;
            let keyer = device.query(KEYER).ok_or("DeckLink keyer is unavailable")?;
            let check_mode: unsafe extern "C" fn(Raw, u32, u32, u32, u32, u32, *mut u32, *mut bool) -> ResultCode = output.method(3);
            let mut actual = 0;
            support = false;
            if check_mode(output.raw(), 0, mode, BGRA, 0, 1, &mut actual, &mut support) != 0 || !support {
                return Err("The selected DeckLink mode does not support BGRA external keying".into());
            }
            let create: unsafe extern "C" fn(Raw, i32, i32, i32, u32, u32, *mut Raw) -> ResultCode = output.method(10);
            let mut frame = null_mut();
            check(create(output.raw(), width as i32, height as i32, (width * 4) as i32, BGRA, 0, &mut frame),
                "Could not allocate a DeckLink BGRA frame")?;
            let frame = Interface::new(frame).ok_or("No DeckLink BGRA frame was allocated")?;
            let row_bytes: unsafe extern "C" fn(Raw) -> libc::c_long = frame.method(5);
            if row_bytes(frame.raw()) != libc::c_long::from(width) * 4 {
                return Err("The native DeckLink frame has an unsupported row stride".into());
            }
            let enable_video: unsafe extern "C" fn(Raw, u32, u32) -> ResultCode = output.method(7);
            check(enable_video(output.raw(), mode, 0), "Could not start DeckLink video output")?;
            let session = Self { frame, keyer, output, width: width as usize, height: height as usize, _library: library };
            // Drop the interfaces owned by the open routine before the framework can unload.
            drop(attributes);
            drop(device);
            drop(iterator);
            let enable_keyer: unsafe extern "C" fn(Raw, bool) -> ResultCode = session.keyer.method(3);
            check(enable_keyer(session.keyer.raw(), true), "Could not enable external keying")?;
            let level: unsafe extern "C" fn(Raw, u8) -> ResultCode = session.keyer.method(4);
            check(level(session.keyer.raw(), 255), "Could not set the DeckLink key level")?;
            Ok(session)
        }
    }

    pub fn display(&self, pixels: &[u8], width: usize, height: usize) -> Result<(), String> {
        unsafe {
            let bytes: unsafe extern "C" fn(Raw, *mut Raw) -> ResultCode = self.frame.method(8);
            let mut buffer = null_mut();
            check(bytes(self.frame.raw(), &mut buffer), "Could not access the DeckLink frame")?;
            if buffer.is_null() { return Err("DeckLink returned an empty frame buffer".into()); }
            let target = std::slice::from_raw_parts_mut(buffer.cast::<u8>(), self.width * self.height * 4);
            crate::decklink_pixels::blit(target, self.width, self.height, pixels, width, height, true);
            let display: unsafe extern "C" fn(Raw, Raw) -> ResultCode = self.output.method(12);
            check(display(self.output.raw(), self.frame.raw()), "Could not send DeckLink Fill and Key")
        }
    }
}

impl Drop for Keyer {
    fn drop(&mut self) {
        unsafe {
            self.keyer.command(7);
            self.output.command(8);
        }
    }
}
