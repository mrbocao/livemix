# Working on LiveMix

The repository root contains documentation and CI. The Rust crate is in `app/`. Read and write text files as UTF-8 without BOM. Check `git status --short` before editing and preserve existing work. See [setup](docs/SETUP.md) for platform prerequisites, the [operator guide](docs/OPERATOR.md) for entry points, and the [roadmap](docs/ROADMAP.md) for open work.

## Source map

| Area | Files under `app/src/` |
| --- | --- |
| Entry point, shared application types, update loop, feature lifecycle | `main.rs`, `app.rs`, `model.rs` |
| Switching desk, monitors, MultiView and output windows | `desk.rs` |
| Input tiles and input settings | `sources.rs` |
| Settings window, dialogs and shared widgets | `settings.rs`, `ui.rs` |
| Replay session, engine, setup and controller | `replay.rs`, `replay/engine.rs`, `replay/ui.rs`, `replay/controller.rs` |
| Recording, scopes and output scheduling | `outputs.rs` |
| Project presets and legacy import/export | `preset.rs` |
| Media backends and shared frame types | `media.rs` (Windows), `media_macos.rs`, `media_common.rs` |
| Program composition, scopes and GPU transfers | `program.rs`, `render.rs`, `scopes.rs`, `performance.rs`, `dx_interop.rs` |
| Mixing, capture, monitoring and per-input processing | `audio.rs`, `dsp.rs`, `mixer.rs` |
| Physical and network I/O | `decklink.rs`, `decklink_macos.rs`, `ndi.rs`, `omt.rs`, `srt.rs`, `stream.rs` |
| Presentation conversion and output integration tests | `slides.rs`, `livecheck.rs` |
| Titles and operator controls | `gt.rs`, `automation.rs`, `midi.rs`, `ptz.rs` |

## Implementation checks

- Follow the feature through its UI entry point, routing and teardown. Keep capture, encoder and network work off the UI thread, retain bounded queues, and handle shutdown and restarts. Preserve timestamp relationships between video, audio and replay angles.
- Preserve adapter identity, resource lifetimes and synchronization when changing GPU paths. Validate both the accelerated path and the fallback. Report hardware acceleration only after the encoder or decoder actually initializes.
- Preserve preset defaults and stable input references for older files. Use synthetic fixtures or fixtures explicitly supplied by the operator. Keep proprietary templates, original binaries, decompilation artifacts, logs, machine-specific settings and credentials out of commits.
- Update [the changelog](CHANGELOG.md) when behavior changes, and update the relevant operator or setup section when controls or dependencies change. Keep diffs scoped to the feature.

## Verification

From the repository root, run `git diff --check`. From `app/`, use the applicable commands.

```text
cargo build --locked --release
cargo test --locked --release -- --skip desktop_capture_delivers_frames --skip first_camera_delivers_frames
cargo check --locked --target aarch64-apple-darwin
```

Install the Apple target through Rustup before cross-checking. A cross-check verifies compilation and types. Native macOS CI also links, runs FFmpeg tests and packages the app. Run camera, desktop, network loopback and GPU integration tests separately on a machine with the required devices, libraries and permissions. Find ignored tests with `cargo test -- --list` and opt into the relevant tests by name. Report exact commands and which paths were exercised. Use [the MediaMTX script](docs/SETUP.md#streaming-integration-test) to verify video, stereo audio and timing through two shared RTMP destinations and one SRT output. For media performance changes, record the source, project format, GPU, selected path, frame drops and whether Program frames returned to system memory.
