fn main() {
    println!("cargo:rerun-if-env-changed=NOURA_SYNC_ORIGIN");
    tauri_build::build()
}
