# Changelog

## Unreleased

- Add a documentation index, a Cameras and capture cards section in the operator guide, a quick start and FAQ in the README, issue and pull request templates, crate metadata, and a project page published with GitHub Pages from `docs/site`.
- Skip drawing and laying out input card controls in rows outside the visible grid when resizing or scrolling.

- Add a LiveMix application icon and a startup screen for the production desk.

- Add a graphics-only DeckLink Key + Fill output on Windows and macOS, with transparent Program overlays and validation of the device profile and video mode.

- Add a Teleprompter input: a script that scrolls at a chosen reading speed (lines per minute) with text size, line spacing, margins, alignment, colours, a highlighted reading line, and mirror or flip for a prompter glass. Play, pause, restart and seek from the input tile or Input Settings → Teleprompter; the script and settings are saved in presets. Load a script from a .txt file (UTF-8, UTF-16 or Windows text) in Add Input or Input Settings, and control the scroll with shortcuts, MIDI or automation: TeleprompterPlayPause, Faster, Slower, Speed, Scroll, Home, End, Bigger, Smaller, FlipSides, FlipVertical and Loop. Settings → Shortcuts → Add Teleprompter keys binds Space, the arrows, Home, + and −.
- Add Input → Camera reads the selected camera's capture options (Resolution, Frame Rate with interlaced modes, Video Format) and an Audio Device row, so capture cards that carry audio on a separate Windows audio input are captured as one input. The same options are in Input Settings, and the audio device is saved in presets.
- Add per-input blur and image alpha masks under Effects, with preset persistence.
- Select the embedded audio bus and delay separately for each DeckLink output.

- Organize External Output into four tabs and automatically detect DeckLink devices at startup and when opening the page.
- List Blackmagic capture devices under Camera. Pause Inputs pauses playable video and both replay channels and stops automatic playlist advancement.

- Add Input → Camera reads the selected camera's capture options (Resolution, Frame Rate with interlaced modes, Video Format) and an Audio Device row, so capture cards that carry audio on a separate Windows audio input are captured as one input. The same options are in Input Settings, and the audio device is saved in presets.

- Fixed crackling and dropouts in audio. The mixer no longer skips an input for a block when a reader holds its buffer, which silenced about 10 ms every few seconds during file playback. File playback uses a clock that moves by at most one sample per update and re-anchors on resume. Recording, streaming, MultiCorder, NDI, OMT and DeckLink take contiguous audio windows, so samples are no longer repeated or skipped between frames, and a short output queue keeps audio and picture in the same frame with no added delay.
- Decoded files that peak above 0 dBFS are rolled off smoothly above 80% of full scale instead of being hard-clipped by the converter.
- Schedule NDI and OMT input audio and video from their sender timestamps on a shared local clock.
- Route delayed source audio through Video Delay inputs and display the newest picture eligible for the configured delay.

- Reuse existing OpenGL texture storage for same-size video uploads and avoid a second upload when an input moves to Preview or Program as a new frame arrives.

- Show the FFmpeg executable in Settings → About and test hardware encoders at the configured output size and frame rate before FFmpeg recording or streaming.
- Upload smaller video textures for input cards that only appear in the grid and restore the original resolution when needed.

- Show the total duration of Media Foundation video files as soon as the reader opens.

- Changing Preview while the T-bar is between endpoints updates the incoming source in Program without moving the fader.

- Replaced the Replay panel with a Replay controller: A/B players beside the camera grid, four Events lists, eight playlists, per-angle Selected and Favourite marks, comments and 56 tags, Shift In/Out, quick marks for the last 5 to 20 seconds, Play Selected, Preview Mode with Take, and an undocked window.
- Playlists keep their own In/Out points, speed and mix per clip, mix between Replay A and B on Program, and are stored with the Events lists and tags in the session manifest.
- Settings and Input Settings use the available screen height and a wider page before scrolling, with compact navigation. Display shows the project format beside the Fullscreen outputs. Audio groups the meter toggles and Fade To Black audio. Options contains Start External Output. Per-input automatic audio mixing is in General, and effect switches appear on the Effects rows.
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
