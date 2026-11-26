//! Embed assets/voicebar.ico as Windows icon resource 1 so Explorer, Task Manager,
//! Start Menu, and shortcuts display Voicebar's icon.

fn main() {
    println!("cargo:rerun-if-changed=assets/voicebar.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/voicebar.ico")
        .set("ProductName", "Voicebar")
        .set("FileDescription", "Voicebar — Local Real-Time GPU Dictation")
        .set("CompanyName", "Voicebar");
    if let Err(e) = res.compile() {
        println!("cargo:warning=voicebar icon embedding skipped: {e}");
    }
}
