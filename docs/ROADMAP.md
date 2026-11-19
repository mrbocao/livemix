# Roadmap

See the [LiveMix stabilization plan](PLANO-ESTABILIZACAO.md) for acceptance criteria and [Contributing](../CONTRIBUTING.md) for test instructions.

| Priority | Open work | What a contribution can verify |
| --- | --- | --- |
| Production stability | A/V drift, crackle and audio loss with capture, recording and simultaneous outputs | Record a 30-minute rehearsal and compare flash/click timing on monitor, file and receiver. |
| Replay | Validate indexed marks in two-angle and eight-angle sessions, plus slow motion and reverse | Use footage with visible frame numbers and deliberately different drops per camera. |
| GPU | Keep recording surfaces on the GPU while network or physical outputs consume the same composition | Compare CPU, readback bytes and dropped frames at 1080p59.94 and 4K60. |
| Platform I/O | Validate Blackmagic capture and external output on Apple Silicon, and NDI/OMT across machines | Include hardware, drivers, project format, signal and reconnect behavior. |
| Compatibility | Embedded browser audio, video calls, GT gaps and remaining configuration fields | Add synthetic fixtures and list the expected result for each unsupported case. |

## Video and recording performance

- Expand the Windows D3D11 texture path to more sources and processing stages. Chroma key and colour adjustment now run in the OpenGL input pass for D3D11 textures. The scope trace and intensity passes also run in OpenGL, with the CPU fallback retained. The verified hardware path used H.264 playback through D3D11 and OpenGL to Program recording at 1080p29.97 on Intel Arc and NVIDIA RTX 4060, with zero Program readback during the tested path. Startup drops were observed. A mismatched adapter configuration exercised the fallback.
- Profile chroma key, color effects, capture and simultaneous outputs. CPU-backed capture and some processing or sink combinations still require system-memory transfers. Measure 1080p60 and 4K60 with a common scene and report render latency, encoder queue pressure, frame drops, copies and memory use.
- Validate additional GPU and driver combinations, including AMD and hybrid laptops. Profile the Apple Silicon backend and evaluate a native VideoToolbox and GPU surface pipeline. Current macOS media paths use FFmpeg.

## Timing tower

- Validate the tower against a live timing feed over a full session, including reads that fail and a reconnect. The reader accepts a file or an `http`/`https` URL with no authentication; header or key based services need a proxy for now.
- Add a Browse button for the timing file, per-row colours taken from the data (for example team colours), and several classes shown side by side. A shortcut that points the cockpit layer at a chosen car is also open.
- Check `cargo check --target aarch64-apple-darwin` and the macOS package: the HTTPS client brings `ring`, which needs a C toolchain for the target.

## GT titles

- Still reported against a vMix title library: 10 titles whose pictures are not in their package (put the picture beside the title, at the path the title names). Drawn approximately: masks (the bounding box of their object; an exact mask needs an off-screen pass), `Bounding` (the saved size is used and does not follow changed text), 3D rotation other than half turns (no perspective), pages, and image sequences (one picture a second). A negative rectangle `Radius` (12 titles) is ignored because its meaning is not known. Rectangle `Style="Italic"` (8 files), text `Bounding`, text decorations and a negative `Radius` are ignored without a notice in the editor; `livemix --gt-report` lists them.

## Live timing

- Switch the cockpit camera by driver, sector times and best-lap highlights, custom request headers for authenticated feeds, and a file picker for the data source. The tower follows the data's own order and does not run a race clock.

## Replay and operator tools

- Camera Match can request exposure and white balance on compatible Windows UVC drivers. Add VISCA matching and hardware gain control. Validate the Sony SDK bridge, Camera Match writes and simultaneous SRT reception on a physical PXW-Z200. Validate the advice against physical cameras and a colour chart, and add an automatic white balance for a single input that uses the same measurements.

- Extend the Replay controller with recorded thumbnails for all angles, a music bed and imported clips in playlists, cueing a quick mark as soon as its segment closes, and control-surface mapping for Shift In/Out, jog and the speed fader. Validate playlist mixes between Replay A and B and Preview Mode Take on recorded sessions.

- Reuse encoders across segment boundaries and support reading the active recording without finalizing it. Normal-speed LiveMix events already play directly from finalized segments. The indexed transport plays finalized segments directly and provides frame stepping and seek-based reverse. OMT VMX1 and optional fragmented MP4 can prepare a clip from the active file. Direct playback from the active file without preparing a clip and GPU-assisted reverse playback remain open. Legacy replay manifests open read-only, with MPEG track preparation and codec compatibility still requiring validation against production sessions. Validate eight live angles and synchronization on production hardware.
- Extend Telestrator beyond freehand drawing and verify operation with multiple configured Telestrator inputs. Browser drawing currently targets the first one. The local controller listens on loopback. Remote LAN control would need its own access controls.
- Expand executable shortcut coverage beyond the current switching, media, audio, output and Replay commands. The function reference includes 713 names from the inspected version, including commands that depend on unavailable features. Per-device MIDI controller profiles, vendor feedback protocols, foreign preset GUID mapping and physical MIDI/VISCA validation remain open.

## Compatibility notes from the original Windows build

| Area | Observed behavior | LiveMix coverage |
| --- | --- | --- |
| Capture latency | The original passes `CaptureLowLatency` to the input and its DirectShow allocator. | Windows camera inputs can request Media Foundation's low-latency mode. DeckLink, network sources and macOS keep their own capture paths. |
| Windows Update | The application requests startup through its named pipe service. The service watches the application process, stops and disables `wuauserv` and `UsoSvc`, then sets their startup modes after the process exits. | No Windows service changes. |
| External Output | The reference offers display or streaming format presets, alpha, audio device, channel count and delay, alongside device and port. | LiveMix routes eight outputs with independent DeckLink Auto size and timing, device mode, Key + Fill and up to 16 embedded channels on supported paths. Streaming-format presets and an audio device per External route remain open. |
| Audio | The original has separate renderer and channel assignments for Master, headphones and buses, audio gap handling, matrix presets, Solo PFL, and plugins. | LiveMix has independent stereo devices for Master and A–G, headphone monitoring and per-input DSP. ASIO/VST, full channel matrix and audio gap settings remain open. |
| Web Browser | The original bundles an off-screen Chromium process and CefSharp. | The Web Browser input captures HTTP and HTTPS pages through a shared Chromium process. Input Preview forwards mouse and keyboard, and supported page audio is routed to the mixer. Embedded frame audio and protected media remain open. |
| XAML titles | Legacy titles are loaded through WPF `XamlReader`. | The cross-platform title renderer currently accepts GT Designer packages. Legacy XAML rendering remains open. |
| Production clocks | The original stores settings for two clocks and includes recording, streaming and remaining-time modes. | LiveMix has two configurable clocks with messages, elapsed time, countdown, recording, streaming and Program remaining modes. Shortcut and API control remain open. |

## Compatibility and distribution

- Validate the static presentation input with LibreOffice on macOS, large decks and embedded fonts. A separate live presentation path is needed for animations and embedded media.
- Expand Virtual Sets with imported set packages and editable camera presets. The initial input uses LiveMix layers and three built-in animated views.

- Extend GT feature coverage, and validate legacy XML round trips with representative fixtures. Across the 743 titles that ship with vMix 28, the remaining gaps are, by number of titles: pictures referenced outside the package or missing (28), Ticker (19), Mask (15), Skew on pictures and text (8), Cube geometry on objects other than pictures (6), 3D Transform rotation (5), FlipX (4), QR codes (4), extra storyboard pages (3), image sequences (2) and Scroll (1). Crop feathering is also missing, and X and Y continuous rotation of flat objects is drawn face-on (width or height shrinks) rather than in perspective. Run `report_unsupported_gt_features` with `LIVEMIX_GT_FIXTURES` set to count them again. Compare missing sources, asset paths and unsupported features in the import report before running an imported show.
- Validate Replay storage across physical drives, including disconnect and reconnect during capture, indexed playback, export and retention.
- Validate compressed OMT in long multicamera sessions, including mixed source formats, changing signal formats and multiple disks. Add direct-to-MKV NDI capture with a supported SDK and integrate compressed NDI into Replay.
- Verify physical macOS capture and DeckLink input and output, plus NDI, OMT and SRT between separate machines. Native CI exercises media files and builds the Mac package. It does not provide physical production hardware.
- Validate channel routing and long-duration A/V synchronization in simultaneous recordings and MultiCorder at production resolutions. VMX AVI and WMV Streaming remain unavailable.
- Windows has an optional DirectShow video path with MPEG-2 and H.264 selection and a blocklist. Selected filters can open their own property pages, and the per-input field blend works on DirectShow video. MPEG audio selection and higher-quality native deinterlacing remain open. See the [modal audit](AUDITORIA-MODAIS.md) for additional differences.
- Add Developer ID signing and notarization for public macOS distribution.
