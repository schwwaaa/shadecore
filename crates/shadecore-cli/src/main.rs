//! # ShadeCore engine (single-binary architecture)
//! 
//! This crate is intentionally "flat" (a small number of modules) so it can be read top-to-bottom
//! without jumping through a large abstraction tree.
//!
//! ## Mental model
//! - **Render target**: ShadeCore renders every frame into an offscreen framebuffer (FBO). That texture is the
//!   *source of truth* for everything else (preview + Syphon/Spout/NDI/Stream + recording).
//! - **Preview**: The window is only a *presenter* of the render texture. It may scale to fit the window and does
//!   not define output resolution.
//! - **Outputs**: Routing is selected at runtime via `assets/output.json` hotkeys (or by editing the file).
//! - **Parameters**: Uniforms are driven by a single parameter store which supports smoothing and range mapping
//!   from MIDI and OSC.
//!
//!
//! ## Asset JSON mental model
//! ShadeCore's runtime behavior is intentionally driven by a *small set of JSON files* under `assets/`.
//! Think of them as separate knobs with minimal overlap:
//!
//! - `assets/render.json` — **which shader(s) are active** (paths + optional shader-variant list).
//!   *Does not* define uniforms, MIDI/OSC mappings, or output routing.
//! - `assets/params.json` — **what parameters exist** (uniform names + ranges + smoothing) and how
//!   MIDI/OSC routes into them. This is the “param store contract”.
//! - `assets/output.json` — **where the rendered texture is published** (Syphon/Spout/Stream/NDI/none)
//!   and the hotkeys that switch output modes at runtime.
//! - `assets/recording.json` (+ optional `assets/recording.profiles.json`) — **recording settings + recording hotkeys**
//!   (start/stop/toggle). Recording is treated as a separate subsystem from output routing.
//!
//! Hot-reload expectations:
//! - `render.json` and shader source changes typically apply on the *next redraw tick*.
//! - `params.json` can apply live (new targets/ranges/mappings) without restarting, but may not affect
//!   currently-running recordings until recording stops (by design).
//! - `output.json` hotkeys/mode changes apply immediately (they only change publishing behavior).
//!
//! ## Threads
//! - **Render thread** (main): owns the GL context; compiles shaders; draws; presents; publishes outputs.
//! - **MIDI thread**: listens for CC messages and updates parameter targets.
//! - **OSC thread**: listens for UDP packets (including optional introspection endpoints).
//! - **Recording worker**: encodes frames without stalling the render loop (best-effort, can drop).
//!
//! ## Files that matter
//! - `assets/shaders/<name>.frag` — fragment shader source (live-reloaded)
//! - `assets/params.json` — parameter definitions + MIDI mapping schema
//! - `assets/output.json` — output routing (texture/syphon/spout/ndi/stream) + hotkeys + preview prefs
//! - `assets/recording.json` — recording settings + hotkeys (if enabled)
//!
//! Everything else in this file is mostly "plumbing": load config → create GL objects → run event loop.
//!

mod osc_introspection_helpers;

use glow::HasContext;

use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentContext, Version};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::DisplayBuilder;

use raw_window_handle::HasRawWindowHandle;

use midir::{Ignore, MidiInput};
use rosc::{OscPacket, OscType};
use serde::Deserialize;

use std::collections::HashMap;
use std::ffi::CString;
use std::io::Write;
use std::num::NonZeroU32;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use shadecore_engine::assets::read_to_string;
use shadecore_engine::config::{load_engine_config_from};
use shadecore_engine::config::load_render_selection;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

// -----------------------------------------------------------------------------
// Logging conventions (dev experience)
// -----------------------------------------------------------------------------
// ShadeCore is a multi-subsystem app (render + hot-reload + MIDI + OSC + outputs + recording).
// A small amount of structure in logs goes a long way when debugging live sessions.
//
// Conventions used below:
// - Prefix every log line with a TAG in brackets: [INIT] [CONFIG] [STATE] [RENDER] [OUTPUT] [RECORD] [MIDI] [OSC] [WATCH] [WARN] [ERROR]
// - When something happens automatically, log the *reason* (e.g. "because file changed", "because hotkey pressed").
// - Keep "per-frame" logs off by default. (Key presses are still logged since they help explain state changes.)

mod logging;
mod validate;
mod recording;
use recording::{Recorder, RecordingCfg};

mod presenter;
use presenter::{NullPresenter, Presenter, WindowPresenter};

use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoopBuilder};
use winit::keyboard::{KeyCode, PhysicalKey};

/// -------------------------------
/// Output routing configuration
/// -------------------------------
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum OutputMode {
    Texture,
    Syphon,
    Spout,
    Stream,
    Ndi,
}

/// Preview scaling configuration (presentation only; does NOT affect recording/FBO)
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum PreviewScaleMode {
    Fit,
    Fill,
    Stretch,
    Pixel,
}

impl PreviewScaleMode {
    fn as_i32(self) -> i32 {
        match self {
            PreviewScaleMode::Fit => 0,
            PreviewScaleMode::Fill => 1,
            PreviewScaleMode::Stretch => 2,
            PreviewScaleMode::Pixel => 3,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            PreviewScaleMode::Fit => "fit",
            PreviewScaleMode::Fill => "fill",
            PreviewScaleMode::Stretch => "stretch",
            PreviewScaleMode::Pixel => "pixel",
        }
    }
}

fn default_preview_scale_mode() -> PreviewScaleMode {
    PreviewScaleMode::Fit
}

fn default_preview_enabled() -> bool {
    true
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PreviewHotkeysCfg {
    #[serde(default = "default_preview_hotkeys_fit")]
    fit: Vec<String>,
    #[serde(default = "default_preview_hotkeys_fill")]
    fill: Vec<String>,
    #[serde(default = "default_preview_hotkeys_stretch")]
    stretch: Vec<String>,
    #[serde(default = "default_preview_hotkeys_pixel")]
    pixel: Vec<String>,
}

fn default_preview_hotkeys_fit() -> Vec<String> {
    vec!["Digit7".into(), "Numpad7".into()]
}
fn default_preview_hotkeys_fill() -> Vec<String> {
    vec!["Digit8".into(), "Numpad8".into()]
}
fn default_preview_hotkeys_stretch() -> Vec<String> {
    vec!["Digit9".into(), "Numpad9".into()]
}
fn default_preview_hotkeys_pixel() -> Vec<String> {
    vec!["Digit0".into(), "Numpad0".into()]
}

impl Default for PreviewHotkeysCfg {
    fn default() -> Self {
        Self {
            fit: default_preview_hotkeys_fit(),
            fill: default_preview_hotkeys_fill(),
            stretch: default_preview_hotkeys_stretch(),
            pixel: default_preview_hotkeys_pixel(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PreviewCfg {
    #[serde(default = "default_preview_enabled")]
    enabled: bool,

    #[serde(default = "default_preview_scale_mode")]
    scale_mode: PreviewScaleMode,

    #[serde(default)]
    hotkeys: PreviewHotkeysCfg,
}

impl Default for PreviewCfg {
    fn default() -> Self {
        Self {
            enabled: default_preview_enabled(),
            scale_mode: default_preview_scale_mode(),
            hotkeys: PreviewHotkeysCfg::default(),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]

/// --------------------------------
/// output.json schema (output routing + preview controls)
/// --------------------------------
///
/// This file answers the question: **"where does the rendered FBO texture go?"**
///
/// - It defines the active `output_mode` (texture-only preview, Syphon, Spout, Stream, NDI).
/// - It defines hotkeys that *switch publishing mode* at runtime.
/// - It may include per-backend config like Syphon server name or Stream URL/bitrate.
///
/// Importantly: output routing is *separate* from `params.json` (uniforms/mappings) and from
/// recording configuration (`recording.json`).
struct OutputConfigFile {
    #[serde(default = "default_output_mode")]
    output_mode: OutputMode,

    #[serde(default)]
    syphon: SyphonCfg,

    #[serde(default)]
    spout: SpoutCfg,

    #[serde(default)]
    stream: StreamCfg,

    #[serde(default)]
    ndi: NdiCfg,

    #[serde(default)]
    hotkeys: HotkeysCfg,

    #[serde(default)]
    preview: PreviewCfg,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct StreamCfg {
    /// Master on/off for Stream output.
    #[serde(default)]
    enabled: bool,

    /// "rtsp" (push to an RTSP server) or "rtmp" (push to a streaming platform ingest).
    #[serde(default = "default_stream_target")]
    target: StreamTarget,

    /// RTSP publish URL (requires an RTSP server like MediaMTX running on the URL host/port).
    #[serde(default = "default_rtsp_url")]
    rtsp_url: String,

    /// RTMP publish URL, including stream key if your platform expects it.
    #[serde(default)]
    rtmp_url: Option<String>,

    /// Frames per second to encode/stream.
    #[serde(default = "default_stream_fps")]
    fps: u32,

    /// Video bitrate in kbps.
    #[serde(default = "default_stream_bitrate_kbps")]
    bitrate_kbps: u32,

    /// Keyframe interval (GOP) in frames.
    #[serde(default = "default_stream_gop")]
    gop: u32,

    /// Apply a vertical flip before encoding (OpenGL readback is typically upside-down).
    #[serde(default = "default_true")]
    vflip: bool,

    /// Optional ffmpeg binary path. If not set, we'll try "ffmpeg" from PATH.
    #[serde(default)]
    ffmpeg_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum StreamTarget {
    Rtsp,
    Rtmp,
}

fn default_stream_target() -> StreamTarget {
    StreamTarget::Rtsp
}

fn default_rtsp_url() -> String {
    // Common local default when using an RTSP server like MediaMTX.
    "rtsp://127.0.0.1:8554/shadecore".to_string()
}

fn default_stream_fps() -> u32 {
    60
}

fn default_stream_bitrate_kbps() -> u32 {
    8000
}

fn default_stream_gop() -> u32 {
    // 2 seconds @ 60fps.
    120
}

impl Default for StreamCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            target: default_stream_target(),
            rtsp_url: default_rtsp_url(),
            rtmp_url: None,
            fps: default_stream_fps(),
            bitrate_kbps: default_stream_bitrate_kbps(),
            gop: default_stream_gop(),
            vflip: true,
            ffmpeg_path: None,
        }
    }
}


#[derive(Debug, Clone, serde::Deserialize)]
struct NdiCfg {
    /// Master on/off for NDI output.
    #[serde(default)]
    enabled: bool,

    /// NDI source name as seen by receivers (e.g. OBS).
    #[serde(default)]
    name: Option<String>,

    /// Optional comma-separated NDI groups.
    #[serde(default)]
    groups: Option<String>,

    /// Whether to clock video (recommended true).
    #[serde(default = "default_true")]
    clock_video: bool,

    /// Frame rate numerator.
    #[serde(default = "default_ndi_fps_n")]
    fps_n: i32,

    /// Frame rate denominator.
    #[serde(default = "default_ndi_fps_d")]
    fps_d: i32,

    /// Apply a vertical flip (OpenGL readback is typically upside-down).
    #[serde(default = "default_true")]
    vflip: bool,
}

fn default_ndi_fps_n() -> i32 {
    60
}
fn default_ndi_fps_d() -> i32 {
    1
}

impl Default for NdiCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            name: None,
            groups: None,
            clock_video: true,
            fps_n: default_ndi_fps_n(),
            fps_d: default_ndi_fps_d(),
            vflip: true,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SyphonCfg {
    #[serde(default = "default_true")]
    enabled: bool,

    #[serde(default)]
    server_name: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SpoutCfg {
    #[serde(default)]
    enabled: bool,

    #[serde(default)]
    sender_name: Option<String>,

    #[serde(default = "default_true")]
    invert: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct HotkeysCfg {
    #[serde(default = "default_hotkeys_texture")]
    texture: Vec<String>,
    #[serde(default = "default_hotkeys_syphon")]
    syphon: Vec<String>,
    #[serde(default = "default_hotkeys_spout")]
    spout: Vec<String>,
    #[serde(default = "default_hotkeys_stream")]
    stream: Vec<String>,
    #[serde(default = "default_hotkeys_ndi")]
    ndi: Vec<String>,
}

fn default_hotkeys_texture() -> Vec<String> {
    vec!["Digit1".into(), "Numpad1".into()]
}
fn default_hotkeys_syphon() -> Vec<String> {
    vec!["Digit2".into(), "Numpad2".into()]
}
fn default_hotkeys_spout() -> Vec<String> {
    vec!["Digit3".into(), "Numpad3".into()]
}
fn default_hotkeys_stream() -> Vec<String> {
    vec!["Digit4".into(), "Numpad4".into()]
}
fn default_hotkeys_ndi() -> Vec<String> {
    vec!["Digit6".into(), "Numpad6".into()]
}

impl Default for HotkeysCfg {
    fn default() -> Self {
        Self {
            texture: default_hotkeys_texture(),
            syphon: default_hotkeys_syphon(),
            spout: default_hotkeys_spout(),
            stream: default_hotkeys_stream(),
            ndi: default_hotkeys_ndi(),
        }
    }
}

fn preview_scale_mode_name(mode: i32) -> &'static str {
    match mode {
        0 => "FIT",
        1 => "FILL",
        2 => "STRETCH",
        3 => "PIXEL",
        _ => "UNKNOWN",
    }
}

fn parse_keycode(name: &str) -> Option<KeyCode> {
    match name {
        "Digit0" => Some(KeyCode::Digit0),
        "Digit1" => Some(KeyCode::Digit1),
        "Digit2" => Some(KeyCode::Digit2),
        "Digit3" => Some(KeyCode::Digit3),
        "Digit4" => Some(KeyCode::Digit4),
        "Digit5" => Some(KeyCode::Digit5),
        "Digit6" => Some(KeyCode::Digit6),
        "Digit7" => Some(KeyCode::Digit7),
        "Digit8" => Some(KeyCode::Digit8),
        "Digit9" => Some(KeyCode::Digit9),

        "Numpad0" => Some(KeyCode::Numpad0),
        "Numpad1" => Some(KeyCode::Numpad1),
        "Numpad2" => Some(KeyCode::Numpad2),
        "Numpad3" => Some(KeyCode::Numpad3),
        "Numpad4" => Some(KeyCode::Numpad4),
        "Numpad5" => Some(KeyCode::Numpad5),
        "Numpad6" => Some(KeyCode::Numpad6),
        "Numpad7" => Some(KeyCode::Numpad7),
        "Numpad8" => Some(KeyCode::Numpad8),
        "Numpad9" => Some(KeyCode::Numpad9),

        // Common aliases (feel free to extend)
        "Insert" => Some(KeyCode::Insert),
        "PageUp" => Some(KeyCode::PageUp),
        "KeyT" => Some(KeyCode::KeyT),
        "KeyR" => Some(KeyCode::KeyR),
        "KeyS" => Some(KeyCode::KeyS),

        // Profile switching defaults / common picks
        "BracketLeft" => Some(KeyCode::BracketLeft),
        "BracketRight" => Some(KeyCode::BracketRight),
        "KeyP" => Some(KeyCode::KeyP),
        "KeyO" => Some(KeyCode::KeyO),
        "KeyL" => Some(KeyCode::KeyL),
        "KeyD" => Some(KeyCode::KeyD),
        "KeyN" => Some(KeyCode::KeyN),
        "KeyB" => Some(KeyCode::KeyB),
        "KeyM" => Some(KeyCode::KeyM),

        _ => None,
    }
}

fn build_hotkey_map(cfg: &HotkeysCfg) -> HashMap<KeyCode, OutputMode> {
    let mut map = HashMap::new();
    for k in &cfg.texture {
        if let Some(code) = parse_keycode(k) {
            map.insert(code, OutputMode::Texture);
        }
    }
    for k in &cfg.syphon {
        if let Some(code) = parse_keycode(k) {
            map.insert(code, OutputMode::Syphon);
        }
    }
    for k in &cfg.spout {
        if let Some(code) = parse_keycode(k) {
            map.insert(code, OutputMode::Spout);
        }
    }
    for k in &cfg.stream {
        if let Some(code) = parse_keycode(k) {
            map.insert(code, OutputMode::Stream);
        }
    }
    for k in &cfg.ndi {
        if let Some(code) = parse_keycode(k) {
            map.insert(code, OutputMode::Ndi);
        }
    }
    map
}

fn build_preview_hotkey_map(cfg: &PreviewHotkeysCfg) -> HashMap<KeyCode, i32> {
    let mut map = HashMap::new();
    let mut insert_keys = |keys: &Vec<String>, mode: PreviewScaleMode| {
        for name in keys {
            if let Some(code) = parse_keycode(name) {
                map.insert(code, mode.as_i32());
            }
        }
    };
    insert_keys(&cfg.fit, PreviewScaleMode::Fit);
    insert_keys(&cfg.fill, PreviewScaleMode::Fill);
    insert_keys(&cfg.stretch, PreviewScaleMode::Stretch);
    insert_keys(&cfg.pixel, PreviewScaleMode::Pixel);
    map
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecHotkeyAction {
    Toggle,
    Start,
    Stop,
}

fn build_recording_hotkey_map(cfg: &RecordingCfg) -> HashMap<KeyCode, RecHotkeyAction> {
    let mut map = HashMap::new();
    let mut add_key = |name: &str, action: RecHotkeyAction| {
        if let Some(code) = parse_keycode(name) {
            map.insert(code, action);
            match code {
                KeyCode::Numpad0 => {
                    map.insert(KeyCode::Digit0, action);
                    map.insert(KeyCode::Insert, action);
                }
                KeyCode::Numpad9 => {
                    map.insert(KeyCode::Digit9, action);
                    map.insert(KeyCode::PageUp, action);
                }
                _ => {}
            }
        }
    };
    for k in &cfg.toggle_keys { add_key(k, RecHotkeyAction::Toggle); }
    for k in &cfg.start_keys { add_key(k, RecHotkeyAction::Start); }
    for k in &cfg.stop_keys { add_key(k, RecHotkeyAction::Stop); }
    map
}


/// Load recording configuration.
///
/// Recording config supports two shapes for long-term compatibility:
///
/// 1) **Legacy single-file**: `recording.json` directly matches `RecordingCfg` (one profile).
/// 2) **Controller + profiles**: `recording.json` contains `active_profile` + hotkeys, and
///    `recording.profiles.json` contains named profile objects. We merge them to produce a final
///    `RecordingCfg` at runtime.
///
/// Why two files? It lets you switch recording “quality presets” without duplicating hotkey bindings,
/// and it keeps `output.json` focused purely on publishing.
fn load_recording_config(path: &Path) -> RecordingCfg {
    // Backwards compatible loader:
    // - If recording.json is a "controller" with active_profile + hotkeys, merge with recording.profiles.json.
    // - Otherwise, treat recording.json as a full RecordingCfg (legacy single-profile format).
    #[derive(Debug, Clone, Deserialize, Default, PartialEq)]
    struct RecordingHotkeys {
        #[serde(default)]
        toggle: Vec<String>,
        #[serde(default)]
        start: Vec<String>,
        #[serde(default)]
        stop: Vec<String>,
    }

    #[derive(Debug, Clone, Deserialize, PartialEq)]
    struct RecordingController {
        #[serde(default)]
        enabled: bool,
        #[serde(default)]
        active_profile: Option<String>,
        #[serde(default)]
        hotkeys: RecordingHotkeys,
    }

    #[derive(Debug, Clone, Deserialize, PartialEq)]
    struct RecordingProfilesFile {
        #[serde(default)]
        profiles: HashMap<String, RecordingProfile>,
    }

    #[derive(Debug, Clone, Deserialize, PartialEq)]
    struct RecordingProfile {
        #[serde(default)]
        out_dir: Option<PathBuf>,
        #[serde(default)]
        container: Option<recording::Container>,
        #[serde(default)]
        codec: Option<recording::Codec>,
        #[serde(default)]
        fps: Option<u32>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
        #[serde(default)]
        ffmpeg_path: Option<String>,
        #[serde(default)]
        h264_crf: Option<u32>,
        #[serde(default)]
        h264_preset: Option<String>,
        #[serde(default)]
        pix_fmt_out: Option<String>,
        #[serde(default)]
        prores_profile: Option<u32>,
        #[serde(default)]
        vflip: Option<bool>,
    }

    fn apply_profile(dst: &mut RecordingCfg, p: &RecordingProfile) {
        if let Some(v) = &p.out_dir { dst.out_dir = v.clone(); }
        if let Some(v) = p.container { dst.container = v; }
        if let Some(v) = p.codec { dst.codec = v; }
        if let Some(v) = p.fps { dst.fps = v; }
        if let Some(v) = p.width { dst.width = v; }
        if let Some(v) = p.height { dst.height = v; }
        if let Some(v) = &p.ffmpeg_path { dst.ffmpeg_path = v.clone(); }
        if let Some(v) = p.h264_crf { dst.h264_crf = v; }
        if let Some(v) = &p.h264_preset { dst.h264_preset = v.clone(); }
        if let Some(v) = &p.pix_fmt_out { dst.pix_fmt_out = v.clone(); }
        if let Some(v) = p.prores_profile { dst.prores_profile = v; }
        if let Some(v) = p.vflip { dst.vflip = v; }
    }

    let default_cfg = RecordingCfg::default();

    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return default_cfg,
    };

    // First try controller format.
    if let Ok(controller) = serde_json::from_str::<RecordingController>(&data) {
        if controller.active_profile.is_some() || !controller.hotkeys.start.is_empty() || !controller.hotkeys.stop.is_empty() || !controller.hotkeys.toggle.is_empty() {
            let mut cfg = RecordingCfg::default();
            cfg.enabled = controller.enabled;
            cfg.toggle_keys = controller.hotkeys.toggle.clone();
            cfg.start_keys = controller.hotkeys.start.clone();
            cfg.stop_keys = controller.hotkeys.stop.clone();

            // Load profiles file from the same assets directory.
            let profiles_path = path.parent().unwrap_or_else(|| Path::new(".")).join("recording.profiles.json");
            let profiles_data = std::fs::read_to_string(&profiles_path).ok();

            // Validate controller <-> profiles linkage (friendly warnings)
            if let Ok(rec_v) = serde_json::from_str::<serde_json::Value>(&data) {
                if let Some(pdata) = &profiles_data {
                    if let Ok(prof_v) = serde_json::from_str::<serde_json::Value>(pdata) {
                        let issues = crate::validate::validate_recording_profiles(&rec_v, &prof_v);
                        crate::validate::emit_summary("CONFIG", "recording profiles", &issues);
                        crate::validate::emit_issues("CONFIG", &issues);
                    }
                }
            }


            if let (Some(active), Some(pdata)) = (controller.active_profile.clone(), profiles_data) {
                match serde_json::from_str::<RecordingProfilesFile>(&pdata) {
                    Ok(pf) => {
                        if let Some(p) = pf.profiles.get(&active) {
                            apply_profile(&mut cfg, p);
                            logi!("RECORDING", "active profile -> {} ({}x{}@{} {:?}/{:?})",
                                active, cfg.width, cfg.height, cfg.fps, cfg.container, cfg.codec
                            );
                        } else {
                            logw!("RECORDING", "active_profile '{}' not found in {}", active, profiles_path.display());}
                    }
                    Err(e) => logw!("RECORDING", "failed to parse {}: {e}", profiles_path.display()),
                }
            } else if controller.active_profile.is_some() {
                logw!("RECORDING", "active_profile set but {} missing/unreadable", profiles_path.display());}

            return cfg;
        }
    }

    // Legacy: full RecordingCfg in recording.json
    match serde_json::from_str::<RecordingCfg>(&data) {
        Ok(cfg) => cfg,
        Err(e) => {
            logw!("RECORDING", "Failed to parse {}: {e}", path.display());default_cfg
        }
    }
}


fn default_true() -> bool {
    true
}


fn build_profile_hotkey_map(pf: &ParamsFile) -> HashMap<KeyCode, ProfileAction> {
    let mut map: HashMap<KeyCode, ProfileAction> = HashMap::new();

    // Configured hotkeys from params.json
    for k in &pf.profile_hotkeys.next {
        if let Some(code) = parse_keycode(k) {
            map.insert(code, ProfileAction::Next);
        }
    }
    for k in &pf.profile_hotkeys.prev {
        if let Some(code) = parse_keycode(k) {
            map.insert(code, ProfileAction::Prev);
        }
    }
    for (profile_name, keys) in &pf.profile_hotkeys.set {
        for k in keys {
            if let Some(code) = parse_keycode(k) {
                map.insert(code, ProfileAction::Set(profile_name.clone()));
            }
        }
    }

    // Always provide the classic default behavior:
    //   ] = next profile
    //   [ = prev profile
    // unless the user explicitly bound those keys already.
    map.entry(KeyCode::BracketRight).or_insert(ProfileAction::Next);
    map.entry(KeyCode::BracketLeft).or_insert(ProfileAction::Prev);

    map
}


fn sorted_profile_names_for_shader(
    pf: &ParamsFile,
    assets: &std::path::Path,
    shader_frag: &std::path::Path,
) -> Vec<String> {
    // Prefer per-shader profiles if present
    for (k, per_shader) in &pf.shader_profiles {
        let resolved = resolve_assets_path(assets, k);
        if resolved == shader_frag {
            let mut names: Vec<String> = per_shader.keys().cloned().collect();
            names.sort();
            return names;
        }
    }

    // Fallback: global profiles
    let mut names: Vec<String> = pf.profiles.keys().cloned().collect();
    names.sort();
    names
}

fn pick_active_profile_for_shader(
    pf: &ParamsFile,
    assets: &std::path::Path,
    shader_frag: &std::path::Path,
) -> Option<String> {
    // If there is a per-shader active profile entry, use it
    for (k, active_name) in &pf.active_shader_profiles {
        let resolved = resolve_assets_path(assets, k);
        if resolved == shader_frag {
            return Some(active_name.clone());
        }
    }

    // Otherwise: per-shader "default" if present, else first
    for (k, per_shader) in &pf.shader_profiles {
        let resolved = resolve_assets_path(assets, k);
        if resolved == shader_frag {
            if per_shader.contains_key("default") {
                return Some("default".to_string());
            }
            let mut names: Vec<String> = per_shader.keys().cloned().collect();
            names.sort();
            return names.first().cloned();
        }
    }

    // Fallback to legacy global selection behavior
    if let Some(n) = pf.active_profile.clone() {
        return Some(n);
    }
    if pf.profiles.contains_key("default") {
        return Some("default".to_string());
    }
    let mut names: Vec<String> = pf.profiles.keys().cloned().collect();
    names.sort();
    names.first().cloned()
}

fn set_active_profile_for_shader(
    pf: &mut ParamsFile,
    assets: &std::path::Path,
    shader_frag: &std::path::Path,
    profile_name: &str,
) {
    // Update existing entry if present
    for (k, v) in pf.active_shader_profiles.iter_mut() {
        let resolved = resolve_assets_path(assets, k);
        if resolved == shader_frag {
            *v = profile_name.to_string();
            return;
        }
    }

    // Otherwise, create a new entry using the best matching shader_profiles key string if present;
    // if not, fall back to storing the absolute path string.
    for (k, per_shader) in &pf.shader_profiles {
        let resolved = resolve_assets_path(assets, k);
        if resolved == shader_frag && per_shader.contains_key(profile_name) {
            pf.active_shader_profiles.insert(k.clone(), profile_name.to_string());
            return;
        }
    }

    pf.active_shader_profiles
        .insert(shader_frag.to_string_lossy().to_string(), profile_name.to_string());
}

fn default_output_mode() -> OutputMode {
    OutputMode::Texture
}

impl Default for SyphonCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            server_name: None,
        }
    }
}
impl Default for SpoutCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            sender_name: None,
            invert: true,
        }
    }
}

fn load_output_config(path: &Path, default_mode: OutputMode) -> OutputConfigFile {
    let default_cfg = OutputConfigFile {
        output_mode: default_mode,
        syphon: SyphonCfg::default(),
        spout: SpoutCfg::default(),
        stream: StreamCfg::default(),
        ndi: NdiCfg::default(),
        hotkeys: HotkeysCfg::default(),
        preview: PreviewCfg::default(),
    };

    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return default_cfg,
    };

    match serde_json::from_str::<OutputConfigFile>(&data) {
        Ok(cfg) => cfg,
        Err(e) => {
            logi!("OUTPUT", "Failed to parse output config ({}): {}. Using defaults.",
                path.display(),
                e
            );
            default_cfg
        }
    }
}

// Fullscreen triangle vertex shader
const VERT_SRC: &str = r#"#version 330 core
out vec2 v_uv;
void main() {
    vec2 pos;
    if (gl_VertexID == 0) pos = vec2(-1.0, -1.0);
    else if (gl_VertexID == 1) pos = vec2( 3.0, -1.0);
    else pos = vec2(-1.0,  3.0);
    v_uv = pos * 0.5 + 0.5;
    gl_Position = vec4(pos, 0.0, 1.0);
}"#;

/// -------------------------------
/// params.json schema (matches your uploaded file)
/// -------------------------------
#[derive(Debug, Clone, Deserialize, PartialEq)]
struct ParamsFile {
    version: u32,
    #[serde(default)]
    midi: MidiGlobalCfg,
    #[serde(default)]
    osc: OscCfg,
    #[serde(default)]
    params: Vec<ParamDef>,

    /// Optional named presets that override per-param defaults.
    /// Example:
    /// "profiles": { "default": { "u_gain": 1.0 }, "lofi": { "u_gain": 0.3 } }
    #[serde(default)]
    profiles: HashMap<String, ProfilePreset>,

    /// Per-shader profiles. Keys are frag paths (same strings you use in render.json),
    /// values are maps of profile_name -> preset.
    /// This enforces: a shader only cycles through its own profiles.
    #[serde(default)]
    shader_profiles: HashMap<String, HashMap<String, ProfilePreset>>,

    /// Per-shader active profile name. Keyed by frag path.
    #[serde(default)]
    active_shader_profiles: HashMap<String, String>,


    /// Which profile is active on startup (and on hot-reload), if present.
    #[serde(default)]
    active_profile: Option<String>,

    /// Optional profile switching hotkeys.
    #[serde(default)]
    profile_hotkeys: ProfileHotkeysCfg,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
struct MidiGlobalCfg {
    #[serde(default)]
    preferred_device_contains: Option<String>,
    #[serde(default)]
    channel: Option<u8>,
}


#[derive(Debug, Clone, Deserialize, PartialEq)]
struct OscMappingCfg {
    /// OSC address pattern. Can be:
    /// - Full address (e.g. "/shadecore/param/gain")
    /// - Prefix-relative (e.g. "/param/gain" or "param/gain")
    addr: String,
    /// Target param/uniform name (e.g. "u_gain")
    param: String,
    /// Optional override range for this mapping (used when normalized=true or for clamping in raw mode)
    #[serde(default)]
    min: Option<f32>,
    #[serde(default)]
    max: Option<f32>,
    /// Optional smoothing override for this mapping
    #[serde(default)]
    smooth: Option<f32>,
    /// Optional override for normalized handling ("normalized" or "raw")
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct OscCfg {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_osc_bind")]
    bind: String,
    #[serde(default = "default_osc_prefix")]
    prefix: String,
    #[serde(default = "default_true")]
    normalized: bool,

    /// Optional mapping table (same spirit as MIDI mappings):
    /// maps OSC addresses to uniform/param names with optional min/max/smooth overrides.
    #[serde(default)]
    mappings: Vec<OscMappingCfg>,
}

fn default_osc_bind() -> String { "0.0.0.0:9000".into() }
fn default_osc_prefix() -> String { "/shadecore".into() }

impl Default for OscCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_osc_bind(),
            prefix: default_osc_prefix(),
            normalized: true,
            mappings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct OscMappingResolved {
    param: String,
    min: Option<f32>,
    max: Option<f32>,
    smooth: Option<f32>,
    // true = normalized, false = raw
    normalized: bool,
}

#[derive(Debug, Clone)]
struct OscRuntime {
    cfg: OscCfg,
    map: HashMap<String, OscMappingResolved>, // full addr -> mapping
}

impl OscRuntime {
    fn new(cfg: OscCfg) -> Self {
        let mut map = HashMap::new();
        let prefix = cfg.prefix.trim_end_matches('/').to_string();
        for m in &cfg.mappings {
            let a = m.addr.trim();
            if a.is_empty() { continue; }

            let full = if a.starts_with(&prefix) {
                a.to_string()
            } else if a.starts_with('/') {
                // if it's prefix-relative like "/param/...", join with prefix
                if a.starts_with("/param/") || a.starts_with("/raw/") {
                    format!("{}{}", prefix, a)
                } else {
                    // treat as absolute path
                    a.to_string()
                }
            } else {
                // "param/foo" or "raw/foo"
                format!("{}/{}", prefix, a)
            };

            let mode_norm = match m.mode.as_deref().map(|s| s.to_lowercase()) {
                Some(s) if s == "raw" => false,
                Some(s) if s == "normalized" || s == "norm" || s == "param" => true,
                _ => cfg.normalized, // default
            };

            map.insert(
                full,
                OscMappingResolved {
                    param: m.param.clone(),
                    min: m.min,
                    max: m.max,
                    smooth: m.smooth,
                    normalized: mode_norm,
                },
            );
        }
        Self { cfg, map }
    }
}



#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(untagged)]
enum ProfilePreset {
    /// Back-compat: { "u_gain": 1.0, "u_zoom": 2.0 }
    Legacy(HashMap<String, f32>),

    /// New:
    /// {
    ///   "uniforms": { "u_gain": 1.0 },
    ///   "midi": { "preferred_device_contains": "akai", "channel": 0 },
    ///   "cc_overrides": { "u_gain": 12, "u_zoom": 13 }
    /// }
    V2(ProfilePresetV2),
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
struct ProfilePresetV2 {
    #[serde(default)]
    uniforms: HashMap<String, f32>,
    #[serde(default)]
    midi: Option<MidiGlobalCfg>,
    #[serde(default)]
    cc_overrides: HashMap<String, u8>,
}

impl ProfilePreset {
    fn uniforms(&self) -> HashMap<String, f32> {
        match self {
            ProfilePreset::Legacy(m) => m.clone(),
            ProfilePreset::V2(v) => v.uniforms.clone(),
        }
    }

    fn midi_override(&self) -> Option<MidiGlobalCfg> {
        match self {
            ProfilePreset::Legacy(_) => None,
            ProfilePreset::V2(v) => v.midi.clone(),
        }
    }

    fn cc_overrides(&self) -> HashMap<String, u8> {
        match self {
            ProfilePreset::Legacy(_) => HashMap::new(),
            ProfilePreset::V2(v) => v.cc_overrides.clone(),
        }
    }
}

fn merge_midi_cfg(base: &MidiGlobalCfg, ov: Option<MidiGlobalCfg>) -> MidiGlobalCfg {
    if let Some(o) = ov {
        MidiGlobalCfg {
            preferred_device_contains: o.preferred_device_contains.or_else(|| base.preferred_device_contains.clone()),
            channel: o.channel.or(base.channel),
        }
    } else {
        base.clone()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct ParamDef {
    name: String,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    default: f32,
    #[serde(default)]
    min: f32,
    #[serde(default = "default_one")]
    max: f32,
    #[serde(default)]
    smoothing: f32,
    #[serde(default)]
    midi: Option<MidiBinding>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct MidiBinding {
    cc: u8,
    #[serde(default)]
    channel: Option<u8>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
struct ProfileHotkeysCfg {
    /// Cycle forward through profiles (default: BracketRight)
    #[serde(default = "default_profile_next")]
    next: Vec<String>,
    /// Cycle backward through profiles (default: BracketLeft)
    #[serde(default = "default_profile_prev")]
    prev: Vec<String>,
    /// Optional direct bindings: { "lofi": ["KeyL"], "default": ["KeyD"] }
    #[serde(default)]
    set: HashMap<String, Vec<String>>,
}

fn default_profile_next() -> Vec<String> {
    vec!["BracketRight".into()]
}
fn default_profile_prev() -> Vec<String> {
    vec!["BracketLeft".into()]
}

#[derive(Debug, Clone)]
enum ProfileAction {
    Next,
    Prev,
    Set(String),
}

fn default_one() -> f32 {
    1.0
}

/// -------------------------------
/// Runtime parameter store
/// -------------------------------
#[derive(Debug, Clone)]
struct ParamMapping {
    name: String,
    min: f32,
    max: f32,
    smoothing: f32,
}

#[derive(Debug)]
/// Shared runtime store for all *uniform* parameters.
///
/// The store tracks **current** values (what the renderer uses this frame) and **targets**
/// (where inputs want the value to move toward). Each frame the render loop applies simple
/// exponential smoothing:
///
/// `value += (target - value) * smooth`
///
/// where `smooth` is typically a small number like `0.05..0.2`.
///
/// Why separate `values` and `targets`?
/// - MIDI / OSC can update targets at any time (other threads).
/// - The render loop can advance values deterministically once per frame.
///
/// Ranges are stored separately so different parameters can share the same incoming "normalized"
/// control space (0..1) but map to different semantic ranges (e.g. zoom 0.25..4.0).
struct ParamStore {
    /// Current (smoothed) value used by the renderer this frame.
    values: HashMap<String, f32>,
    /// Latest desired value coming from inputs (MIDI/OSC/UI).
    targets: HashMap<String, f32>,
    /// Per-parameter smoothing coefficient in the range 0..1.
    smooth: HashMap<String, f32>,
    /// Per-parameter (min,max) range used when mapping normalized values.
    ranges: HashMap<String, (f32, f32)>,
    /// MIDI CC mapping table: (channel, cc) -> mapping.
    ///
    /// Channel may be a wildcard (255) to mean "any channel" depending on the mapping layer.
    mappings: HashMap<(u8, u8), ParamMapping>,
}


fn normalize_midi_channel(ch: u8) -> u8 {
    // Accept both 0-based (0..15) and 1-based (1..16) channels from JSON/GUI.
    // - If user provides 1..16, treat it as MIDI channel 1..16 and normalize to 0..15.
    // - If user provides 0..15, treat it as already normalized.
    // - Any other value is passed through (allows internal wildcard 255).
    match ch {
        1..=16 => ch - 1,
        0..=15 => ch,
        _ => ch,
    }
}

fn normalize_midi_channel_opt(ch: Option<u8>) -> Option<u8> {
    ch.map(normalize_midi_channel)
}

impl ParamStore {
    fn new(pf: &ParamsFile) -> Self {
        let mut values = HashMap::new();
        let mut targets = HashMap::new();
        let mut smooth = HashMap::new();
        let mut ranges = HashMap::new();

        for p in &pf.params {
            values.insert(p.name.clone(), p.default);
            targets.insert(p.name.clone(), p.default);
            smooth.insert(p.name.clone(), p.smoothing);
            ranges.insert(p.name.clone(), (p.min, p.max));
        }

        let mappings = Self::build_mappings(pf, &pf.midi, &HashMap::new());
        logi!("MIDI", "mappings[startup] count={}", mappings.len());for ((ch, cc), map) in mappings.iter().take(32) {
            logi!("MIDI", "map ch={} cc={} -> {} (min={} max={} smooth={})", ch, cc, map.name, map.min, map.max, map.smoothing);}

        Self {
            values,
            targets,
            smooth,
            ranges,
            mappings,
        }
    }

    fn build_mappings(
        pf: &ParamsFile,
        effective_midi: &MidiGlobalCfg,
        cc_overrides: &HashMap<String, u8>,
    ) -> HashMap<(u8, u8), ParamMapping> {
        let mut mappings = HashMap::new();
        let global_chan_opt = normalize_midi_channel_opt(effective_midi.channel);

        for p in &pf.params {
            if let Some(b) = &p.midi {
                let ch_opt = normalize_midi_channel_opt(b.channel).or(global_chan_opt);
                let cc = cc_overrides.get(&p.name).copied().unwrap_or(b.cc);

                // If neither param nor global specify a channel, treat as wildcard.
                let ch = ch_opt.unwrap_or(255);

                mappings.insert(
                    (ch, cc),
                    ParamMapping {
                        name: p.name.clone(),
                        min: p.min,
                        max: p.max,
                        smoothing: p.smoothing,
                    },
                );
            }
        }

        mappings
    }


    fn apply_params_file(
        &mut self,
        new_pf: &ParamsFile,
        active_profile: Option<&str>,
    ) -> MidiGlobalCfg {
        // Preserve any currently "targeted" values (likely driven by MIDI),
        // but refresh defaults (and create/remove params) from the new file.
        let old_targets = self.targets.clone();

        let mut new_values: HashMap<String, f32> = HashMap::new();
        let mut new_targets: HashMap<String, f32> = HashMap::new();
        let mut new_smooth: HashMap<String, f32> = HashMap::new();
        let mut new_ranges: HashMap<String, (f32, f32)> = HashMap::new();

        // Base MIDI settings from the file (profile can override later)
        let base_midi = new_pf.midi.clone();

        // Base mappings from the new file (profile can override CCs later)
        let mut effective_midi = base_midi.clone();
        let mut cc_overrides: HashMap<String, u8> = HashMap::new();

        for p in &new_pf.params {
            let name = p.name.clone();

            // If this param was being targeted (e.g. active MIDI input), keep current/target.
            if let Some(t) = old_targets.get(&name).copied() {
                let cur = *self.values.get(&name).unwrap_or(&t);
                new_values.insert(name.clone(), cur);
                new_targets.insert(name.clone(), t);
                new_smooth.insert(
                    name.clone(),
                    *self.smooth.get(&name).unwrap_or(&p.smoothing),
                );
                new_ranges.insert(name.clone(), (p.min, p.max));
            } else {
                new_values.insert(name.clone(), p.default);
                new_targets.insert(name.clone(), p.default);
                new_smooth.insert(name.clone(), p.smoothing);
                new_ranges.insert(name.clone(), (p.min, p.max));
            }
        }

        self.values = new_values;
        self.targets = new_targets;
        self.smooth = new_smooth;
        self.ranges = new_ranges;

        // If there is an active profile, it can override uniforms AND MIDI settings.
        if let Some(profile) = active_profile {
            if let Some(preset) = new_pf.profiles.get(profile) {
                effective_midi = merge_midi_cfg(&base_midi, preset.midi_override());
                cc_overrides = preset.cc_overrides();

                // Apply uniform overrides
                for (k, v) in preset.uniforms() {
                    self.values.insert(k.clone(), v);
                    self.targets.insert(k.clone(), v);
                }

                logi!("PARAMS", "applied profile: {profile}");} else {
                logw!("PARAMS", "profile not found: {profile}");}
        }

        self.mappings = Self::build_mappings(new_pf, &effective_midi, &cc_overrides);
        logi!("MIDI", "mappings[params_reload] count={}", self.mappings.len());for ((ch, cc), map) in self.mappings.iter().take(32) {
            logi!("MIDI", "map ch={} cc={} -> {} (min={} max={} smooth={})", ch, cc, map.name, map.min, map.max, map.smoothing);}

        effective_midi
    }

    
    fn apply_profile(
        &mut self,
        pf: &ParamsFile,
        assets: &std::path::Path,
        shader_frag: Option<&std::path::Path>,
        profile_name: &str,
    ) -> MidiGlobalCfg {
        // Resolve preset from per-shader profiles first (if present), otherwise fall back to global `profiles`.
        let mut preset_opt: Option<&ProfilePreset> = None;

        if let Some(shader_path) = shader_frag {
            // Find the matching shader key by resolving keys relative to assets/
            for (k, per_shader) in &pf.shader_profiles {
                let resolved = resolve_assets_path(assets, k);
                if resolved == shader_path {
                    preset_opt = per_shader.get(profile_name);
                    break;
                }
            }
        }

        if preset_opt.is_none() {
            preset_opt = pf.profiles.get(profile_name);
        }

        if let Some(preset) = preset_opt {
            // 1) Apply uniform values
            let uniforms = preset.uniforms();
            for (k, v) in &uniforms {
                self.values.insert(k.clone(), *v);
                self.targets.insert(k.clone(), *v);
            }

            // 2) Apply MIDI overrides for this profile (device/channel) and rebuild CC mapping table
            let effective_midi = merge_midi_cfg(&pf.midi, preset.midi_override());
            let cc_overrides = preset.cc_overrides();
            self.mappings = Self::build_mappings(pf, &effective_midi, &cc_overrides);
            logi!("MIDI", "mappings[profile_apply] count={}", self.mappings.len());for ((ch, cc), map) in self.mappings.iter().take(32) {
                logi!("MIDI", "map ch={} cc={} -> {} (min={} max={} smooth={})", ch, cc, map.name, map.min, map.max, map.smoothing);}

            if let Some(shader_path) = shader_frag {
                logi!("PARAMS", "applied profile: {profile_name} (shader: {})", shader_path.display());} else {
                logi!("PARAMS", "applied profile: {profile_name}");}

            effective_midi
        } else {
            logw!("PARAMS", "profile not found: {profile_name} (keeping existing MIDI mappings)");// Do NOT clobber mappings here; keep last-good routing.
            pf.midi.clone()
        }
    }

    fn set_cc(&mut self, ch: u8, cc: u8, val_0_127: u8) -> bool {
        // Primary: exact channel+cc match
        if let Some(map) = self.mappings.get(&(ch, cc)) {
            let x = (val_0_127 as f32) / 127.0;
            let t = map.min + (map.max - map.min) * x;
            self.targets.insert(map.name.clone(), t);
            self.smooth.insert(map.name.clone(), map.smoothing);
            return true;
        }

        // Secondary: wildcard channel (255) for this CC
        if let Some(map) = self.mappings.get(&(255, cc)) {
            let x = (val_0_127 as f32) / 127.0;
            let t = map.min + (map.max - map.min) * x;
            self.targets.insert(map.name.clone(), t);
            self.smooth.insert(map.name.clone(), map.smoothing);
            return true;
        }

        // Tertiary: CC-only fallback (if there is exactly one mapping for this CC, use it).
        // This prevents "mapped=false" black-holing when a device reports a different channel than expected.
        let mut found: Option<&ParamMapping> = None;
        for ((_, c), map) in &self.mappings {
            if *c == cc {
                if found.is_some() {
                    return false; // ambiguous
                }
                found = Some(map);
            }
        }
        if let Some(map) = found {
            let x = (val_0_127 as f32) / 127.0;
            let t = map.min + (map.max - map.min) * x;
            self.targets.insert(map.name.clone(), t);
            self.smooth.insert(map.name.clone(), map.smoothing);
            return true;
        }

        false
    }

    fn set_target_raw(&mut self, name: &str, val: f32) -> bool {
        if !self.values.contains_key(name) {
            return false;
        }
        let (mn, mx) = self.ranges.get(name).copied().unwrap_or((val, val));
        let v = val.clamp(mn, mx);
        self.targets.insert(name.to_string(), v);
        // keep existing smoothing
        let s = self.smooth.get(name).copied().unwrap_or(0.0);
        self.smooth.insert(name.to_string(), s);
        true
    }

    fn set_target_normalized(&mut self, name: &str, x01: f32) -> bool {
        if !self.values.contains_key(name) {
            return false;
        }
        let (mn, mx) = self.ranges.get(name).copied().unwrap_or((0.0, 1.0));
        let x = x01.clamp(0.0, 1.0);
        let v = mn + (mx - mn) * x;
        self.targets.insert(name.to_string(), v);
        let s = self.smooth.get(name).copied().unwrap_or(0.0);
        self.smooth.insert(name.to_string(), s);
        true
    }

    fn apply_osc_runtime(&mut self, rt: &OscRuntime, addr: &str, args: &[OscType]) -> Option<(String, f32, bool)> {
        // 1) mapping table (address -> param)
        if let Some(m) = rt.map.get(addr) {
            // extract numeric arg
            let v = match args.get(0)? {
                OscType::Float(f) => *f,
                OscType::Double(d) => *d as f32,
                OscType::Int(i) => *i as f32,
                OscType::Long(l) => *l as f32,
                _ => return None,
            };
            let name = m.param.as_str();
            if !self.values.contains_key(name) {
                return None;
            }

            let (mn, mx) = match (m.min, m.max) {
                (Some(a), Some(b)) => (a, b),
                _ => self.ranges.get(name).copied().unwrap_or((0.0, 1.0)),
            };

            let target = if m.normalized {
                let x = v.clamp(0.0, 1.0);
                mn + (mx - mn) * x
            } else {
                v.clamp(mn.min(mx), mn.max(mx))
            };

            self.targets.insert(name.to_string(), target);
            if let Some(s) = m.smooth {
                self.smooth.insert(name.to_string(), s);
            }
            return Some((name.to_string(), target, m.normalized));
        }

        // 2) fallback to built-in direct routes: /prefix/param/<name> and /prefix/raw/<name>
        self.apply_osc(&rt.cfg, addr, args)
    }


    fn apply_osc(&mut self, osc: &OscCfg, addr: &str, args: &[OscType]) -> Option<(String, f32, bool)> {
        // Returns (param_name, target_value, used_normalized) if applied
        let prefix = osc.prefix.trim_end_matches('/');
        let addr = addr.trim();

        let p_param = format!("{}/param/", prefix);
        let p_raw = format!("{}/raw/", prefix);

        let (mode, name) = if let Some(rest) = addr.strip_prefix(&p_param) {
            ("param", rest)
        } else if let Some(rest) = addr.strip_prefix(&p_raw) {
            ("raw", rest)
        } else {
            return None;
        };

        let name = name.trim_matches('/');
        if name.is_empty() {
            return None;
        }
        if args.is_empty() {
            return None;
        }

        let v = match &args[0] {
            OscType::Float(f) => *f as f32,
            OscType::Double(d) => *d as f32,
            OscType::Int(i) => *i as f32,
            OscType::Long(l) => *l as f32,
            _ => return None,
        };

        let used_norm = (mode == "param") && osc.normalized;
        let ok = if used_norm {
            self.set_target_normalized(name, v)
        } else {
            self.set_target_raw(name, v)
        };
        if !ok {
            return None;
        }
        let target = *self.targets.get(name).unwrap_or(&v);
        Some((name.to_string(), target, used_norm))
    }


    fn tick(&mut self) {
        let keys: Vec<String> = self.values.keys().cloned().collect();
        for name in keys {
            let cur = *self.values.get(&name).unwrap_or(&0.0);
            let target = *self.targets.get(&name).unwrap_or(&cur);
            let s = self.smooth.get(&name).copied().unwrap_or(0.0).clamp(0.0, 1.0);

            let alpha = if s <= 0.0 { 1.0 } else { (1.0 - s).clamp(0.001, 1.0) };
            let next = cur + (target - cur) * alpha;
            self.values.insert(name, next);
        }
    }
}

// -------------------------------
// Syphon C-ABI bridge (macOS only, only when Syphon is vendored)
//
// build.rs emits `--cfg has_syphon` when it finds vendor/Syphon-Framework/Syphon.framework
// and compiles native/syphon_bridge.m. This keeps macOS builds working even when Syphon
// is not present.
// -------------------------------
#[cfg(all(target_os = "macos", has_syphon))]
extern "C" {
    fn syphon_server_create(name_utf8: *const i8) -> *mut std::ffi::c_void;
    fn syphon_server_publish_texture(
        server_ptr: *mut std::ffi::c_void,
        tex_id: u32,
        width: i32,
        height: i32,
    );
    fn syphon_server_destroy(server_ptr: *mut std::ffi::c_void);
}

#[cfg(all(target_os = "macos", has_syphon))]
struct SyphonServer {
    ptr: *mut std::ffi::c_void,
}

#[cfg(all(target_os = "macos", has_syphon))]
impl SyphonServer {
    fn new(name: &str) -> Option<Self> {
        let c = CString::new(name).ok()?;
        let ptr = unsafe { syphon_server_create(c.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(Self { ptr })
        }
    }

    fn publish_texture(&self, tex_id: u32, w: i32, h: i32) {
        unsafe { syphon_server_publish_texture(self.ptr, tex_id, w, h) };
    }
}

#[cfg(all(target_os = "macos", has_syphon))]
impl Drop for SyphonServer {
    fn drop(&mut self) {
        unsafe { syphon_server_destroy(self.ptr) };
    }
}

/// -------------------------------
/// Spout2 C-ABI bridge (Windows only)
/// -------------------------------
#[cfg(target_os = "windows")]
extern "C" {
    fn spout_init_sender(sender_name_utf8: *const i8, width: i32, height: i32) -> i32;
    fn spout_send_gl_texture(gl_tex_id: u32, width: i32, height: i32, invert: i32) -> i32;
    fn spout_shutdown();
}

#[cfg(target_os = "windows")]
struct SpoutSender {
    invert: bool,
}

#[cfg(target_os = "windows")]
impl SpoutSender {
    fn new(name: &str, w: i32, h: i32, invert: bool) -> Option<Self> {
        let c = CString::new(name).ok()?;
        let ok = unsafe { spout_init_sender(c.as_ptr(), w, h) };
        if ok == 1 {
            Some(Self { invert })
        } else {
            None
        }
    }

    fn send_texture(&self, tex_id: u32, w: i32, h: i32) -> bool {
        let ok = unsafe {
            spout_send_gl_texture(tex_id, w, h, if self.invert { 1 } else { 0 })
        };
        ok == 1
    }
}

#[cfg(target_os = "windows")]
impl Drop for SpoutSender {
    fn drop(&mut self) {
        unsafe { spout_shutdown() };
    }
}

/// -------------------------------
/// FFmpeg stream output (cross-platform)
///
/// This uses CPU readback (glReadPixels) and pipes raw RGBA frames to ffmpeg.
///
/// IMPORTANT (RTSP): by default we *push* to an RTSP server. That means you must
/// have an RTSP server running at the host/port in `stream.rtsp_url` (e.g. MediaMTX),
/// then connect VLC to that URL. ffmpeg itself is not automatically an RTSP server.
/// (ffmpeg protocols docs show publishing to an RTSP server.)
/// -------------------------------

enum StreamMsg {
    Frame(Vec<u8>),
    Stop,
}

struct StreamSender {
    cfg: StreamCfg,
    w: i32,
    h: i32,

    // CPU readback buffer (reused)
    buf_rgba: Vec<u8>,

    // writer thread control
    tx: Option<mpsc::SyncSender<StreamMsg>>,
    worker: Option<thread::JoinHandle<()>>,

    // throttling (avoid sending more frames than requested)
    last_send: Instant,

    warned: bool,
}

impl StreamSender {
    fn new(cfg: StreamCfg) -> Self {
        Self {
            cfg,
            w: 0,
            h: 0,
            buf_rgba: Vec::new(),
            tx: None,
            worker: None,
            last_send: Instant::now(),
            warned: false,
        }
    }

    fn is_enabled(&self) -> bool {
        self.cfg.enabled
    }

    fn ensure_running(&mut self, w: i32, h: i32) {
        if !self.cfg.enabled {
            self.stop();
            return;
        }

        // restart if size changed or not running
        let needs_restart = self.tx.is_none() || self.w != w || self.h != h;
        if !needs_restart {
            return;
        }

        self.stop();
        self.w = w;
        self.h = h;

        let bytes = (w.max(1) as usize) * (h.max(1) as usize) * 4;
        self.buf_rgba.resize(bytes, 0);

        let ffmpeg = self
            .cfg
            .ffmpeg_path
            .clone()
            .unwrap_or_else(|| "ffmpeg".to_string());

        let mut args: Vec<String> = Vec::new();

        // Input: raw RGBA frames via stdin
        args.extend([
            "-hide_banner",
            "-loglevel",
            "warning",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{}x{}", w, h),
            "-r",
            &self.cfg.fps.to_string(),
            "-i",
            "-",
        ].into_iter().map(|s| s.to_string()));

        if self.cfg.vflip {
            args.extend(["-vf", "vflip"].into_iter().map(|s| s.to_string()));
        }

        // Encode: H.264 low-latency
        args.extend([
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-tune",
            "zerolatency",
            "-pix_fmt",
            "yuv420p",
            "-g",
            &self.cfg.gop.to_string(),
            "-b:v",
            &format!("{}k", self.cfg.bitrate_kbps),
        ].into_iter().map(|s| s.to_string()));

        match self.cfg.target {
            StreamTarget::Rtsp => {
                // Push to an RTSP server (e.g. MediaMTX).
                // ffmpeg protocols docs: `ffmpeg -re -i input -f rtsp ... rtsp://server/live.sdp`
                args.extend([
                    "-f",
                    "rtsp",
                    "-rtsp_transport",
                    "tcp",
                    "-muxdelay",
                    "0.1",
                ].into_iter().map(|s| s.to_string()));
                args.push(self.cfg.rtsp_url.clone());

                if !self.warned {
                    logi!("OUTPUT", "RTSP mode is PUSH: you need an RTSP server running at {} (e.g. MediaMTX), then open that URL in VLC.", self.cfg.rtsp_url);logi!("OUTPUT", "If no RTSP server is running, ffmpeg can block while connecting and you won't see a stream in VLC.");self.warned = true;
                }
            }
            StreamTarget::Rtmp => {
                let Some(url) = self.cfg.rtmp_url.clone() else {
                    if !self.warned {
                        logi!("OUTPUT", "target=rtmp but rtmp_url is missing in output.json.");self.warned = true;
                    }
                    return;
                };
                // Most platforms expect FLV over RTMP.
                args.extend(["-f", "flv"].into_iter().map(|s| s.to_string()));
                args.push(url);
            }
        }

        let (tx, rx) = mpsc::sync_channel::<StreamMsg>(2);

        let worker = std::thread::Builder::new().name("stream".to_string()).spawn(move || {
            let mut cmd = Command::new(ffmpeg);
            cmd.args(&args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    logi!("OUTPUT", "Failed to start ffmpeg: {}", e);logi!("OUTPUT", "Tip: install ffmpeg or set stream.ffmpeg_path in output.json");return;
                }
            };

            
            // Pipe ffmpeg output through ShadeCore logging so everything is timestamped/tagged.
            if let Some(out) = child.stdout.take() {
                crate::logging::spawn_pipe_thread("ffmpeg_stream_out", "FFMPEG_STREAM", out, false);
            }
            if let Some(err) = child.stderr.take() {
                crate::logging::spawn_pipe_thread("ffmpeg_stream_err", "FFMPEG_STREAM", err, true);
            }

let Some(mut stdin) = child.stdin.take() else {
                logi!("OUTPUT", "Failed to open ffmpeg stdin.");let _ = child.kill();
                let _ = child.wait();
                return;
            };

            logi!("OUTPUT", "ffmpeg started ({}x{}, writing frames)", w, h);// Writer loop. If ffmpeg is blocked connecting (e.g. no RTSP server),
            // writes may block — but this is on a background thread so the UI won't freeze.
            while let Ok(msg) = rx.recv() {
                match msg {
                    StreamMsg::Frame(frame) => {
                        if let Err(e) = stdin.write_all(&frame) {
                            logi!("OUTPUT", "ffmpeg stdin write failed: {}", e);break;
                        }
                    }
                    StreamMsg::Stop => {
                        break;
                    }
                }
            }

            // Cleanup
            let _ = child.kill();
            let _ = child.wait();
            logi!("OUTPUT", "ffmpeg stopped");}).expect("spawn stream thread");

        self.tx = Some(tx);
        self.worker = Some(worker);
        self.last_send = Instant::now();
        // reset warn once per start
        // (warned flag is used for config warnings; keep current value)
    }

    fn send_current_fbo_frame(
        &mut self,
        gl: &glow::Context,
        fbo: glow::NativeFramebuffer,
        w: i32,
        h: i32,
    ) {
        if !self.cfg.enabled {
            return;
        }

        self.ensure_running(w, h);
        let Some(tx) = self.tx.as_ref() else { return; };

        // Throttle to configured fps.
        let interval = Duration::from_secs_f64(1.0 / self.cfg.fps.max(1) as f64);
        if self.last_send.elapsed() < interval {
            return;
        }
        self.last_send = Instant::now();

        // Read back RGBA from the render target FBO.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.read_pixels(
                0,
                0,
                w,
                h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(self.buf_rgba.as_mut_slice())),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }

        // Copy bytes into an owned frame for the worker thread.
        // (Keeping it simple + safe; performance can be optimized later.)
        let frame = self.buf_rgba.clone();

        // Non-blocking send: drop frames if the worker is behind (prevents UI stalls).
        if tx.try_send(StreamMsg::Frame(frame)).is_err() {
            // drop frame
        }
    }

    fn stop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.try_send(StreamMsg::Stop);
        }

        // Do NOT join here (worker may be blocked in IO in bad network situations).
        // It will exit once ffmpeg unblocks or is killed by OS on process exit.
        self.worker.take();
    }
}

impl Drop for StreamSender {
    fn drop(&mut self) {
        self.stop();
    }
}



/// -------------------------------
/// NDI output (optional, feature-gated)
///
/// Uses CPU readback (glReadPixels) and publishes frames as an NDI source for OBS.
/// Build with: `cargo run --features ndi`
///
/// Notes:
/// - We send BGRA (common NDI format). OpenGL readback gives RGBA, so we swizzle.
/// - We optionally vflip because OpenGL readback is typically upside-down.
/// -------------------------------

#[cfg(feature = "ndi")]
mod ndi_out {
    use super::*;

    use grafton_ndi::{
        LineStrideOrSize, NDI, PixelFormat, ScanType, Sender, SenderOptions, VideoFrame,
    };

    enum NdiMsg {
        Frame { bgra: Vec<u8>, w: i32, h: i32 },
        Stop,
    }

    pub struct NdiSender {
        cfg: NdiCfg,
        w: i32,
        h: i32,

        // CPU buffers (reused)
        buf_rgba: Vec<u8>,
        buf_bgra: Vec<u8>,

        tx: Option<mpsc::SyncSender<NdiMsg>>,
        worker: Option<thread::JoinHandle<()>>,
        last_send: Instant,
        warned: bool,
    }

    impl NdiSender {
        pub fn new(cfg: NdiCfg) -> Self {
            Self {
                cfg,
                w: 0,
                h: 0,
                buf_rgba: Vec::new(),
                buf_bgra: Vec::new(),
                tx: None,
                worker: None,
                last_send: Instant::now(),
                warned: false,
            }
        }

        pub fn is_enabled(&self) -> bool {
            self.cfg.enabled
        }

        fn fps_f64(&self) -> f64 {
            let n = self.cfg.fps_n.max(1) as f64;
            let d = self.cfg.fps_d.max(1) as f64;
            n / d
        }

        pub fn ensure_running(&mut self, w: i32, h: i32) {
            if !self.cfg.enabled {
                self.stop();
                return;
            }

            let needs_restart = self.tx.is_none() || self.w != w || self.h != h;
            if !needs_restart {
                return;
            }

            self.stop();
            self.w = w;
            self.h = h;

            let bytes = (w.max(1) as usize) * (h.max(1) as usize) * 4;
            self.buf_rgba.resize(bytes, 0);
            self.buf_bgra.resize(bytes, 0);

            let (tx, rx) = mpsc::sync_channel::<NdiMsg>(2);

            let cfg = self.cfg.clone();
            let name = cfg
                .name
                .clone()
                .unwrap_or_else(|| "shadecore".to_string());
            let groups = cfg.groups.clone();
            let w0 = w;
            let h0 = h;

            let handle = std::thread::Builder::new().name("ndi".to_string()).spawn(move || {
                let ndi = match NDI::new() {
                    Ok(v) => v,
                    Err(e) => {
                        logw!("OUTPUT", "Failed to init NDI: {e:?}");return;
                    }
                };

                let mut builder = SenderOptions::builder(&name);
                if let Some(g) = groups.as_deref() {
                    builder = builder.groups(g);
                }
                builder = builder.clock_video(cfg.clock_video);
                let opts = builder.build();

                let sender = match Sender::new(&ndi, &opts) {
                    Ok(s) => s,
                    Err(e) => {
                        logw!("OUTPUT", "Failed to create sender: {e:?}");return;
                    }
                };

                logi!("OUTPUT", "Sender started: {}", name);logi!("OUTPUT", "NDI enabled. Receiver: OBS via DistroAV/OBS-NDI should see source \"{}\".", name);

                // Pre-build a frame shell we can reuse and just swap data/stride each time.
                // We'll still rebuild if resolution changes (it shouldn't while running).
                let mut frame_shell = VideoFrame::builder()
                    .resolution(w0, h0)
                    .pixel_format(PixelFormat::BGRA)
                    .frame_rate(cfg.fps_n.max(1), cfg.fps_d.max(1))
                    .aspect_ratio((w0 as f32) / (h0.max(1) as f32))
                    .scan_type(ScanType::Progressive)
                    .build()
                    .expect("VideoFrame::build failed");

                loop {
                    match rx.recv() {
                        Ok(NdiMsg::Frame { bgra, w, h }) => {
                            if w != frame_shell.width || h != frame_shell.height {
                                // Should not happen with our restart logic, but be safe.
                                frame_shell = VideoFrame::builder()
                                    .resolution(w, h)
                                    .pixel_format(PixelFormat::BGRA)
                                    .frame_rate(cfg.fps_n.max(1), cfg.fps_d.max(1))
                                    .aspect_ratio((w as f32) / (h.max(1) as f32))
                                    .scan_type(ScanType::Progressive)
                                    .build()
                                    .expect("VideoFrame::build failed");
                            }

                            frame_shell.data = bgra;
                            frame_shell.line_stride_or_size =
                                LineStrideOrSize::LineStrideBytes(w.saturating_mul(4));
                            sender.send_video(&frame_shell);
                        }
                        Ok(NdiMsg::Stop) | Err(_) => break,
                    }
                }

                logi!("OUTPUT", "Sender stopped");}).expect("spawn ndi thread");

            self.tx = Some(tx);
            self.worker = Some(handle);
            self.warned = false;
            self.last_send = Instant::now();
        }

        fn rgba_to_bgra(&mut self, w: i32, h: i32) {
            let w = w.max(1) as usize;
            let h = h.max(1) as usize;
            let n = w * h;
            let src = &self.buf_rgba;
            let dst = &mut self.buf_bgra;

            for i in 0..n {
                let si = i * 4;
                let r = src[si + 0];
                let g = src[si + 1];
                let b = src[si + 2];
                let a = src[si + 3];
                dst[si + 0] = b;
                dst[si + 1] = g;
                dst[si + 2] = r;
                dst[si + 3] = a;
            }
        }

        fn vflip_inplace(buf: &mut [u8], w: i32, h: i32) {
            let w = w.max(1) as usize;
            let h = h.max(1) as usize;
            let row = w * 4;
            for y in 0..(h / 2) {
                let a0 = y * row;
                let b0 = (h - 1 - y) * row;
                for x in 0..row {
                    buf.swap(a0 + x, b0 + x);
                }
            }
        }

        pub fn send_current_fbo_frame(
            &mut self,
            gl: &glow::Context,
            fbo: glow::NativeFramebuffer,
            w: i32,
            h: i32,
        ) {
            if !self.cfg.enabled {
                return;
            }

            self.ensure_running(w, h);
            let Some(tx0) = self.tx.as_ref() else { return; };
            let tx = tx0.clone();

            // simple rate-limit
            let target_dt = Duration::from_secs_f64(1.0 / self.fps_f64().max(1.0));
            if self.last_send.elapsed() < target_dt {
                return;
            }
            self.last_send = Instant::now();

            unsafe {
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(fbo));
                gl.read_pixels(
                    0,
                    0,
                    w,
                    h,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    // glow 0.16 expects an Option<&mut [u8]> here.
                    glow::PixelPackData::Slice(Some(self.buf_rgba.as_mut_slice())),
                );
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            }

            if self.cfg.vflip {
                Self::vflip_inplace(&mut self.buf_rgba, w, h);
            }

            self.rgba_to_bgra(w, h);

            // Copy out for the worker (bounded channel keeps it from piling up).
            let frame = self.buf_bgra.clone();
            if tx.try_send(NdiMsg::Frame { bgra: frame, w, h }).is_err() && !self.warned {
                self.warned = true;
                logw!("OUTPUT", "Dropping frames (sender busy). Consider lowering fps or resolution.");}
        }

        pub fn stop(&mut self) {
            if let Some(tx) = self.tx.take() {
                let _ = tx.try_send(NdiMsg::Stop);
            }
            if let Some(h) = self.worker.take() {
                let _ = h.join();
            }
        }
    }

    impl Drop for NdiSender {
        fn drop(&mut self) {
            self.stop();
        }
    }
}

#[cfg(not(feature = "ndi"))]
mod ndi_out {
    use super::*;

    pub struct NdiSender;
    impl NdiSender {
        pub fn new(_cfg: NdiCfg) -> Self {
            Self
        }
        pub fn is_enabled(&self) -> bool {
            false
        }
        pub fn send_current_fbo_frame(
            &mut self,
            _gl: &glow::Context,
            _fbo: glow::NativeFramebuffer,
            _w: i32,
            _h: i32,
        ) {
        }
        pub fn stop(&mut self) {}
    }
}

/// -------------------------------
/// FBO render target
/// -------------------------------
#[derive(Debug)]
struct RenderTarget {
    fbo: glow::NativeFramebuffer,
    tex: glow::NativeTexture,
    w: i32,
    h: i32,
}

unsafe fn create_render_target(gl: &glow::Context, w: i32, h: i32) -> RenderTarget {
    let tex = gl.create_texture().expect("create_texture failed");
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);

    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA as i32,
        w,
        h,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(None),
    );
    gl.bind_texture(glow::TEXTURE_2D, None);

    let fbo = gl.create_framebuffer().expect("create_framebuffer failed");
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(tex),
        0,
    );

    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    if status != glow::FRAMEBUFFER_COMPLETE {
        panic!("FBO incomplete: 0x{:x}", status);
    }
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);

    RenderTarget { fbo, tex, w, h }
}

unsafe fn resize_render_target(gl: &glow::Context, rt: &mut RenderTarget, w: i32, h: i32) {
    if w == rt.w && h == rt.h {
        return;
    }
    rt.w = w;
    rt.h = h;

    gl.bind_texture(glow::TEXTURE_2D, Some(rt.tex));
    gl.tex_image_2d(
        glow::TEXTURE_2D,
        0,
        glow::RGBA as i32,
        w,
        h,
        0,
        glow::RGBA,
        glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(None),
    );
    gl.bind_texture(glow::TEXTURE_2D, None);
}

// Convert glow::NativeTexture -> OpenGL texture name (u32)
fn tex_id_u32(tex: glow::NativeTexture) -> u32 {
    tex.0.get()
}



fn connect_midi(midi: &MidiGlobalCfg, store: Arc<Mutex<ParamStore>>) -> Option<midir::MidiInputConnection<()>> {
    let mut midi_in = MidiInput::new("shadecore-midi").ok()?;
    midi_in.ignore(Ignore::None);

    let ports = midi_in.ports();
    if ports.is_empty() {
        logi!("MIDI", "No MIDI input ports detected.");return None;
    }

    let preferred = midi
        .preferred_device_contains
        .as_ref()
        .map(|s| s.to_lowercase());

    let mut chosen = ports.get(0).cloned();

    if let Some(pref) = preferred {
        for p in &ports {
            if let Ok(name) = midi_in.port_name(p) {
                if name.to_lowercase().contains(&pref) {
                    chosen = Some(p.clone());
                    break;
                }
            }
        }
    }

    let in_port = chosen?;
    let port_name = midi_in.port_name(&in_port).unwrap_or_else(|_| "Unknown".into());
    logi!("MIDI", "Connecting input: {}", port_name);let conn = midi_in.connect(
        &in_port,
        "shadecore-midi-in",
        move |_ts, msg, _| {
            if msg.len() == 3 && (msg[0] & 0xF0) == 0xB0 {
                let ch = msg[0] & 0x0F;
                let cc = msg[1];
                let val = msg[2];

                // Debug logging: print the first N CC messages, and always print unmapped CCs.
                static MIDI_LOG_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let n = MIDI_LOG_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let mut mapped = false;
                if let Ok(mut s) = store.lock() {
                    mapped = s.set_cc(ch, cc, val);
                }

                if n < 80 || !mapped {
                    logi!("MIDI", "ch={} cc={} val={} mapped={}", ch, cc, val, mapped);
                }
                          }
        },
        (),
    );

    match conn {
        Ok(c) => Some(c),
        Err(e) => {
            logi!("MIDI", "Failed to connect MIDI input: {e}");None
        }
    }
}


/// -------------------------------
/// OSC input (UDP)
/// -------------------------------
struct OscHandle {
    stop_tx: crossbeam_channel::Sender<()>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for OscHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn connect_osc(rt: Arc<RwLock<OscRuntime>>, store: Arc<Mutex<ParamStore>>) -> Option<OscHandle> {
    let osc_cfg = { rt.read().ok().map(|g| g.cfg.clone()).unwrap_or_default() };
    if !osc_cfg.enabled {
        return None;
    }

    let bind = osc_cfg.bind.clone();
    let prefix = osc_cfg.prefix.clone();
    let normalized = osc_cfg.normalized;

    let sock = match UdpSocket::bind(&bind) {
        Ok(s) => s,
        Err(e) => {
            logi!("OSC", "Failed to bind {bind}: {e}");return None;
        }
    };

    let _ = sock.set_nonblocking(true);

    logi!("OSC", "listening on {bind} prefix={prefix} normalized={normalized}");let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);

    let join = std::thread::Builder::new().name("osc".to_string()).spawn(move || {
        let mut buf = [0u8; 2048];
        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }

            match sock.recv_from(&mut buf) {
                Ok((sz, from)) => {
                    let pkt = match rosc::decoder::decode_udp(&buf[..sz]) {
                        Ok((_rest, p)) => p,
                        Err(_e) => continue,
                    };

                    /// Handle a single OSC packet.
///
/// ShadeCore supports two styles of OSC control:
/// - **Normalized**: `/prefix/param/<name>` with a float in 0..1 that is mapped via `(min,max)`.
/// - **Raw**:        `/prefix/raw/<name>` with a float that is used directly.
///
/// In addition, optional *introspection* endpoints can be enabled (see
/// `osc_introspection_helpers.rs`) so controllers can discover params/mappings at runtime.
fn handle_packet(pkt: OscPacket, store: &Arc<Mutex<ParamStore>>, rt: &OscRuntime, sock: &UdpSocket, from: std::net::SocketAddr) {
                        match pkt {
                            OscPacket::Message(msg) => {
                                let addr = msg.addr;
                                let args = msg.args;
                                
// OSC introspection (list/get/mappings). If handled, stop further processing.
if crate::osc_introspection_helpers::osc_try_introspect(
    &rt.cfg.prefix,
    &addr,
    store,
    sock,
    from,
) {
    return;
}

if let Ok(mut s) = store.lock() {
                                    if let Some((name, target, used_norm)) = s.apply_osc_runtime(rt, &addr, args.as_slice()) {
                                        let mode = if used_norm { "NORM" } else { "RAW" };
                                        logi!("OSC", "{mode} {addr} -> {name} target={target}");}
                                }
                            }
                            OscPacket::Bundle(b) => {
                                for p in b.content {
                                    handle_packet(p, store, rt, sock, from);
                                }
                            }
                        }
                    }

                    if let Ok(rt_guard) = rt.read() { handle_packet(pkt, &store, &*rt_guard, &sock, from); }
                }
                Err(_e) => {
                    // no data
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
        logi!("OSC", "stopped");}).expect("spawn osc thread");

    Some(OscHandle { stop_tx, join: Some(join) })
}


unsafe fn compile_program(gl: &glow::Context, vert_src: &str, frag_src: &str) -> glow::NativeProgram {
    let vs = gl.create_shader(glow::VERTEX_SHADER).expect("create_shader failed");
    gl.shader_source(vs, vert_src);
    gl.compile_shader(vs);
    if !gl.get_shader_compile_status(vs) {
        panic!("Vertex shader compile error:\n{}", gl.get_shader_info_log(vs));
    }

    let fs = gl.create_shader(glow::FRAGMENT_SHADER).expect("create_shader failed");
    gl.shader_source(fs, frag_src);
    gl.compile_shader(fs);
    if !gl.get_shader_compile_status(fs) {
        panic!("Fragment shader compile error:\n{}", gl.get_shader_info_log(fs));
    }

    let program = gl.create_program().expect("create_program failed");
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        panic!("Program link error:\n{}", gl.get_program_info_log(program));
    }

    gl.detach_shader(program, vs);
    gl.detach_shader(program, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);

    program
}

unsafe fn try_compile_program(gl: &glow::Context, vert_src: &str, frag_src: &str) -> anyhow::Result<glow::NativeProgram> {
    let vs = gl.create_shader(glow::VERTEX_SHADER).map_err(|e| anyhow::anyhow!("create vertex shader: {e}"))?;
    gl.shader_source(vs, vert_src);
    gl.compile_shader(vs);
    if !gl.get_shader_compile_status(vs) {
        let log = gl.get_shader_info_log(vs);
        gl.delete_shader(vs);
        return Err(anyhow::anyhow!("Vertex shader compile error:\n{log}"));
    }

    let fs = gl.create_shader(glow::FRAGMENT_SHADER).map_err(|e| anyhow::anyhow!("create fragment shader: {e}"))?;
    gl.shader_source(fs, frag_src);
    gl.compile_shader(fs);
    if !gl.get_shader_compile_status(fs) {
        let log = gl.get_shader_info_log(fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        return Err(anyhow::anyhow!("Fragment shader compile error:\n{log}"));
    }

    let program = gl.create_program().map_err(|e| anyhow::anyhow!("create program: {e}"))?;
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);

    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.detach_shader(program, vs);
        gl.detach_shader(program, fs);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        gl.delete_program(program);
        return Err(anyhow::anyhow!("Program link error:\n{log}"));
    }

    gl.detach_shader(program, vs);
    gl.detach_shader(program, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);

    Ok(program)
}


// NOTE: glow uniform calls are unsafe in your build; wrap them here.
//
// We support multiple common uniform naming conventions so "random .frag packs"
// work out of the box across ShaderToy/ISF-ish ports, etc.
fn set_u_resolution(gl: &glow::Context, prog: glow::NativeProgram, w: i32, h: i32) {
    unsafe {
        // shadecore default / legacy
        if let Some(loc) = gl.get_uniform_location(prog, "u_resolution") {
            gl.uniform_2_f32(Some(&loc), w as f32, h as f32);
        }
        // camelCase variant (common in some packs)
        if let Some(loc) = gl.get_uniform_location(prog, "uResolution") {
            gl.uniform_2_f32(Some(&loc), w as f32, h as f32);
        }
        // ShaderToy-style
        if let Some(loc) = gl.get_uniform_location(prog, "iResolution") {
            gl.uniform_3_f32(Some(&loc), w as f32, h as f32, 1.0);
        }
    }
}



fn set_u_src_resolution(gl: &glow::Context, prog: glow::NativeProgram, w: i32, h: i32) {
    unsafe {
        if let Some(loc) = gl.get_uniform_location(prog, "u_src_resolution") {
            gl.uniform_2_f32(Some(&loc), w as f32, h as f32);
        }
        if let Some(loc) = gl.get_uniform_location(prog, "uSrcResolution") {
            gl.uniform_2_f32(Some(&loc), w as f32, h as f32);
        }
        if let Some(loc) = gl.get_uniform_location(prog, "iResolution_src") {
            gl.uniform_3_f32(Some(&loc), w as f32, h as f32, 1.0);
        }
    }
}

fn set_u_scale_mode(gl: &glow::Context, prog: glow::NativeProgram, mode: i32) {
    unsafe {
        for name in ["u_scale_mode", "uScaleMode"] {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_1_i32(Some(&loc), mode);
            }
        }
    }
}
fn set_u_time(gl: &glow::Context, prog: glow::NativeProgram, t: f32) {
    unsafe {
        for name in ["u_time", "uTime", "iTime", "time"] {
            if let Some(loc) = gl.get_uniform_location(prog, name) {
                gl.uniform_1_f32(Some(&loc), t);
            }
        }
    }
}

// Render selection is now defined in shadecore-engine (single source of truth).
type RenderSel = shadecore_engine::config::RenderSelection;


/// Optional render config (assets/render.json) for hot-swapping shaders without changing code.
/// If the file is missing or invalid, we fall back to defaults.
///
/// Example:
/// {
///   "frag": "shaders/shader_kaleido.frag",
///   "present_frag": "shaders/present.frag"
/// }
#[derive(Debug, Clone, serde::Deserialize)]

/// --------------------------------
/// render.json schema (shader selection)
/// --------------------------------
///
/// This file answers the question: **"which fragment shader(s) should be used right now?"**
///
/// - `frag` selects a single fragment shader path.
/// - `frag_variants` (optional) defines a list of fragment shaders you can cycle through.
/// - `active_frag` (optional) selects which entry in `frag_variants` starts active.
/// - `frag_profile_map` (optional) links a fragment shader to a params profile name from `params.json`
///   so the engine can auto-apply per-shader MIDI/OSC/uniform defaults.
///
/// This file does **not** define MIDI/OSC mappings, uniform ranges, or output routing.
struct RenderJson {
    #[serde(default)]
    frag: Option<String>,

    /// Optional list of fragment shader variants.
    /// Example: { "frag_variants": ["shaders/a.frag", "shaders/b.frag"] }
    #[serde(default)]
    frag_variants: Option<Vec<String>>,

    /// Optional active fragment selection by exact string match against entries in `frag_variants`.
    #[serde(default)]
    active_frag: Option<String>,

    /// Optional mapping from frag variant string -> params profile name.
    /// Example:
    /// { "frag_profile_map": { "shaders/a.frag": "lofi", "shaders/b.frag": "crunch" } }
    #[serde(default)]
    frag_profile_map: Option<std::collections::HashMap<String, String>>,

    #[serde(default)]
    present_frag: Option<String>,
}

fn resolve_assets_path(assets: &std::path::Path, s: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(s);
    if p.is_absolute() {
        p
    } else {
        assets.join(p)
    }
}

// (moved to shadecore-engine::config::load_render_selection)

fn file_mtime(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

enum AppEvent {
    ConfigChanged,
}

fn main() {
    
    // --- Logging init (audit-friendly) ---------------------------------------------
    // Optional: --log-file <path> (append) or env SHADECORE_LOG_FILE
    let mut log_file: Option<std::path::PathBuf> = None;
    {
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            if a == "--log-file" {
                if let Some(p) = it.next() {
                    log_file = Some(std::path::PathBuf::from(p));
                }
            }
        }
        if log_file.is_none() {
            if let Ok(p) = std::env::var("SHADECORE_LOG_FILE") {
                if !p.trim().is_empty() {
                    log_file = Some(std::path::PathBuf::from(p));
                }
            }
        }
    }
    let run_id = crate::logging::init(log_file);
    logi!("INIT", "run_id={run_id}");

    let eng_cfg = load_engine_config_from(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap_or_else(|e| {
        eprintln!("ShadeCore init error: {e}");
        std::process::exit(1);
    });

    let assets_root = eng_cfg.assets.clone();
    let assets = eng_cfg.paths.assets_dir.clone();

    let render_cfg_path = eng_cfg.paths.render_json.clone();
    let mut render_sel = eng_cfg.render.clone();
                                                                                    let _ = &render_sel;
let _ = &render_sel;
let mut frag_variants = render_sel.frag_variants.clone();
    let mut frag_profile_map = render_sel.frag_profile_map.clone();
    let mut frag_variant_idx = render_sel.frag_idx;
    let mut frag_path = render_sel.frag_path.clone();
    let mut present_frag_path = render_sel.present_frag_path.clone();
    let params_path = eng_cfg.params.path.clone();
    let output_cfg_path = eng_cfg.output.path.clone();

    logi!("INIT", "assets base: {}", assets.display());
    logi!("INIT", "assets render.json: {}", render_cfg_path.display());
    logi!("INIT", "active shader: {}", frag_path.display());
    logi!("INIT", "present shader: {}", present_frag_path.display());
    logi!("INIT", "assets params.json: {}", params_path.display());
    logi!("INIT", "assets output.json: {}", output_cfg_path.display());
    let recording_cfg_path = eng_cfg.recording.path.clone();
    logi!("INIT", "assets recording.json: {}", recording_cfg_path.display());


    let frag_src = read_to_string(&frag_path);
    let present_frag_src = read_to_string(&present_frag_path);

    // Keep the raw params.json text around for validation + error reporting.
    let params_src = eng_cfg.params.src.clone();

    let mut pf: ParamsFile = serde_json::from_str(&params_src)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {e}", params_path.display()));
    logi!("PARAMS", "loaded version {}", pf.version);

    // Validate params.json relationships (profiles, uniform names, active selections)
    {
        let v: serde_json::Value = serde_json::from_str(&params_src).unwrap_or(serde_json::Value::Null);
        let issues = crate::validate::validate_params_json(&v);
        crate::validate::emit_summary("CONFIG", "params.json", &issues);
        crate::validate::emit_issues("CONFIG", &issues);
    }

    // Choose an initial active profile:
    // 1) explicit active_profile
    // 2) "default" if present
    // 3) first profile (sorted) if present
    let mut active_profile: Option<String> =
        pick_active_profile_for_shader(&pf, &assets, &frag_path);
    if let Some(p) = &active_profile {
        logi!("PARAMS", "active profile: {p}");}

    let store = Arc::new(Mutex::new(ParamStore::new(&pf)));

    // Apply the active params profile (which can also override MIDI settings / CC mapping).
    let mut effective_midi = pf.midi.clone();
    if let Some(p) = active_profile.as_deref() {
        effective_midi = store.lock().unwrap().apply_profile(&pf, &assets, Some(&frag_path), p);
                                                                                    let _ = &effective_midi;
let _ = &effective_midi;
}

let mut profile_hotkeys = build_profile_hotkey_map(&pf);
    let mut profile_names = sorted_profile_names_for_shader(&pf, &assets, &frag_path);


    let event_loop = EventLoopBuilder::<AppEvent>::with_user_event().build().expect("EventLoop::with_user_event failed");
let event_proxy = event_loop.create_proxy();

// Watch config files and auto-reload when they change.
// This makes JSON edits dynamic without rebuilding or restarting.
{
    use std::ffi::OsStr;
    use std::time::Duration;

    let assets_dir_for_watch = assets.clone();
    let proxy_for_watch = event_proxy.clone();

    let _watcher_thread = std::thread::Builder::new().name("watcher".to_string()).spawn(move || {
        use notify::{RecursiveMode, Watcher};

        let interesting: [&OsStr; 4] = [
            OsStr::new("recording.json"),
            OsStr::new("recording.profiles.json"),
            OsStr::new("render.json"),
            OsStr::new("params.json"),
        ];

        let mut watcher = match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            match res {
                Ok(ev) => {
                    // Editors often emit multiple events (modify/create/remove/rename).
                    use notify::EventKind;
                    let kind_ok = matches!(ev.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_));

                    if !kind_ok {
                        return;
                    }

                    // Watch the directory, then filter by filename so "atomic save" (rename) is handled.
                    let hit = ev.paths.iter().any(|p| {
                        // accept any .frag change (shader hot-reload), and a few JSON configs
                        if p.extension().and_then(|e| e.to_str()) == Some("frag") {
                            return true;
                        }
                        p.file_name()
                            .is_some_and(|name| interesting.iter().any(|want| name == *want))
                    });

                    if hit {
                        // Helpful: print what changed (best-effort).
                        if let Some(p) = ev.paths.get(0) {
                            logi!("WATCH", "change detected: {} (because file system event)", p.display());
                        } else {
                            logi!("WATCH", "change detected (because file system event)");
                        }
                        let _ = proxy_for_watch.send_event(AppEvent::ConfigChanged);
                    }
                }
                Err(e) => logw!("WATCH", "notify error: {e}"),
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                loge!("WATCH", "failed to create watcher: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&assets_dir_for_watch, RecursiveMode::NonRecursive) {
            loge!("WATCH", "failed to watch assets dir {}: {e}", assets_dir_for_watch.display());
            return;
        }
        let shaders_dir = assets_dir_for_watch.join("shaders");
        if shaders_dir.is_dir() {
            if let Err(e) = watcher.watch(&shaders_dir, RecursiveMode::NonRecursive) {
                logw!("WATCH", "failed to watch shaders dir {}: {e}", shaders_dir.display());
                // not fatal; we can still watch assets/
            }
        }


        // keep thread alive
        loop { std::thread::sleep(Duration::from_secs(3600)); }
    }).expect("spawn watcher thread");
}
    let window_builder = winit::window::WindowBuilder::new()
        .with_title("shadecore")
        .with_inner_size(PhysicalSize::new(1280, 720));

    let template = ConfigTemplateBuilder::new().with_alpha_size(8).with_depth_size(0);
    let display_builder = DisplayBuilder::new().with_window_builder(Some(window_builder));

    let (window, gl_config) = display_builder
        .build(&event_loop, template, |configs| {
            configs
                .reduce(|a, b| if a.num_samples() > b.num_samples() { a } else { b })
                .unwrap()
        })
        .expect("Failed to build display");

    let window = window.expect("No window created");

    let raw_window_handle = window.raw_window_handle();
    let gl_display = gl_config.display();

    let context_attributes = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
        .build(Some(raw_window_handle));

    let not_current_gl_context: NotCurrentContext = unsafe {
        gl_display
            .create_context(&gl_config, &context_attributes)
            .expect("create_context failed")
    };

    let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
        window.raw_window_handle(),
        NonZeroU32::new(1280).unwrap(),
        NonZeroU32::new(720).unwrap(),
    );

    let gl_surface = unsafe {
        gl_display
            .create_window_surface(&gl_config, &attrs)
            .expect("create_window_surface failed")
    };

    let gl_context = not_current_gl_context
        .make_current(&gl_surface)
        .expect("make_current failed");

    gl_surface
        .set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()))
        .ok();

    let gl = unsafe {
        glow::Context::from_loader_function(|s| {
            gl_display.get_proc_address(&CString::new(s).unwrap()) as *const _
        })
    };

    let mut program = unsafe { compile_program(&gl, VERT_SRC, &frag_src) };
    let mut present_program = unsafe { compile_program(&gl, VERT_SRC, &present_frag_src) };
    let vao = unsafe { gl.create_vertex_array().expect("create_vertex_array failed") };

    let size = window.inner_size();
    let mut rt = unsafe { create_render_target(&gl, size.width as i32, size.height as i32) };

    let mut midi_conn_in = Some(connect_midi(&effective_midi, store.clone()));
    // keep-alive: the connection must be held to stay active
    let _midi_connected = midi_conn_in.is_some();
let osc_rt = Arc::new(RwLock::new(OscRuntime::new(pf.osc.clone())));
    let _osc_handle = connect_osc(osc_rt.clone(), store.clone());


    let default_mode = if cfg!(target_os = "windows") {
        OutputMode::Spout
    } else if cfg!(target_os = "macos") {
        if cfg!(has_syphon) {
            OutputMode::Syphon
        } else {
            OutputMode::Texture
        }
    } else {
        OutputMode::Texture
    };

    let output_cfg = load_output_config(&output_cfg_path, default_mode);
let recording_cfg = load_recording_config(&recording_cfg_path);
logi!("RECORDING", "loaded: enabled={} size={}x{} fps={} start_keys={:?} stop_keys={:?} toggle_keys={:?} out_dir={} ffmpeg_path={}",
    recording_cfg.enabled,
    recording_cfg.width,
    recording_cfg.height,
    recording_cfg.fps,
    recording_cfg.start_keys,
    recording_cfg.stop_keys,
    recording_cfg.toggle_keys,
    recording_cfg.out_dir.display(),
    recording_cfg.ffmpeg_path
);
let mut recording_hotkeys = build_recording_hotkey_map(&recording_cfg);

    // Render target is defined by recording.json (deterministic output). Preview window just scales this texture.
    unsafe { resize_render_target(&gl, &mut rt, recording_cfg.width as i32, recording_cfg.height as i32); }
    let syphon_name = output_cfg
        .syphon
        .server_name
        .clone()
        .unwrap_or_else(|| "shadecore".to_string());
    let syphon_enabled = output_cfg.syphon.enabled;

    let spout_name = output_cfg
        .spout
        .sender_name
        .clone()
        .unwrap_or_else(|| "shadecore".to_string());
    let spout_enabled = output_cfg.spout.enabled;
    let spout_invert = output_cfg.spout.invert;

    let stream_cfg = output_cfg.stream.clone();
    let stream_enabled = stream_cfg.enabled;

    let ndi_cfg = output_cfg.ndi.clone();
    let ndi_enabled = ndi_cfg.enabled;
    let ndi_name = ndi_cfg
        .name
        .clone()
        .unwrap_or_else(|| "shadecore".to_string());
    let hotkey_map = build_hotkey_map(&output_cfg.hotkeys);
    let preview_hotkey_map = build_preview_hotkey_map(&output_cfg.preview.hotkeys);

    // Presenter is a modular "preview" plugin: WindowPresenter draws the render target into the
    // preview window; NullPresenter does nothing (headless/installation mode).
    let mut presenter: Presenter = if output_cfg.preview.enabled {
        Presenter::Window(WindowPresenter { vao })
    } else {
        logi!("PREVIEW", "disabled (presenter=null) — running render + route only");Presenter::Null(NullPresenter::default())
    };

    // If preview is disabled, hide the window so installs can run "headless" (render + route only).
    // A GL surface/context still exists internally for portability/stability across macOS/Windows.
    if !presenter.is_enabled() {
        window.set_visible(false);
    }

    let mut output_mode = output_cfg.output_mode;

    // Preview scaling mode (presentation only; does NOT affect recording/FBO size)
    // 0=fit (letterbox), 1=fill (crop), 2=stretch, 3=pixel (1:1 centered)
    let mut preview_scale_mode: i32 = output_cfg.preview.scale_mode.as_i32();
    logi!("PREVIEW", "initial scale_mode: {} (mode={})", preview_scale_mode_name(preview_scale_mode), preview_scale_mode);logi!("OUTPUT", "startup mode={:?} | syphon.enabled={} name='{}' | spout.enabled={} name='{}' invert={} | stream.enabled={} target={:?} | ndi.enabled={} name='{}' | preview.scale_mode={}",
        output_mode,
        syphon_enabled,
        syphon_name,
        spout_enabled,
        spout_name,
        spout_invert,
        stream_enabled,
        stream_cfg.target,
        ndi_enabled,
        ndi_name,
        output_cfg.preview.scale_mode.as_str()
    );

    logi!("OUTPUT", "stream.enabled={} target={:?} rtsp_url='{}' rtmp_url={:?} fps={} bitrate_kbps={} gop={} vflip={}",
        stream_enabled,
        stream_cfg.target,
        stream_cfg.rtsp_url,
        stream_cfg.rtmp_url,
        stream_cfg.fps,
        stream_cfg.bitrate_kbps,
        stream_cfg.gop,
        stream_cfg.vflip
    );

    logi!("OUTPUT", "ndi.enabled={} name='{}' groups={:?} fps={}/{} clock_video={} vflip={}",
        ndi_enabled,
        ndi_name,
        ndi_cfg.groups,
        ndi_cfg.fps_n,
        ndi_cfg.fps_d,
        ndi_cfg.clock_video,
        ndi_cfg.vflip
    );

    logi!("INIT", "ready (run_id={})", crate::logging::run_id());

    window.set_title(&format!(
        "shadecore - output: {:?} (press 1=Texture, 2=Syphon, 3=Spout, 4=Stream, 6=NDI)",
        output_mode
    ));

    #[cfg(target_os = "macos")]
    // Syphon is only available on macOS when vendored (build.rs sets `has_syphon`).
    #[cfg(all(target_os = "macos", has_syphon))]
    let mut syphon: Option<SyphonServer> = None;
    #[cfg(not(all(target_os = "macos", has_syphon)))]
    let mut syphon: Option<()> = None;

    #[cfg(target_os = "windows")]
    let mut spout: Option<SpoutSender> = None;

    let mut recorder = Recorder::new(recording_cfg.clone());
    let mut configs_dirty: bool = false;
    let mut pending_reload: bool = false;

    // Hot-reload stamps (best-effort). If missing, we still attempt reload on change events.
    let mut render_cfg_mtime = file_mtime(&render_cfg_path);
    let mut frag_mtime = file_mtime(&frag_path);
    let mut present_frag_mtime = file_mtime(&present_frag_path);
    let mut params_mtime = file_mtime(&params_path);

let mut rec_rt: Option<RenderTarget> = None;
let mut rec_pbos: Option<[glow::NativeBuffer; 2]> = None;
let mut rec_pbo_index: usize = 0;
let mut rec_pbo_primed: bool = false;
let mut rec_pbo_bytes: usize = 0;

let mut stream = StreamSender::new(stream_cfg.clone());
    let mut ndi = ndi_out::NdiSender::new(ndi_cfg.clone());

    let mut warned = false;
    let start = Instant::now();

    event_loop
        .run(move |event, target| {
            target.set_control_flow(ControlFlow::Poll);

            match event {
                Event::UserEvent(AppEvent::ConfigChanged) => {
                    configs_dirty = true;
                }

                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => target.exit(),

                                        WindowEvent::KeyboardInput { event, .. } => {
                        if event.state.is_pressed() && !event.repeat {
                            if let PhysicalKey::Code(code) = event.physical_key {
                                logi!("INPUT", "key pressed: {:?}", code);

                                // --- Profile hotkeys (params.json) ---
                                // ------------------------------ Shader profile switching ------------------------------
// Parameter “profiles” are *per-shader default uniform sets* (e.g. a 'default' vs 'wide' vibe).
// These hotkeys do NOT change MIDI/OSC mappings or min/max ranges — they only select which
// named default-uniform set to seed when the shader is (re)loaded.
// See docs: Profiles Mental Model (docs/_docs/10-profiles-mental-model.md).
if let Some(pact) = profile_hotkeys.get(&code).cloned() {
                                    if profile_names.is_empty() {
                                        logi!("PARAMS", "no profiles defined");} else {
                                        let cur_name = active_profile.clone().unwrap_or_else(|| profile_names[0].clone());
                                        let cur_idx = profile_names.iter().position(|n| n == &cur_name).unwrap_or(0);

                                        let next_name = match pact {
                                            ProfileAction::Next => {
                                                profile_names[(cur_idx + 1) % profile_names.len()].clone()
                                            }
                                            ProfileAction::Prev => {
                                                profile_names[(cur_idx + profile_names.len() - 1) % profile_names.len()].clone()
                                            }
                                            ProfileAction::Set(n) => n,
                                        };

                                        active_profile = Some(next_name.clone());
                                        // Persist the selection in memory (you can also write it back to params.json later if desired)
                                        set_active_profile_for_shader(&mut pf, &assets, &frag_path, &next_name);
                                        pf.active_profile = active_profile.clone();

                                        effective_midi = store.lock().unwrap().apply_profile(&pf, &assets, Some(&frag_path), &next_name);
                                                                                                                        let _ = &effective_midi;
let _ = &effective_midi;
midi_conn_in = Some(connect_midi(&effective_midi, store.clone()));
                                        let _midi_connected = midi_conn_in.is_some();
}
                                }

                                
                                // --- Fragment shader variant hotkeys (render.json) ---
// User requested ; and ' for cycling. On some ISO/UK/IE layouts the physical keycodes
// can differ, so we accept a small alias set.
//
// Next: Quote (')  OR Period (.) OR Backquote (`)
// Prev: Semicolon (;) OR Comma (,) OR IntlBackslash (\)
let is_next = matches!(code, KeyCode::Quote | KeyCode::Period | KeyCode::Backquote);
let is_prev = matches!(code, KeyCode::Semicolon | KeyCode::Comma | KeyCode::IntlBackslash);

if is_next || is_prev {
    if frag_variants.len() <= 1 {
        logi!("RENDER", "no frag_variants (or only one). Add `frag_variants` to render.json to enable cycling.");} else {
        if is_next {
            frag_variant_idx = (frag_variant_idx + 1) % frag_variants.len();
        } else {
            frag_variant_idx = (frag_variant_idx + frag_variants.len() - 1) % frag_variants.len();
        }

        frag_path = frag_variants[frag_variant_idx].clone();

        // When switching shaders, also switch to that shader's active profile (and rebuild MIDI mappings).
        active_profile = pick_active_profile_for_shader(&pf, &assets, &frag_path);
        if let Some(pname) = active_profile.clone() {
            logi!("PARAMS", "shader switch -> profile: {}", pname);set_active_profile_for_shader(&mut pf, &assets, &frag_path, &pname);
            effective_midi = store.lock().unwrap().apply_profile(&pf, &assets, Some(&frag_path), &pname);
                                                                                            let _ = &effective_midi;
let _ = &effective_midi;
midi_conn_in = Some(connect_midi(&effective_midi, store.clone()));
                                        let _midi_connected = midi_conn_in.is_some();
} else {
            logi!("PARAMS", "shader switch -> no profiles found (keeping existing mappings)");}

        // Force shader reload next tick (even if the file didn't change on disk).
        frag_mtime = None;
        configs_dirty = true;

        logi!("RENDER", "frag variant -> {} ({} / {})",
            frag_path.display(),
                                            frag_variant_idx + 1,
                                            frag_variants.len()
                                        );
                                    }
                                }

// --- Recording hotkeys (recording.json) ---
//
// Recording is driven by `assets/recording.json` and its own hotkey map.
// This is intentionally separate from output routing:
// - output routing controls publishing the live texture elsewhere
// - recording controls an ffmpeg capture pipeline + readback resources (PBOs)
//
// Config reload rule: we do *not* live-reload recording settings while a recording
// is active, because it would invalidate PBO sizing / ffmpeg expectations mid-stream.
// If recording.json changes while recording, we defer reload until after stop.
if let Some(action) = recording_hotkeys.get(&code).copied() {
                                    logi!("INPUT", "recording hotkey {:?} -> {:?}", code, action);
                                    match action {
                                        RecHotkeyAction::Toggle => {
                                            if recorder.is_recording() {
                                                recorder.stop();
                                                logi!("STATE", "recording -> stopped (because toggle hotkey)");
                                            } else if recorder.is_enabled() {
                                                match recorder.start(&assets) {
                                                    Ok(p) => {
                                                        rec_pbo_index = 0;
                                                        rec_pbo_primed = false;
                                                        let sid = crate::logging::make_session_id("rec");
                                                        logi!("RECORDING", "recording -> started sid={} path={} (because toggle hotkey)", sid, p.display());
                                                    }
                                                    Err(e) => loge!("ERROR", "recording start failed (because toggle hotkey): {e}"),
                                                }
                                            } else {
                                                logw!("WARN", "recording hotkey ignored (recording disabled; enable in recording.json)");
                                            }
                                        }
                                        RecHotkeyAction::Start => {
                                            if recorder.is_recording() {
                                                logw!("WARN", "recording start ignored (already recording)");
                                            } else if recorder.is_enabled() {
                                                match recorder.start(&assets) {
                                                    Ok(p) => {
                                                        rec_pbo_index = 0;
                                                        rec_pbo_primed = false;
                                                        let sid = crate::logging::make_session_id("rec");
                                                        logi!("RECORDING", "recording -> started sid={} path={} (because start hotkey)", sid, p.display());
                                                    }
                                                    Err(e) => loge!("ERROR", "recording start failed (because start hotkey): {e}"),
                                                }
                                            } else {
                                                logw!("WARN", "recording start ignored (recording disabled; enable in recording.json)");
                                            }
                                        }
                                        RecHotkeyAction::Stop => {
                                            if recorder.is_recording() {
                                                recorder.stop();
                                                logi!("STATE", "recording -> stopped (because stop hotkey)");
                                            } else {
                                                logw!("WARN", "recording stop ignored (not recording)");
                                            }
                                        }
                                    }
                                    return;
                                }

// --- Output routing hotkeys (output.json) ---
//
// `hotkey_map` is built from `assets/output.json` and maps physical keys to an
// `OutputMode`. This only changes *where* the authoritative render texture is
// published (Syphon/Spout/Stream/NDI); it does not change shader params, render
// resolution, or recording configuration.
//
// Side effect note: some modes own external resources (FFmpeg process, NDI sender).
// When switching away, we stop/teardown those resources to avoid dangling processes.

                                let new_mode = hotkey_map.get(&code).copied();
                                if let Some(m) = new_mode {
                                    if output_mode == OutputMode::Stream && m != OutputMode::Stream { stream.stop(); }
                                    if output_mode == OutputMode::Ndi && m != OutputMode::Ndi { ndi.stop(); }
                                    output_mode = m;
                                    warned = false;
                                    logi!(
                                        "STATE",
                                        "output mode -> {:?} (because hotkey {:?})",
                                        output_mode,
                                        code
                                    );
                                    window.set_title(&format!(
                                        "shadecore - output: {:?} (press 1=Texture, 2=Syphon, 3=Spout, 4=Stream, 6=NDI)",
                                        output_mode
                                    ));
                                }
                            }


// --- Preview scaling hotkeys (presentation only; JSON-configurable) ---
if let PhysicalKey::Code(code) = event.physical_key {
    if let Some(pm) = preview_hotkey_map.get(&code).copied() {
        if pm != preview_scale_mode {
            preview_scale_mode = pm;
            let name = preview_scale_mode_name(preview_scale_mode);
            logi!("PREVIEW", "hotkey pressed: {:?} -> {} (mode={})",
                code,
                name,
                preview_scale_mode
            );
        }
    }
}
                        }
                    }

                    WindowEvent::Resized(new_size) => {
                        // Preview window is resizable; render target stays fixed (recording resolution).
                        let w = new_size.width.max(1);
                        let h = new_size.height.max(1);
                        presenter.resize_window_surface(&gl_context, &gl_surface, w, h, |surf, ctx, ww, hh| {
                            unsafe {
                                surf.resize(ctx, NonZeroU32::new(ww).unwrap(), NonZeroU32::new(hh).unwrap());
                            }
                        });
                        window.request_redraw();
                    },

                    WindowEvent::RedrawRequested => unsafe {

// ---------------------------------------------------------------------
// Render tick (winit redraw)
//
// This is the *one place* we do GPU work each frame:
//   1) Apply hot-reload changes (shader/config) if any flags/mtimes changed.
//   2) Update smoothed params/uniform inputs (time, resolution, MIDI/OSC-driven params).
//   3) Render into the authoritative offscreen RenderTarget (FBO texture).
//   4) Publish that texture to the currently-selected output backend (Syphon/Spout/Stream/NDI).
//   5) Present a scaled preview of the same texture to the local window.
//   6) Optionally perform recording readback (PBO ping-pong) without stalling the GPU.
//
// Important: preview window size is *not* the render size. The render size comes from the
// render config / recording config and is the source of truth for outputs and captures.
// ---------------------------------------------------------------------

                        let win_size = window.inner_size();
                        let win_w = win_size.width as i32;
                        let win_h = win_size.height as i32;

// Hot-reload boundary (shader + JSON configs)
//
// We keep hot-reload *outside* the inner render calls:
// - file watcher events set cheap flags (`configs_dirty`)
// - on redraw we check mtimes and do the heavier work (re-parse JSON, recompile program)
//
// This avoids doing I/O or shader compilation inside event callbacks, and keeps the
// render tick as the single consistent place where GL state changes happen.

                        // Authoritative render size (used for uniforms, outputs, and recording).
                        let w = rt.w;
                        let h = rt.h;
if let Ok(mut s) = store.lock() {
                            s.tick();
                        }

                        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(rt.fbo));
                        gl.viewport(0, 0, w, h);
                        gl.clear_color(0.0, 0.0, 0.0, 1.0);
                        gl.clear(glow::COLOR_BUFFER_BIT);

                        gl.use_program(Some(program));
                        gl.bind_vertex_array(Some(vao));

                        set_u_resolution(&gl, program, w, h);

                        if let Ok(s) = store.lock() {
                            for (k, v) in s.values.iter() {
                                if let Some(loc) = gl.get_uniform_location(program, k) {
                                    gl.uniform_1_f32(Some(&loc), *v);
                                }
                            }
                        }

                        let t = start.elapsed().as_secs_f32();
                        set_u_time(&gl, program, t);

                        gl.draw_arrays(glow::TRIANGLES, 0, 3);

                        gl.bind_vertex_array(None);
                        gl.use_program(None);
                        gl.bind_framebuffer(glow::FRAMEBUFFER, None);

                        let tex_id = tex_id_u32(rt.tex);
// ------------------------------------------------------------
// Recording capture (FBO-only) - async PBO readback
// ------------------------------------------------------------
if recorder.is_recording() {
    let rec_w = recorder.cfg().width as i32;
    let rec_h = recorder.cfg().height as i32;

    if rec_w > 0 && rec_h > 0 {
        let needs_new = rec_rt
            .as_ref()
            .map(|r| r.w != rec_w || r.h != rec_h)
            .unwrap_or(true);

        if needs_new {
            if rec_rt.is_none() {
                rec_rt = Some(create_render_target(&gl, rec_w, rec_h));
            } else if let Some(rr) = rec_rt.as_mut() {
                resize_render_target(&gl, rr, rec_w, rec_h);
            }

            // (Re)allocate double PBOs for async readback
            let bytes = (rec_w as usize) * (rec_h as usize) * 4;
            if rec_pbo_bytes != bytes || rec_pbos.is_none() {
                if let Some(pbos) = rec_pbos.take() {
                    gl.delete_buffer(pbos[0]);
                    gl.delete_buffer(pbos[1]);
                }

                let pbo0 = gl.create_buffer().expect("create_buffer failed");
                let pbo1 = gl.create_buffer().expect("create_buffer failed");

                for pbo in [pbo0, pbo1] {
                    gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(pbo));
                    gl.buffer_data_size(
                        glow::PIXEL_PACK_BUFFER,
                        bytes as i32,
                        glow::STREAM_READ,
                    );
                }
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);

                rec_pbos = Some([pbo0, pbo1]);
                rec_pbo_index = 0;
                rec_pbo_primed = false;
                rec_pbo_bytes = bytes;
            }
        }

        if let (Some(rr), Some(pbos)) = (rec_rt.as_ref(), rec_pbos.as_ref()) {
            // Blit from main render target -> record target (scale)
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(rt.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(rr.fbo));
            gl.blit_framebuffer(
                0, 0, w, h,
                0, 0, rec_w, rec_h,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, None);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);

            let write_pbo = pbos[rec_pbo_index];
            let read_pbo = pbos[(rec_pbo_index + 1) & 1];

            // GPU -> PBO
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(rr.fbo));
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(write_pbo));
            gl.read_pixels(
                0,
                0,
                rec_w,
                rec_h,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::BufferOffset(0),
            );
            gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

// -----------------------------------------------------------------
// Recording readback (PBO ping-pong)
//
// We read frames asynchronously using two Pixel Pack Buffers:
// - each frame: issue glReadPixels into "write_pbo" (GPU command)
// - next frame: map "read_pbo" on CPU and feed bytes to ffmpeg
//
// This avoids a hard GPU->CPU sync each frame. If mapping fails or the queue backs up,
// we prefer dropping frames over stalling the render loop.
// -----------------------------------------------------------------
            // CPU: map previous PBO and send to ffmpeg
            if rec_pbo_primed {
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, Some(read_pbo));
                let ptr = gl.map_buffer_range(
                    glow::PIXEL_PACK_BUFFER,
                    0,
                    rec_pbo_bytes as i32,
                    glow::MAP_READ_BIT,
                );
                if !ptr.is_null() {
                    let slice = std::slice::from_raw_parts(
                        ptr as *const u8,
                        rec_pbo_bytes,
                    );
                    recorder.try_send_frame_owned(slice.to_vec());
                    gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                } else {
                    let _ = gl.unmap_buffer(glow::PIXEL_PACK_BUFFER);
                }
                gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
            } else {
                rec_pbo_primed = true;
            }

            rec_pbo_index = (rec_pbo_index + 1) & 1;
        }
    }
}



// -----------------------------------------------------------------
// Output publishing
//
// At this point the shader has rendered into the offscreen RenderTarget texture.
// This switch decides which *external* sink we publish that texture to.
//
// Rule of thumb:
// - Texture: do nothing (preview-only)
// - Syphon/Spout/NDI: publish the GL texture handle through the platform bridge
// - Stream: push CPU frames into an ffmpeg process (requires readback or compatible path)
// -----------------------------------------------------------------
                        match output_mode {
                            OutputMode::Texture => {}

                            OutputMode::Stream => {
                                if !stream.is_enabled() {
                                    if !warned {
                                        logi!("OUTPUT", "Stream requested but disabled in output.json. Falling back to Texture.");warned = true;
                                    }
                                } else {
                                    stream.send_current_fbo_frame(&gl, rt.fbo, w, h);
                                }
                            }


                            OutputMode::Ndi => {
                                if !ndi.is_enabled() {
                                    if !warned {
                                        logi!("OUTPUT", "NDI requested but disabled in output.json (or built without --features ndi). Falling back to Texture.");warned = true;
                                    }
                                } else {
                                    ndi.send_current_fbo_frame(&gl, rt.fbo, w, h);
                                }
                            }

                            OutputMode::Syphon => {
                                #[cfg(all(target_os = "macos", has_syphon))]
                                {
                                    if !syphon_enabled {
                                        if !warned {
                                            logi!("OUTPUT", "Syphon requested but disabled in output.json. Falling back to Texture.");warned = true;
                                        }
                                    } else {
                                        if syphon.is_none() {
                                            syphon = SyphonServer::new(&syphon_name);
                                            if syphon.is_none() && !warned {
                                                logi!("OUTPUT", "Syphon init failed. Falling back to Texture.");warned = true;
                                            }
                                        }
                                        if let Some(ref server) = syphon {
                                            server.publish_texture(tex_id, w, h);
                                        }
                                    }
                                }

                                #[cfg(all(target_os = "macos", not(has_syphon)))]
                                {
                                    if !warned {
                                        logi!("OUTPUT", "Syphon requested but Syphon.framework is not vendored. Falling back to Texture.");warned = true;
                                    }
                                }

                                #[cfg(not(target_os = "macos"))]
                                {
                                    if !warned {
                                        logi!("OUTPUT", "Syphon requested but macOS-only. Falling back to Texture.");warned = true;
                                    }
                                }
                            }

                            OutputMode::Spout => {
                                #[cfg(target_os = "windows")]
                                {
                                    if !spout_enabled {
                                        if !warned {
                                            logi!("OUTPUT", "Spout requested but disabled in output.json. Falling back to Texture.");warned = true;
                                        }
                                    } else {
                                        if spout.is_none() {
                                            spout = SpoutSender::new(&spout_name, w, h, spout_invert);
                                            if spout.is_none() && !warned {
                                                logi!("OUTPUT", "Spout init failed. Falling back to Texture.");warned = true;
                                            }
                                        }
                                        if let Some(ref sender) = spout {
                                            let ok = sender.send_texture(tex_id, w, h);
                                            if !ok && !warned {
                                                logi!("OUTPUT", "Spout send failed. Falling back to Texture.");warned = true;
                                            }
                                        }
                                    }
                                }

                                #[cfg(not(target_os = "windows"))]
                                {
                                    if !warned {
                                        logi!("OUTPUT", "Spout requested but Windows-only. Falling back to Texture.");warned = true;
                                    }
                                }
                            }
                        }

                        presenter.present(
                            &gl,
                            present_program,
                            rt.tex,
                            w,
                            h,
                            win_w,
                            win_h,
                            preview_scale_mode,
                            &gl_context,
                            &gl_surface,
                            |surf, ctx| {
                                surf.swap_buffers(ctx).expect("swap_buffers failed");
                            },
                            set_u_resolution,
                            set_u_src_resolution,
                            set_u_scale_mode,
                        );
                    }

                    _ => {}
                },

                Event::AboutToWait => {
                    if configs_dirty {
                        configs_dirty = false;
                        // --- Hot reload shaders (frag + present) and shader selection (render.json) ---
                        // We never crash on shader errors here: if compilation fails, we keep the last good program.
                        {
                            // 1) Did render.json change? If so, reload selection (swap shader paths).
                            let new_render_mtime = file_mtime(&render_cfg_path);
                            let mut selection_changed = false;
                            if new_render_mtime.is_some() && new_render_mtime != render_cfg_mtime {
                                render_cfg_mtime = new_render_mtime;
                                match load_render_selection(&assets_root) {
                                    Ok(new_sel) => render_sel = new_sel,
                                    Err(e) => logw!("RENDER", "render.json reload failed: {e}"),
                                }
                                                                                                                let _ = &render_sel;
let _ = &render_sel;
frag_variants = render_sel.frag_variants.clone();
                                frag_profile_map = render_sel.frag_profile_map.clone();
                                frag_variant_idx = render_sel.frag_idx;
                                if render_sel.frag_path != frag_path {
                                    frag_path = render_sel.frag_path.clone();
                                    selection_changed = true;
                                    frag_mtime = None; // force reload
                                    logi!("RENDER", "frag -> {}", frag_path.display());}
                                if render_sel.present_frag_path != present_frag_path {
                                    present_frag_path = render_sel.present_frag_path.clone();
                                    selection_changed = true;
                                    present_frag_mtime = None; // force reload
                                    logi!("RENDER", "present_frag -> {}", present_frag_path.display());}
                            }

                                // If render.json defines a frag->profile mapping, apply it on selection changes too.
                                if let Some(pname) = frag_profile_map.get(&frag_path).cloned() {
                                    logi!("PARAMS", "frag mapped -> profile: {}", pname);active_profile = Some(pname.clone());
                                    set_active_profile_for_shader(&mut pf, &assets, &frag_path, &pname);
// (legacy) pf.active_profile no longer used; per-shader active profile is stored in active_shader_profiles
                                    effective_midi = store.lock().unwrap().apply_profile(&pf, &assets, Some(&frag_path), &pname);
                                                                                                                    let _ = &effective_midi;
let _ = &effective_midi;
midi_conn_in = Some(connect_midi(&effective_midi, store.clone()));
                                        let _midi_connected = midi_conn_in.is_some();
}


                            // 2) Did the active frag file change?
                            let new_frag_mtime = file_mtime(&frag_path);
                            if selection_changed || (new_frag_mtime.is_some() && new_frag_mtime != frag_mtime) {
                                frag_mtime = new_frag_mtime;
                                let new_src = read_to_string(&frag_path);
                                match unsafe { try_compile_program(&gl, VERT_SRC, &new_src) } {
                                    Ok(new_prog) => unsafe {
                                        gl.delete_program(program);
                                        program = new_prog;
                                        logi!("HOT", "reloaded frag: {}", frag_path.display());},
                                    Err(e) => {
                                        logw!("HOT", "frag compile failed (keeping previous): {e:?}");}
                                }
                            }

                            // 3) Did the present frag file change?
                            let new_present_mtime = file_mtime(&present_frag_path);
                            if selection_changed || (new_present_mtime.is_some() && new_present_mtime != present_frag_mtime) {
                                present_frag_mtime = new_present_mtime;
                                let new_src = read_to_string(&present_frag_path);
                                match unsafe { try_compile_program(&gl, VERT_SRC, &new_src) } {
                                    Ok(new_prog) => unsafe {
                                        gl.delete_program(present_program);
                                        present_program = new_prog;
                                        logi!("HOT", "reloaded present frag: {}", present_frag_path.display());},
                                    Err(e) => {
                                        logw!("HOT", "present compile failed (keeping previous): {e:?}");}
                                }
                            }
                        }
                        // --- end hot reload ---

                        // --- Hot reload params.json (uniform defaults + profiles) ---
                        {
                            let new_params_mtime = file_mtime(&params_path);
                            if new_params_mtime.is_some() && new_params_mtime != params_mtime {
                                params_mtime = new_params_mtime;
                                let params_src = match shadecore_engine::config::load_json_file(&params_path) {
                                    Ok(lj) => lj.src,
                                    Err(e) => {
                                        logw!("PARAMS", "reload failed (keeping previous): {e}");
                                        String::new()
                                    }
                                };
                                if params_src.is_empty() {
                                    // error already logged; keep previous
                                } else {
                                    match serde_json::from_str::<ParamsFile>(&params_src) {
                                        Ok(new_pf) => {
                                            pf = new_pf;
                                            logi!("PARAMS", "reloaded version {}", pf.version);
                                            // Re-resolve active profile (same precedence as startup).
                                            let mut next_active: Option<String> = pf.active_profile.clone();
                                            if next_active.is_none() && pf.profiles.contains_key("default") {
                                                next_active = Some("default".to_string());
                                            }
                                            if next_active.is_none() {
                                                let names = sorted_profile_names_for_shader(&pf, &assets, &frag_path);
                                                if let Some(first) = names.first() {
                                                    next_active = Some(first.clone());
                                                }
                                            }
                                            active_profile = next_active;
                                
                                            profile_hotkeys = build_profile_hotkey_map(&pf);
                                            profile_names = sorted_profile_names_for_shader(&pf, &assets, &frag_path);
                                
                                            effective_midi = store.lock().unwrap().apply_params_file(&pf, active_profile.as_deref());
                                            let _ = &effective_midi;
                                            midi_conn_in = Some(connect_midi(&effective_midi, store.clone()));
                                            let _midi_connected = midi_conn_in.is_some();
                                        }
                                        Err(e) => {
                                            logw!("PARAMS", "reload failed (keeping previous): {e}");
                                        }
                                    }
                                }
                            }
                        }


                        if recorder.is_recording() {
                            pending_reload = true;
                            logi!("RECORDING", "config changed on disk; will reload after stop");} else {
                            let rec_path = recording_cfg_path.clone();
                            let new_cfg = load_recording_config(&rec_path);
                            recording_hotkeys = build_recording_hotkey_map(&new_cfg);
                            recorder.set_cfg(new_cfg.clone());
                        unsafe {
                            resize_render_target(&gl, &mut rt, new_cfg.width as i32, new_cfg.height as i32);
                        }

                            unsafe {
                                resize_render_target(&gl, &mut rt, new_cfg.width as i32, new_cfg.height as i32);
                            }

                            rec_rt = None;
                            rec_pbos = None;
                            rec_pbo_bytes = 0;
                            rec_pbo_index = 0;
                            rec_pbo_primed = false;
                            logi!("RECORDING", "reloaded: enabled={} {}x{}@{} {:?}/{:?}",
                                new_cfg.enabled,
                                new_cfg.width,
                                new_cfg.height,
                                new_cfg.fps,
                                new_cfg.container,
                                new_cfg.codec
                            );
                        }
                    }
// Deferred reload for recording.json
//
// If the recording config file changes while we're recording, we set `pending_reload=true`
// and apply it only after the recording stops. That keeps the capture pipeline coherent:
// width/height/fps/container/codec are treated as "session parameters".
                    if pending_reload && !recorder.is_recording() {
                        pending_reload = false;
                        let rec_path = recording_cfg_path.clone();
                        let new_cfg = load_recording_config(&rec_path);
                        recording_hotkeys = build_recording_hotkey_map(&new_cfg);
                        recorder.set_cfg(new_cfg.clone());
                        rec_rt = None;
                        rec_pbos = None;
                        rec_pbo_bytes = 0;
                        rec_pbo_index = 0;
                        rec_pbo_primed = false;
                        logi!("RECORDING", "reloaded after stop: enabled={} {}x{}@{} {:?}/{:?}",
                            new_cfg.enabled,
                            new_cfg.width,
                            new_cfg.height,
                            new_cfg.fps,
                            new_cfg.container,
                            new_cfg.codec
                        );
                    }
                    window.request_redraw();
                }

                _ => {}
            }
        })
        .expect("Event loop failed");
}
