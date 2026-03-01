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
    // Copy CHANGELOG.md to OUT_DIR for embedding in binary
    let changelog_path = std::path::Path::new("../../CHANGELOG.md");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = std::path::Path::new(&out_dir).join("CHANGELOG.md");
    if changelog_path.exists() {
        std::fs::copy(changelog_path, &dest).expect("Failed to copy CHANGELOG.md");
    } else {
        std::fs::write(&dest, "Changelog not available.").expect("Failed to write fallback");
    }
    println!("cargo:rerun-if-changed=../../CHANGELOG.md");

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
