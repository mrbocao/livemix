# Changelog

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
- Added direct playback of finalized replay segments, frame stepping, seek-based reverse, and read-only import of native vMix replay metadata and supported media.
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

- Expanded the project overview and documented setup, optional runtimes, operator workflows, contributor and agent guidance, and remaining work. Added the MIT notice for adapted LiveCraft components.
