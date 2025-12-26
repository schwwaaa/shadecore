---
title: State Ownership and Authority
order: 47
---

# State Ownership and Authority

ShadeCore has a handful of “current states” that can change at runtime:

- which shader is active
- which parameter profile is active for that shader
- which output mode is publishing
- whether recording is running + which recording profile is selected
- the current smoothed uniform values (live performance state)

This page explains **where each piece of state lives**, what is the **source of truth**, and what gets **persisted to disk**.

---

## Three kinds of state

### 1) Persistent configuration (on disk)
These are JSON files under `assets/`. They can be edited and (usually) hot-reloaded:

- `render.json`
- `params.json`
- `output.json`
- `recording.json` + `recording.profiles.json`

### 2) Runtime “selection state”
These are current choices made by hotkeys/OSC/MIDI during a session:

- active shader
- active shader profile name
- active output mode
- recording on/off
- active recording profile name

Some of these choices are **mirrored back** into in-memory copies of the loaded JSON so the rest of the system sees a consistent picture.

### 3) Runtime “performance state”
These are live values that should *not* be treated as config:

- smoothed uniform values (`values`)
- uniform targets (`targets`) driven by MIDI/OSC
- time, mouse, resolution uniforms

This is the stuff you *perform* with — it changes every frame and generally should not be written back into JSON.

---

## Authority table (what is the source of truth?)

### Active shader (which `.frag` runs)
**Authority:** `assets/render.json` at load time, then runtime hotkeys can change it.

- `render.json` is the canonical place to define:
  - shader variants (`frag_variants`)
  - which shader is the starting point (`active_frag`)
  - hotkeys for selecting shaders (`frag_hotkeys`)
- At runtime, the current “selected shader” is stored in memory (and can be written back if you later add persistence for that).

**Key idea:** render.json decides *which shader file*, not parameter defaults inside the shader.

---

### Active shader profile (parameter defaults for the active shader)
**Authority:** `assets/params.json` → `active_shader_profiles[frag_path]`

- `shader_profiles[frag_path]` defines named profiles (each is a set of default uniform values).
- `active_shader_profiles[frag_path]` selects which name is currently active for that shader.
- `profile_hotkeys` cycles profiles.

When the shader is loaded/reloaded, the selected profile is used to **seed** uniform defaults.

**Key idea:** shader profiles are *defaults*; live MIDI/OSC still drives targets after that.

---

### Output mode (texture / syphon / spout / stream…)
**Authority:** `assets/output.json` at load time, then runtime hotkeys can change it.

- `output.json` defines hotkeys and backend settings.
- At runtime, hotkeys switch the currently active publishing mode.
- The render loop treats the offscreen FBO texture as the authoritative render target, then publishes it to the selected backend.

**Key idea:** output mode changes *where the texture goes*, not how the shader renders.

---

### Recording state (on/off) and active recording profile
**Authority:** `assets/recording.json` for selection + hotkeys, and `assets/recording.profiles.json` for definitions.

- `recording.json.active_profile` chooses the name.
- `recording.profiles.json.profiles[name]` defines the preset.

When recording starts, the current profile is “snapshotted” into a session:
- buffer sizes / PBO sizes
- ffmpeg process arguments

Because recording is session-based, some recording-related reloads are intentionally **deferred** until recording stops.

**Key idea:** recording profiles are “capture presets”, not performance parameters.

---

## What gets persisted back to disk?

By default, ShadeCore treats JSON as **configuration input**, not an always-on state database.

Typical philosophy:

- **Do persist:**
  - project-level defaults you want on the next run (e.g., active profile per shader)
- **Do not persist:**
  - frame-by-frame uniform values
  - transient toggles that are only useful for the current jam session

If you later add “write-back”, the safest approach is:
- write selection state only (active shader, active shader profile per shader, output mode, active recording profile)
- keep performance state in memory only

---

## Practical mental model
A good way to think about it:

- `render.json` = *what shader file am I running?*
- `params.json` = *what knobs exist, and what are the default knob positions per shader?*
- `output.json` = *where does the rendered texture go?*
- `recording.json` + `recording.profiles.json` = *how do I capture frames to disk?*
- runtime ParamStore = *the live jam state of the knobs*

