fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("icons/icon.ico")
            .set("ProductName", "Noura")
            .set("FileDescription", "Noura")
            .compile()
            .expect("failed to embed the Noura Windows icon");
    }
}
