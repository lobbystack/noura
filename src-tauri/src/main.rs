fn main() {
    let mut arguments = std::env::args().skip(1);
    match arguments.next().as_deref() {
        Some("--noura-crdt-worker") => {
            if local_core::sync::collaboration::run_worker_stdio().is_err() {
                std::process::exit(1);
            }
        }
        // `noura mcp --workspace <path>` serves a workspace to external AI
        // tools over stdio, so users don't need a separate binary.
        Some("mcp") => {
            let arguments = arguments.collect::<Vec<_>>();
            let Some(workspace) = noura_mcp::workspace_argument(&arguments) else {
                eprintln!("usage: noura mcp --workspace <path>");
                std::process::exit(2);
            };
            let served = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| {
                    runtime
                        .block_on(noura_mcp::serve_stdio(workspace))
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = served {
                eprintln!("noura mcp: {error}");
                std::process::exit(1);
            }
        }
        _ => noura_desktop::run(),
    }
}
