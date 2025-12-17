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
use std::ffi::{CString};
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::{Window, WindowBuilder};

/// ===============================
/// SEGMENT 4: Syphon output (macOS)
/// ===============================
///
/// Rendering pipeline now becomes:
///   1) Bind our FBO (backed by a GL_TEXTURE_2D)
///   2) Render the shader into that texture
///   3) Publish that texture to Syphon
///   4) Present the same texture to the window
///
/// This is the "right" foundation for feedback later:
/// - The texture is a first-class object (can be re-used as input next frame).
/// - Syphon just publishes the texture ID (no CPU copies).
///
/// IMPORTANT:
/// - Syphon is macOS-only.
/// - You must supply vendor/Syphon.framework for this to link (see README).

// Fullscreen triangle vertex shader (same as earlier segments)
const VERT_SRC: &str = r#"#version 330 core
out vec2 v_uv;
void main() {
    vec2 pos;
    if (gl_VertexID == 0) pos = vec2(-1.0, -1.0);
    else if (gl_VertexID == 1) pos = vec2( 3.0, -1.0);
    else pos = vec2(-1.0,  3.0);

    gl_Position = vec4(pos, 0.0, 1.0);
    v_uv = 0.5 * (pos + 1.0);
}
"#;

// -------------------------------
// Syphon C-ABI bridge (macOS only)
// -------------------------------
#[cfg(target_os = "macos")]
extern "C" {
    fn syphon_server_create(name_utf8: *const i8) -> *mut std::ffi::c_void;
    fn syphon_server_publish_texture(server_ptr: *mut std::ffi::c_void, tex_id: u32, width: i32, height: i32);
    fn syphon_server_destroy(server_ptr: *mut std::ffi::c_void);
}

#[cfg(target_os = "macos")]
struct SyphonServer {
    ptr: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
impl SyphonServer {
    fn new(name: &str) -> Option<Self> {
        let c = CString::new(name).ok()?;
        let ptr = unsafe { syphon_server_create(c.as_ptr()) };
        if ptr.is_null() { None } else { Some(Self { ptr }) }
    }

    fn publish_texture(&self, tex_id: u32, w: i32, h: i32) {
        unsafe { syphon_server_publish_texture(self.ptr, tex_id, w, h) }
    }
}

#[cfg(target_os = "macos")]
impl Drop for SyphonServer {
    fn drop(&mut self) {
        unsafe { syphon_server_destroy(self.ptr) }
    }
}

// -------------------------------
// JSON schema (same as Segment 3)
// -------------------------------
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ParamsFile {
    version: u32,
    #[serde(default)]
    midi: MidiSettings,
    params: Vec<ParamDef>,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct MidiSettings {
    #[serde(default)]
    preferred_device_contains: String,
    #[serde(default)]
    channel: u8, // 0=any, 1..16
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ParamDef {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    default: f32,
    min: f32,
    max: f32,
    #[serde(default)]
    smoothing: f32,
    #[serde(default)]
    midi: Option<MidiMap>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MidiMap {
    cc: u8,
    #[serde(default)]
    channel: u8, // 0=any, 1..16
}

#[derive(Debug, Clone)]
struct ParamState {
    def: ParamDef,
    target: f32,
    value: f32,
}

#[derive(Debug)]
struct ParamStore {
    by_name: HashMap<String, ParamState>,
    cc_map: HashMap<(u8, u8), String>,
}

impl ParamStore {
    fn from_defs(defs: &[ParamDef]) -> Self {
        let mut by_name = HashMap::new();
        let mut cc_map = HashMap::new();

        for d in defs {
            let v = d.default.clamp(d.min, d.max);
            by_name.insert(
                d.name.clone(),
                ParamState { def: d.clone(), target: v, value: v },
            );
            if let Some(m) = &d.midi {
                cc_map.insert((m.channel, m.cc), d.name.clone());
            }
        }

        Self { by_name, cc_map }
    }

    fn set_from_norm(&mut self, name: &str, norm: f32) {
        if let Some(p) = self.by_name.get_mut(name) {
            let n = norm.clamp(0.0, 1.0);
            p.target = p.def.min + n * (p.def.max - p.def.min);
        }
    }

    fn tick(&mut self) {
        for p in self.by_name.values_mut() {
            let s = p.def.smoothing.clamp(0.0, 1.0);
            if s <= 0.0 {
                p.value = p.target;
            } else {
                p.value = p.value + (p.target - p.value) * s;
            }
        }
    }

    fn handle_cc(&mut self, ch_1_16: u8, cc: u8, val_0_127: u8, global_ch: u8) {
        if global_ch != 0 && ch_1_16 != global_ch {
            return;
        }
        let norm = (val_0_127 as f32) / 127.0;

        let key_specific = (ch_1_16, cc);
        let key_any = (0u8, cc);

        if let Some(name) = self.cc_map.get(&key_specific).cloned().or_else(|| self.cc_map.get(&key_any).cloned()) {
            self.set_from_norm(&name, norm);
        }
    }

    fn get_value(&self, name: &str) -> Option<f32> {
        self.by_name.get(name).map(|p| p.value)
    }

    fn names(&self) -> impl Iterator<Item = &String> {
        self.by_name.keys()
    }
}

fn find_assets_base() -> PathBuf {
    let dev = PathBuf::from("assets");
    if dev.exists() {
        return dev;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(mac_os_dir) = exe.parent() {
            if let Some(contents_dir) = mac_os_dir.parent() {
                let resources = contents_dir.join("Resources");
                if resources.exists() {
                    return resources;
                }
            }
        }
    }
    PathBuf::from(".")
}

fn read_to_string(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("Failed to read {}: {e}", path.display()))
}

unsafe fn compile_program(gl: &glow::Context, vs_src: &str, fs_src: &str) -> glow::Program {
    let program = gl.create_program().expect("create_program failed");

    let vs = gl.create_shader(glow::VERTEX_SHADER).expect("create_shader VS failed");
    gl.shader_source(vs, vs_src);
    gl.compile_shader(vs);
    if !gl.get_shader_compile_status(vs) {
        panic!("Vertex shader compile error:\n{}", gl.get_shader_info_log(vs));
    }

    let fs = gl.create_shader(glow::FRAGMENT_SHADER).expect("create_shader FS failed");
    gl.shader_source(fs, fs_src);
    gl.compile_shader(fs);
    if !gl.get_shader_compile_status(fs) {
        panic!("Fragment shader compile error:\n{}", gl.get_shader_info_log(fs));
    }

    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);

    gl.detach_shader(program, vs);
    gl.detach_shader(program, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);

    if !gl.get_program_link_status(program) {
        panic!("Program link error:\n{}", gl.get_program_info_log(program));
    }

    program
}

/// -------------------------------
/// MIDI connect (same pattern as seg3)
/// -------------------------------
fn connect_midi(params: &ParamsFile, store: Arc<Mutex<ParamStore>>) -> Option<midir::MidiInputConnection<()>> {
    let mut midi_in = MidiInput::new("glsl_engine").ok()?;
    midi_in.ignore(Ignore::None);

    let ports = midi_in.ports();
    if ports.is_empty() {
        println!("No MIDI input ports found.");
        return None;
    }

    println!("--- MIDI INPUT PORTS ---");
    for (i, p) in ports.iter().enumerate() {
        let name = midi_in.port_name(p).unwrap_or_else(|_| "<unknown>".to_string());
        println!("{i}: {name}");
    }
    println!("------------------------");

    let hint = params.midi.preferred_device_contains.trim().to_string();
    let chosen_index = if !hint.is_empty() {
        ports.iter().enumerate().find(|(_, p)| {
            midi_in.port_name(p).map(|n| n.contains(&hint)).unwrap_or(false)
        }).map(|(i, _)| i)
    } else { None }.unwrap_or(0);

    let chosen_port = ports.get(chosen_index)?;
    let chosen_name = midi_in.port_name(chosen_port).unwrap_or_else(|_| "<unknown>".to_string());
    println!("Connecting to MIDI input [{chosen_index}]: {chosen_name}");

    let global_channel = params.midi.channel;

    let conn = midi_in.connect(
        chosen_port,
        "glsl_engine-midi-in",
        move |_ts, message, _| {
            if message.len() >= 3 && (message[0] & 0xF0) == 0xB0 {
                let ch = (message[0] & 0x0F) + 1;
                let cc = message[1];
                let val = message[2];

                if let Ok(mut s) = store.lock() {
                    s.handle_cc(ch, cc, val, global_channel);
                }
            }
        },
        (),
    );

    match conn {
        Ok(c) => Some(c),
        Err(e) => {
            println!("Failed to connect MIDI input: {e}");
            None
        }
    }
}

/// -------------------------------
/// FBO render target for Syphon
/// -------------------------------
#[derive(Debug)]
struct RenderTarget {
    fbo: glow::Framebuffer,
    tex: glow::Texture,
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
        glow::RGBA8 as i32,
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
    gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);

    let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
    if status != glow::FRAMEBUFFER_COMPLETE {
        panic!("FBO not complete, status={status:#x}");
    }

    gl.bind_framebuffer(glow::FRAMEBUFFER, None);

    RenderTarget { fbo, tex, w, h }
}

unsafe fn destroy_render_target(gl: &glow::Context, rt: &mut RenderTarget) {
    gl.delete_framebuffer(rt.fbo);
    gl.delete_texture(rt.tex);
}

fn main() {
    // ---------------------------
    // Load assets (shader + params)
    // ---------------------------
    let assets = find_assets_base();

    let frag_path = assets.join("shaders").join("default.frag");
    let present_frag_path = assets.join("shaders").join("present.frag");
    let params_path = assets.join("params.json");

    println!("Assets base: {}", assets.display());
    println!("Frag shader: {}", frag_path.display());
    println!("Present shader: {}", present_frag_path.display());
    println!("Params JSON: {}", params_path.display());

    let frag_src = read_to_string(&frag_path);
    let present_frag_src = read_to_string(&present_frag_path);
    let params_text = read_to_string(&params_path);
    let params_file: ParamsFile = serde_json::from_str(&params_text).expect("Failed to parse params.json");

    let store = Arc::new(Mutex::new(ParamStore::from_defs(&params_file.params)));

    // ---------------------------
    // Window + GL setup
    // ---------------------------
    let event_loop = EventLoop::new().expect("Failed to create EventLoop");

    let window_builder = Some(
        WindowBuilder::new()
            .with_title("GLSL Engine – Segment 4 (Syphon + MIDI + JSON Params)")
            .with_inner_size(PhysicalSize::new(1280, 720)),
    );

    let template = ConfigTemplateBuilder::new().with_alpha_size(8);
    let display_builder = DisplayBuilder::new().with_window_builder(window_builder);

    let (window, gl_config) = display_builder
        .build(&event_loop, template, |mut configs| configs.next().unwrap())
        .expect("DisplayBuilder build failed");

    let window: Window = window.expect("Window creation failed");

    let raw_window_handle = window.raw_window_handle();
    let gl_display = gl_config.display();

    let context_attributes = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 3))))
        .build(Some(raw_window_handle));

    let fallback_context_attributes = ContextAttributesBuilder::new()
        .with_context_api(ContextApi::OpenGl(Some(Version::new(3, 0))))
        .build(Some(raw_window_handle));

    let not_current_context: NotCurrentContext = unsafe {
        gl_display
            .create_context(&gl_config, &context_attributes)
            .or_else(|_| gl_display.create_context(&gl_config, &fallback_context_attributes))
            .expect("create_context failed")
    };

    let size = window.inner_size();
    let width = NonZeroU32::new(size.width.max(1)).unwrap();
    let height = NonZeroU32::new(size.height.max(1)).unwrap();

    let attrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(raw_window_handle, width, height);
    let gl_surface = unsafe { gl_display.create_window_surface(&gl_config, &attrs).expect("create_window_surface failed") };
    let gl_context = not_current_context.make_current(&gl_surface).expect("make_current failed");

    let _ = gl_surface.set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()));

    let gl = unsafe {
        glow::Context::from_loader_function(|name| {
            let c_name = CString::new(name).expect("CString::new failed");
            gl_display.get_proc_address(c_name.as_c_str()) as *const _
        })
    };

    // Compile programs
    let program = unsafe { compile_program(&gl, VERT_SRC, &frag_src) };
    let present_program = unsafe { compile_program(&gl, VERT_SRC, &present_frag_src) };

    let vao = unsafe { gl.create_vertex_array().expect("create_vertex_array failed") };

    // Uniforms for main shader
    let u_time_loc = unsafe { gl.get_uniform_location(program, "u_time") };
    let u_res_loc = unsafe { gl.get_uniform_location(program, "u_resolution") };

    // Uniforms for present shader
    let u_present_res = unsafe { gl.get_uniform_location(present_program, "u_resolution") };
    let u_present_tex = unsafe { gl.get_uniform_location(present_program, "u_tex") };

    // Cache uniform locations for JSON params
    let mut param_uniforms: HashMap<String, Option<glow::UniformLocation>> = HashMap::new();
    {
        let s = store.lock().unwrap();
        for name in s.names() {
            let loc = unsafe { gl.get_uniform_location(program, name.as_str()) };
            if loc.is_none() {
                println!("Warning: uniform '{}' not found in shader", name);
            }
            param_uniforms.insert(name.clone(), loc);
        }
    }

    unsafe {
        gl.disable(glow::DEPTH_TEST);
        gl.disable(glow::CULL_FACE);
    }

    // Create initial render target (size = window size)
    let mut rt = unsafe { create_render_target(&gl, size.width as i32, size.height as i32) };

    // MIDI connect
    let _midi_conn_in = connect_midi(&params_file, store.clone());

    // Syphon server (macOS only)
    #[cfg(target_os = "macos")]
    let syphon = {
        // Create AFTER GL context is current.
        // SyphonOpenGLServer init uses [NSOpenGLContext currentContext] internally.
        SyphonServer::new("glsl_engine").or_else(|| {
            println!("Syphon server create failed (is Syphon.framework present?)");
            None
        })
    };

    let start = Instant::now();

    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { event, .. } => match event {
                WindowEvent::CloseRequested => target.exit(),
                WindowEvent::Resized(new_size) => {
                    let w = NonZeroU32::new(new_size.width.max(1)).unwrap();
                    let h = NonZeroU32::new(new_size.height.max(1)).unwrap();
                    gl_surface.resize(&gl_context, w, h);

                    // Recreate render target to match new window size
                    unsafe {
                        destroy_render_target(&gl, &mut rt);
                        rt = create_render_target(&gl, new_size.width as i32, new_size.height as i32);
                    }
                }
                _ => {}
            },

            Event::AboutToWait => {
                window.request_redraw();

                if let Ok(mut s) = store.lock() {
                    s.tick();
                }

                let t = start.elapsed().as_secs_f32();
                let size = window.inner_size();
                let w = size.width as i32;
                let h = size.height as i32;

                unsafe {
                    // ---------------------------
                    // Pass 1: render into FBO texture
                    // ---------------------------
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(rt.fbo));
                    gl.viewport(0, 0, w, h);
                    gl.clear_color(0.0, 0.0, 0.0, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);

                    gl.use_program(Some(program));
                    gl.bind_vertex_array(Some(vao));

                    if let Some(loc) = &u_time_loc {
                        gl.uniform_1_f32(Some(loc), t);
                    }
                    if let Some(loc) = &u_res_loc {
                        gl.uniform_2_f32(Some(loc), w as f32, h as f32);
                    }

                    if let Ok(s) = store.lock() {
                        for (name, loc_opt) in param_uniforms.iter() {
                            if let Some(loc) = loc_opt {
                                if let Some(v) = s.get_value(name) {
                                    gl.uniform_1_f32(Some(loc), v);
                                }
                            }
                        }
                    }

                    gl.draw_arrays(glow::TRIANGLES, 0, 3);

                    gl.bind_vertex_array(None);
                    gl.use_program(None);
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);

                    // ---------------------------
                    // Publish to Syphon (macOS only)
                    // ---------------------------
                    #[cfg(target_os = "macos")]
                    if let Some(ref srv) = syphon {
                    srv.publish_texture(rt.tex.0.get(), w, h);
                    }

                    // ---------------------------
                    // Pass 2: present that texture to the window
                    // ---------------------------
                    gl.viewport(0, 0, w, h);
                    gl.clear_color(0.02, 0.02, 0.02, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);

                    gl.use_program(Some(present_program));
                    gl.bind_vertex_array(Some(vao));

                    if let Some(loc) = &u_present_res {
                        gl.uniform_2_f32(Some(loc), w as f32, h as f32);
                    }
                    if let Some(loc) = &u_present_tex {
                        // use texture unit 0
                        gl.uniform_1_i32(Some(loc), 0);
                    }

                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(rt.tex));

                    gl.draw_arrays(glow::TRIANGLES, 0, 3);

                    gl.bind_texture(glow::TEXTURE_2D, None);
                    gl.bind_vertex_array(None);
                    gl.use_program(None);
                }

                gl_surface.swap_buffers(&gl_context).expect("swap_buffers failed");
            }

            _ => {}
        }
    }).expect("Event loop failed");
}
