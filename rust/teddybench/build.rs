/// Embeds the icon and version information into teddybench.exe, so Explorer and shortcuts show it.
fn main() {
    println!("cargo:rerun-if-changed=assets/teddybench.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/teddybench.ico");
        res.set("ProductName", "TeddyBench");
        res.set("FileDescription", "TeddyBench");
        res.compile().expect("embedding the Windows resources");
    }
}
