fn main() {
    // Git launches this binary as editor and as askpass, and a task's environment carries the
    // variables of both: the editor is recognized by its flag and goes first.
    if let Some(code) = rama_lib::editor::client_main().or_else(rama_lib::askpass::client_main) {
        std::process::exit(code);
    }
    rama_lib::run()
}
