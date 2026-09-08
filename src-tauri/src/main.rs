fn main() {
    if std::env::args().nth(1).as_deref() == Some("--noura-crdt-worker") {
        if local_core::sync::collaboration::run_worker_stdio().is_err() {
            std::process::exit(1);
        }
        return;
    }
    noura_desktop::run();
}
