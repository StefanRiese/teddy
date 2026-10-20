//! Finding a Toniebox SD card: a mounted volume with a CONTENT folder.

use std::path::{Path, PathBuf};

/// CONTENT folders of mounted removable volumes.
pub fn find_content_dirs() -> Vec<PathBuf> {
    volume_roots().into_iter().map(|r| r.join("CONTENT")).filter(|c| c.is_dir()).collect()
}

#[cfg(windows)]
fn volume_roots() -> Vec<PathBuf> {
    /* skip A: and B:, probing floppy drives can block */
    (b'C'..=b'Z')
        .map(|l| PathBuf::from(format!("{}:\\", l as char)))
        .filter(|p| p.exists())
        .collect()
}

#[cfg(target_os = "macos")]
fn volume_roots() -> Vec<PathBuf> {
    /* the root volume is linked into /Volumes as well */
    subdirs(Path::new("/Volumes")).into_iter().filter(|p| std::fs::read_link(p).is_err()).collect()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn volume_roots() -> Vec<PathBuf> {
    let user = std::env::var("USER").unwrap_or_default();
    let mut roots = Vec::new();
    for base in [format!("/media/{user}"), format!("/run/media/{user}"), "/media".into(), "/mnt".into()] {
        roots.extend(subdirs(Path::new(&base)));
    }
    roots
}

#[allow(dead_code)]
fn subdirs(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}
