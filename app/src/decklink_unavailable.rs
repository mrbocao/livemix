//! The current SDK adapter uses Windows COM. Keep the routes readable on other builds.
use std::sync::Arc;

pub const CONNECTIONS: [(&str, u32); 1] = [("Auto", 0)];
const UNAVAILABLE: &str = "DeckLink SDK capture and playback are available on Windows only";

pub fn connection_name(_: u32) -> &'static str { "Auto" }
pub fn device_index(link: &str) -> Option<usize> { link.strip_prefix("decklink:")?.parse().ok() }

#[derive(Clone)]
pub struct Device {
    pub index: usize,
    pub name: String,
    pub capture: bool,
    pub playback: bool,
}

pub fn devices_cached() -> Vec<Device> { Vec::new() }
pub fn refresh_devices() -> Vec<Device> { Vec::new() }

#[derive(Clone)]
pub struct Mode { pub mode: u32 }
impl Mode { pub fn label(&self) -> String { String::new() } }
pub fn display_modes(_: usize, _: bool) -> Vec<Mode> { Vec::new() }

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct CaptureConfig { pub device: usize, pub port: u32, pub mode: u32 }

pub fn run_capture(shared: Arc<crate::media::Shared>, _: CaptureConfig) { shared.set_status(UNAVAILABLE); }

#[derive(Clone, Copy, PartialEq)]
pub struct OutputConfig {
    pub device: usize,
    pub port: u32,
    pub mode: u32,
    pub size: (u32, u32),
    pub fps: f32,
    pub interlaced: bool,
}

#[derive(Clone)]
pub struct OutputSink;
impl OutputSink {
    pub fn start(_: OutputConfig) -> Self { Self }
    pub fn submit(&self, _: Arc<Vec<u8>>, _: usize, _: usize) {}
    pub fn stop(&self) {}
    pub fn error(&self) -> Option<String> { Some(UNAVAILABLE.into()) }
    pub fn finished(&self) -> bool { true }
    pub fn frames(&self) -> u64 { 0 }
}
