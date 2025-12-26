//! Hot-reload watcher
//!
//! We watch **directories** (not individual files) because file replacement on save is often implemented as:
//! write temp → rename/replace → delete old. Directory watching is the most reliable cross-platform approach.
//!
//! The watcher sends lightweight "reload" signals to the render thread, which then re-reads:
//! - shader source (fragment shader)
//! - params/output/recording JSON
//!
//! Any heavy work (shader compile, GL resource updates) remains on the render thread.
//!

use crossbeam_channel::{unbounded, Receiver, Sender};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::{path::PathBuf, time::Duration};

#[derive(Debug, Clone)]
pub enum HotEvent {
    /// Current frag was edited (save in IDE)
    FragChanged(PathBuf),
    /// Shader config JSON changed (may point to new frag)
    ShaderConfigChanged(PathBuf),
    /// Generic change if you want it later
    Other,
}

pub struct HotReload {
    _watcher: RecommendedWatcher,
    rx: Receiver<HotEvent>,
}

impl HotReload {
    pub fn rx(&self) -> &Receiver<HotEvent> {
        &self.rx
    }

    pub fn new(shader_config_path: PathBuf, frag_path: PathBuf) -> anyhow::Result<Self> {
        let (tx, rx) = unbounded::<HotEvent>();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(ev) = res {
                    // Many editors do "write temp + rename", so treat any event as "changed"
                    for p in ev.paths {
                        // We only care about the two files (for now)
                        // NOTE: we can't compare here because we don't own those paths,
                        // so we’ll tag based on filename later in app code too if desired.
                        // For now, emit generic file-change events.
                        // We'll categorize by path extension:
                        if p.extension().and_then(|s| s.to_str()) == Some("frag") {
                            let _ = tx.send(HotEvent::FragChanged(p));
                        } else if p.extension().and_then(|s| s.to_str()) == Some("json") {
                            let _ = tx.send(HotEvent::ShaderConfigChanged(p));
                        } else {
                            let _ = tx.send(HotEvent::Other);
                        }
                    }
                }
            },
            Config::default()
                // debounce-ish: notify 6 doesn’t do classic debounce, but polling less noisy helps
                .with_poll_interval(Duration::from_millis(250)),
        )?;

        // Watch parent dirs so we catch atomic-save (rename) events reliably
        watch_parent(&mut watcher, &shader_config_path)?;
        watch_parent(&mut watcher, &frag_path)?;

        Ok(Self { _watcher: watcher, rx })
    }
}

fn watch_parent(w: &mut RecommendedWatcher, file: &PathBuf) -> anyhow::Result<()> {
    let parent = file
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    w.watch(&parent, RecursiveMode::NonRecursive)?;
    Ok(())
}
