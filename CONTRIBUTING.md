# Contributing to LiveMix

LiveMix needs operators who can test actual productions as much as it needs Rust contributors. Check the [roadmap](docs/ROADMAP.md) and [open issues](https://github.com/mrbocao/livemix/issues) for an existing report before starting work.

| If you have… | A useful first contribution |
| --- | --- |
| A Windows laptop with two GPUs, or an AMD machine | Run the same 1080p59.94 project on each adapter. Report the OpenGL and D3D11 adapters, decoder and encoder paths, Program readbacks and dropped frames. |
| A Mac, capture device, or second machine | Test Blackmagic, NDI, OMT or SRT with embedded stereo audio. Include OS, driver/runtime, device, resolution, frame rate and whether reconnect works. |
| A two-camera Replay setup | Film a flash visible to both cameras. Mark the same event across intentional frame losses and report whether A and B start on the expected picture. |
| A recording or streaming setup | Run a 30-minute rehearsal and capture a visible flash and audible click near the beginning and end. Report drift and where the first audio glitch occurs. |
| Experience with titles or presentations | Make a synthetic GT fixture or slide deck that isolates one issue. Include the expected frame or field behavior. |
| A language you know well | Extend a catalog using the [translation guide](app/locales/README.md). Include screenshots of long labels in the Settings and Add Input dialogs. |

## Report a problem

Include the LiveMix version or commit, OS, GPU and driver, input type, project format, enabled outputs and steps to reproduce. Copy relevant diagnostics from **Settings → Performance**, **Input Settings → General → Decoder** or the streaming log. Remove credentials, private media paths and personal data before attaching files.

## Work on the code

Read [AGENTS.md](AGENTS.md) for the source map and [Setup](docs/SETUP.md) for dependencies. Use a small synthetic fixture and exercise the normal UI and lifecycle when changing a feature. From the repository root:

```sh
git diff --check
cargo build --locked --release --manifest-path app/Cargo.toml
cargo test --locked --release --manifest-path app/Cargo.toml -- --skip desktop_capture_delivers_frames --skip first_camera_delivers_frames
```

On Windows, run the [local MediaMTX check](docs/SETUP.md#streaming-integration-test) for changes to streaming, output queues or A/V timestamps. Physical capture tests are run separately with the relevant devices attached.

Describe the behavior change, test command, hardware exercised and any remaining platform gaps in the PR. Keep proprietary media, installation files, decompilation artifacts, logs and machine-specific settings out of commits. Credit any code adapted from another project and include the applicable notice.
