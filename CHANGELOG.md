# Changelog

## Unreleased

### Production and operator controls

- Added multicamera replay with up to eight source angles, shared event marks and export of an angle to Preview.
- Added a Telestrator input with a freehand editor and local browser drawing. Added a Video Delay input with a configurable buffer.
- Added local web control, VISCA-over-IP PTZ, command scripts and Program-entry triggers. Added MIDI backend and binding models. Device selection and event dispatch are pending.
- Expanded per-input audio processing and color adjustment. Expanded preset import reporting for unsupported inputs and missing assets.

### Media paths

- Added Windows D3D11 decoder and recorder interop with OpenGL for supported adapter and driver combinations. Program recording can use GPU textures directly when the fast path is available.
- Added shared encoder groups for compatible RTMP destinations. Added replay and delay buffering, with explicit source selection and bounded queues on the live media paths.

### Documentation

- Expanded the project overview and documented setup, optional runtimes, operator workflows, contributor and agent guidance, and remaining work. Added the MIT notice for adapted LiveCraft components.
