// build.rs
//
// macOS (unchanged):
// 1) Compiles our Objective-C Syphon bridge (native/syphon_bridge.m) into a static lib.
// 2) Links against the Syphon.framework we vendor inside this repo (vendor/Syphon.framework).
// 3) Adds an LC_RPATH so the runtime loader can actually *find* Syphon.framework when you run
//    `cargo run` (or any raw executable build).
// 4) Copies vendor/Syphon.framework into target/{debug|release}/Syphon.framework so the rpath
//    we add will resolve correctly.
//
// Windows (added):
// - Builds a C++ Spout bridge (native/spout_bridge) via CMake and links against spout_bridge.
//
// NOTE:
// - We key off CARGO_CFG_TARGET_OS (the *target*), not the host OS.
// - This means: macOS builds only run Syphon; Windows builds only run Spout.
// - Nothing here overwrites your Syphon wiring; it remains intact.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Re-run if build script changes
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    println!("cargo:rerun-if-env-changed=PROFILE");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_DIR");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_else(|_| "unknown".into());

    // -------------------------
    // macOS: Syphon wiring (your original behavior)
    // -------------------------
    if target_os == "macos" {
        // Rebuild if these change
        println!("cargo:rerun-if-changed=native/syphon_bridge.m");
        println!("cargo:rerun-if-changed=native/syphon_bridge.h");
        println!("cargo:rerun-if-changed=vendor/Syphon.framework");

        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let vendor_dir = manifest_dir.join("vendor");
        let syphon_framework = vendor_dir.join("Syphon.framework");

        if !syphon_framework.exists() {
            panic!(
                "Syphon.framework not found at {}. Did you vendor it in /vendor?",
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
        // We add:
        //   -Wl,-rpath,@executable_path
        // so the loader looks next to the executable for Syphon.framework.
        //
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");

        // -------------------------
        // 4) Copy Syphon.framework into target/<profile>/Syphon.framework
        // -------------------------
        let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());

        // Respect CARGO_TARGET_DIR if set, otherwise default to <manifest>/target
        let target_dir = env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| manifest_dir.join("target"));

        let dest_dir = target_dir.join(&profile).join("Syphon.framework");

        // Copy only if missing (your original heuristic)
        if !dest_dir.exists() {
            copy_dir_recursive(&syphon_framework, &dest_dir).unwrap_or_else(|e| {
                panic!(
                    "Failed to copy Syphon.framework -> {}: {e}",
                    dest_dir.display()
                )
            });
        }
    }

    // -------------------------
    // Windows: Spout bridge (CMake)
    // -------------------------
    if target_os == "windows" {
        // Rebuild triggers
        println!("cargo:rerun-if-changed=native/spout_bridge/CMakeLists.txt");
        println!("cargo:rerun-if-changed=native/spout_bridge/spout_bridge.cpp");
        println!("cargo:rerun-if-changed=native/spout_bridge/spout_bridge.h");
        println!("cargo:rerun-if-changed=native/spout2");

        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());

        // MSVC multi-config expects Debug/Release folders
        let cmake_cfg = if profile.eq_ignore_ascii_case("release") {
            "Release"
        } else {
            "Debug"
        };

        // Path to spout2 sources you vendored/downloaded
        let spout2_dir = manifest_dir.join("native").join("spout2");
        if !spout2_dir.exists() {
            panic!(
                "Spout2 source dir not found at {}. Expected native/spout2 to exist.",
                spout2_dir.display()
            );
        }

        // Build the spout_bridge target (NOT 'install' — that caused install.vcxproj failure)
        let mut cfg = cmake::Config::new("native/spout_bridge");
        cfg.define("SPOUT2_DIR", spout2_dir.to_string_lossy().to_string());
        cfg.profile(cmake_cfg);
        cfg.build_target("spout_bridge");

        let dst = cfg.build();

        // cmake crate output layout for VS generators:
        // <dst>/build/<Debug|Release> contains:
        //   - spout_bridge.lib (import library)
        //   - spout_bridge.dll (runtime)
        let bin_lib_dir = dst.join("build").join(cmake_cfg);

        println!("cargo:rustc-link-search=native={}", bin_lib_dir.display());
        // Link against the DLL import lib (not a static lib)
        println!("cargo:rustc-link-lib=dylib=spout_bridge");

        // Ensure spout_bridge.dll is beside the final exe (cargo run).
        let dll_path = bin_lib_dir.join("spout_bridge.dll");
        if dll_path.exists() {
            let target_dir = env::var("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| manifest_dir.join("target"));
            let profile_dir = if profile.to_lowercase() == "release" {
                target_dir.join("release")
            } else {
                target_dir.join("debug")
            };
            let out_dll = profile_dir.join("spout_bridge.dll");
            let _ = fs::create_dir_all(&profile_dir);
            let _ = fs::copy(&dll_path, &out_dll);
            // Also copy to deps/ for some cargo invocations
            let deps_dir = profile_dir.join("deps");
            let _ = fs::create_dir_all(&deps_dir);
            let _ = fs::copy(&dll_path, deps_dir.join("spout_bridge.dll"));
        } else {
            eprintln!("warning: spout_bridge.dll not found at {}", dll_path.display());
        }

        // Common Windows libs (safe even if some are unused)
        println!("cargo:rustc-link-lib=opengl32");
        println!("cargo:rustc-link-lib=user32");
        println!("cargo:rustc-link-lib=gdi32");
        println!("cargo:rustc-link-lib=shell32");
        println!("cargo:rustc-link-lib=ole32");
        println!("cargo:rustc-link-lib=uuid");
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
            let _target = fs::read_link(&from)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(_target, &to)?;
        }
    }
    Ok(())
}
