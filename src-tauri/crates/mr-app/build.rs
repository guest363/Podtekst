fn main() {
    // Tauri-метаданные (иконки, config) генерируются только в CI-сборке
    // с фичей `tauri-ui`, где доступен крейт tauri-build и системный WebView.
    // tauri.conf.json лежит рядом с пакетом (crates/mr-app/) — канон Tauri.
    #[cfg(feature = "tauri-ui")]
    {
        tauri_build::build()
    }
    #[cfg(not(feature = "tauri-ui"))]
    {
        println!("cargo:rerun-if-changed=../../ui/web/static/index.html");
    }
}
