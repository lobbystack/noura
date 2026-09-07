//! Test-only Yrs ↔ Yjs fixture generator. No production keys or workspace data.
use local_core::sync::collaboration::TextDocument;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = TextDocument::fresh_generation("A😀 e\u{301} 中文\r\nremove me", "interop-v1")?;
    let (edited, delta) = base.replace_text("A😀 e\u{301} 中文\r\nreplacement 🦀")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "version": 1, "producer": "yrs@0.26.0", "base": base.state()?,
            "baseVector": base.state_vector(), "baseText": base.text(),
            "delta": delta, "state": edited.state()?, "text": edited.text(),
            "vector": edited.state_vector()
        }))?
    );
    Ok(())
}
