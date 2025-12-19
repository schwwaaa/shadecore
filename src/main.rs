use glow::HasContext;

use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextApi, ContextAttributesBuilder, NotCurrentContext, Version};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::DisplayBuilder;

use raw_window_handle::HasRawWindowHandle;

use midir::{Ignore, MidiInput};
use serde::Deserialize;

use std::collections::HashMap;
use std::ffi::CString;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
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
}

#[derive(Debug, Clone, serde::Deserialize)]
struct OutputConfigFile {
    #[serde(default = "default_output_mode")]
    output_mode: OutputMode,

    #[serde(default)]
    syphon: SyphonCfg,

    #[serde(default)]
    spout: SpoutCfg,
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

fn default_true() -> bool {
    true
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
    };

    let data = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return default_cfg,
    };

    match serde_json::from_str::<OutputConfigFile>(&data) {
        Ok(cfg) => cfg,
        Err(e) => {
            println!(
                "[output] Failed to parse output config ({}): {}. Using defaults.",
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
#[derive(Debug, Clone, Deserialize)]
struct ParamsFile {
    version: u32,
    #[serde(default)]
    midi: MidiGlobalCfg,
    #[serde(default)]
    params: Vec<ParamDef>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct MidiGlobalCfg {
    #[serde(default)]
    preferred_device_contains: Option<String>,
    #[serde(default)]
    channel: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
struct MidiBinding {
    cc: u8,
    #[serde(default)]
    channel: Option<u8>,
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
struct ParamStore {
    values: HashMap<String, f32>,
    targets: HashMap<String, f32>,
    smooth: HashMap<String, f32>,
    mappings: HashMap<(u8, u8), ParamMapping>, // (channel, cc) -> mapping
}

impl ParamStore {
    fn new(pf: &ParamsFile) -> Self {
        let mut values = HashMap::new();
        let mut targets = HashMap::new();
        let mut smooth = HashMap::new();
        let mut mappings = HashMap::new();

        let global_chan = pf.midi.channel.unwrap_or(0);

        for p in &pf.params {
            values.insert(p.name.clone(), p.default);
            targets.insert(p.name.clone(), p.default);
            smooth.insert(p.name.clone(), p.smoothing);

            if let Some(b) = &p.midi {
                let ch = b.channel.unwrap_or(global_chan);
                mappings.insert(
                    (ch, b.cc),
                    ParamMapping {
                        name: p.name.clone(),
                        min: p.min,
                        max: p.max,
                        smoothing: p.smoothing,
                    },
                );
            }
        }

        Self {
            values,
            targets,
            smooth,
            mappings,
        }
    }

    fn set_cc(&mut self, ch: u8, cc: u8, val_0_127: u8) {
        if let Some(map) = self.mappings.get(&(ch, cc)) {
            let x = (val_0_127 as f32) / 127.0;
            let t = map.min + (map.max - map.min) * x;
            self.targets.insert(map.name.clone(), t);
            self.smooth.insert(map.name.clone(), map.smoothing);
        }
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

fn read_to_string(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

fn find_assets_base() -> PathBuf {
    if let Ok(p) = std::env::var("SHADECORE_ASSETS") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

fn pick_platform_json(assets: &Path, stem: &str) -> PathBuf {
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


fn connect_midi(pf: &ParamsFile, store: Arc<Mutex<ParamStore>>) -> Option<midir::MidiInputConnection<()>> {
    let mut midi_in = MidiInput::new("shadecore-midi").ok()?;
    midi_in.ignore(Ignore::None);

    let ports = midi_in.ports();
    if ports.is_empty() {
        println!("[midi] No MIDI input ports detected.");
        return None;
    }

    let preferred = pf
        .midi
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
    println!("[midi] Connecting input: {}", port_name);

    let conn = midi_in.connect(
        &in_port,
        "shadecore-midi-in",
        move |_ts, msg, _| {
            if msg.len() == 3 && (msg[0] & 0xF0) == 0xB0 {
                let ch = msg[0] & 0x0F;
                let cc = msg[1];
                let val = msg[2];
                if let Ok(mut s) = store.lock() {
                    s.set_cc(ch, cc, val);
                }
            }
        },
        (),
    );

    match conn {
        Ok(c) => Some(c),
        Err(e) => {
            println!("[midi] Failed to connect MIDI input: {e}");
            None
        }
    }
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

// NOTE: glow uniform calls are unsafe in your build; wrap them here.
fn set_u_resolution(gl: &glow::Context, prog: glow::NativeProgram, w: i32, h: i32) {
    unsafe {
        if let Some(loc) = gl.get_uniform_location(prog, "u_resolution") {
            gl.uniform_2_f32(Some(&loc), w as f32, h as f32);
        }
    }
}

fn main() {
    let assets = find_assets_base();

    let frag_path = assets.join("shaders").join("default.frag");
    let present_frag_path = assets.join("shaders").join("present.frag");
    let params_path = pick_platform_json(&assets, "params");
    let output_cfg_path = pick_platform_json(&assets, "output");

    println!("[assets] base: {}", assets.display());
    println!("[assets] frag: {}", frag_path.display());
    println!("[assets] present: {}", present_frag_path.display());
    println!("[assets] params: {}", params_path.display());
    println!("[assets] output: {}", output_cfg_path.display());

    let frag_src = read_to_string(&frag_path);
    let present_frag_src = read_to_string(&present_frag_path);

    let params_src = read_to_string(&params_path);
    let pf: ParamsFile = serde_json::from_str(&params_src)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {e}", params_path.display()));
    println!("[params] loaded version {}", pf.version);

    let store = Arc::new(Mutex::new(ParamStore::new(&pf)));

    let event_loop = EventLoop::new().expect("EventLoop::new failed");
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

    let program = unsafe { compile_program(&gl, VERT_SRC, &frag_src) };
    let present_program = unsafe { compile_program(&gl, VERT_SRC, &present_frag_src) };
    let vao = unsafe { gl.create_vertex_array().expect("create_vertex_array failed") };

    let size = window.inner_size();
    let mut rt = unsafe { create_render_target(&gl, size.width as i32, size.height as i32) };

    let _midi_conn_in = connect_midi(&pf, store.clone());

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

    let mut output_mode = output_cfg.output_mode;

    println!(
        "[output] startup mode={:?} | syphon.enabled={} name='{}' | spout.enabled={} name='{}' invert={}",
        output_mode, syphon_enabled, syphon_name, spout_enabled, spout_name, spout_invert
    );

    window.set_title(&format!(
        "shadecore – output: {:?} (press 1=Texture, 2=Syphon, 3=Spout)",
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

    let mut warned = false;
    let start = Instant::now();

    event_loop
        .run(move |event, target| {
            target.set_control_flow(ControlFlow::Poll);

            match event {
                Event::WindowEvent { event, .. } => match event {
                    WindowEvent::CloseRequested => target.exit(),

                    WindowEvent::KeyboardInput { event, .. } => {
                        if event.state.is_pressed() {
                            if let PhysicalKey::Code(code) = event.physical_key {
                                let new_mode = match code {
                                    KeyCode::Digit1 | KeyCode::Numpad1 => Some(OutputMode::Texture),
                                    KeyCode::Digit2 | KeyCode::Numpad2 => Some(OutputMode::Syphon),
                                    KeyCode::Digit3 | KeyCode::Numpad3 => Some(OutputMode::Spout),
                                    _ => None,
                                };

                                if let Some(m) = new_mode {
                                    output_mode = m;
                                    warned = false;
                                    println!("[output] switched -> {:?}", output_mode);
                                    window.set_title(&format!(
                                        "shadecore – output: {:?} (press 1=Texture, 2=Syphon, 3=Spout)",
                                        output_mode
                                    ));
                                }
                            }
                        }
                    }

                    WindowEvent::Resized(new_size) => unsafe {
                        resize_render_target(&gl, &mut rt, new_size.width as i32, new_size.height as i32);
                    },

                    WindowEvent::RedrawRequested => unsafe {
                        let size = window.inner_size();
                        let w = size.width as i32;
                        let h = size.height as i32;
                        resize_render_target(&gl, &mut rt, w, h);

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

                        if let Some(loc) = gl.get_uniform_location(program, "u_time") {
                            let t = start.elapsed().as_secs_f32();
                            gl.uniform_1_f32(Some(&loc), t);
                        }

                        gl.draw_arrays(glow::TRIANGLES, 0, 3);

                        gl.bind_vertex_array(None);
                        gl.use_program(None);
                        gl.bind_framebuffer(glow::FRAMEBUFFER, None);

                        let tex_id = tex_id_u32(rt.tex);

                        match output_mode {
                            OutputMode::Texture => {}

                            OutputMode::Syphon => {
                                #[cfg(all(target_os = "macos", has_syphon))]
                                {
                                    if !syphon_enabled {
                                        if !warned {
                                            println!("[output] Syphon requested but disabled in output.json. Falling back to Texture.");
                                            warned = true;
                                        }
                                    } else {
                                        if syphon.is_none() {
                                            syphon = SyphonServer::new(&syphon_name);
                                            if syphon.is_none() && !warned {
                                                println!("[output] Syphon init failed. Falling back to Texture.");
                                                warned = true;
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
                                        println!("[output] Syphon requested but Syphon.framework is not vendored. Falling back to Texture.");
                                        warned = true;
                                    }
                                }

                                #[cfg(not(target_os = "macos"))]
                                {
                                    if !warned {
                                        println!("[output] Syphon requested but macOS-only. Falling back to Texture.");
                                        warned = true;
                                    }
                                }
                            }

                            OutputMode::Spout => {
                                #[cfg(target_os = "windows")]
                                {
                                    if !spout_enabled {
                                        if !warned {
                                            println!("[output] Spout requested but disabled in output.json. Falling back to Texture.");
                                            warned = true;
                                        }
                                    } else {
                                        if spout.is_none() {
                                            spout = SpoutSender::new(&spout_name, w, h, spout_invert);
                                            if spout.is_none() && !warned {
                                                println!("[output] Spout init failed. Falling back to Texture.");
                                                warned = true;
                                            }
                                        }
                                        if let Some(ref sender) = spout {
                                            let ok = sender.send_texture(tex_id, w, h);
                                            if !ok && !warned {
                                                println!("[output] Spout send failed. Falling back to Texture.");
                                                warned = true;
                                            }
                                        }
                                    }
                                }

                                #[cfg(not(target_os = "windows"))]
                                {
                                    if !warned {
                                        println!("[output] Spout requested but Windows-only. Falling back to Texture.");
                                        warned = true;
                                    }
                                }
                            }
                        }

                        gl.viewport(0, 0, w, h);
                        gl.clear_color(0.02, 0.02, 0.02, 1.0);
                        gl.clear(glow::COLOR_BUFFER_BIT);

                        gl.use_program(Some(present_program));
                        gl.bind_vertex_array(Some(vao));

                        set_u_resolution(&gl, present_program, w, h);

                        if let Some(loc) = gl.get_uniform_location(present_program, "u_tex") {
                            gl.uniform_1_i32(Some(&loc), 0);
                        }
                        gl.active_texture(glow::TEXTURE0);
                        gl.bind_texture(glow::TEXTURE_2D, Some(rt.tex));

                        gl.draw_arrays(glow::TRIANGLES, 0, 3);

                        gl.bind_texture(glow::TEXTURE_2D, None);
                        gl.bind_vertex_array(None);
                        gl.use_program(None);

                        gl_surface.swap_buffers(&gl_context).expect("swap_buffers failed");
                    }

                    _ => {}
                },

                Event::AboutToWait => {
                    window.request_redraw();
                }

                _ => {}
            }
        })
        .expect("Event loop failed");
}
