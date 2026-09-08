use rmcp::ServiceExt;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--noura-crdt-worker") {
        local_core::sync::collaboration::run_worker_stdio()?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let workspace = arguments
        .windows(2)
        .find(|pair| pair[0] == "--workspace")
        .map(|pair| pair[1].clone())
        .ok_or("usage: noura-mcp --workspace <path>")?;
    let engine = local_core::WorkspaceEngine::open(workspace)?;
    let handler = noura_mcp::NouraMcp::new(engine);
    tokio::spawn(handler.clone().run_reconciliation());
    let server = handler.serve(rmcp::transport::stdio()).await?;
    server.waiting().await?;
    Ok(())
}
