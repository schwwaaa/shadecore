#[derive(Debug, Clone)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigKind {
    Render,
    Params,
    Output,
    Recording,
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// General-purpose log line.
    Log {
        level: LogLevel,
        tag: &'static str,
        msg: String,
    },

    /// A configuration file was successfully loaded.
    ConfigLoaded { kind: ConfigKind, path: PathBuf },

    /// A configuration file failed to load or validate.
    ConfigError { kind: ConfigKind, path: PathBuf, error: String },

    /// Shader compile succeeded.
    ShaderCompileOk { shader: PathBuf },

    /// Shader compile failed.
    ShaderCompileErr { shader: PathBuf, log: String },

    /// Optional runtime stats (fps, frame time, etc.) for UI clients.
    Stats { fps: f32 },
}
