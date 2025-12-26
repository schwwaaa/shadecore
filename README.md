<p align="center">
  <img width="45%" height="45%" src="https://github.com/schwwaaa/shadecore/blob/main/media/shadecore-logo.png?raw=true"/>  
</p>

<p align="center"><em>A native, high-performance GLSL live-coding engine written in Rust, designed for real-time shader manipulation, hardware control, and live video routing.</em></p>

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

## Code Map

If you're trying to understand the engine quickly:

- `src/main.rs` — GL context setup, render loop, input threads (MIDI/OSC), hot-reload wiring
- `src/presenter.rs` — preview window presentation (scaling/letterboxing)
- `src/output/*` — Syphon/Spout/NDI/Stream routing glue + backends
- `src/recording.rs` — FFmpeg recording worker (non-blocking design)
- `assets/*.json` — runtime configuration (params/output/recording)

The docs site mirrors this structure under `docs/_docs/`.

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

## Live Coding Model

While `shadecore` does not embed a traditional text editor, it is designed around a **live-coding workflow**.

Shaders are written externally, but once running:

- parameters are pre-declared and always “live”
- MIDI mappings act as latent control bindings
- routing, structure, and behavior can be reshaped in real time
- no recompilation or UI-layer indirection is required

This allows a performer to *play*, *reconfigure*, and *record* a shader as a live system.

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

## Parameters & MIDI

Parameters are defined declaratively in JSON and updated every frame.

By declaring parameters and MIDI bindings ahead of time, `shadecore`
supports a live-coding style where control surfaces can be connected,
repurposed, or reinterpreted in real time without stopping the renderer.
