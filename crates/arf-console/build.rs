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
