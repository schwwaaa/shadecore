//! Preview presenter
//!
//! The presenter is intentionally separated from the renderer:
//! - The renderer draws into an offscreen texture at the *authoritative* resolution.
//! - The presenter decides how that texture appears in the local window (scaling / letterboxing).
//!
//! This keeps "preview UX" changes (e.g. fit/fill, showing debug overlays, headless mode) from
//! spilling into the core rendering path or output backends.
//!
use glow::HasContext;

#[derive(Debug)]
pub enum Presenter {
    /// Uses a real window surface to present frames (normal interactive mode).
    Window(WindowPresenter),
    /// Does not present anything (useful for headless output/record-only runs).
    Null(NullPresenter),
}

impl Presenter {
    pub fn is_enabled(&self) -> bool {
        matches!(self, Presenter::Window(_))
    }

    /// Called when the preview window surface should be resized.
    ///
    /// For the null presenter, this is a no-op.
    pub fn resize_window_surface<GlContext, GlSurface>(
        &mut self,
        gl_context: &GlContext,
        gl_surface: &GlSurface,
        w: u32,
        h: u32,
        resize_fn: impl FnOnce(&GlSurface, &GlContext, u32, u32),
    ) {
        match self {
            Presenter::Window(_) => resize_fn(gl_surface, gl_context, w, h),
            Presenter::Null(_) => {}
        }
    }

    /// Present the render target texture to the preview window.
    ///
    /// `swap_fn` is injected so this module doesn't need to know glutin surface types.
    pub fn present<GlContext, GlSurface>(
        &mut self,
        gl: &glow::Context,
        program: glow::NativeProgram,
        rt_tex: glow::NativeTexture,
        src_w: i32,
        src_h: i32,
        win_w: i32,
        win_h: i32,
        preview_scale_mode: i32,
        gl_context: &GlContext,
        gl_surface: &GlSurface,
        swap_fn: impl FnOnce(&GlSurface, &GlContext),
        set_u_resolution: impl FnOnce(&glow::Context, glow::NativeProgram, i32, i32),
        set_u_src_resolution: impl FnOnce(&glow::Context, glow::NativeProgram, i32, i32),
        set_u_scale_mode: impl FnOnce(&glow::Context, glow::NativeProgram, i32),
    ) {
        match self {
            Presenter::Window(p) => {
                p.present(
                    gl,
                    program,
                    rt_tex,
                    src_w,
                    src_h,
                    win_w,
                    win_h,
                    preview_scale_mode,
                    gl_context,
                    gl_surface,
                    swap_fn,
                    set_u_resolution,
                    set_u_src_resolution,
                    set_u_scale_mode,
                );
            }
            Presenter::Null(_) => {}
        }
    }
}

#[derive(Debug)]
pub struct WindowPresenter {
    pub vao: glow::NativeVertexArray,
}

impl WindowPresenter {
    #[allow(clippy::too_many_arguments)]
    pub fn present<GlContext, GlSurface>(
        &mut self,
        gl: &glow::Context,
        program: glow::NativeProgram,
        rt_tex: glow::NativeTexture,
        src_w: i32,
        src_h: i32,
        win_w: i32,
        win_h: i32,
        preview_scale_mode: i32,
        gl_context: &GlContext,
        gl_surface: &GlSurface,
        swap_fn: impl FnOnce(&GlSurface, &GlContext),
        set_u_resolution: impl FnOnce(&glow::Context, glow::NativeProgram, i32, i32),
        set_u_src_resolution: impl FnOnce(&glow::Context, glow::NativeProgram, i32, i32),
        set_u_scale_mode: impl FnOnce(&glow::Context, glow::NativeProgram, i32),
    ) {
        unsafe {
            gl.viewport(0, 0, win_w, win_h);
            gl.clear_color(0.02, 0.02, 0.02, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(program));
            gl.bind_vertex_array(Some(self.vao));

            set_u_resolution(gl, program, win_w, win_h);
            set_u_src_resolution(gl, program, src_w, src_h);
            set_u_scale_mode(gl, program, preview_scale_mode);

            if let Some(loc) = gl.get_uniform_location(program, "u_tex") {
                gl.uniform_1_i32(Some(&loc), 0);
            }
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(rt_tex));

            gl.draw_arrays(glow::TRIANGLES, 0, 3);

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);
        }

        swap_fn(gl_surface, gl_context);
    }
}

#[derive(Debug, Default)]
pub struct NullPresenter;
