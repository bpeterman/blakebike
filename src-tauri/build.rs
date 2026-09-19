fn main() {
    // Tauri embeds this file into macOS development builds for the Dock icon.
    // Track it explicitly so `pnpm restart` rebuilds after icon generation.
    println!("cargo:rerun-if-changed=icons/icon.icns");
    tauri_build::build()
}
