//! Packaging smoke probe for the credential-free collaboration subprocess.

use local_core::sync::collaboration::TextDocument;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--noura-crdt-worker") {
        local_core::sync::collaboration::run_worker_stdio()?;
        return Ok(());
    }
    let document = TextDocument::fresh("subprocess probe 😀")?;
    if document.text() != "subprocess probe 😀" {
        return Err("collaboration subprocess returned the wrong text".into());
    }
    println!("collaboration subprocess probe passed");
    Ok(())
}
