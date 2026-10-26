fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=icon.ico");
        winresource::WindowsResource::new()
            .set_icon("icon.ico")
            .set("FileDescription", "ggPdf PDF viewer")
            .set("ProductName", "ggPdf")
            .compile()
            .expect("embed Windows app icon");
    }
}
