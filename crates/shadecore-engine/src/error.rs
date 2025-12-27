use std::{fmt, path::PathBuf};

#[derive(Debug)]
pub enum EngineError {
    /// The `assets/` folder could not be found or was invalid.
    AssetsNotFound { start_dir: PathBuf },
    /// I/O error reading a file.
    Io { path: PathBuf, source: std::io::Error },
    /// JSON parse error for a file.
    Json { path: PathBuf, source: serde_json::Error },

    /// JSON-to-typed deserialization error (when the JSON is already parsed).
    JsonValue { path: PathBuf, source: serde_json::Error },

    /// Config is syntactically valid but semantically invalid.
    InvalidConfig { path: PathBuf, msg: String },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::AssetsNotFound { start_dir } => {
                write!(f, "Could not locate assets/ starting from {}", start_dir.display())
            }
            EngineError::Io { path, source } => {
                write!(f, "I/O error for {}: {}", path.display(), source)
            }
            EngineError::Json { path, source } => {
                write!(f, "JSON parse error for {}: {}", path.display(), source)
            }
            EngineError::JsonValue { path, source } => {
                write!(f, "JSON deserialize error for {}: {}", path.display(), source)
            }
            EngineError::InvalidConfig { path, msg } => {
                write!(f, "Invalid config {}: {}", path.display(), msg)
            }
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EngineError::Io { source, .. } => Some(source),
            EngineError::Json { source, .. } => Some(source),
            EngineError::JsonValue { source, .. } => Some(source),
            _ => None,
        }
    }
}
