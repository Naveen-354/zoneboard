fn main() {
    // Cargo passes the package's features to build scripts as cfgs, so this only runs for the desktop build.
    #[cfg(feature = "desktop")]
    tauri_build::build();
}
