# Roadmap

## Video and recording performance

- Expand the Windows D3D11 texture path to more sources and processing stages. The verified hardware path used H.264 playback through D3D11 and OpenGL to Program recording at 1080p29.97 on Intel Arc and NVIDIA RTX 4060, with zero Program readback during the tested path. Startup drops were observed. A mismatched adapter configuration exercised the fallback.
- Profile chroma key, color effects, capture and simultaneous outputs. CPU-backed capture and some processing or sink combinations still require system-memory transfers. Measure 1080p60 and 4K60 with a common scene and report render latency, encoder queue pressure, frame drops, copies and memory use.
- Validate additional GPU and driver combinations, including AMD and hybrid laptops. Profile the Apple Silicon backend and evaluate a native VideoToolbox and GPU surface pipeline. Current macOS media paths use FFmpeg.

## Replay and operator tools

- Keep encoders running across replay segments, add playback speed control and an A/B replay workflow, and persist event metadata across sessions. Validate eight live angles with synchronized audio. A two-angle synthetic recorder test exercised the shared event timebase.
- Extend Telestrator beyond freehand drawing and verify operation with multiple configured Telestrator inputs. Browser drawing currently targets the first one. The local controller listens on loopback. Remote LAN control would need its own access controls.
- Connect MIDI device selection and received messages to shortcut actions, then validate MIDI and VISCA with physical devices and add examples for scripts and triggers. Improve operator diagnostics for rejected commands and unavailable devices.

## Compatibility and distribution

- Extend GT feature coverage and vMix XML round trips with representative fixtures. Compare missing sources, asset paths and unsupported features in the import report before running an imported show.
- Verify physical macOS capture and DeckLink input and output, plus NDI, OMT and SRT between separate machines. Native CI exercises media files and builds the Mac package. It does not provide physical production hardware.
- Expand recording compatibility testing for multiple codecs, simultaneous recordings and MultiCorder. Windows WMV currently records video only. Verify channel routing and A/V synchronization with each transport and recording container.
- Add Developer ID signing and notarization for public macOS distribution.
