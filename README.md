# LiveMix — open-source live video switcher and streaming studio in Rust

[![Rust CI](https://github.com/mrbocao/livemix/actions/workflows/rust.yml/badge.svg)](https://github.com/mrbocao/livemix/actions/workflows/rust.yml)
[![Latest release](https://img.shields.io/github/v/release/mrbocao/livemix?include_prereleases&label=release)](https://github.com/mrbocao/livemix/releases)
[![Platforms](https://img.shields.io/badge/platforms-Windows%20x64%20%7C%20Apple%20Silicon-blue)](docs/SETUP.md)
[![Status](https://img.shields.io/badge/status-alpha-orange)](docs/ROADMAP.md)
[![Project page](https://img.shields.io/badge/project%20page-mrbocao.github.io%2Flivemix-informational)](https://mrbocao.github.io/livemix/)

**Switch cameras, clips, graphics and network video, mix stereo audio, record, stream and run instant replay and a teleprompter from one desktop app.** Project page: [mrbocao.github.io/livemix](https://mrbocao.github.io/livemix/).

LiveMix is a Rust live video production switcher for Windows and Apple Silicon Macs. It combines camera feeds, clips, desktop capture, network video, graphics and stereo audio in a Preview and Program workflow. Operators can prepare a shot in Preview, switch it to Program, and send independently routed feeds to local displays, recorders and network receivers. The egui interface offers Classic and Modern themes.

![LiveMix Preview and Program monitors, transition controls and demo inputs](docs/screenshots/switcher.png)

## Start here

Test builds for Windows x64 and Apple Silicon are published under [GitHub Releases](https://github.com/mrbocao/livemix/releases). See the setup guide below for FFmpeg, device runtimes and platform requirements.

| Guide | What it covers |
| --- | --- |
| [Project page](https://mrbocao.github.io/livemix/) | Overview, screenshots and download links |
| [Documentation index](docs/README.md) | Every guide, grouped by what you want to do |
| [Setup and dependencies](docs/SETUP.md) | Windows and Mac builds, FFmpeg, NDI, OMT, DeckLink, permissions and GPU setup |
| [Operator guide](docs/OPERATOR.md) | Switching, audio, recording, streaming, replay, telestration and automation |
| [Changes](CHANGELOG.md) | Implemented changes and the conditions under which they work |
| [Roadmap](docs/ROADMAP.md) | Open work, hardware validation and performance priorities |
| [Agent guide](AGENTS.md) | Source map, test commands, safety of project state and contribution workflow |

## What it does

- **Switch and compose.** Use Cut, timed transitions, T-bar, GT stingers, eight overlay channels, layered inputs, position, crop, chroma key, luma key, 3D LUTs and colour adjustments. Monitor the picture with waveform, RGB parade, vectorscope and safe areas.
- **Mix stereo audio.** Capture and monitor 48 kHz audio, route it through Master, A and B, and apply per-input EQ, gate, compression, gain and delay.
- **Record and distribute.** Route four independent outputs to local windows, DeckLink, NDI, OMT or SRT as supported. Record two configured routes or use MultiCorder. Compatible RTMP destinations can share an encoder.
- **Operate a show.** Mark events from up to eight Replay angles and cue the A/B players independently. The recorder indexes captured frames per angle. Use the Telestrator, keyboard and MIDI controls, local web switcher, scripts and VISCA over UDP.
- **Capture hardware.** Add webcams, USB cameras, HDMI/SDI capture cards, Blackmagic DeckLink devices, NDI and OMT sources. For cameras, pick the resolution, frame rate (interlaced modes included) and video format the device reports, and attach the card's audio input.
- **Read a script.** The Teleprompter input scrolls a script at a chosen speed, with a reading line, mirror and flip for a prompter glass, text file import and keyboard, MIDI and automation controls. Send it to the presenter's display without putting it on Program.
- **Build studio scenes.** Capture a web page as an input or add a static PowerPoint or LibreOffice deck with manual slide navigation. Build a Virtual Set with backdrop, presenter and foreground layers, then switch between animated camera views.
- **Move projects between sessions.** Save and reopen LiveMix project presets. The import report lists missing media and unsupported input types. Compatibility with third-party presets and GT templates is partial.

## Screenshots

| Scopes | Replay |
| --- | --- |
| ![Waveform and vectorscope in Preview](docs/screenshots/video-scopes.png) | ![Advanced Replay camera monitors and event controls](docs/screenshots/replay.png) |

![Virtual Set in Preview and static presentation in Program](docs/screenshots/studio.png)

## Quick start

1. Download a test build from [Releases](https://github.com/mrbocao/livemix/releases), or build from source with `cargo build --locked --release` in `app/` (see [Setup](docs/SETUP.md)).
2. Install FFmpeg (with FFprobe) for RTMP and SRT streaming, audio from video files, Replay playback and export, and on a Mac for media and camera capture. The [setup guide](docs/SETUP.md#install-only-what-you-use) lists what needs it. Then start LiveMix.
3. Open **Add Input**, add a camera, clip or test source, click its card to put it in **Preview**, and press **Cut** or a transition to send it to **Program**.
4. Open the **Record** or **Stream** gear to send Program to a file, RTMP or SRT. The [operator guide](docs/OPERATOR.md) covers each step.

## Frequently asked questions

**Which systems does it run on?** Windows x64 and Apple Silicon Macs. The two platforms have separate media backends; see [Setup](docs/SETUP.md) for what each one supports.

**Does it need special hardware?** No. A webcam or the built-in test sources are enough to try it. DeckLink, NDI and OMT need their own drivers or runtimes, listed in [Setup](docs/SETUP.md).

**Can I open presets from other switchers?** Partly. The import report lists missing media and unsupported input types. See the [operator guide](docs/OPERATOR.md).

**Is it ready for a live show?** It is alpha software. Rehearse first, and see the [roadmap](docs/ROADMAP.md) for combinations that still need hardware testing.

**How do I help?** Test it on your hardware and report what you find. See [Contributing](CONTRIBUTING.md).

## Contribute

Operators can help validate DeckLink and network video on real hardware, check Replay sync across cameras or record a 30-minute A/V test. Developers can take on browser inputs, GPU readback reduction, or a reproducible GT fixture. See [Contributing](CONTRIBUTING.md) and the [roadmap](docs/ROADMAP.md) for starting points.

## Translations

Choose the interface language in **Settings → Options**. See the [translation guide](app/locales/README.md) to contribute another language.

## Project status

LiveMix is in alpha. Windows can keep compatible decoded clips and eligible hardware recording surfaces on the GPU when OpenGL and D3D11 use the same adapter. Other configurations may use system memory. Apple Silicon has a separate build and media backend. Consult the [roadmap](docs/ROADMAP.md) for the hardware and workflow combinations that still need testing.

The [screenshots](docs/screenshots) show the Windows interface with demo sources. Report bugs with the operating system, GPU and driver, input type, project format, output route and steps to reproduce. [Open an issue](https://github.com/mrbocao/livemix/issues)

Third-party DSP and VISCA code is covered by the [MIT notice](docs/THIRD-PARTY-MIT.txt).
