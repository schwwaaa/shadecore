// build.rs
//
// This build script does four things (macOS only):
// 1) Compiles our Objective‑C Syphon bridge (native/syphon_bridge.m) into a static lib.
// 2) Links against the Syphon.framework we vendor inside this repo (vendor/Syphon.framework).
// 3) Adds an LC_RPATH so the runtime loader can actually *find* Syphon.framework when you run
//    `cargo run` (or any raw executable build).
// 4) Copies vendor/Syphon.framework into target/{debug|release}/Syphon.framework so the rpath
//    we add will resolve correctly.
//
// Why this is needed:
// - Our Rust binary references Syphon as: @rpath/Syphon.framework/Versions/A/Syphon
// - If the binary has *no* rpaths, dyld can’t resolve @rpath and you get:
//     "Reason: no LC_RPATH's found"
//
// For app-bundle distribution later, we’ll instead copy Syphon.framework into:
//   MyApp.app/Contents/Frameworks/Syphon.framework
// and rely on the rpath @executable_path/../Frameworks (we also add that here).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Only do Syphon wiring on macOS. Other platforms should compile fine without it.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }

    // Rebuild if these change
    println!("cargo:rerun-if-changed=native/syphon_bridge.m");
    println!("cargo:rerun-if-changed=native/syphon_bridge.h");
    println!("cargo:rerun-if-changed=vendor/Syphon.framework");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_dir = manifest_dir.join("vendor");
    let syphon_framework = vendor_dir.join("Syphon.framework");

    if !syphon_framework.exists() {
        panic!(
            "Syphon.framework not found at {}. Put a built Syphon.framework in vendor/.",
            syphon_framework.display()
        );
    }

    // -------------------------
    // 1) Compile the ObjC bridge into libsyphon_bridge.a
    // -------------------------
    let mut cc_build = cc::Build::new();
    cc_build
        .file("native/syphon_bridge.m")
        .flag("-fobjc-arc")
        .flag("-ObjC")
        .include(syphon_framework.join("Headers"))
        // Some Syphon distributions also have Headers under Versions/A/Headers
        .include(syphon_framework.join("Versions/A/Headers"))
        // Allow finding frameworks via -F
        .flag(&format!("-F{}", vendor_dir.display()))
        // Silence noisy deprecation warnings (optional; remove if you want them visible)
        .flag("-Wno-deprecated-declarations");

    cc_build.compile("syphon_bridge"); // -> libsyphon_bridge.a

    // -------------------------
    // 2) Link against Syphon.framework + required Apple frameworks
    // -------------------------
    println!("cargo:rustc-link-search=framework={}", vendor_dir.display());
    println!("cargo:rustc-link-lib=framework=Syphon");
    println!("cargo:rustc-link-lib=framework=Cocoa");
    println!("cargo:rustc-link-lib=framework=OpenGL");

    // -------------------------
    // 3) Add runtime rpaths so dyld can resolve @rpath/Syphon.framework/...
    // -------------------------
    //
    // We add TWO rpaths:
    // - @executable_path : so `cargo run` works if Syphon.framework sits next to the binary
    // - @executable_path/../Frameworks : so future .app bundles can place frameworks in Contents/Frameworks
    //
    // IMPORTANT: rustc-link-arg is stable and works across profiles.
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    // -------------------------
    // 4) Copy Syphon.framework next to the built binary for `cargo run`
    // -------------------------
    //
    // Cargo puts the executable at:
    //   <workspace>/target/<profile>/glsl_engine
    //
    // So we copy:
    //   vendor/Syphon.framework -> target/<profile>/Syphon.framework
    //
    // Then @executable_path (the directory containing the binary) is also the directory
    // containing Syphon.framework, and dyld can resolve @rpath/Syphon.framework...
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());

    // Respect CARGO_TARGET_DIR if set, otherwise default to <manifest>/target
    let target_dir = env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"));

    let dest_dir = target_dir.join(&profile).join("Syphon.framework");

    // Copy only if missing or obviously stale (simple heuristic: missing dest)
    if !dest_dir.exists() {
        copy_dir_recursive(&syphon_framework, &dest_dir)
            .unwrap_or_else(|e| panic!("Failed to copy Syphon.framework -> {}: {e}", dest_dir.display()));
    }
}

/// Recursively copy a directory (framework bundles are directories).
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        // If something exists, remove it so we don't end up with mixed versions.
        fs::remove_dir_all(dst)?;
    }
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if file_type.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)?;
        } else if file_type.is_symlink() {
            // Preserve symlinks in framework bundles (common in Versions layout)
            let target = fs::read_link(&from)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &to)?;
        }
    }
    Ok(())
}
