fn main() {
    #[cfg(feature = "settings-ui")]
    tauri_build::build();
}
