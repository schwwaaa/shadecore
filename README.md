<p align="center">
  <img width="45%" height="45%" src="https://github.com/schwwaaa/shadecore/blob/main/media/shadecore-logo.png?raw=true"/>  
</p>

<p align="center"><em>A native, high‑performance GLSL rendering engine written in Rust, designed for real‑time shader experimentation, MIDI control, and live video routing.</em></p> 

---

## Description

`shadecore` is a **standalone OpenGL shader engine** that renders a fullscreen GLSL fragment shader and can route the output as:

- local window preview (FBO texture)
- **Syphon** (macOS)
- **Spout2** (Windows)
- **Stream** via FFmpeg (RTSP for local network; RTMP for platforms)

It is designed to be:
- fast enough for feedback systems,
- deterministic enough for installations,
- flexible enough to act as a base for many future tools.

There is **no GUI framework**, **no WebView**, and **no runtime abstraction layer** between your shader and the GPU.

---

## Purpose

This project exists to solve a common problem in creative coding:

> *“I want to build my own visual tools without shipping an entire framework.”*

`shadecore` is intended to be:
- a **foundation** for custom shader‑based applications,
- a **bridge** between GLSL and external control systems,
- a **standalone binary** rather than a patch inside another tool.

---

## Features

- Native OpenGL rendering (via `glow`)
- Fullscreen GLSL fragment shader pipeline
- MIDI parameter control (CoreMIDI)
- JSON‑defined parameter schema
- Syphon server output (macOS)
- Spout2 sender output (Windows)
- FFmpeg streaming output (RTSP/RTMP)
- Vendored framework dependencies (no system installs for Syphon/Spout)
- Deterministic build & runtime behavior

---

## Running the Project

### Requirements
- macOS or Windows (Linux builds for local preview are possible)
- Rust (stable)

Platform extras:
- macOS: Xcode Command Line Tools (Syphon.framework is vendored)
- Windows: Visual Studio Build Tools (Spout2 is vendored)
- Streaming: FFmpeg available in PATH, or set `stream.ffmpeg_path` in `assets/output*.json`

### Build & Run

```bash
cargo run
```

This will:
- compile the engine
- launch the renderer
- load defaults from `assets/params.json` and `assets/output.json`
- show a local preview window (always)

Switch outputs at runtime (defaults, configurable in `assets/output*.json`):
- `1` = Texture (preview only)
- `2` = Syphon (macOS)
- `3` = Spout2 (Windows)
- `4` = Stream (FFmpeg RTSP/RTMP)
---

## Project Structure

```
shadecore/
├─ src/
│  └─ main.rs              # Core engine loop
├─ native/
│  ├─ spout_bridge/         # C++ Spout2 bridge (Windows)
│  ├─ syphon_bridge.m      # Objective‑C Syphon bridge
│  └─ syphon_bridge.h
├─ vendor/
│  └─ Syphon.framework     # Vendored framework
├─ assets/
│  ├─ params.json          # Parameter & MIDI schema
│  ├─ output.json          # Output routing (mode, hotkeys, stream settings)
│  └─ shaders/             # default.frag / present.frag
├─ build.rs                # Framework linking + rpath logic
└─ Cargo.toml
```

---

## Dependencies

### Rust Crates
- `glow` – OpenGL bindings
- `winit` / `glutin` – window + context
- `midir` – MIDI input
- `serde` / `serde_json` – parameter parsing

### Native Frameworks
- OpenGL
- Cocoa / AppKit
- CoreMIDI
- **Syphon** (vendored)

---

## How It Works

### 1. OpenGL Rendering
- A window and OpenGL context are created using `winit` + `glutin`
- A fullscreen triangle is drawn every frame
- The fragment shader is responsible for all visual output

### 2. Shader Uniforms
The engine provides:
- `u_time` – seconds since start
- `u_resolution` – window size in pixels
- user‑defined parameters (from JSON + MIDI)

### 3. Render Target
- Rendering occurs into an offscreen framebuffer
- The framebuffer texture is:
  - drawn to the window
  - published to Syphon

### 4. Syphon Publishing
- The OpenGL texture ID is passed to Syphon
- Other apps can receive frames in real time

---

## Parameters & MIDI

Parameters are defined in a JSON file:

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

- MIDI CC values (0–127) are normalized
- Values are scaled into parameter ranges
- Parameters are updated every frame

This makes controller layouts reproducible and portable.

---

## Use Cases

- Live shader performance
- Visual instruments
- Generative art installations
- Feedback‑based video systems
- Custom tools for OBS / Resolume / TouchDesigner pipelines

---

## Roadmap

- Hot‑loading shaders from disk
- Multi‑pass rendering / feedback buffers
- `.app` bundling
- Windows backend (Spout)
- OSC / network control

---

## Philosophy

This tool is intentionally **minimal and explicit**.

Instead of abstracting creativity behind interfaces,  
`shadecore` gives you **direct control of the GPU** and lets you decide what the software should become.

---

## License

TBD
