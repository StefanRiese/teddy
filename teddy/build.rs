use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Version string like the original: <version>-<branch>+<commit>[,dirty]
fn main() {
    let version = env!("CARGO_PKG_VERSION");
    let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let commit = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).map(|s| !s.is_empty()).unwrap_or(false);
    println!("cargo:rustc-env=TEDDY_VERSION={version}-{branch}+{commit}{}", if dirty { ",dirty" } else { "" });
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");

    /* icon and version information for teddy.exe */
    println!("cargo:rerun-if-changed=../teddybench/assets/teddybench.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../teddybench/assets/teddybench.ico");
        res.set("ProductName", "Teddy");
        res.set("FileDescription", "Teddy - Tonie file tool");
        res.compile().expect("embedding the Windows resources");
    }
}
