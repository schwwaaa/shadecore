# ShadeCore Engine Architecture

This document describes the **system-level architecture** of ShadeCore:
how the project is structured, how responsibilities are divided, and why the
engine is designed this way.

If you are looking for **render pipeline, threading, and frame lifecycle details**,
see:

- `docs/08-architecture.md` — *Architecture Notes (Runtime & Pipeline)*

This document intentionally focuses on **structure and intent**, not low-level rendering mechanics.

---

## High-Level Overview

ShadeCore is structured around a simple principle:

> **One engine, many clients.**

The project is split into three conceptual layers:

1. **Engine** — reusable core logic
2. **Clients** — ways users interact with the engine
3. **Assets** — shaders and configuration data

This separation allows ShadeCore to remain stable while supporting multiple tools,
UIs, and future expansion.

---

## Repository Layout

```text
shadecore/
├─ Cargo.toml              # Workspace manifest
├─ Cargo.lock
│
├─ assets/                 # Shaders + JSON configuration (data, not code)
│
├─ native/ vendor/ ...     # Platform-specific native bridges (Syphon, Spout)
│
└─ crates/
   ├─ shadecore-engine/     # Core engine library (reusable, publishable)
   ├─ shadecore-cli/        # CLI runner (current stable entry point)
   └─ shadecore-scratchpad/ # Editor + preview tool (future, currently stub)
```

---

## The Engine (`shadecore-engine`)

The **engine crate** is the authoritative core of ShadeCore.

It is intentionally designed to be:
- reusable by multiple binaries,
- independent of UI or CLI assumptions,
- safe to publish as a standalone Rust crate,
- stable even as new tools are added.

### Responsibilities of the Engine

#### Asset Discovery
The engine owns all logic related to locating assets:

- Finds the `assets/` directory safely
- Supports `SHADECORE_ASSETS` overrides
- Works correctly inside a Cargo workspace or packaged builds

This prevents every client from re-implementing path logic.

---

#### Configuration Resolution & Loading

The engine resolves and loads all standard configuration files:

- `render.json`
- `params.json`
- `output.json`
- `recording.json`

Platform-specific overrides are supported automatically:

- `params.macos.json`
- `params.windows.json`
- etc.

All file loading and validation happens in **one place**.

---

#### EngineConfig (Session Aggregate)

The engine exposes a single aggregate configuration object:

```rust
EngineConfig
```

This represents a fully resolved **runtime session**, including:

- validated asset paths,
- selected shader(s),
- loaded configuration files,
- raw JSON sources for debugging and hot-reload.

Future tools only need **one call** to obtain a complete session configuration.

---

#### Strict vs Lenient Configuration Modes

The engine supports two configuration modes:

- **Lenient** (default)
  - tolerant parsing
  - suitable for live use and end users

- **Strict**
  - rejects unknown fields
  - enforces version and schema rules
  - useful for CI, debugging, and validation

This allows ShadeCore to evolve without breaking existing setups.

---

#### Generic JSON Parsing

The engine provides generic helpers to:

- load JSON files from disk,
- preserve raw text for debugging,
- deserialize into typed structures when needed.

This avoids duplicated IO and parsing logic across clients.

---

#### Error Handling

The engine exposes structured errors:

- no panics for normal user mistakes,
- clear diagnostics including file paths,
- safe failure behavior.

This is critical for long-term reliability.

---

#### Event Contract (Foundation)

The engine defines event types for:

- structured log messages,
- configuration load success/failure,
- shader compile success/failure.

These are **contracts**, not UI code.
They allow future clients (editors, GUIs) to react without tight coupling.

---

## Clients

### CLI Runner (`shadecore`)

The CLI is a **client of the engine**, not the owner of core logic.

It:
- loads `EngineConfig` from the engine,
- runs the current stable renderer and runtime loop,
- handles hotkeys, I/O, and power-user workflows.

Design rule:

> The CLI must never define configuration schema or asset discovery logic.

That responsibility lives in the engine.

---

### Scratchpad (`shadecore-scratchpad`)

The scratchpad is a future **editor + preview tool**.

Its purpose:
- provide an all-in-one shader editing experience,
- enable live coding workflows,
- lower the barrier to experimentation.

Architectural rule:

> The scratchpad uses the engine as a client.

This ensures:
- no duplicated configuration logic,
- no renderer forks,
- no fragile UI-engine coupling.

The scratchpad can evolve independently without destabilizing the core.

---

## Assets (`assets/`)

Assets are treated strictly as **data**, not code.

They include:
- GLSL shaders
- JSON configuration files

The engine:
- discovers them,
- validates paths,
- loads them safely.

This design supports:
- development builds,
- packaged binaries,
- embedded or library use cases.

---

## Why This Architecture Exists

This structure avoids common long-term problems:

### Separation of Concerns
UI, CLI, and engine logic evolve independently.

### Reusability
The engine can be embedded in other Rust projects or tools.

### Stability
Renderer changes are delayed until contracts are stable.

### Growth
Future features such as:
- shader graphs,
- multipass rendering,
- live editors,
- GUI applications

can be added without rewriting the core.

---

## Current Boundary: No Renderer Moves

At this stage:

- renderer code remains where it currently works,
- all recent changes focus on structure, contracts, and safety.

This boundary is intentional and temporary.

---

## Summary

ShadeCore follows a deliberate architectural rule:

> **The engine defines behavior; clients define interaction.**

- `shadecore-engine` defines *what ShadeCore is*.
- `shadecore-cli` defines *how it runs today*.
- `shadecore-scratchpad` defines *how users will interact with it next*.

This separation is what makes ShadeCore sustainable long-term.
