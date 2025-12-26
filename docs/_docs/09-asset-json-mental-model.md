---
title: Asset JSON Mental Model
order: 45
---

# Asset JSON Mental Model

ShadeCore is intentionally driven by a *small set of JSON files* under `assets/`.  
Each file answers **one primary question**.

If you keep those questions separate, the project stays easy to extend without turning into a “mega config”.

---

## Quick map (which file controls what)

### `assets/render.json` — shader selection
**Question:** *Which fragment shader(s) are active?*

Typical contents:
- `frag`: the current fragment shader path (relative to `assets/`).
- `present_frag`: optional “present” shader used when drawing the render texture to the preview window.
- `frag_variants`: optional list of fragment shaders you can cycle through.
- `active_frag`: optional selection by exact string match against `frag_variants`.
- `frag_profile_map`: optional mapping of **frag path → params profile name** (from `params.json`).

**Does NOT control**
- uniform ranges / smoothing
- MIDI / OSC mappings
- output routing (Syphon/Spout/etc)
- recording settings

**Hot reload**
- Changing `render.json` or the shader source applies on the next redraw tick.

---

### `assets/params.json` — parameters + MIDI/OSC mapping
**Question:** *What parameters exist, and how do inputs drive them?*

This is the “contract” for:
- **uniform names** (e.g. `u_gain`, `u_zoom`, `u_spin`)
- ranges (`min`, `max`)
- smoothing (`smooth`)
- MIDI mappings (CC → param)
- OSC mappings (address → param), including normalized vs raw endpoints

**Does NOT control**
- which shader file is active (that’s `render.json`)
- which output backend is active (that’s `output.json`)
- recording settings/hotkeys (that’s `recording.json`)

**Hot reload**
- Usually safe to live-reload during playback.
- If a recording is running, some settings may be *deferred* until recording stops to avoid mid-session encoder surprises.

---

### `assets/output.json` — output routing + output hotkeys
**Question:** *Where does the rendered FBO texture get published?*

This file controls:
- `output_mode` (texture-only preview / Syphon / Spout / Stream / NDI)
- backend configuration (e.g. Syphon server name, stream URL + encoder settings)
- hotkeys for switching output modes

**Does NOT control**
- parameter mappings (`params.json`)
- shader selection (`render.json`)
- recording (`recording.json`)

**Hot reload**
- Output-mode switches apply immediately (they change publishing behavior).
- When leaving a mode (e.g. Stream), we teardown the backend resources.

---

### `assets/recording.json` — recording hotkeys + active profile
**Question:** *How do I start/stop recording, and what profile is active?*

ShadeCore supports two shapes:

1) **Legacy (single file):** `recording.json` is a full recording config.
2) **Controller + profiles:**
   - `recording.json` contains hotkeys + `active_profile`
   - `recording.profiles.json` contains the named profile objects

This keeps hotkeys stable while you swap “quality presets”.

**Hot reload**
- If recording is idle: config can reload freely.
- If recording is active: profile changes are typically deferred until stop.

---

### `assets/recording.profiles.json` — named recording presets
**Question:** *What recording presets exist?*

Contains a map of profile name → settings (container, codec, fps, size, ffmpeg path, etc).

You normally edit this when you want:
- “preview-quality” vs “final-quality”
- ProRes vs H.264
- fixed dimensions vs “match render target”

---

### `assets/output.<platform>.json` — optional platform defaults
You may keep optional per-platform defaults (macOS/Windows/Linux) and copy/symlink them to `output.json`
depending on your packaging workflow.

The engine’s runtime logic still reads `assets/output.json` unless you explicitly point it elsewhere.

---

### `assets/osc_mappings.example.json` — reference / template
This file is intended as a **starter template** for OSC mapping formats.
It’s useful when you want to copy/paste patterns into `params.json`.

---

## Priority + merge rules (high level)

- `render.json` selects a shader **and can optionally select a params profile** (via `frag_profile_map`).
- `params.json` defines the parameter universe. Profiles can override defaults, and input mappings drive targets.
- `output.json` selects where the texture goes. It does not affect the parameter store.
- `recording.json` controls recording hotkeys and selects a recording profile (optionally from `recording.profiles.json`).

A good rule: **If you find yourself adding unrelated fields to one file, it probably belongs in a different asset.**


---

## Related: Profiles

Profiles can mean different things (shader defaults vs recording presets).  
See: **[Profiles Mental Model](/docs/10-profiles-mental-model.html)**
