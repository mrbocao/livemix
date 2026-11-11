# Roadmap

## Video and recording performance

- Expand the Windows D3D11 texture path to more sources and processing stages. The verified hardware path used H.264 playback through D3D11 and OpenGL to Program recording at 1080p29.97 on Intel Arc and NVIDIA RTX 4060, with zero Program readback during the tested path. Startup drops were observed. A mismatched adapter configuration exercised the fallback.
- Profile chroma key, color effects, capture and simultaneous outputs. CPU-backed capture and some processing or sink combinations still require system-memory transfers. Measure 1080p60 and 4K60 with a common scene and report render latency, encoder queue pressure, frame drops, copies and memory use.
- Validate additional GPU and driver combinations, including AMD and hybrid laptops. Profile the Apple Silicon backend and evaluate a native VideoToolbox and GPU surface pipeline. Current macOS media paths use FFmpeg.

## Replay and operator tools

- Reuse encoders across segment boundaries and support direct indexed playback while capture continues. The indexed transport plays finalized segments directly and provides frame stepping and seek-based reverse. Reading the active segment before finalization and GPU-assisted reverse playback remain open. Native vMix manifests open read-only, with MPEG track preparation and codec compatibility still requiring validation against production sessions. Validate eight live angles and synchronization on production hardware.
- Extend Telestrator beyond freehand drawing and verify operation with multiple configured Telestrator inputs. Browser drawing currently targets the first one. The local controller listens on loopback. Remote LAN control would need its own access controls.
- Connect MIDI device selection and received messages to shortcut actions, then validate MIDI and VISCA with physical devices and add examples for scripts and triggers. Improve operator diagnostics for rejected commands and unavailable devices.

## Compatibility notes from the original Windows build

| Area | Observed behavior | LiveMix coverage |
| --- | --- | --- |
| Capture latency | The original passes `CaptureLowLatency` to the input and its DirectShow allocator. | Windows camera inputs can request Media Foundation's low-latency mode. DeckLink, network sources and macOS keep their own capture paths. |
| Windows Update | The application requests startup through its named pipe service. The service watches the vMix process, stops and disables `wuauserv` and `UsoSvc`, then sets their startup modes after the process exits. | No Windows service changes. |
| External Output | The original offers display or streaming settings as presets, output size and frame rate, alpha, audio device, channel count and audio delay, alongside the hardware device and port. | LiveMix routes four outputs, has DeckLink device, connector and mode controls, and sends stereo audio. Independent External format presets, alpha, external audio device selection and channel configuration remain open. |
| Audio | The original has separate renderer and channel assignments for Master, headphones and buses, audio gap handling, matrix presets, Solo PFL, and plugins. | LiveMix has Master/A/B mixing, headphone monitoring, per-input DSP and stereo routing. Independent physical bus outputs, ASIO/VST, full channel matrix and audio gap settings remain open. |
| Web Browser | The original bundles an off-screen Chromium process and CefSharp. | Web Browser input remains unavailable. |
| XAML titles | Legacy titles are loaded through WPF `XamlReader`. | The cross-platform title renderer currently accepts GT Designer packages. Legacy XAML rendering remains open. |
| Production clocks | The original stores settings for two clocks and includes recording, streaming and remaining-time modes. | LiveMix currently shows a single elapsed clock with pause, resume and reset. |

## Compatibility and distribution

- Extend GT feature coverage and vMix XML round trips with representative fixtures. Compare missing sources, asset paths and unsupported features in the import report before running an imported show.
- Verify physical macOS capture and DeckLink input and output, plus NDI, OMT and SRT between separate machines. Native CI exercises media files and builds the Mac package. It does not provide physical production hardware.
- Validate channel routing and long-duration A/V synchronization in simultaneous recordings and MultiCorder at production resolutions. VMX AVI and WMV Streaming remain unavailable.
- Decoders currently routes files between FFmpeg and the platform default. DirectShow filter priorities and blocklists from the reference implementation remain open.
- Add Developer ID signing and notarization for public macOS distribution.
