use rmcp::ServiceExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
