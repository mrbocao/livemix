# Setup and dependencies

## Windows

Install stable Rust and the Visual Studio Build Tools with **Desktop development with C++** and a Windows SDK. The CI build uses the MSVC toolchain. From the repository root:

```powershell
cd app
cargo run --locked --release
```

The executable is `app/target/release/livemix.exe`. Windows uses Media Foundation for supported camera, video decode and recording paths, CPAL for system audio, and a WinMM backend for MIDI in development. Install current drivers for your hardware. Windows editions with optional media components may require those components for Media Foundation codecs.

## macOS

Use Apple Silicon and macOS 12 or newer. Install stable Rust, Xcode Command Line Tools, and FFmpeg with its companion FFprobe. The Mac backend uses AVFoundation through FFmpeg for camera and screen capture, and can use VideoToolbox for recording. Allow camera, microphone and screen-recording access as needed. The MIDI backend in development uses CoreMIDI.

```sh
cd app
cargo run --locked --release
```

Successful macOS CI runs offer `LiveMix-macOS-arm64.zip` in their artifacts. It contains an ad hoc signed development build. Developer ID signing and notarization are open work. [Workflow runs](https://github.com/mrbocao/livemix/actions/workflows/rust.yml)

## Install only what you use

| Feature | Extra dependency |
| --- | --- |
| Embedded audio from video files on Windows, Mac media processing, RTMP/RTMPS and SRT | Install FFmpeg or point `LIVEMIX_FFMPEG` to the executable. For Mac playback metadata, keep `ffprobe` beside the selected `ffmpeg` executable. If LiveMix finds FFmpeg through PATH, make both commands available there. Verify that `ffmpeg -protocols` lists SRT for SRT use. |
| Audio-only file inputs | Install FFmpeg and FFprobe. Set `LIVEMIX_FFPROBE` if FFprobe is installed in a separate location. |
| Recording and file decoders | FFmpeg and FFprobe are required for the FFmpeg decoder path and extended recording formats. The recording profiles use the encoders provided by the installed FFmpeg build. Windows MP4 and WMV9 use Media Foundation. |
| Replay playback and export | Install FFmpeg with the `libx264` encoder. Replay recording uses the platform recorder. Playback and export prepare an MP4 from the selected time range and angle. |
| FFmpeg hardware streaming | Use a build with the encoder you need and a compatible driver. Windows tries NVENC, AMF and Quick Sync where available. Mac can use VideoToolbox. LiveMix checks whether the encoder can actually initialize. |
| NDI | Install the platform's native NDI runtime. Windows also checks `NDI_RUNTIME_DIR_V6` and `NDI_RUNTIME_DIR_V5`. Mac can use `LIVEMIX_NDI_DIR` for the directory containing `libndi.dylib`. |
| OMT | Supply matching native `libomt.dll` and `libvmx.dll` on Windows, or `libomt.dylib` and `libvmx.dylib` on macOS. Place both beside the executable or set `LIVEMIX_OMT_DIR` to their directory. [OMT releases](https://github.com/openmediatransport/libomtnet/releases) |
| DeckLink | Install Blackmagic Desktop Video. Windows uses its installed interfaces. On Mac, set `LIVEMIX_DECKLINK_FFMPEG` to an FFmpeg build compiled with DeckLink support and configure the playback connector in Desktop Video Setup. [FFmpeg DeckLink devices](https://ffmpeg.org/ffmpeg-devices.html#decklink) |
| PTZ | A VISCA-over-IP camera reachable by UDP, normally on port 52381. |


FFmpeg lookup prefers `LIVEMIX_FFMPEG`, then an executable beside LiveMix, then the per-user dependency directory, standard Homebrew paths on Mac, and the system path. The per-user directories include `%LOCALAPPDATA%\LiveMix\dependencies\ffmpeg` on Windows and `~/Library/Application Support/LiveMix/bin` on Mac.

```powershell
$env:LIVEMIX_FFMPEG = 'C:\Tools\ffmpeg\bin\ffmpeg.exe'
.\app\target\release\livemix.exe
```

```sh
LIVEMIX_DECKLINK_FFMPEG=/path/to/decklink-ffmpeg /Applications/LiveMix.app/Contents/MacOS/livemix
```

## Hybrid GPUs on Windows

Set the executable's graphics preference in Windows if you want OpenGL on the dedicated GPU. In **Settings → Performance**, choose **Automatic** to let LiveMix match the OpenGL renderer to a DXGI adapter when it can identify it. An explicit D3D11 choice applies on restart. The GPU texture path requires matching adapters and driver support for OpenGL/D3D11 interoperability. Check **D3D11 input** in recording diagnostics and compare the Program readback counter before and during a recording. See the [roadmap](ROADMAP.md) for the tested scope.
