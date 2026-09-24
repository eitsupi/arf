//! Build script for arf-console
//!
//! On Windows, this embeds an Application Manifest that enables UTF-8 support
//! for R 4.2.0 UCRT builds. Without this manifest, embedded R uses the system's
//! ANSI code page (e.g., CP932 for Japanese Windows), causing encoding issues.
//!
//! This approach is based on ark's build script:
//! <https://github.com/posit-dev/ark/blob/main/crates/ark/build.rs>
//!
//! ark is licensed under the MIT License:
//! Copyright (c) 2024 Posit Software, PBC

fn main() {
    // Reserve enough native stack for R's detected stack region on Windows.
    // Limit this link argument to the product binary so test/example targets
    // retain their own linker defaults.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => println!("cargo:rustc-link-arg-bin=arf=/STACK:10485760"),
            Ok("gnu") => println!("cargo:rustc-link-arg-bin=arf=-Wl,--stack,10485760"),
            other => println!(
                "cargo:warning=No explicit Windows stack reserve for target environment {other:?}"
            ),
        }
    }
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_ENV");

    // Copy CHANGELOG.md to OUT_DIR for embedding in binary.
    // Navigate from CARGO_MANIFEST_DIR (crate root) to the workspace root.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let changelog_path = std::path::Path::new(&manifest_dir)
        .join("..")
        .join("..")
        .join("CHANGELOG.md");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = std::path::Path::new(&out_dir).join("CHANGELOG.md");
    if changelog_path.exists() {
        std::fs::copy(&changelog_path, &dest).expect("Failed to copy CHANGELOG.md");
    } else {
        std::fs::write(&dest, "Changelog not available.").expect("Failed to write fallback");
    }
    println!("cargo:rerun-if-changed={}", changelog_path.display());

    // Re-run if manifest files change
    println!("cargo:rerun-if-changed=resources/manifest");

    #[cfg(windows)]
    {
        // Embed an Application Manifest file on Windows.
        // Turns on UTF-8 support and declares our Windows version compatibility.
        // See <crates/arf-console/resources/manifest/arf.exe.manifest>.
        //
        // We use `compile_for_everything()` to ensure the manifest is embedded
        // in both the main binary and test binaries.
        // https://github.com/nabijaczleweli/rust-embed-resource/issues/69
        let resource = std::path::Path::new("resources")
            .join("manifest")
            .join("arf-manifest.rc");
        let _ = embed_resource::compile_for_everything(resource, embed_resource::NONE);
    }
}
