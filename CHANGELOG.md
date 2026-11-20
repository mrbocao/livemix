# Changelog

## Unreleased

## 0.0.1-alpha.2 - 2026-11-20

- Documentation: new screenshots of the Replay controller, the Teleprompter and the live timing tower in the README, the project page and the operator guide. `LIVEMIX_OPEN=teleprompter` with `--screenshot` renders the Teleprompter scene.

- External Output Settings: a **Format preset** for the External Renderer (720p50/59.94, 1080i50/59.94, 1080p23.98–59.94, 2160p25–59.94) sets size, rate and interlacing in one step.
- Web Browser input: audio from embedded frames (such as a video player inside the page) now reaches the mixer, and several sounds playing at once are mixed instead of queued one after another.
- Replay: the end-of-segment report also counts frames a camera's encoder could not take in time.

- Add title countdowns: `SetCountdown`, `StartCountdown`, `PauseCountdown`, `SuspendCountdown`, `StopCountdown`, `AdjustCountdown` and `ChangeCountdown` drive a time in a GT title field, following the original's countdown rules (rounding up to the shown second, pause toggling, adjustments kept within the duration).
- Replay reports cameras that skipped frames when a segment closes ("Last segment: camera 2 skipped 14 frames"), counting each missing frame interval as the original's replay writer does.

- Added **Replay → Setup → Record cameras from capture** (off by default). Camera angles are encoded from the source's capture thread (Media Foundation, DirectShow, DeckLink, NDI, OMT, SRT and FFmpeg devices) instead of the Program renderer and a GPU readback, and the update loop sends only their audio. Only inputs whose picture is exactly what the source captures qualify: an input with a position, crop, layers, colour correction or effects, key, mirror, sharpen, deinterlace, aspect ratio or video delay is still rendered, so the recording keeps those settings. Pictures are kept at the replay frame rate. On Windows, a camera that matches the replay size is converted to NV12 once, in integer arithmetic, before reaching the encoder. Recorded inputs switch to system-memory frames. A camera that sends no picture within two seconds is rendered for the rest of the recording. Replay encoders queue 10 frames instead of 5 to hold the separate audio frames.
- Replay performance with many cameras: Windows Media Foundation replay segments are encoded with a keyframe every half second, no B-frames and low-latency mode (an encoder that rejects these settings records with its defaults), so cueing, jog and reverse decode fewer frames. Reverse and backward jog keep the pictures decoded on the way to an earlier frame on the GPU (up to 256 MB per player) and show the next backward steps without seeking again. The Program readback allows 32 frames in flight instead of 16, and its buffer pool holds frames for every output (up to 512 MB, NV12 and BGRA together) instead of one recording's queue, so eight replay cameras reuse buffers instead of allocating each frame.

- GT brushes: a brush with `Type="Transparent"` no longer paints its `Color` (frames with a transparent fill drew as solid panels), gradient stops without a colour are transparent instead of taking the brush colour, gradients follow the brush's `WrapX` (Clamp, Wrap or Mirror), and a shape with a transparent fill casts a shadow only from its outline.

- GT titles: rectangles with the Cube geometry draw as a box that can spin, `Page1`, `Page2`… storyboards play once on request (**Play page** in the Title Editor, or `TitleBeginAnimation <input> Page1`), the Scroll animation moves a brush across its object, images with several pictures in their resource cycle through them (ImageSequenceLoop), 3D rotation about X or Y squashes and mirrors any object, and text can be mirrored. A mask that names an object missing from the title, and the `Style`, `SizeMode`, `Locked`, `DataFlags` and empty `TextDecoration` defaults, no longer count as gaps. Only titles whose pictures are missing from their package still report an unsupported feature: 733 of the 743 titles in a vMix install now open clean (722 before).
- GT titles: `QRCode` objects draw a QR code of the text of the field named like the object, the Skew effect now applies to pictures and text, half turns about X or Y and the FlipX and FlipY effects mirror shapes and pictures, and a mask limits an object to the area of the mask object. A mask is an approximation, so `livemix --gt-report` lists it on an `approximate:` line instead of as unsupported. Against the 743 titles in a vMix install, 722 now open with no unsupported feature (698 before).
- Windows Replay packs eligible H.264 angles to NV12 on the GPU before reading frames into system memory, with BGRA fallback.

- GT titles: `Ticker` objects scroll their items in a loop, horizontally or vertically, with plain text or with the item template of the title. The items are the text of the ticker's field, one per line (`|` separates the fields of a template item), so they can be edited in the title editor. All 19 ticker titles in a vMix install now open with no unsupported feature (698 of 743, up from 592).
- GT titles: rectangles with a `Radius` are drawn with rounded corners, `TextEffect` Uppercase and Lowercase and `FontStretch` are applied, the FlipX and FlipY effects mirror pictures, and bitmaps the package lacks are read from beside the title or from a `textures` folder (the vMix texture library when installed, or the folder in `LIVEMIX_GT_TEXTURES`). The transparent "Clear Image" placeholder no longer counts as missing. Against the 743 titles in a vMix install, 682 now open with no unsupported feature (592 before).
- Fixed Image and Photos inputs failing with "invalid PNG signature" when a picture's extension does not match its contents (for example a JPEG saved as `.png`). The format is now detected from the file contents.

- Added `livemix --gt-report <file or folder>`, which loads every `.gtzip` under a path and lists the GT features LiveMix does not support and the attributes it ignores, per file and by how many files use each. The reader now also reports a layer without a composition and an image without a bitmap, which it used to skip silently.
- GT titles now play continuous **FillOffset** and **StrokeOffset** (scrolling gradient fills and outlines), **Bounce** entrances and exits, and **RotateContinuous** on any object. They also accept a continuous **None**, and draw gradient outlines (shapes previously drew every outline in one colour). Gradients and bitmap fills mirror past their ends like GT brushes, which changes how some static gradients look. Fixed swapped **CubicEasingIn** and **CubicEasingOut** curves, which made eased GT animations start slow where they should start fast and vice versa.
- Draw GT Designer objects rotated with **Transform** (shapes, gradients, pictures and text turn about their centre). 96 of the 101 bundled vMix titles that use it now draw it; 3D tilt, rotated layers and Transform scale or pan are still reported as unsupported. Add an ignored `report_unsupported_gt_features` test that counts missing GT features across a folder of titles.

- Added Live Timing for GT titles: a JSON or XML file or http(s) URL is read on a background thread and mapped onto `Row1…RowN` fields, with lap time, gap to the leader and interval, class filtering or rotation, rows that slide when positions change, and one data-change animation per changed field. Shortcuts `SetTimingMode` and `SetTimingClass` change the view. The `docs/examples/timing-tower` folder has a template generator, sample data and a race simulator.
- Added a live timing tower for GT titles. A title reads a JSON or XML file or URL on a background thread and fills its `Row1…RowN` fields, with lap time, gap to the leader or interval, class filtering and rotation, rows that slide when the order changes, and one data-change animation per changed field. `docs/examples/timing-tower` has a generator for a template, sample data and a race simulator. GT titles can now animate several fields at once; this adds the `ureq` HTTPS client.

- Input and Master faders and the headphone knob now follow the original's audio taper, gain = (fader / 100)⁴ (100 = 0 dB, 50 ≈ −24 dB), instead of a linear gain; fader and knob tooltips show gain and dB. New inputs and Master start at 100. Imported and exported legacy presets store `VolumeF` as gain, and a missing value means full volume. LiveMix presets keep their saved fader positions, so their levels change only through the new curve.

- Speed up Windows Media Foundation recording from system-memory frames: scaling to the Output Size and the RGBA→BGRA swap now happen in one multi-threaded pass straight into the encoder buffer, each recording queues 5 frames like the original encoder pool instead of 3, and the Program readback pool keeps enough frames to recycle buffers during a recording.
- Record MP4 AAC audio at 192 Kbps on Windows (was 128 Kbps), matching the original's default Audio Bit Rate capped to what the Windows AAC encoder accepts.

- Let each Teleprompter follow a selected input's pre-fader audio level and pause scrolling during silence.

- Add a Sony Camera / SRT listener input with optional Sony Camera Remote SDK control through a local REST bridge and Camera Match.

- Add optional fragmented MP4 recording for H.264 and HEVC Replay angles, so Play can read an event from the active recording.
- Isolate Replay encoders from the compositor's Windows graphics device. Replay defaults to software encoding and automatically switches to software after an error in an encoded angle when hardware encoding is enabled.

- Add browser preview mouse and keyboard controls and stereo audio from eligible page sources, GT letter spacing and text shadows, and HyperDeck transport status and ordered playlists.
- Add Windows DirectShow filter properties and per-input field blending, and Camera Match exposure and white balance requests on supported UVC drivers.
- Prepare OMT Replay events from the active recording without forcing segment rotation.
- Allow compatible Windows Media Foundation capture devices to supply GPU textures, retaining CPU frames for raw MultiCorder and effects that require them.

- Add two configurable production clocks, independent stereo output devices for Master and A–G, independent DeckLink Auto formats and optional indexed Replay resume above 0%.
- Add HyperDeck clip, range and recording controls to capture input settings.

- Replay quick marks (**-5** to **-20**) now park the selected player on the new highlight's In Point, waiting for the open recording segment to close when recording live. **Play Selected** and **PL** explain when there is nothing to play, and imported read-only sessions no longer offer playlist edits that could not be saved.
- Add an optional slow-frame log, enabled with `LIVEMIX_SLOW_FRAMES`, that records how long each phase of a slow interface update took.
- Add Camera Match: compare a reference and a target camera, get advice for exposure (shutter, iris, ISO), white balance, tint, contrast and saturation, and optionally match the target in software through its Colour Correction.

- Record original OMT VMX1 video to MKV in MultiCorder and Replay, with source audio and indexed replay timestamps.
- Add optional MKV stream copy after native NDI recording in MultiCorder.

- Add optional native NDI MOV recording in MultiCorder through the NDI Record utility.
- Add separate storage folders per Replay camera and retain each segment's original location for playback and export.

- Add selectable DeckLink embedded audio capture on macOS, 16-channel DeckLink playback for Master and A–G, and per-input reverb tail reduction.

- Add input noise suppression using RNNoise, up to 16 channels of DeckLink capture on Windows and optional v210 DeckLink output.

- Add independent Preview selection, Cut and Fade to each Mix input.

- Add reusable Mix inputs composed of existing layers, with optional Program and Preview tally passthrough.

- Add buses C–G for input routing and monitoring, and select single or combined buses for MultiCorder audio.

- Route original capture channels independently to stereo Master, A, B and headphones.

- Add per-input GO shortcut actions, SRT destinations in Streaming Settings, eight output routes and an optional post-Master headphone feed.

- Add input AGC, adjustable dynamics and routing for additional capture channels on audio devices and NDI/OMT.

- Add pre- and post-fader input meters, a per-input mixer visibility toggle and right-channel gain trim.
- Save named stereo matrix presets with each input.

- Smooth compressor gain changes and preserve its envelope when adjusting or bypassing it during playback.

- Expand input Audio Settings and add Channel Mixer, a stereo Channel Matrix, Bus Mixer and EQ, gate and compressor bypass controls.

- Add a cancellable sequential disk write test to Recording Setup.

- Move advanced External window controls to Outputs / NDI / OMT / SRT.

- Exclude global overlays when showing the Teleprompter in Preview, Program or a routed output.
- Show a status message when Pause Inputs is clicked, including how many video inputs were paused.

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
