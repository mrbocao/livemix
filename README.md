# LiveMix

[![Rust CI](https://github.com/mrbocao/livemix/actions/workflows/rust.yml/badge.svg)](https://github.com/mrbocao/livemix/actions/workflows/rust.yml)

LiveMix is an experimental live video production switcher built in Rust for Windows and Apple Silicon Macs. It brings cameras, clips, desktop capture, graphics, and network feeds into a Preview and Program workflow for live events and studio productions. The interface uses egui and offers Classic and Modern themes.

![LiveMix showing the Preview and Program monitors, transitions, overlay controls, and demo inputs](docs/screenshots/switcher.png)

## What you can do

- Switch live sources with Cut, timed transitions, a manual T-bar, GT stingers, and eight overlay channels. Open a larger preview from any input card.
- Build scenes with picture-in-picture layers, crop, position, and chroma key. Load supported GT Designer `.gtzip` titles, edit text fields, and use template clocks.
- Check the picture with safe areas, waveform, RGB parade, and vectorscope. The status bar reports the measured project cadence separately from interface repaints.
- Route Program, Preview, MultiView, or an input to four independent outputs. Use local output windows, NDI, OMT, SRT, or supported DeckLink devices on Windows.
- Record two configurable video outputs, record selected sources with MultiCorder, and stream RTMP/RTMPS through FFmpeg. Save projects as `.vmix` presets for the supported input types.

## Screenshots

The captures show the Windows build with its built-in demo sources.

| Video scopes | Network routing | Input layers |
| --- | --- | --- |
| [![Waveform, vectorscope, and safe areas on Preview](docs/screenshots/video-scopes.png)](docs/screenshots/video-scopes.png) | [![Independent NDI, OMT, and SRT settings for four outputs](docs/screenshots/network-outputs.png)](docs/screenshots/network-outputs.png) | [![Input settings with a picture-in-picture layer on Program](docs/screenshots/input-layers.png)](docs/screenshots/input-layers.png) |

## Build on Windows

Install stable Rust and the Visual Studio Build Tools with the Desktop development with C++ workload. From the repository root, run:

```powershell
cd app
cargo run --locked --release
cargo test --locked --release -- --skip first_camera_delivers_frames
```

The desktop capture test needs an interactive Windows session. The camera test also needs a physical camera. The standard GitHub Actions job skips both.

## Build on macOS

The first Mac build targets **Apple Silicon** with macOS 12 or newer. It shares the switcher, presets, GT titles, and scopes with Windows. FFmpeg provides AVFoundation input for cameras and screen capture, file playback, MP4 recording, RTMP, and SRT. H.264 and HEVC recording can use Apple VideoToolbox. Camera and screen capture require the corresponding macOS permissions.

Install stable Rust and the Xcode Command Line Tools. Install FFmpeg and FFprobe, for example with `brew install ffmpeg`. From the repository root, run:

```sh
cd app
cargo run --locked --release
cargo test --locked --release
cargo test --locked --release ffmpeg_video_roundtrip -- --ignored
```

The macOS CI job packages an ad hoc signed development build as `LiveMix-macOS-arm64.zip`. The archive contains `LiveMix.app` and is available in the artifacts section of successful workflow runs. Developer ID signing and notarization are pending. [Rust workflow runs](https://github.com/mrbocao/livemix/actions/workflows/rust.yml)

## Network and capture dependencies

| Feature | Setup |
| --- | --- |
| RTMP/RTMPS and SRT | Install FFmpeg or set `LIVEMIX_FFMPEG` to its full path. Verify SRT is listed by `ffmpeg -protocols`. SRT supports Caller and Listener with H.264 or HEVC. |
| NDI | Install the native NDI runtime for the platform. On macOS, set `LIVEMIX_NDI_DIR` when `libndi.dylib` is outside the standard install locations. |
| OMT | Provide `libomt.dll` and `libvmx.dll` on Windows, or `libomt.dylib` and `libvmx.dylib` on macOS. Place the pair beside the executable or set `LIVEMIX_OMT_DIR` to their folder. [OMT SDK releases](https://github.com/openmediatransport/libomtnet/releases) |
| DeckLink | Windows only. Install Blackmagic Desktop Video and use **Blackmagic** in **Add Input** or configure a device under **External Output**. Hardware verification is still needed. |

Discover LAN sources under **Add Input → NDI / OMT / Desktop Capture**, or choose **Stream / SRT** for an SRT input. Configure publishing under **Settings → Outputs / NDI / OMT / SRT**. To run local network checks with the runtimes installed, use `cargo test --locked --release loopback -- --ignored` from `app`.

## Current limits

LiveMix is in alpha. Audio controls and meters are still experimental. RTMP and SRT send silent AAC, NDI and OMT carry video, and recording is video-only. GT templates and project presets support a subset of their respective features. The Mac port still needs device and network interoperability testing, and its recording formats are limited to H.264 and HEVC MP4. Sustained network frame rates depend on the source, encoder, machine, and receiver.

## Contribute

The project welcomes focused fixes and reproducible reports, especially around GPU readback, sustained network output, audio mixing, and capture hardware. Include the operating system, GPU and driver, input types, project format, and steps to reproduce. For performance reports, include the measured project FPS and receiver FPS separately when available. [Open an issue](https://github.com/mrbocao/livemix/issues)
