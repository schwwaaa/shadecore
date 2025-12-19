#[cfg(target_os = "windows")]
mod win {
  use std::ffi::CString;

  #[link(name = "spout_bridge")]
  extern "C" {
    fn spout_init_sender(sender_name: *const i8, width: i32, height: i32) -> i32;
    fn spout_send_gl_texture(gl_tex_id: u32, width: i32, height: i32, invert: i32) -> i32;
    fn spout_set_sender_name(sender_name: *const i8) -> i32;
    fn spout_shutdown();
  }

  pub struct SpoutOut {
    name: CString,
    invert: bool,
  }

  impl SpoutOut {
    pub fn new(name: &str, width: i32, height: i32, invert: bool) -> anyhow::Result<Self> {
      let c = CString::new(name)?;
      let ok = unsafe { spout_init_sender(c.as_ptr(), width, height) };
      anyhow::ensure!(ok == 1, "Spout init sender failed");
      Ok(Self { name: c, invert })
    }

    pub fn set_name(&mut self, name: &str) -> anyhow::Result<()> {
      self.name = CString::new(name)?;
      let ok = unsafe { spout_set_sender_name(self.name.as_ptr()) };
      anyhow::ensure!(ok == 1, "Spout set name failed");
      Ok(())
    }

    /// Call once per frame after you’ve rendered into `gl_tex_id`.
    pub fn send_gl_texture(&self, gl_tex_id: u32, width: i32, height: i32) -> anyhow::Result<()> {
      let ok = unsafe { spout_send_gl_texture(gl_tex_id, width, height, if self.invert { 1 } else { 0 }) };
      anyhow::ensure!(ok == 1, "Spout send texture failed");
      Ok(())
    }
  }

  impl Drop for SpoutOut {
    fn drop(&mut self) {
      unsafe { spout_shutdown() };
    }
  }

  pub use SpoutOut as PlatformSpoutOut;
}

#[cfg(target_os = "windows")]
pub use win::PlatformSpoutOut;

#[cfg(not(target_os = "windows"))]
pub struct PlatformSpoutOut;

#[cfg(not(target_os = "windows"))]
impl PlatformSpoutOut {
  pub fn new(_: &str, _: i32, _: i32, _: bool) -> anyhow::Result<Self> {
    anyhow::bail!("Spout is Windows-only");
  }
}
