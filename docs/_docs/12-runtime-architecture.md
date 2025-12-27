# ShadeCore Runtime Architecture

This document explains **how ShadeCore runs at runtime**:
how rendering works, how threads interact, and how frames flow through the system.

This is a **runtime / pipeline-focused document**.

If you are looking for **project structure, engine boundaries, and system design**, see:

- `docs/ENGINE-ARCHITECTURE.md` — *Engine & Workspace Architecture*

---

## Core Rule: One Authoritative Render Texture

ShadeCore always renders into **one offscreen framebuffer (FBO)**.

That single texture is the authoritative render output and is used for:

- Preview window (presentation only)
- Syphon / Spout (GPU texture sharing)
- NDI / streaming outputs (when enabled)
- Recording (FFmpeg worker)

**There is never more than one render target.**

This guarantees:
- consistent resolution across outputs,
- predictable performance,
- simple mental model.

Changing the render target resolution automatically affects **all outputs**.

---

## Thread Model

ShadeCore uses a deliberately simple threading model to avoid stalls.

### Render Thread (Main Thread)
- Owns the OpenGL context
- Performs **all** OpenGL calls
- Runs the render loop
- Presents the preview window

This thread must never block.

---

### MIDI Thread
- Listens for MIDI input
- Updates parameter targets only
- Does **not** perform rendering or GL calls

---

### OSC Thread
- Listens for OSC messages
- Updates parameter targets only
- Does **not** perform rendering or GL calls

---

### Recording Worker
- Reads completed frames from a bounded queue
- Performs pixel readback and encoding
- Feeds frames to FFmpeg

**Preferred failure mode:** drop frames rather than stall rendering.

---

## Configuration Files

All runtime behavior is driven by JSON configuration files under `assets/`.

### `assets/params.json`
Defines:
- uniform parameter names
- value ranges
- smoothing behavior
- MIDI mappings

Parameters are smoothed on the render thread to avoid jitter.

---

### `assets/output.json`
Defines:
- active output mode (preview / Syphon / Spout / NDI / recording)
- hotkeys for switching outputs
- preview scaling behavior

---

### `assets/recording.json`
Defines:
- recording resolution
- frame rate
- encoding settings
- hotkeys for start/stop

---

### Hot Reloading
All configuration files are designed to be **hot-reloadable**.

Edits take effect without restarting the application.

---

## Frame Lifecycle

Each preview frame (triggered by `winit::RedrawRequested`) follows this sequence:

1. **Hot-reload checks**
   - File watcher flags are evaluated
   - Modified configs or shaders are reloaded if needed

2. **Parameter update**
   - Time and resolution uniforms updated
   - MIDI/OSC inputs applied
   - Smoothed parameters finalized

3. **Shader render**
   - Active shader renders into the offscreen FBO
   - This FBO texture is now the authoritative frame

4. **Output publish**
   - Depending on `output_mode`, the FBO texture is:
     - shared via Syphon / Spout
     - sent to NDI / stream encoder
     - queued for recording

5. **Preview present**
   - The same texture is drawn into the preview window
   - Preview scaling mode is applied

6. **Optional recording**
   - If recording is active:
     - PBO ping-pong readback occurs
     - frames are sent to FFmpeg
     - dropped if the queue is full

Rendering always takes priority over recording.

---

## Performance Guarantees

The runtime is designed so that:

- Rendering never waits on disk I/O
- Rendering never waits on encoding
- Rendering never waits on network output

If the system is overloaded:
- recording drops frames first,
- preview remains responsive.

---

## Code Ownership Map

Key runtime files (current layout):

- `src/main.rs`
  - event loop
  - orchestration
  - thread coordination

- `src/presenter.rs`
  - preview scaling
  - window presentation

- `src/output/*`
  - output routing
  - Syphon / Spout / NDI backends

- `src/recording.rs`
  - FFmpeg worker
  - frame queues
  - encoder lifecycle

- `src/hotreload.rs`
  - file watching
  - reload signaling

---

## Design Intent

The runtime architecture prioritizes:

- predictable performance
- minimal blocking
- simple mental models
- safe failure modes

This allows ShadeCore to function reliably in live and experimental contexts.

---

## Summary

ShadeCore runtime behavior follows a strict rule:

> **Render once. Share everywhere. Never stall the render loop.**

This principle guides all threading, output, and recording decisions.
