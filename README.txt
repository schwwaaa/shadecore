GLSL Engine – Segment 4 (Syphon output, macOS)
========================================

What this is
------------
- Renders your fragment shader into an offscreen GL texture (FBO)
- Publishes that GL texture to Syphon (zero-copy GPU sharing)
- Presents the same texture in the app window
- Keeps the Segment 3 MIDI+JSON param system

Project layout
--------------
- src/main.rs
- build.rs
- native/syphon_bridge.h
- native/syphon_bridge.m
- assets/shaders/default.frag
- assets/shaders/present.frag
- assets/params.json
- vendor/Syphon.framework   <-- YOU must provide this

How to get Syphon.framework
---------------------------
Syphon is the standard macOS GPU frame sharing system and is open source.
Repository: https://github.com/Syphon/Syphon-Framework

Option A (recommended): use their built framework/SDK release
- Download a Syphon SDK release from the Syphon-Framework GitHub releases page.
- Copy Syphon.framework into:
    glsl_engine_segment4_syphon/vendor/Syphon.framework

Option B: build from source
- Clone Syphon/Syphon-Framework
- Build Syphon.framework using Xcode
- Copy the built Syphon.framework into:
    vendor/Syphon.framework

Run
---
1) Put Syphon.framework in vendor/
2) From this folder:
   cargo run

Verify Syphon output
--------------------
- Open a Syphon-capable client (e.g. Resolume, MadMapper, VDMX, OBS with Syphon plugin)
- You should see a server named: "glsl_engine"

Notes
-----
- Syphon is macOS-only.
- For Windows you'll use Spout (later we’ll add a cross-platform abstraction: FrameShare { Syphon | Spout | None }).
