---
title: How It Works
order: 40
---

# How It Works

## Requirements

- macOS or Windows
- Rust (stable toolchain)

Platform-specific:

- **macOS**
  - Xcode Command Line Tools
  - Syphon.framework (vendored in project)  
- **Windows**
  - Visual Studio Build Tools (C++ workload)
  - Spout2 (vendored in project)  
- **Streaming**
  - FFmpeg available in `PATH`
  - or set `stream.ffmpeg_path` in `assets/output*.json`

### Rust Crates
- `glow` — OpenGL bindings
- `winit` / `glutin` — windowing + GL context
- `midir` — MIDI input
- `serde` / `serde_json` — configuration parsing

### Native APIs
- OpenGL
- Cocoa / AppKit (macOS)
- CoreMIDI
- Syphon (vendored)
- Spout2 (vendored)

## Project Structure
---
```text
shadecore/
├─ src/
│  ├─ main.rs                     # Core engine loop (GL context + render loop + input threads)
│  ├─ presenter.rs                # Preview window presenter (fit/fill scaling, headless mode)
│  ├─ hotreload.rs                # Directory watcher → reload signals
│  ├─ recording.rs                # FFmpeg recording worker (bounded queue)
│  ├─ output/
│  │  ├─ mod.rs                   # Output abstraction + routing glue
│  │  └─ spout.rs                 # Windows Spout2 backend (via native bridge)
│  └─ osc_introspection_helpers.rs# Optional OSC discovery endpoints
├─ native/
│  ├─ spout_bridge/               # C++ Spout2 bridge (Windows)
│  ├─ syphon_bridge.m             # Objective-C Syphon bridge (macOS)
│  └─ syphon_bridge.h
├─ vendor/
│  └─ Syphon.framework            # Vendored macOS framework (if present)
├─ assets/
│  ├─ params.json                 # Parameters + MIDI schema
│  ├─ output.json                 # Output routing & hotkeys
│  ├─ output_ndi.json             # NDI-specific configuration (optional)
│  ├─ recording.json              # Recording settings (optional)
│  └─ shaders/
│     └─ *.frag                   # Fragment shaders (live reloaded)
│     ├─ default.frag
│     └─ present.frag
├─ build.rs                # Native linking & platform logic
└─ Cargo.toml
```
## Reading order

If you're new to the codebase, this order tends to be the least confusing:

1. `src/main.rs` (top docs + config load + render loop)
2. `src/presenter.rs` (how the preview window scales the render texture)
3. `src/output/mod.rs` then the platform backend (`spout.rs` / Syphon bridge)
4. `src/recording.rs` (how recording avoids blocking rendering)
5. `src/hotreload.rs` (how file changes trigger reload)




## Build & Run (Standard Engine)

```bash
cargo run
```

## Build & Run (Standard Engine + NDI)

```bash
cargo run --features ndi
```

## Pipeline overview
### 1. OpenGL Rendering

- A window and GL context are created with `winit` + `glutin`
- A fullscreen triangle is rendered every frame
- All visuals are produced in the fragment shader

### 2. Shader Uniforms

Built-in uniforms:

- `u_time` — seconds since start
- `u_resolution` — framebuffer resolution

Plus:

- user-defined parameters from JSON
- live-updated MIDI values

### 3. Render Target

- Rendering occurs into an offscreen framebuffer
- The framebuffer texture is:
  - drawn to the preview window
  - shared with Syphon, Spout, Stream, or NDI depending on mode
---

## Pipeline

For the end-to-end flow (inputs → render tick → publish → record), see: **[Pipeline Overview](/docs/12-pipeline.html)**
