//! Output backends
//!
//! ShadeCore can publish the rendered texture to different destinations:
//! - **Texture**: preview-only (no external sharing)
//! - **Syphon**: macOS texture sharing
//! - **Spout**: Windows texture sharing
//! - **NDI / Stream**: network / encoder outputs (when enabled)
//!
//! The render thread stays in control: outputs are invoked *after* drawing the frame into the FBO.
//! Backends should be thin shims that translate "GL texture + dimensions" into the target API.
//!
// src/output/mod.rs
pub mod spout;
