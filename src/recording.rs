//! Recording pipeline (FFmpeg worker)
//!
//! Recording is designed to be **non-blocking** for the render loop:
//! - The render thread produces frames and pushes them into a bounded queue.
//! - A worker thread reads frames and feeds an FFmpeg process.
//!
//! If the worker can't keep up (slow disk/encoder), frames may be **dropped** rather than stalling
//! rendering. The goal is "keep the visuals live", not "never drop a frame".
//!
// src/recording.rs
//
// FBO-only recording via FFmpeg: reads pixels from a dedicated "record" FBO at a configurable
// resolution and pipes raw RGBA frames to FFmpeg over stdin.
//
// Design goals:
// - Cross-platform (macOS/Windows/Linux) as long as ffmpeg is available
// - Toggle start/stop by hotkey
// - Keep render thread responsive: bounded channel + drop frames when writer is behind
//
// NOTE: This is a simple synchronous glReadPixels path. If you want 4K/60 on modest GPUs,
// upgrade to PBO async readback later.

use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc,
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    Mp4,
    Mov,
}

impl Default for Container {
    fn default() -> Self { Container::Mp4 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    H264,
    Prores,
}

impl Default for Codec {
    fn default() -> Self { Codec::H264 }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RecordingCfg {
    #[serde(default)]
    pub enabled: bool,

    // Hotkeys (KeyCode names like "Numpad0"). If toggle is set, it is used as a single-key
    // start/stop toggle. If start/stop are set, they are used separately.
    #[serde(default = "default_toggle_keys")]
    pub toggle_keys: Vec<String>,
    #[serde(default = "default_start_keys")]
    pub start_keys: Vec<String>,
    #[serde(default = "default_stop_keys")]
    pub stop_keys: Vec<String>,

    #[serde(default = "default_out_dir")]
    pub out_dir: PathBuf,

    #[serde(default = "default_ffmpeg")]
    pub ffmpeg_path: String,

    #[serde(default = "default_fps")]
    pub fps: u32,

    #[serde(default = "default_width")]
    pub width: u32,

    #[serde(default = "default_height")]
    pub height: u32,

    #[serde(default)]
    pub container: Container,

    #[serde(default)]
    pub codec: Codec,

    // H.264 settings
    #[serde(default = "default_h264_crf")]
    pub h264_crf: u32,

    #[serde(default = "default_h264_preset")]
    pub h264_preset: String,

    #[serde(default = "default_pix_fmt_out")]
    pub pix_fmt_out: String,

    // ProRes settings
    #[serde(default = "default_prores_profile")]
    pub prores_profile: u32,

    // Orientation
    #[serde(default = "default_vflip")]
    pub vflip: bool,
}


fn default_toggle_keys() -> Vec<String> {
    vec![]
}
fn default_start_keys() -> Vec<String> {
    vec!["KeyR".into()]
}
fn default_stop_keys() -> Vec<String> {
    vec!["KeyS".into()]
}

fn default_out_dir() -> PathBuf {
    PathBuf::from("captures")
}
fn default_ffmpeg() -> String {
    "ffmpeg".to_string()
}
fn default_fps() -> u32 {
    60
}
fn default_width() -> u32 {
    1920
}
fn default_height() -> u32 {
    1080
}
fn default_h264_crf() -> u32 {
    18
}
fn default_h264_preset() -> String {
    "veryfast".to_string()
}
fn default_pix_fmt_out() -> String {
    "yuv420p".to_string()
}
fn default_prores_profile() -> u32 {
    3
}
fn default_vflip() -> bool {
    true
}

impl Default for RecordingCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            toggle_keys: default_toggle_keys(),
            start_keys: default_start_keys(),
            stop_keys: default_stop_keys(),
            out_dir: default_out_dir(),
            ffmpeg_path: default_ffmpeg(),
            fps: default_fps(),
            width: default_width(),
            height: default_height(),
            container: Container::Mp4,
            codec: Codec::H264,
            h264_crf: default_h264_crf(),
            h264_preset: default_h264_preset(),
            pix_fmt_out: default_pix_fmt_out(),
            prores_profile: default_prores_profile(),
            vflip: default_vflip(),
        }
    }
}

enum RecMsg {
    Frame(Vec<u8>),
    Stop,
}

pub struct Recorder {
    cfg: RecordingCfg,
    is_recording: bool,

    // reuse readback buffer on the render thread
    buf_rgba: Vec<u8>,

    // writer thread
    tx: Option<SyncSender<RecMsg>>,
    stop_flag: Option<Arc<AtomicBool>>,
    join: Option<std::thread::JoinHandle<()>>,
    child: Option<Child>,
}

impl Recorder {
    pub fn new(cfg: RecordingCfg) -> Self {
        let bytes = (cfg.width.max(1) as usize) * (cfg.height.max(1) as usize) * 4;
        Self {
            cfg,
            is_recording: false,
            buf_rgba: vec![0u8; bytes],
            tx: None,
            stop_flag: None,
            join: None,
            child: None,
        }
    }

    pub fn cfg(&self) -> &RecordingCfg {
        &self.cfg
    }

    /// Replace recording configuration (only safe when not recording).
    pub fn set_cfg(&mut self, cfg: RecordingCfg) {
        self.cfg = cfg;
        self.buf_rgba.clear();
    }

    pub fn is_enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording
    }
    #[allow(dead_code)]
    pub fn ensure_buf_size(&mut self) {
        let bytes = (self.cfg.width.max(1) as usize) * (self.cfg.height.max(1) as usize) * 4;
        if self.buf_rgba.len() != bytes {
            self.buf_rgba.resize(bytes, 0);
        }
    }
    #[allow(dead_code)]
    pub fn buf_mut(&mut self) -> &mut [u8] {
        self.ensure_buf_size();
        self.buf_rgba.as_mut_slice()
    }

    pub fn start(&mut self, assets_base: &Path) -> Result<PathBuf> {
        if !self.cfg.enabled {
            return Err(anyhow!("Recording is disabled in recording.json"));
        }
        if self.is_recording {
            return Err(anyhow!("Recorder already started"));
        }

        // out_dir relative to assets base is convenient for app bundles; but allow absolute.
        let out_dir = if self.cfg.out_dir.is_absolute() {
            self.cfg.out_dir.clone()
        } else {
            assets_base.join(&self.cfg.out_dir)
        };
        fs::create_dir_all(&out_dir)?;

        let out_path = out_dir.join(make_filename(self.cfg.container));

        let (child, stdin) = spawn_ffmpeg(&self.cfg, &out_path)?;
        let (tx, rx) = mpsc::sync_channel::<RecMsg>(3); // bounded to prevent RAM runaway
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_thread = stop_flag.clone();

        let join = thread::spawn(move || {
            writer_thread(rx, stdin, stop_flag_thread);
        });

        self.tx = Some(tx);
        self.stop_flag = Some(stop_flag);
        self.join = Some(join);
        self.child = Some(child);
        self.is_recording = true;

        Ok(out_path)
    }

    pub fn stop(&mut self) {
        if !self.is_recording {
            return;
        }

        if let Some(tx) = self.tx.take() {
            let _ = tx.try_send(RecMsg::Stop);
        }
        if let Some(flag) = &self.stop_flag {
            flag.store(true, Ordering::Relaxed);
        }

        if let Some(join) = self.join.take() {
            let _ = join.join();
        }

        // Allow ffmpeg to exit cleanly now that stdin is closed.
        if let Some(mut child) = self.child.take() {
            if child.wait().is_err() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }

        self.stop_flag.take();
        self.is_recording = false;
    }

    /// Send an already-owned RGBA frame to the writer thread (preferred for PBO async path).
    ///
    /// This avoids cloning internal buffers. Frame must be exactly width*height*4 bytes.
    pub fn try_send_frame_owned(&self, frame: Vec<u8>) {
        if !self.is_recording {
            return;
        }
        let Some(tx) = self.tx.as_ref() else { return; };
        let _ = tx.try_send(RecMsg::Frame(frame));
    }
    #[allow(dead_code)]
    pub fn try_send_frame(&self) {
        if !self.is_recording {
            return;
        }
        let Some(tx) = self.tx.as_ref() else { return; };

        // Copy into owned frame for worker thread.
        let frame = self.buf_rgba.clone();

        // Non-blocking send: drop frames if the worker is behind.
        let _ = tx.try_send(RecMsg::Frame(frame));
    }
}

fn make_filename(container: Container) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let ext = match container {
        Container::Mp4 => "mp4",
        Container::Mov => "mov",
    };
    format!("shadecore_capture_{ts}.{ext}")
}

fn writer_thread(
    rx: mpsc::Receiver<RecMsg>,
    mut stdin: ChildStdin,
    stop_flag: Arc<AtomicBool>,
) {
    while !stop_flag.load(Ordering::Relaxed) {
        match rx.recv() {
            Ok(RecMsg::Frame(frame)) => {
                if stdin.write_all(&frame).is_err() {
                    break;
                }
            }
            Ok(RecMsg::Stop) => break,
            Err(_) => break,
        }
    }

    // Closing stdin signals ffmpeg to finalize the file.
    drop(stdin);
}


fn spawn_ffmpeg(cfg: &RecordingCfg, out_path: &Path) -> Result<(Child, ChildStdin)> {
    let size = format!("{}x{}", cfg.width.max(1), cfg.height.max(1));
    let fps = cfg.fps.max(1).to_string();

    let mut cmd = Command::new(&cfg.ffmpeg_path);
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());

    // raw RGBA frames in
    cmd.args([
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "-video_size",
        &size,
        "-r",
        &fps,
        "-i",
        "pipe:0",
    ]);

    // Optional vflip
    if cfg.vflip {
        cmd.args(["-vf", "vflip"]);
    }

    match (cfg.container, cfg.codec) {
        (Container::Mp4, Codec::H264) => {
            cmd.args([
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                &cfg.h264_preset,
                "-crf",
                &cfg.h264_crf.to_string(),
                "-pix_fmt",
                &cfg.pix_fmt_out,
                out_path.to_string_lossy().as_ref(),
            ]);
        }
        (Container::Mov, Codec::Prores) => {
            cmd.args([
                "-an",
                "-c:v",
                "prores_ks",
                "-profile:v",
                &cfg.prores_profile.to_string(),
                out_path.to_string_lossy().as_ref(),
            ]);
        }
        // allow MOV+H264 (common)
        (Container::Mov, Codec::H264) => {
            cmd.args([
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                &cfg.h264_preset,
                "-crf",
                &cfg.h264_crf.to_string(),
                "-pix_fmt",
                &cfg.pix_fmt_out,
                out_path.to_string_lossy().as_ref(),
            ]);
        }
        _ => return Err(anyhow!("Unsupported container/codec combination")),
    }

    let mut child = cmd.spawn()?;

    // Pipe ffmpeg output through ShadeCore logging so everything is timestamped/tagged.
    if let Some(out) = child.stdout.take() {
        crate::logging::spawn_pipe_thread("ffmpeg_record_out", "FFMPEG_RECORD", out, false);
    }
    if let Some(err) = child.stderr.take() {
        crate::logging::spawn_pipe_thread("ffmpeg_record_err", "FFMPEG_RECORD", err, true);
    }

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Failed to open ffmpeg stdin"))?;
    Ok((child, stdin))
}
