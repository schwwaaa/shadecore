use std::path::{Path, PathBuf};

use crate::error::EngineError;

/// A validated root directory containing ShadeCore runtime assets (JSON + shaders).
///
/// This is the canonical way to pass asset locations into the engine crate,
/// keeping path resolution consistent across CLI, scratchpad, and future GUIs.
#[derive(Debug, Clone)]
pub struct AssetsRoot {
    path: PathBuf,
}

impl AssetsRoot {
    /// Locate the `assets/` directory.
    ///
    /// Resolution order:
    /// 1) `SHADECORE_ASSETS` env var (if set)
    /// 2) Search upward from `start_dir` for a folder named `assets`
    pub fn discover(start_dir: &Path) -> Result<Self, EngineError> {
        if let Ok(p) = std::env::var("SHADECORE_ASSETS") {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Ok(Self { path: pb });
            }
        }

        let mut cur = start_dir.to_path_buf();
        loop {
            let cand = cur.join("assets");
            if cand.exists() {
                return Ok(Self { path: cand });
            }
            if !cur.pop() {
                break;
            }
        }

        Err(EngineError::AssetsNotFound {
            start_dir: start_dir.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.path.join(rel)
    }

    /// Choose OS-specific JSON config if present, otherwise fall back to `<stem>.json`.
    ///
    /// Example: `params.macos.json` overrides `params.json` on macOS.
    pub fn pick_platform_json(&self, stem: &str) -> PathBuf {
        pick_platform_json(&self.path, stem)
    }
}

/// Back-compat helper: return the assets folder path (panics on failure).
/// Prefer `AssetsRoot::discover` for Result-based handling.
pub fn find_assets_base_from(start_dir: &Path) -> PathBuf {
    AssetsRoot::discover(start_dir)
        .map(|a| a.path)
        .unwrap_or_else(|_| start_dir.join("assets"))
}

/// Choose OS-specific JSON config if present, otherwise fall back to `<stem>.json`.
pub fn pick_platform_json(assets: &Path, stem: &str) -> PathBuf {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    };

    let platform = assets.join(format!("{stem}.{os}.json"));
    if platform.exists() {
        platform
    } else {
        assets.join(format!("{stem}.json"))
    }
}


/// Resolve a JSON-provided path relative to the assets directory unless it is already absolute.
pub fn resolve_assets_path(assets_dir: &Path, s: &str) -> PathBuf {
    let p = PathBuf::from(s);
    if p.is_absolute() {
        p
    } else {
        assets_dir.join(p)
    }
}
/// Read a UTF-8 file into a String (Result-based).
pub fn read_to_string_result(path: &Path) -> Result<String, EngineError> {
    std::fs::read_to_string(path).map_err(|e| EngineError::Io {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Deserialize JSON from a file (Result-based).
pub fn load_json_result<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, EngineError> {
    let s = read_to_string_result(path)?;
    serde_json::from_str(&s).map_err(|e| EngineError::Json {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Legacy helpers (panic-on-error). Prefer `*_result` versions.
pub fn read_to_string(path: &Path) -> String {
    read_to_string_result(path).unwrap_or_else(|e| panic!("{e}"))
}

pub fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    load_json_result(path).unwrap_or_else(|e| panic!("{e}"))
}
