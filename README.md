<p align="center">
  <img width="45%" height="45%" src="https://github.com/schwwaaa/shadecore/blob/main/media/shadecore-logo.png?raw=true"/>  
</p>

<p align="center"><em>A native, high-performance GLSL rendering engine written in Rust, designed for real-time shader experimentation, hardware control, and live video routing.</em></p>

---

## Description

`shadecore` is a **standalone OpenGL shader engine** that renders a fullscreen GLSL fragment shader and routes the result to multiple real-time video outputs.

Supported outputs:

- Local window preview (always on)
- **Syphon** (macOS)
- **Spout2** (Windows)
- **FFmpeg streaming** (RTSP / RTMP)
- **NDI** (separate execution mode)

The engine is intentionally **low-level and explicit**:

- No GUI framework  
- No WebView  
- No runtime abstraction layer between your shader and the GPU  

What you write in GLSL is what runs.

---

## Purpose

This project exists to solve a common creative-coding problem:

> *“I want to build my own visual tools without shipping an entire framework.”*

`shadecore` is designed to be:

- a **foundation** for custom shader-based tools,
- a **bridge** between GLSL and external control systems,
- a **standalone binary**, not a plugin locked into another host.

It is equally suited for:
- live performance,
- installations,
- research tools,
- experimental pipelines.

---

## Features

- Native OpenGL rendering (via `glow`)
- Fullscreen GLSL fragment shader pipeline
- JSON-defined parameter schema
- MIDI control (CoreMIDI on macOS, cross-platform via `midir`)
- **Syphon server output** (macOS)
- **Spout2 sender output** (Windows)
- **FFmpeg streaming output** (RTSP / RTMP)
- **NDI output (separate run mode)**
- Vendored native dependencies (no system installs required)
- Deterministic build & runtime behavior

---

## Running the Project

### Requirements

- macOS or Windows  
- Rust (stable toolchain)

Platform-specific:

- **macOS**
  - Xcode Command Line Tools
  - Syphon.framework is vendored (no install required)
- **Windows**
  - Visual Studio Build Tools (C++ workload)
  - Spout2 is vendored
- **Streaming**
  - FFmpeg available in `PATH`
  - or set `stream.ffmpeg_path` in `assets/output*.json`

---

## Build & Run (Standard Engine)

```bash
cargo run
```

This will:

- compile the engine
- launch the OpenGL renderer
- load:
  - `assets/params.json`
  - `assets/output.json`
- open a local preview window (**always active**)

---

## Output Routing

Output behavior is controlled by `assets/output.json` (or alternate output configs).

### Runtime Hotkeys (default)

- `1` — Texture only (preview)
- `2` — Syphon (macOS)
- `3` — Spout2 (Windows)
- `4` — Stream (FFmpeg RTSP / RTMP)
- `6` — NDI (see below)

Hotkeys are configurable in the output JSON.

---

## ⚠️ NDI Output (Important)

NDI is **not enabled in the default execution path**.

This is intentional.

### Why NDI Is Separate

NDI requires:
- a different runtime lifecycle,
- different threading assumptions,
- tighter timing guarantees.

Rather than complicate the core render loop, NDI runs in a **dedicated execution mode**.

### Running with NDI

```bash
cargo run --features ndi
```

or

```bash
cargo run --bin shadecore-ndi
```

Check `Cargo.toml` for the active NDI configuration.

### NDI Notes

- NDI output is discoverable by OBS, Resolume, and other NDI-capable software
- Local preview still runs unless explicitly disabled
- NDI uses its own output configuration file

This separation is **by design**, not a limitation.

---

## Project Structure

```text
shadecore/
├─ src/
│  └─ main.rs              # Core engine loop
├─ native/
│  ├─ spout_bridge/        # C++ Spout2 bridge (Windows)
│  ├─ syphon_bridge.m      # Objective-C Syphon bridge (macOS)
│  └─ syphon_bridge.h
├─ vendor/
│  └─ Syphon.framework     # Vendored macOS framework
├─ assets/
│  ├─ params.json          # Parameters + MIDI schema
│  ├─ output.json          # Output routing & hotkeys
│  ├─ output_ndi.json      # NDI-specific configuration
│  └─ shaders/
│     ├─ default.frag
│     └─ present.frag
├─ build.rs                # Native linking & platform logic
└─ Cargo.toml
```

---

## Dependencies

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

---

## How It Works

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

## Parameters & MIDI

Parameters are defined declaratively in JSON.

```json
{
  "version": 1,
  "params": [
    {
      "name": "speed",
      "ty": "float",
      "min": 0.0,
      "max": 5.0,
      "default": 1.0,
      "midi_cc": 1
    }
  ]
}
```

Behavior:

- MIDI CC values (0–127) are normalized
- Values are mapped into parameter ranges
- Parameters update every frame
- No hidden smoothing or automation

Controller layouts are portable and reproducible.

---

## Use Cases

- Live shader performance
- Visual instruments
- Generative installations
- Feedback-based video systems
- Custom GPU tools for OBS, Resolume, TouchDesigner pipelines

---

## Roadmap

- Shader hot-reloading
- Multi-pass rendering / feedback buffers
- `.app` / `.exe` packaging
- OSC / network control
- Expanded NDI configuration options

---

## Philosophy

`shadecore` is intentionally **minimal, explicit, and opinionated**.

It does not try to be a framework.  
It does not hide the GPU.  

It exists to give you **direct ownership of the rendering pipeline** and let the software grow into whatever tool you need.

---

## License

TBD
