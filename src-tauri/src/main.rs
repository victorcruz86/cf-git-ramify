fn main() {
    // Git launches this binary as editor and as askpass, and a task's environment carries the
    // variables of both: the editor is recognized by its flag and goes first.
    if let Some(code) = ramify_lib::editor::client_main().or_else(ramify_lib::askpass::client_main)
    {
        std::process::exit(code);
    }
    // Before Tauri creates the webview, which opens its storage.
    if let Some(home) = std::env::var_os("HOME") {
        let library = std::path::Path::new(&home).join("Library");
        match ramify_lib::migration::migrate_webview_storage(&library) {
            Ok(ramify_lib::migration::Migration::Copied) => {
                eprintln!("Copied rama's webview storage for Ramify")
            }
            Ok(ramify_lib::migration::Migration::NotNeeded) => {}
            Err(error) => eprintln!("Could not copy rama's webview storage: {error}"),
        }
    }
    ramify_lib::run()
}
