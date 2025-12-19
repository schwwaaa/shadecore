// build.rs
//
// macOS:
//   - optionally compiles the Syphon ObjC bridge
//   - links Syphon.framework if vendored
//   - emits cfg(has_syphon) when Syphon.framework exists
//
// Windows:
//   - builds native/spout_bridge via CMake (which builds Spout + a small C-ABI bridge DLL)
//   - links to spout_bridge import library so Rust can resolve spout_* symbols
//   - copies spout_bridge.dll next to the built exe for `cargo run`
//
// Also:
//   - declares cfg(has_syphon) to rustc (silences unexpected_cfgs warnings on non-mac targets)

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    // Tell rustc that `cfg(has_syphon)` is an allowed cfg key (silences warnings on Windows/Linux).
    println!("cargo:rustc-check-cfg=cfg(has_syphon)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "macos" {
        build_syphon_macos();
        return;
    }

    if target_os == "windows" {
        build_spout_windows();
        return;
    }

    // Other platforms: nothing special.
}

fn build_syphon_macos() {
    // Rebuild if these change
    println!("cargo:rerun-if-changed=native/syphon_bridge.m");
    println!("cargo:rerun-if-changed=native/syphon_bridge.h");
    println!("cargo:rerun-if-changed=vendor/Syphon.framework");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let vendor_dir = manifest_dir.join("vendor");
    let syphon_framework = vendor_dir.join("Syphon.framework");

    // Syphon is OPTIONAL. If missing, compile Texture-only on macOS.
    if !syphon_framework.exists() {
        println!(
            "cargo:warning=Syphon.framework not found at {} — building WITHOUT Syphon support (Texture-only on macOS).",
            syphon_framework.display()
        );
        return;
    }

    // Tell Rust code that Syphon is available in this build.
    println!("cargo:rustc-cfg=has_syphon");

    // 1) Compile the ObjC bridge into libsyphon_bridge.a
    let mut cc_build = cc::Build::new();
    cc_build
        .file("native/syphon_bridge.m")
        .flag("-fobjc-arc")
        .flag("-ObjC")
        .include(syphon_framework.join("Headers"))
        .include(syphon_framework.join("Versions/A/Headers"))
        .flag(&format!("-F{}", vendor_dir.display()))
        .flag("-Wno-deprecated-declarations");

    cc_build.compile("syphon_bridge");

    // 2) Link Syphon.framework + required Apple frameworks
    println!("cargo:rustc-link-search=framework={}", vendor_dir.display());
    println!("cargo:rustc-link-lib=framework=Syphon");
    println!("cargo:rustc-link-lib=framework=Cocoa");
    println!("cargo:rustc-link-lib=framework=OpenGL");

    // 3) Add runtime rpaths so dyld can resolve @rpath/Syphon.framework/...
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");

    // 4) Copy Syphon.framework next to the built binary for `cargo run`
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let target_dir = env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"));
    let dest_dir = target_dir.join(&profile).join("Syphon.framework");

    if !dest_dir.exists() {
        copy_dir_recursive(&syphon_framework, &dest_dir)
            .unwrap_or_else(|e| panic!("Failed to copy Syphon.framework -> {}: {e}", dest_dir.display()));
    }
}

fn build_spout_windows() {
    // If these change, rerun
    println!("cargo:rerun-if-changed=native/spout_bridge/CMakeLists.txt");
    println!("cargo:rerun-if-changed=native/spout_bridge/spout_bridge.cpp");
    println!("cargo:rerun-if-changed=native/spout_bridge/spout_bridge.h");
    println!("cargo:rerun-if-changed=native/spout2");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let spout2_dir = manifest_dir.join("native").join("spout2");

    if !spout2_dir.exists() {
        println!(
            "cargo:warning=Spout2 directory not found at {} — building WITHOUT Spout support.",
            spout2_dir.display()
        );
        return;
    }

    // Map Cargo profile -> CMake config
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let cmake_build_type = if profile.eq_ignore_ascii_case("release") {
        "Release"
    } else {
        "Debug"
    };

    // Build the CMake project (spout_bridge DLL + import lib)
    let dst = cmake::Config::new("native/spout_bridge")
        .define("SPOUT2_DIR", spout2_dir.to_string_lossy().to_string())
        .profile(cmake_build_type)
        .build_target("spout_bridge")   // <-- THIS is the key fix
        .build();

    // Find spout_bridge import library + dll (cmake crate layouts vary by generator)
    let (lib_dir, dll_path) = find_spout_bridge_artifacts(&dst)
        .unwrap_or_else(|| panic!("Could not find spout_bridge.lib / spout_bridge.dll under {}", dst.display()));

    // Link against the import library so Rust can resolve spout_* externs
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=dylib=spout_bridge");

    // Copy DLL next to the exe for `cargo run`
    let target_dir = env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| manifest_dir.join("target"));
    let exe_dir = target_dir.join(&profile);

    let dest_dll = exe_dir.join("spout_bridge.dll");
    if let Err(e) = fs::create_dir_all(&exe_dir) {
        panic!("Failed to create target dir {}: {e}", exe_dir.display());
    }
    if let Err(e) = fs::copy(&dll_path, &dest_dll) {
        panic!(
            "Failed to copy {} -> {} : {e}",
            dll_path.display(),
            dest_dll.display()
        );
    }
}

fn find_spout_bridge_artifacts(dst: &Path) -> Option<(PathBuf, PathBuf)> {
    // Common locations produced by cmake crate across generators:
    // - dst/lib + dst/bin
    // - dst/build/<cfg>/...
    // - dst/<cfg>/...
    let candidates = [
        dst.join("lib"),
        dst.join("bin"),
        dst.join("build"),
        dst.join("build").join("Debug"),
        dst.join("build").join("Release"),
        dst.join("Debug"),
        dst.join("Release"),
    ];

    // Helper: search recursively (limited depth) for a filename.
    fn find_file(root: &Path, filename: &str, depth: usize) -> Option<PathBuf> {
        if depth == 0 || !root.exists() {
            return None;
        }
        let rd = fs::read_dir(root).ok()?;
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file() {
                if p.file_name().map(|s| s.to_string_lossy().eq_ignore_ascii_case(filename)) == Some(true) {
                    return Some(p);
                }
            } else if p.is_dir() {
                if let Some(found) = find_file(&p, filename, depth - 1) {
                    return Some(found);
                }
            }
        }
        None
    }

    // Find lib first (import library)
    let mut lib_path: Option<PathBuf> = None;
    for c in &candidates {
        if let Some(p) = find_file(c, "spout_bridge.lib", 6) {
            lib_path = Some(p);
            break;
        }
    }
    let lib_path = lib_path.or_else(|| find_file(dst, "spout_bridge.lib", 8))?;

    // Find dll
    let mut dll_path: Option<PathBuf> = None;
    for c in &candidates {
        if let Some(p) = find_file(c, "spout_bridge.dll", 6) {
            dll_path = Some(p);
            break;
        }
    }
    let dll_path = dll_path.or_else(|| find_file(dst, "spout_bridge.dll", 8))?;

    Some((lib_path.parent()?.to_path_buf(), dll_path))
}

/// Recursively copy a directory (framework bundles are directories).
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
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
            let target = fs::read_link(&from)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &to)?;
        }
    }
    Ok(())
}
