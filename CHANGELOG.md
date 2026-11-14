# Changelog

## Unreleased

- Replaced the Replay panel with a Replay controller: A/B players beside the camera grid, four Events lists, eight playlists, per-angle Selected and Favourite marks, comments and 56 tags, Shift In/Out, quick marks for the last 5 to 20 seconds, Play Selected, Preview Mode with Take, and an undocked window.
- Playlists keep their own In/Out points, speed and mix per clip, mix between Replay A and B on Program, and are stored with the Events lists and tags in the session manifest.
- Settings and Input Settings use the available screen height and a wider page before scrolling, with compact navigation. Display shows the project format beside the Fullscreen outputs. Each option now appears once: audio meter toggles on Audio, Fade To Black audio and Start External Output on Options, automatic audio mixing on the input's Audio page, and effect switches on the Effects rows.
- Split `main.rs` into `app`, `outputs`, `desk`, `sources`, `mixer`, `settings`, `ui` and `replay` submodules without behavior changes.

- New project presets and automatic recovery use the .livemix extension. Older XML presets open by content, and XML settings exports use a LiveMix section.

- Added per-input 3D LUTs from .cube or tiled PNG files and a luma key with threshold, softness and inversion. D3D11 inputs apply both effects in the OpenGL shader.
- Added Web Browser inputs with an isolated, shared Chromium process and optional transparent backgrounds.

- Added optional D3D11VA decoding for high-resolution FFmpeg file playback on Windows, with background format detection and software recovery.
- Added optional VideoToolbox decoding for high-resolution FFmpeg file playback on macOS with the same software recovery.
- Standardized product-facing labels and documentation, retaining legacy file compatibility and the third-party MIT notice.

- Index Replay frames per angle using capture time and the encoder's file position, including when frames are dropped.
- Added static PowerPoint and LibreOffice presentation inputs with manual slide navigation.
- Added Virtual Set inputs with configurable scene layers and animated wide, left and right views.
- Added a MediaMTX end-to-end test for two RTMP destinations sharing one encoder and one SRT output.

- Kept D3D11 video textures on the GPU while applying chroma key and colour adjustments, with the existing CPU path as fallback.
- Rendered the waveform, RGB parade and vectorscope on the GPU when available. Matched the reference scope texture dimensions and retained the CPU fallback.

- Added Legacy XML settings import and export for mapped preferences alongside native JSON backups.
- Added an optional DirectShow video decoder on Windows with MPEG-2 and H.264 selection, a filter blocklist and FFmpeg fallback.

- Added Settings import, export and confirmed reset with a backup of the previous preferences. Exports omit RTMP destinations and keys and SRT secrets.
- Added per-input decoder diagnostics, including Windows Media Foundation fallback to FFmpeg before the first picture.
- Added live Program, Preview, recording, streaming and overlay status to the local Web Controller, with overlay controls.
- Made unsupported GT features visible in the title editor and reused mix buffers in the headphone audio callback.
- Added a Program readback frame counter and adjusted spacing in Add Input, Input Settings and External settings.

- Added English and Brazilian Portuguese interface options with a persistent language setting and contributor translation catalogs.

- Added the Replay A compact strip with recording, marks, last-event playback, speed and setup.
- Added editable keyboard and MIDI bindings, learn mode, partial shortcut template import, a searchable function reference and shared action dispatch.
- Added configurable MIDI Note and CC feedback for tally and operating states.

- Added Master/source audio selection per stream, a YouTube stereo AAC profile at 44.1 kHz, and copyable RTMP/RTMPS and SRT diagnostics.
- Added live camera previews in the Replay controller and improved recovery and reporting for the Last preset button.
- Expanded Legacy shortcut commands for layers, routing, title text, audio and Replay event management.

- Added session-level Replay monitor options and an optional advanced view with camera assignment and independent Replay B cue, jog and take controls.
- Changed Last to load a separate automatic project recovery preset, refreshed every minute and before New or normal shutdown.

- Exposed independent A/B replay speed controls from 0% to 100%, with 0% pausing the selected channel.
- Added a recorded-position slider for marking replay events from retained footage.
- Replay marks appear immediately. Normal-speed playback on an individual channel opens the recorded segments directly and closes an active segment early only when playback needs it.

## 0.0001.alpha

### Production and operator controls

- Added Graphite, Slate and Midnight themes, editable Preview and Output colors, dual Fullscreen routes and a Production Clock.
- Added audio-only file inputs, recent media selection and optional NDI/OMT source thumbnails.
- Expanded streaming to five slots and added independent sources, resolutions up to 2160p, bitrate overrides and HEVC over RTMP/RTMPS.
- Added a Windows camera low-latency option, headphone device selection and defaults for input audio delay, routing and automatic mixing.

- Reduced the default height of Settings, Input Settings and Audio Settings. Longer pages scroll within the dialog.
- Standardized spacing and added toggle switches in Settings, Input Settings, Recording Setup, MultiCorder and Streaming Settings.

- Redesigned the docked Audio Mixer with collapsible output and input groups, stereo dBFS meters, compact faders, an Audio Engine statistics window, independent headphone volume, and dedicated Master and per-input audio settings. Added headphone Solo with a pre-fader option and made automatic input mixing follow Program transitions.

- Added persistent multicamera replay sessions, rolling capture, shared event time ranges, Replay A/B inputs, per-channel speed, event selection, and MP4 export.
- Added direct playback of finalized replay segments, frame stepping, seek-based reverse, and read-only import of legacy replay metadata and supported media.
- Added stereo meters to audio-capable inputs and configurable Program and input meters to both MultiViews.
- Stabilized the headphone monitor callback clock and kept resampling phase across blocks to reduce discontinuities from callback jitter. Buffered system audio capture so a mixer read can no longer discard a device callback.
- Added a Telestrator input with a freehand editor and local browser drawing. Added a Video Delay input with a configurable buffer.
- Added local web control, VISCA-over-IP PTZ, command scripts and Program-entry triggers. Added MIDI backend and binding models. Device selection and event dispatch are pending.
- Expanded per-input audio processing and color adjustment. Expanded preset import reporting for unsupported inputs and missing assets.

### Media paths

- Added per-slot stereo audio controls and recording profiles for AVI, MKV, MPEG-2, WMV, MOV and MXF. Windows WMV9 now muxes stereo WMA2.
- Enabled Settings → Decoders with extension-based FFmpeg routing on Windows and optional FFmpeg deinterlacing on Windows and macOS.

- Added Windows D3D11 decoder and recorder interop with OpenGL for supported adapter and driver combinations. Program recording can use GPU textures directly when the fast path is available.
- Added shared encoder groups for compatible RTMP destinations. Added replay and delay buffering, with explicit source selection and bounded queues on the live media paths.

### Documentation

- Expanded the project overview and documented setup, optional runtimes, operator workflows, contributor and agent guidance, and remaining work. Added the MIT notice for third-party components.
