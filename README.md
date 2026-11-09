# LiveMix

LiveMix is an experimental live video switcher for Windows written in Rust. It uses egui for the interface and Windows Media Foundation for video capture, playback, and MP4 recording.

The project is looking for contributors, especially people who can reproduce performance problems with real capture devices or help improve the video pipeline.

## Build and run

Install Rust and a Windows toolchain with a working linker. From the repository root, run these commands in PowerShell:

```powershell
cd app
cargo run --locked --release
cargo test --locked --release
```

Some media tests exercise Windows Media Foundation, the desktop, and an available camera. Run them from a Windows desktop session.

## Where help is needed

- Profile CPU use, GPU use, and frame drops with multiple cameras and video files while recording.
- Render the program output into its own texture. Recording currently reads back the interface framebuffer and crops it to the program view.
- Reduce CPU copies between Media Foundation, the compositor, and the encoder. File playback can use Direct3D 11 with a software fallback.
- Reproduce bugs in transitions, capture device negotiation, and recording.

For a bug or performance report, include your Windows version, GPU and driver, input types, resolution and frame rate, and the steps to reproduce. If available, include a short capture or CPU and GPU measurements. Open an issue or send a pull request with a focused fix.
