---
title: Profiles Mental Model
order: 46
---

# Profiles Mental Model

“Profiles” show up in ShadeCore in **two different places**:

1. **Shader (parameter) profiles** — live performance defaults per shader
2. **Recording profiles** — capture presets (resolution + codec + container)

They solve different problems. If you keep them mentally separated, the system stays simple.

---

## 1) Shader profiles (in `assets/params.json`)

### What problem they solve
A shader might expose the same “universe” of uniforms (`u_gain`, `u_zoom`, `u_spin`, etc.), but you often want **different starting values** depending on the shader or the vibe.

Shader profiles are **named sets of uniform defaults** scoped to a specific shader file.

### Where they live
`assets/params.json` contains:

- `shader_profiles` — a dictionary keyed by **shader path**, each containing named profiles.
- `active_shader_profiles` — which profile name is currently selected for each shader.
- `profile_hotkeys` — keybinds to cycle profiles (usually next/prev).

### What they do at runtime
When a shader becomes active:

- ShadeCore finds the profile name from `active_shader_profiles[frag_path]`
- Loads the profile’s `uniforms` defaults
- Seeds the param targets/values so the shader starts in a predictable state

### What they **do not** do
Shader profiles **do not**:
- change MIDI CC assignments
- change OSC routes
- change ranges (`min/max`) or smoothing constants
- change output routing or recording settings

Think of them as: **“starting positions for the knobs”**, not “a whole new controller mapping”.

---

## 2) Recording profiles (in `assets/recording.profiles.json`)

### What problem they solve
Recording is inherently “preset-driven”: you want a repeatable capture format like:

- 720p quick test files
- 1080p ProRes “real” captures
- 4K H.264 exports

Recording profiles are named presets that bundle:
- width/height
- fps
- codec/container settings
- ffmpeg arguments (via structured fields)

### Where they live
Recording is split into two files:

- `assets/recording.json` — “controller”: enabled flag, hotkeys, active profile name
- `assets/recording.profiles.json` — “library”: the actual named profile definitions

### What they do at runtime
When recording starts:

- ShadeCore loads `active_profile` from `recording.json`
- Looks up the profile in `recording.profiles.json`
- Initializes the recorder (buffers/PBOs) and spawns ffmpeg using that profile

### Why recording configs reload differently
Recording is an active session with assumptions (buffer sizes, readback format, ffmpeg command line).
So changes to recording config typically:
- take effect **when you start the next recording**, or
- are applied **after stopping**, depending on the project’s current “deferred reload” rules.

That keeps the render loop stable: **drop frames > freeze the render loop**.

---

## 3) “Variants” are not profiles (in `assets/render.json`)

`assets/render.json` has shader switching concepts like:
- `frag_variants`
- `frag_hotkeys`
- `active_frag` / `present_frag`

These are about **which shader file is running**, not the parameter defaults *inside* a given shader.

If you need “same shader, different starting values” → use **shader profiles**.  
If you need “different shader” → use **render.json**.

---

## Quick cheat sheet

- “I want different defaults for this shader” → **Shader profile** (`params.json`)
- “I want different capture formats” → **Recording profile** (`recording.profiles.json`)
- “I want to switch shaders” → **render.json**
- “I want to switch Syphon/Spout/Texture” → **output.json**

---

## Recommended reading order
1. [Asset JSON Mental Model](/docs/09-asset-json-mental-model.html)
2. This page
3. [Architecture](/docs/08-architecture.html)

---

## Related: State authority

Selections like “active profile”, “active shader”, and “active output mode” have clear sources of truth.  
See: **[State Ownership and Authority](/docs/11-state-ownership-and-authority.html)**
