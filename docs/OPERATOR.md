# Operator guide

1. Open **Add Input** and add a camera, clip, graphic or network source. Click a card to prepare it in **Preview**. Use **Cut**, a transition or the T-bar to send it to **Program**.
2. Configure layers, positioning, picture and sound in **Input Settings**. Assign an input to one of the eight overlays when it should appear above the video.
3. Choose the source for each output in **Settings → Outputs / NDI / OMT / SRT**. Set the corresponding destination or hardware connector before enabling it.
4. Save the show as a `.vmix` preset. Reopening it shows an import report with unsupported inputs and missing asset references. Keep the media files accessible from the machine that opens the show.

## Display and source selection

In **Settings → Display**, set the theme and the Preview and Output colors. The colors also apply to the matching input and MultiView title bars. Set the monitor, source and overlay option separately for **Fullscreen 1** and **Fullscreen 2**. Open them from the top toolbar. Active outputs use separate monitors. Click the Production Clock under the overlay buttons to pause or resume, and right-click to reset.

Use **Add Input → Audio** for an audio-only file. **Recent media** remembers the last successfully added video, image, audio files and photo folders. In **Settings → Performance**, turn NDI/OMT thumbnails on or off. The browser requests a single frame per source and keeps source selection available while previews load.

In **Streaming Settings**, choose one of five slots, then select its source, resolution, bitrate and codec. HEVC over RTMP/RTMPS requires a compatible FFmpeg build and receiver. [YouTube Live encoder settings](https://support.google.com/youtube/answer/2853702)

## Audio, recording and streaming

Add a microphone or interface with **Add Input → Audio Input**. In **Audio Mixer**, use **HP** and the adjacent selector to monitor Master, A or B through the system output. Drag the round knob vertically to change headphone volume. Double-click to reset it to 100%. Use **i** on the Master strip to inspect Audio Engine statistics. Click **OUTPUTS** or **INPUTS** to collapse a group, and use a channel gear to open its audio settings. Right-click the mixer title to make the input strips follow the selected input category. **Input Settings → Audio → Audio Settings** opens the same per-input dialog as the mixer gear. It provides channel selection, gain and delay under General, with separate pages for EQ, compression and noise gate. On each input strip, use **M**, **A** and **B** to assign buses and the speaker button to mute. **S** isolates the selected inputs in headphone monitoring. Right-click **S** for pre-fader Solo, which ignores the input fader and mute. The automatic mixing button follows Program with a crossfade on timed transitions and preserves mute and bus assignments. Each input card with audio has a stereo meter. In **Settings → Display → MultiView**, enable **Program audio meter** or **Input audio meters** for each layout. Program and MultiView routes use the Master mix. Preview and per-input routes take the corresponding input audio.

Use the Record gear to configure Recording 1 and Recording 2. Each slot has a video source, codec and **Audio track** toggle. Composed outputs record the stereo Program mix. MultiCorder uses the same codec choices and can record each selected source separately. Camera files use their source audio when available. Both recording slots share the destination folder and split interval.

| Container | Video profiles | Stereo audio |
| --- | --- | --- |
| MP4 and MKV | H.264, HEVC | AAC |
| AVI | MJPEG, MagicYUV | PCM |
| MPEG-2 TS and PS | MPEG-2 | MP2 |
| WMV | WMV8, WMV9 on Windows | WMA2 |
| MOV and MXF | ProRes 422 (MOV), DNxHR HQ | PCM |

Open the Stream gear for RTMP/RTMPS destinations. Destinations with matching source, quality and hardware preferences can share an encoder. Changing group membership restarts that encoder.

In **Settings → Decoders**, choose FFmpeg for specific file extensions or all video files on Windows. Windows otherwise uses Media Foundation. macOS uses FFmpeg for video files. **BWDIF** and **YADIF** deinterlace frames marked as interlaced on the FFmpeg path. Changes apply to video files and playlists.

## Replay and Telestrator

1. Use **Add Input → Instant Replay** to create **Replay A** and **Replay B**. In **Replay → Setup**, select a session folder, assign up to eight cameras and configure format, audio and rolling storage. Choose **New Session** for an empty folder or **Open Session** to recover a LiveMix session.
2. Press **Record** and use **Mark In / Mark Out** or **Last 5s**, **Last 10s** and **Last 20s**. Marking references the session timeline and leaves recording active. An event appears when its underlying segments have finalized. With retention enabled, segments outside the rolling window are removed if no saved or pending event references them.
3. Select an event, choose **A**, **B** or **AB**, and choose cameras and speeds. **Play** cues Preview and **Play to Program** takes the selected channel to Program. In AB mode both channels are prepared before playback and A takes Program. **Pause / Resume** controls the selected channel or pair. Speed changes apply the next time you press **Play**.
4. **Live buffer** plays the latest finalized camera segment directly and follows new segments as they close. **−1 frame** and **+1 frame** seek through the selected channel's indexed recording. **Reverse** seeks backward through that recording and runs without audio. These controls use finalized segments, so live playback trails the current write.
5. **Open Session** also accepts a vMix session folder with `replay.xml` or `replay2.xml`. LiveMix opens it read-only, lists its events, and prepares supported camera media with FFmpeg. If the recording uses separate MPEG `.vdata` and `.adata` tracks, use **Play** or **Export** to prepare them. Derived files for imported sessions are stored under **Imported Replay** in the LiveMix storage directory.
6. **Export** writes an MP4 into a LiveMix session's `export/` folder. The session saves event IDs, time ranges, available angles, descriptions and event speeds in `livemix-replay.json`. FFmpeg prepares clips for playback and export. Keep the manifest and original camera segments together.

Add a **Telestrator** input and assign it to an overlay. Draw freehand in its input settings or start **Settings → Web Controller** and open `http://127.0.0.1:8088/telestrator/` on the LiveMix machine. The browser sends its strokes to the first Telestrator input. Use the editor's undo or clear controls during the show.

## Control and automation

- **Settings → Shortcuts** configures keyboard bindings.
- **Settings → Web Controller** starts the loopback-only browser controls at `http://127.0.0.1:8088/`.
- **Settings → Scripting** runs sequences of LiveMix commands. Enter one command per line, such as `Preview 2`, `Wait 1000`, `Fade 500` and `Cut`. Wait and Fade durations are in milliseconds.
- **Input Settings → Triggers** runs commands when that input goes to Program. **Input Settings → PTZ** accepts the camera's IP and exposes pan, tilt, zoom, home and presets for supported VISCA devices.

See [setup](SETUP.md) for the runtimes and permissions used by each route.
