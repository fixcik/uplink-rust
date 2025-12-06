extern crate bindgen;

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) {
    fs::create_dir_all(dst).expect("Failed to create destination directory");
    for entry in fs::read_dir(src).expect("Failed to read source directory") {
        let entry = entry.expect("Failed to read directory entry");
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            fs::copy(&src_path, &dst_path).expect("Failed to copy file");
        }
    }
}

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not defined"));
    let is_windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");

    // Directory containing uplink-c project source
    let uplink_c_src = PathBuf::from("uplink-c");

    // Don't compile the uplink-c libraries when building the docs for not requiring Go to be
    // installed in the Docker image for building them used by docs.rs
    if env::var("DOCS_RS").is_err() {
        // Build uplink-c generates precompiled lib and header files in .build directory.
        let build_dir = uplink_c_src.join(".build");
        fs::create_dir_all(&build_dir).ok();
        fs::create_dir_all(build_dir.join("uplink")).ok();

        if is_windows {
            // On Windows, build DLL directly with go build to avoid loader lock deadlock
            let status = Command::new("go")
                .args([
                    "build",
                    "-ldflags=-s -w",
                    "-buildmode=c-shared",
                    "-o",
                    ".build/libuplink.dll",
                    ".",
                ])
                .current_dir(&uplink_c_src)
                .status()
                .expect("Failed to run go build for Windows DLL");
            if !status.success() {
                panic!("go build failed for Windows DLL");
            }
        } else {
            // On Unix, use make
            Command::new("make")
                .arg("build")
                .current_dir(&uplink_c_src)
                .status()
                .expect("Failed to run make command from build.rs.");
        }

        // Copy header files
        let headers = ["uplink_definitions.h", "uplink_compat.h"];
        for header in &headers {
            let src = uplink_c_src.join(header);
            let dst = build_dir.join("uplink").join(header);
            if src.exists() {
                fs::copy(&src, &dst).ok();
            }
        }
        // Copy generated header - go build creates libuplink.h next to the dll
        if is_windows {
            let generated_header = build_dir.join("libuplink.h");
            if generated_header.exists() {
                fs::copy(&generated_header, build_dir.join("uplink/uplink.h")).ok();
            }
        } else {
            let generated_header = build_dir.join("uplink.h");
            if generated_header.exists() {
                fs::copy(&generated_header, build_dir.join("uplink/uplink.h")).ok();
            }
        }
    }

    // Directory containing uplink-c project for building
    let uplink_c_dir = out_dir.join("uplink-c");

    // Copy project to OUT_DIR for building
    if uplink_c_dir.exists() {
        fs::remove_dir_all(&uplink_c_dir).ok();
    }
    copy_dir_recursive(&uplink_c_src, &uplink_c_dir);

    if env::var("DOCS_RS").is_ok() {
        // Use the precompiled uplink-c libraries for building the docs by docs.rs.
        let docs_rs_dir = PathBuf::from(".docs-rs");
        let build_dir = uplink_c_dir.join(".build");
        if docs_rs_dir.exists() {
            copy_dir_recursive(&docs_rs_dir, &build_dir);
        }
    } else {
        // Delete the generated build files for avoiding `cargo publish` to complain about modifying
        // things outside of the OUT_DIR.
        let build_dir = uplink_c_src.join(".build");
        if build_dir.exists() {
            fs::remove_dir_all(&build_dir).ok();
        }
    }

    // Directory containing uplink-c build
    let uplink_c_build = uplink_c_dir.join(".build");

    // Header file with complete API interface
    let uplink_c_header = uplink_c_build.join("uplink/uplink.h");

    // Link to uplink-c library during build
    // On Windows, use dynamic linking to avoid Go runtime loader lock deadlock
    // On other platforms, use static linking
    if is_windows {
        println!("cargo:rustc-link-lib=dylib=uplink");
        // Windows system libraries required by Go runtime
        println!("cargo:rustc-link-lib=ws2_32");
        println!("cargo:rustc-link-lib=userenv");
        println!("cargo:rustc-link-lib=bcrypt");
        println!("cargo:rustc-link-lib=ntdll");
    } else {
        println!("cargo:rustc-link-lib=static=uplink");
    }

    // Add uplink-c build directory to library search path
    println!(
        "cargo:rustc-link-search={}",
        uplink_c_build.to_string_lossy()
    );

    // Also copy DLL to OUT_DIR for runtime (Windows)
    if is_windows {
        let dll_src = uplink_c_build.join("libuplink.dll");
        let dll_dst = out_dir.join("libuplink.dll");
        if dll_src.exists() {
            fs::copy(&dll_src, &dll_dst).ok();
            // Also try to copy to target directory for easier access
            if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
                let target_dir = PathBuf::from(manifest_dir).join("../target/debug");
                if target_dir.exists() {
                    fs::copy(&dll_src, target_dir.join("libuplink.dll")).ok();
                }
            }
        }
    }

    // Make uplink-c interface header a dependency of the build
    println!(
        "cargo:rerun-if-changed={}",
        uplink_c_header.to_string_lossy()
    );

    // Manually link to core and security libs on MacOS
    //
    // N.B.: `CARGO_CFG_TARGET_OS` should be read instead of `cfg(target_os = "macos")`. The latter
    // detects the host OS that is building the `build.rs` script, not the target OS.
    if env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS is not defined") == "macos" {
        println!("cargo:rustc-flags=-l framework=CoreFoundation -l framework=Security");
    }

    bindgen::Builder::default()
        // Use 'allow lists' to avoid generating bindings for system header includes
        // a lot of which isn't required and can't be handled safely anyway.
        // uplink-c uses consistent naming so an allow list is much easier than a block list.
        // All uplink types start with Uplink
        .allowlist_type("Uplink.*")
        // All edge services types start with Edge
        .allowlist_type("Edge.*")
        // except for uplink_const_char
        .allowlist_type("uplink_const_char")
        // All uplink functions start with uplink_
        .allowlist_function("uplink_.*")
        // All edge services functions start with edge_
        .allowlist_function("edge_.*")
        // Uplink error code #define's start with UPLINK_ERROR_
        .allowlist_var("UPLINK_ERROR_.*")
        // Edge services error code #define's start with EDGE_ERROR_
        .allowlist_var("EDGE_ERROR_.*")
        // This header file is the main API interface and includes all other header files that are required
        // (bindgen runs c preprocessor so we don't need to include nested headers)
        .header(
            uplink_c_dir
                .join(".build/uplink/uplink.h")
                .to_string_lossy(),
        )
        // Also make headers included by main header dependencies of the build
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        // Generate bindings
        .generate()
        .expect("Error generating bindings.")
        // Write bindings to file to be referenced by main build
        .write_to_file(out_dir.join("bindings.rs"))
        .expect("Error writing bindings to file.");
}
