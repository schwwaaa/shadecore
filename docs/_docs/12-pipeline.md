---
title: Pipeline Overview
order: 48
---

# Pipeline Overview

This page describes ShadeCore’s runtime pipeline end-to-end — from “inputs” (JSON + MIDI/OSC) to “outputs” (preview + Syphon/Spout + recording).

The most important concept is:

> ShadeCore always renders into an **authoritative offscreen texture** (FBO).  
> Everything else is either **publishing** that texture somewhere, or **recording** it.

---


## Diagram

![ShadeCore pipeline diagram](/docs/assets/pipeline-diagram.png)

## 1) Inputs

### Files (configuration inputs)
All runtime behavior is driven by a small set of JSON files in `assets/`:

- `render.json`  
  Chooses **which shader file** is active and provides hotkeys to switch shaders.

- `params.json`  
  Defines the **parameter universe** (uniform names, ranges, smoothing) and the mapping from MIDI/OSC into those parameters.  
  Also defines per-shader **parameter profiles** (default uniform sets) and hotkeys to cycle them.

- `output.json`  
  Defines **output routing** (texture-only preview vs Syphon vs Spout, etc.) and the hotkeys to switch output modes.

- `recording.json` + `recording.profiles.json`  
  Defines recording hotkeys, the active recording preset name, and the preset definitions (resolution/fps/codec/container).

See also:
- [Asset JSON Mental Model](/docs/09-asset-json-mental-model.html)
- [Profiles Mental Model](/docs/10-profiles-mental-model.html)
- [State Ownership and Authority](/docs/11-state-ownership-and-authority.html)

### Live control (performance inputs)
- **MIDI** updates parameter *targets* (e.g., knob to `u_zoom`) via mappings in `params.json`.
- **OSC** can do the same (and may also expose introspection endpoints).

These inputs do **not** render directly — they only update the parameter state used by the render tick.

---

## 2) The authoritative render target (FBO)

ShadeCore renders each frame into an offscreen framebuffer object (FBO).  
The color attachment texture of that FBO is the single “source of truth” output of the renderer.

Why this design is useful:
- Preview window can resize independently.
- Syphon/Spout publishing can be toggled without changing rendering.
- Recording can read back from the same stable texture.
- Future outputs (NDI, RTSP, WebRTC, wgpu, etc.) can treat the FBO texture as a common input.

---

## 3) Frame lifecycle (the “render tick”)

On each redraw:

1. **Apply hot-reload signals (if any)**  
   File watchers mark “dirty” flags; the heavy work (reload/compile) happens on the render tick.

2. **Resolve shader + profile selection**  
   - active shader comes from `render.json` + runtime selection state
   - active parameter profile comes from `params.json` (`active_shader_profiles[frag_path]`)

3. **Update uniforms for this frame**
   - time / mouse / resolution uniforms
   - smoothed parameter values:  
     each parameter moves from `target` → `value` using its configured smoothing

4. **Render into the FBO**  
   The fragment shader draws into the offscreen texture.

5. **Publish the FBO texture (output routing)**  
   Based on the active output mode (from `output.json` hotkeys):
   - texture-only (preview only)
   - Syphon (macOS)
   - Spout (Windows)
   - (optional) stream outputs, depending on enabled features

6. **Present preview window**  
   The preview window draws the same texture (usually scaled to window size).

7. **Optional: recording readback**
   If recording is active:
   - pixels are read back (typically via PBO ping-pong)
   - the recorder feeds ffmpeg according to the active recording profile

---

## 4) Output routing (publishing)

Output routing is intentionally separate from rendering.

- Rendering produces the FBO texture.
- Publishing consumes that texture and sends it somewhere.

Switching output modes should feel like:
- “same shader output, different destination”

See: [Output Routing](/docs/04-output-routing.html)

---

## 5) Recording pipeline (capture)

Recording is a session-based pipeline:

- You start recording (hotkey from `recording.json`)
- ShadeCore looks up the active preset in `recording.profiles.json`
- It allocates buffers/PBOs sized to the preset
- It spawns ffmpeg with the preset’s codec/container settings
- Each frame, it feeds frames from the FBO readback into ffmpeg

Key philosophy:
- the render loop must stay responsive  
  → drop frames if needed rather than stalling the entire app

---

## 6) Where to look in code

If you want the “spine” of the program:

- `src/main.rs`  
  - window/event loop  
  - render tick  
  - shader switching + hotreload triggers  
  - output mode switching  
  - recording hotkeys + deferred reload behavior

- `src/output/*`  
  Output backends (publishers)

- `src/recording.rs`  
  ffmpeg process + readback strategy

- `src/hotreload.rs`  
  File watching + dirty-flag signaling

---

## 7) Mental model summary

If you only remember one sentence:

**ShadeCore renders into one stable texture, then routes that texture to preview/publish/record based on hotkey-selected modes and JSON-defined presets.**
