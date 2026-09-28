fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--noura-crdt-worker") {
        local_core::sync::collaboration::run_worker_stdio()?;
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let workspace =
        noura_mcp::workspace_argument(&arguments).ok_or("usage: noura-mcp --workspace <path>")?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(noura_mcp::serve_stdio(workspace))
}
