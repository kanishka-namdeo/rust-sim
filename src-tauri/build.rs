// src-tauri/build.rs
// M-T2 (Tauri repurpose): the standard Tauri build script — runs
// `tauri-build` to embed the `tauri.conf.json`, icons, and bundled
// assets at compile time via `tauri::generate_context!()` in main.rs.
fn main() {
    tauri_build::build()
}
