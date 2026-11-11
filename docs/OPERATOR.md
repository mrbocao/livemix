# Operator guide

1. Open **Add Input** and add a camera, clip, graphic or network source. Click a card to prepare it in **Preview**. Use **Cut**, a transition or the T-bar to send it to **Program**.
2. Configure layers, positioning, picture and sound in **Input Settings**. Assign an input to one of the eight overlays when it should appear above the video.
3. Choose the source for each output in **Settings → Outputs / NDI / OMT / SRT**. Set the corresponding destination or hardware connector before enabling it.
4. Save the show as a `.vmix` preset. Reopening it shows an import report with unsupported inputs and missing asset references. Keep the media files accessible from the machine that opens the show.

## Audio, recording and streaming

Add a microphone or interface with **Add Input → Audio Input**. Use **Audio Mixer → Monitor** to hear Master, A or B on the system output. **Input Settings → Audio** provides channel selection, gate, compression, EQ, gain and delay. Program and MultiView routes use the Master mix. Preview and per-input routes take the corresponding input audio.

Use the Record gear to configure the two recording slots and the MultiCorder gear to configure per-source capture. MP4 carries stereo AAC. Windows WMV recording carries video. Open the Stream gear for RTMP/RTMPS destinations. Destinations with matching source, quality and hardware preferences can share an encoder. Changing group membership restarts that encoder.

## Replay and Telestrator

Open **Replay**, select up to eight angles, and arm capture. Use **Mark In** and **Mark Out** to mark a shared interval across the selected angles. Choose an event and an angle, then **Save angle and add to Preview**. The selected sources are saved in the preset. Events stay available in the current session, and exported angles are saved as MP4 in the recording folder.

Add a **Telestrator** input and assign it to an overlay. Draw freehand in its input settings or start **Settings → Web Controller** and open `http://127.0.0.1:8088/telestrator/` on the LiveMix machine. The browser sends its strokes to the first Telestrator input. Use the editor's undo or clear controls during the show.

## Control and automation

- **Settings → Shortcuts** configures keyboard bindings.
- **Settings → Web Controller** starts the loopback-only browser controls at `http://127.0.0.1:8088/`.
- **Settings → Scripting** runs sequences of LiveMix commands. Enter one command per line, such as `Preview 2`, `Wait 1000`, `Fade 500` and `Cut`. Wait and Fade durations are in milliseconds.
- **Input Settings → Triggers** runs commands when that input goes to Program. **Input Settings → PTZ** accepts the camera's IP and exposes pan, tilt, zoom, home and presets for supported VISCA devices.

See [setup](SETUP.md) for the runtimes and permissions used by each route.
