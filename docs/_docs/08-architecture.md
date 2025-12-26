---
title: Architecture Notes
order: 55
---

# Architecture Notes

This page is a "why it is built this way" companion to the code comments.

## Core rule: one authoritative render texture

ShadeCore always renders into an offscreen framebuffer (FBO). That texture is the source for:

- Preview window (presentation only)
- Syphon / Spout (texture sharing)
- NDI / Stream (network / encoder outputs, when enabled)
- Recording (FFmpeg worker)

This keeps the pipeline predictable: **change the render target resolution → everything else follows**.

## Thread model

- **Render thread (main)** owns the OpenGL context and performs all GL calls.
- **MIDI thread** and **OSC thread** only update parameter targets (shared store).
- **Recording worker** reads frames from a bounded queue and feeds FFmpeg.

The design goal is to avoid stalling the render loop. When the system is overloaded, the
preferred failure mode is *dropping frames in recording* rather than freezing the preview.

## Configuration files

- `assets/params.json` — defines parameter names, ranges, smoothing, and MIDI mappings.
- `assets/output.json` — selects output mode + hotkeys + preview behavior.
- `assets/recording.json` — recording settings + hotkeys.

All of these are designed to be hot-reloaded so you can iterate without restarting.

- **Profiles mental model:** [Profiles Mental Model](/docs/10-profiles-mental-model.html)

- **State ownership + authority:** [State Ownership and Authority](/docs/11-state-ownership-and-authority.html)

- **Pipeline overview:** [Pipeline Overview](/docs/12-pipeline.html)

## Where to look in code

- `src/main.rs` — glue + event loop + orchestration
- `src/presenter.rs` — preview scaling
- `src/output/*` — output routing + backends
- `src/recording.rs` — FFmpeg worker and queue
- `src/hotreload.rs` — file watching / reload signals



## Frame lifecycle

Each preview-frame (winit `RedrawRequested`) follows the same high-level flow:

1. **Hot-reload checks**: watch events set a flag; the redraw tick does mtimes + reload work.
2. **Param update**: time/resolution + smoothed MIDI/OSC params become uniform inputs.
3. **Shader render**: draw into the authoritative offscreen RenderTarget (FBO texture).
4. **Output publish**: depending on `output_mode`, publish the RenderTarget texture to the selected backend.
5. **Preview present**: draw the same texture into the preview window using the current preview scaling mode.
6. **Optional recording**: if recording is active, do PBO readback + feed the ffmpeg writer without stalling rendering.
5. **Preview present**: draw the same texture into the local window with the chosen preview scaling.
6. **Recording readback** (optional): PBO ping-pong readback -> ffmpeg, preferring drop over stall.
