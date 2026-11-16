fn main() {
    // Embed the app icon so Explorer, the taskbar and shortcuts show it.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("wax.rc", embed_resource::NONE).manifest_optional().unwrap();
    }
    println!("cargo:rerun-if-changed=wax.rc");
    println!("cargo:rerun-if-changed=assets/wax.ico");
}
