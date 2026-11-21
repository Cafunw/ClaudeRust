fn main() {
    let mut attributes = tauri_build::Attributes::new();
    // Windows (MSVC): link the app manifest (Common Controls v6, which the
    // native dialogs need) into every binary of the package. tauri-build
    // would only add it to the app's resources, and the unit-test binary then
    // fails to start with STATUS_ENTRYPOINT_NOT_FOUND.
    let target = |k: &str| std::env::var(k).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        let manifest = std::path::Path::new(&target("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        attributes = attributes.windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest());
    }
    tauri_build::try_build(attributes).expect("tauri build");
}
