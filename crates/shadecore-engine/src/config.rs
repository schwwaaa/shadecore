use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::assets::{AssetsRoot, pick_platform_json, resolve_assets_path, load_json_result, read_to_string_result};
use crate::error::EngineError;

/// How strictly to interpret/validate config files.
///
/// - `Lenient` is forward-compatible: unknown fields are ignored and missing optional
///   keys fall back to defaults.
/// - `Strict` is fail-fast: unknown fields (where supported) and obvious shape issues
///   become errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMode {
    Lenient,
    Strict,
}

/// Resolved, OS-aware config paths for a ShadeCore run.
#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub assets_dir: PathBuf,
    pub render_json: PathBuf,
    pub params_json: PathBuf,
    pub output_json: PathBuf,
    pub recording_json: PathBuf,
}

/// Locate `assets/` and resolve the JSON config file paths.
///
/// This is intentionally *path-only* so the CLI and scratchpad can decide how to
/// interpret/validate configs. For typed helpers, see `load_render_selection` below.
pub fn resolve_config_paths_from(start_dir: &std::path::Path) -> Result<ConfigPaths, EngineError> {
    let assets = AssetsRoot::discover(start_dir)?;
    let assets_dir = assets.path().to_path_buf();

    let render_json = assets_dir.join("render.json");
    let params_json = pick_platform_json(&assets_dir, "params");
    let output_json = pick_platform_json(&assets_dir, "output");
    let recording_json = pick_platform_json(&assets_dir, "recording");

    Ok(ConfigPaths {
        assets_dir,
        render_json,
        params_json,
        output_json,
        recording_json,
    })
}

/// Typed view of `assets/render.json`.
///
/// Versioning: `version` defaults to 1 when omitted.
/// Unknown fields are ignored by default (serde default behavior), keeping configs forward-compatible.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RenderJson {
    #[serde(default = "default_version")]
    pub version: u32,

    #[serde(default)]
    pub frag: Option<String>,

    /// Optional list of fragment shader variants.
    /// Example: { "frag_variants": ["shaders/a.frag", "shaders/b.frag"] }
    #[serde(default)]
    pub frag_variants: Option<Vec<String>>,

    /// Optional active fragment selection by exact string match against entries in `frag_variants`.
    #[serde(default)]
    pub active_frag: Option<String>,

    #[serde(default)]
    pub present_frag: Option<String>,

    /// Optional mapping from frag variant string -> params profile name.
    /// Example:
    /// { "frag_profile_map": { "shaders/a.frag": "lofi", "shaders/b.frag": "crunch" } }
    #[serde(default)]
    pub frag_profile_map: Option<HashMap<String, String>>,
}

/// Strict version of `RenderJson` that fails on unknown fields.
///
/// This is used when `ConfigMode::Strict` is requested.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderJsonStrict {
    #[serde(default = "default_version")]
    pub version: u32,

    #[serde(default)]
    pub frag: Option<String>,

    #[serde(default)]
    pub frag_variants: Option<Vec<String>>,

    #[serde(default)]
    pub active_frag: Option<String>,

    #[serde(default)]
    pub present_frag: Option<String>,

    #[serde(default)]
    pub frag_profile_map: Option<HashMap<String, String>>,
}

fn default_version() -> u32 { 1 }

/// Resolved render selection (paths + variant list).
///
/// This struct is used by the CLI runner and will become part of the engine crate's
/// public surface for UI clients (scratchpad/studio).
#[derive(Debug, Clone)]
pub struct RenderSelection {
    pub frag_path: PathBuf,
    pub present_frag_path: PathBuf,

    /// Optional list of fragment shader variants to cycle with hotkeys.
    /// If empty, falls back to `frag_path`.
    pub frag_variants: Vec<PathBuf>,

    /// Active index within `frag_variants`.
    pub frag_idx: usize,

    /// Optional mapping from a frag variant path -> params profile name.
    pub frag_profile_map: HashMap<PathBuf, String>,
}

/// Load `assets/render.json` and resolve all paths against the assets directory.
///
/// This function is *non-panicking* and returns Result for better stability/diagnostics.
pub fn load_render_selection(assets: &AssetsRoot) -> Result<RenderSelection, EngineError> {
    load_render_selection_with_mode(assets, ConfigMode::Lenient)
}

/// Strict version of `load_render_selection`.
pub fn load_render_selection_strict(assets: &AssetsRoot) -> Result<RenderSelection, EngineError> {
    load_render_selection_with_mode(assets, ConfigMode::Strict)
}

fn load_render_selection_with_mode(
    assets: &AssetsRoot,
    mode: ConfigMode,
) -> Result<RenderSelection, EngineError> {
    let assets_dir = assets.path();

    // Defaults (what already works)
    let default_frag = assets_dir.join("shaders").join("default.frag");
    let default_present = assets_dir.join("shaders").join("present.frag");
    let render_cfg = assets_dir.join("render.json");

    // If render.json doesn't exist yet, keep the historical behavior:
    if !render_cfg.exists() {
        return Ok(RenderSelection {
            frag_path: default_frag.clone(),
            present_frag_path: default_present.clone(),
            frag_variants: vec![default_frag],
            frag_idx: 0,
            frag_profile_map: HashMap::new(),
        });
    }

    let data = read_to_string_result(&render_cfg)?;

    // Parse in the requested mode.
    let (version, frag, frag_variants_s, active_frag, present_frag, frag_profile_map_s) = match mode {
        ConfigMode::Lenient => {
            let rj: RenderJson = serde_json::from_str(&data).map_err(|e| EngineError::Json {
                path: render_cfg.clone(),
                source: e,
            })?;
            (
                rj.version,
                rj.frag,
                rj.frag_variants,
                rj.active_frag,
                rj.present_frag,
                rj.frag_profile_map,
            )
        }
        ConfigMode::Strict => {
            let rj: RenderJsonStrict = serde_json::from_str(&data).map_err(|e| EngineError::Json {
                path: render_cfg.clone(),
                source: e,
            })?;
            (
                rj.version,
                rj.frag,
                rj.frag_variants,
                rj.active_frag,
                rj.present_frag,
                rj.frag_profile_map,
            )
        }
    };

    // Minimal semantic validation in strict mode.
    if mode == ConfigMode::Strict && version != 1 {
        return Err(EngineError::InvalidConfig {
            path: render_cfg.clone(),
            msg: format!("unsupported render.json version {version} (expected 1)"),
        });
    }

    // Resolve variants (if present), else fall back to single frag.
    let mut frag_variants: Vec<PathBuf> = Vec::new();
    if let Some(list) = frag_variants_s.as_ref() {
        for s in list {
            frag_variants.push(resolve_assets_path(assets_dir, s));
        }
    }
    if frag_variants.is_empty() {
        let single = frag
            .as_deref()
            .map(|s| resolve_assets_path(assets_dir, s))
            .unwrap_or_else(|| default_frag.clone());
        frag_variants.push(single);
    }

    // Active index by matching active_frag string against the original string list (if supplied),
    // else default 0.
    // Determine the active index by matching `active_frag` against the *string list*
    // in the config (if present). This keeps behavior stable even though we resolve
    // variants into absolute paths.
    let mut frag_idx: usize = 0;
    if let (Some(active), Some(list)) = (active_frag.as_ref(), frag_variants_s.as_ref()) {
        if let Some(pos) = list.iter().position(|s| s == active) {
            frag_idx = pos.min(frag_variants.len().saturating_sub(1));
        }
    }

    let frag_path = frag_variants
        .get(frag_idx)
        .cloned()
        .unwrap_or_else(|| default_frag.clone());

    let present_frag_path = present_frag
        .as_deref()
        .map(|s| resolve_assets_path(assets_dir, s))
        .unwrap_or_else(|| default_present.clone());

    // Optional frag->profile mapping
    let mut frag_profile_map: HashMap<PathBuf, String> = HashMap::new();
    if let Some(map) = frag_profile_map_s.as_ref() {
        for (k, v) in map {
            frag_profile_map.insert(resolve_assets_path(assets_dir, k), v.clone());
        }
    }

    Ok(RenderSelection {
        frag_path,
        present_frag_path,
        frag_variants,
        frag_idx,
        frag_profile_map,
    })
}

/// A JSON file loaded from disk (path + raw text + parsed `serde_json::Value`).
///
/// This is intentionally kept untyped for maximum forward-compatibility:
/// - the engine crate owns discovery + reading + JSON parsing
/// - clients (CLI, scratchpad, future Studio) can deserialize into their own typed structs
///   or operate on raw JSON.
#[derive(Debug, Clone)]
pub struct LoadedJson {
    pub path: PathBuf,
    pub src: String,
    pub value: Value,
}

/// Load any JSON file as `LoadedJson`.
pub fn load_json_file(path: &Path) -> Result<LoadedJson, EngineError> {
    let src = read_to_string_result(path)?;
    let value: Value = serde_json::from_str(&src).map_err(|e| EngineError::Json {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(LoadedJson {
        path: path.to_path_buf(),
        src,
        value,
    })
}

/// Deserialize a previously-loaded JSON file into a typed struct.
///
/// This lets the engine own *reading + JSON parsing*, while callers own their local
/// typed structs (CLI today; scratchpad/studio later).
pub fn parse_loaded_json<T: serde::de::DeserializeOwned>(loaded: &LoadedJson) -> Result<T, EngineError> {
    serde_json::from_value::<T>(loaded.value.clone()).map_err(|e| EngineError::JsonValue {
        path: loaded.path.clone(),
        source: e,
    })
}

fn validate_top_level_object(kind: &str, loaded: &LoadedJson) -> Result<(), EngineError> {
    if !loaded.value.is_object() {
        return Err(EngineError::InvalidConfig {
            path: loaded.path.clone(),
            msg: format!("{kind} must be a JSON object"),
        });
    }
    Ok(())
}

/// Engine-owned loader for `params(.<os>).json`.
pub fn load_params_json(assets: &AssetsRoot) -> Result<LoadedJson, EngineError> {
    let path = assets.pick_platform_json("params");
    let loaded = load_json_file(&path)?;
    // params.json is expected to be an object in all current builds.
    // Treat this as a stability/safety check.
    validate_top_level_object("params.json", &loaded)?;
    Ok(loaded)
}

/// Engine-owned loader for `output(.<os>).json`.
pub fn load_output_json(assets: &AssetsRoot) -> Result<LoadedJson, EngineError> {
    let path = assets.pick_platform_json("output");
    let loaded = load_json_file(&path)?;
    validate_top_level_object("output.json", &loaded)?;
    Ok(loaded)
}

/// Engine-owned loader for `recording(.<os>).json`.
pub fn load_recording_json(assets: &AssetsRoot) -> Result<LoadedJson, EngineError> {
    let path = assets.pick_platform_json("recording");
    let loaded = load_json_file(&path)?;
    validate_top_level_object("recording.json", &loaded)?;
    Ok(loaded)
}

/// Aggregate configuration loaded by the engine crate.
///
/// No renderer internals are exposed here — this is purely about configuration
/// discovery + parsing, to keep the project modular and future-proof.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub assets: AssetsRoot,
    pub paths: ConfigPaths,
    pub render: RenderSelection,
    pub params: LoadedJson,
    pub output: LoadedJson,
    pub recording: LoadedJson,
}

/// Load all standard config files + resolve render selection.
///
/// This is intended as the primary entry point for clients.
pub fn load_engine_config_from(start_dir: &Path) -> Result<EngineConfig, EngineError> {
    load_engine_config_from_mode(start_dir, ConfigMode::Lenient)
}

/// Strict variant of `load_engine_config_from`.
pub fn load_engine_config_from_strict(start_dir: &Path) -> Result<EngineConfig, EngineError> {
    load_engine_config_from_mode(start_dir, ConfigMode::Strict)
}

fn load_engine_config_from_mode(start_dir: &Path, mode: ConfigMode) -> Result<EngineConfig, EngineError> {
    let assets = AssetsRoot::discover(start_dir)?;
    let paths = resolve_config_paths_from(start_dir)?;
    let render = if mode == ConfigMode::Strict {
        load_render_selection_strict(&assets)?
    } else {
        load_render_selection(&assets)?
    };
    let params = load_params_json(&assets)?;
    let output = load_output_json(&assets)?;
    let recording = load_recording_json(&assets)?;

    Ok(EngineConfig {
        assets,
        paths,
        render,
        params,
        output,
        recording,
    })
}

/// Convenience: load any JSON file as a typed struct (Result-based).
pub fn load_typed_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T, EngineError> {
    load_json_result(path)
}
