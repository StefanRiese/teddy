//! The TRASHCAN folder next to CONTENT on the SD card. A Toniebox connected to the cloud moves
//! deleted Tonie files there (`TRASHCAN/<folder>/<name>.043`) instead of deleting them. The
//! names do not tell the UID of the tag, so restoring needs it from the user.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tonie_core::TonieFile;

use crate::library::Analysis;
use crate::uid::file_for_uid;

#[derive(Clone, Debug)]
pub struct TrashItem {
    pub path: PathBuf,
    /// When the file was moved to the TRASHCAN (its modification time)
    pub deleted: Option<SystemTime>,
    pub analysis: Result<Analysis, String>,
}

/// The TRASHCAN folder next to the CONTENT directory, if there is one
pub fn find(content_dir: &Path) -> Option<PathBuf> {
    let root = content_dir.parent()?;
    std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("TRASHCAN")))
}

fn is_tonie_file(path: &Path) -> bool {
    path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("043"))
}

/// The deleted Tonie files in `trashcan`, most recently deleted first
pub fn scan(trashcan: &Path) -> Vec<TrashItem> {
    let mut items: Vec<TrashItem> = std::fs::read_dir(trashcan)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .flat_map(|dir| std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()))
        .filter(|p| is_tonie_file(p))
        .map(|path| TrashItem {
            deleted: std::fs::metadata(&path).and_then(|m| m.modified()).ok(),
            analysis: Analysis::read(&path).map_err(|e| e.to_string()),
            path,
        })
        .collect();
    items.sort_by(|a, b| b.deleted.cmp(&a.deleted).then(a.path.cmp(&b.path)));
    items
}

/// Puts a deleted Tonie back into `content_dir` as the Tonie for `uid` and removes it from the
/// TRASHCAN. Byte for byte, or with `new_audio_id` as a new Tonie (the audio is not re-encoded).
/// A Tonie already on the card for `uid` is only replaced with `replace`; it goes into the TRASHCAN.
pub fn restore(item: &Path, uid: &str, content_dir: &Path, new_audio_id: Option<u32>, replace: bool) -> Result<PathBuf, String> {
    let target = file_for_uid(content_dir, uid);
    let tonie = TonieFile::open(item, new_audio_id.is_some()).map_err(|e| format!("Cannot read '{}': {e}", item.display()))?;
    let displaced = match (target.exists(), replace) {
        (false, _) => None,
        (true, false) => return Err(format!("There already is a Tonie for UID {uid} on the card.")),
        (true, true) => Some(move_to_trashcan(&target, item.parent().unwrap_or(Path::new(".")))?),
    };
    let result = write_restored(item, &tonie, &target, new_audio_id);
    if let (Err(_), Some(displaced)) = (&result, &displaced) {
        /* put the replaced Tonie back */
        std::fs::rename(displaced, &target).ok();
    }
    result?;

    /* the restored file is complete, the copy in the TRASHCAN is not needed anymore */
    delete(item).ok();
    Ok(target)
}

/// Moves a Tonie file from the card into the TRASHCAN folder `dir`, with a free 8.3 name.
fn move_to_trashcan(file: &Path, dir: &Path) -> Result<PathBuf, String> {
    let folder = file
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let dest = std::iter::once(format!("{folder}.043"))
        .chain((1..1000).map(|i| format!("TB{i:06}.043")))
        .map(|n| dir.join(n))
        .find(|p| !p.exists())
        .ok_or("No free file name in the TRASHCAN")?;
    std::fs::rename(file, &dest).map_err(|e| format!("Failed to move '{}' into the TRASHCAN: {e}", file.display()))?;
    Ok(dest)
}

fn write_restored(item: &Path, tonie: &TonieFile, target: &Path, new_audio_id: Option<u32>) -> Result<(), String> {
    std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| format!("Failed to create directory: {e}"))?;

    let written = match new_audio_id {
        Some(id) => tonie
            .with_audio_id(id)
            .map_err(|e| e.to_string())
            .and_then(|bytes| std::fs::write(target, &bytes).map(|_| bytes.len() as u64).map_err(|e| e.to_string())),
        None => std::fs::copy(item, target).map_err(|e| e.to_string()),
    };
    let expected = std::fs::metadata(item).map(|m| m.len()).ok();
    match written {
        Ok(n) if Some(n) == expected => Ok(()),
        Ok(_) => {
            std::fs::remove_file(target).ok();
            Err(format!("Failed to write '{}': incomplete", target.display()))
        }
        Err(e) => {
            std::fs::remove_file(target).ok();
            Err(format!("Failed to write '{}': {e}", target.display()))
        }
    }
}

/// Deletes a file from the TRASHCAN, and its folder if that is empty then.
pub fn delete(item: &Path) -> Result<(), String> {
    std::fs::remove_file(item).map_err(|e| format!("Failed to delete '{}': {e}", item.display()))?;
    if let Some(dir) = item.parent() {
        if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) {
            std::fs::remove_dir(dir).ok();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/reference/album_cbr96/0A0B0C01/500304E0")
    }

    /// An SD card with an empty CONTENT and two deleted Tonies
    fn card() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let content = root.path().join("CONTENT");
        std::fs::create_dir(&content).unwrap();
        let trash = root.path().join("TRASHCAN");
        for (dir, name) in [("E40", "1A2B3C4D.043"), ("E41", "5E6F7A8B.043")] {
            std::fs::create_dir_all(trash.join(dir)).unwrap();
            std::fs::copy(sample(), trash.join(dir).join(name)).unwrap();
        }
        std::fs::write(trash.join("E41").join("notes.txt"), b"x").unwrap();
        (root, content, trash)
    }

    #[test]
    fn finds_and_lists_deleted_tonies() {
        let (_root, content, trash) = card();
        assert_eq!(find(&content), Some(trash.clone()));
        let items = scan(&trash);
        assert_eq!(items.len(), 2);
        let original = Analysis::read(&sample()).unwrap();
        for item in &items {
            assert_eq!(item.analysis.as_ref().unwrap(), &original);
        }

        let other = tempfile::tempdir().unwrap();
        std::fs::create_dir(other.path().join("CONTENT")).unwrap();
        assert_eq!(find(&other.path().join("CONTENT")), None);
    }

    #[test]
    fn restores_byte_for_byte() {
        let (_root, content, trash) = card();
        let item = trash.join("E40").join("1A2B3C4D.043");
        let restored = restore(&item, "E0040350010C0B0A", &content, None, false).unwrap();
        assert_eq!(restored, content.join("0A0B0C01").join("500304E0"));
        assert_eq!(std::fs::read(&restored).unwrap(), std::fs::read(sample()).unwrap());
        /* removed from the TRASHCAN, with its empty folder */
        assert!(!trash.join("E40").exists());

        /* the place is taken now */
        let other = trash.join("E41").join("5E6F7A8B.043");
        assert!(restore(&other, "E0040350010C0B0A", &content, None, false).is_err());
        assert!(other.exists());

        /* replacing moves the Tonie on the card into the TRASHCAN */
        let on_card = std::fs::read(&restored).unwrap();
        restore(&other, "E0040350010C0B0A", &content, Some(0x99), true).unwrap();
        assert!(!other.exists());
        assert_eq!(TonieFile::open(&restored, false).unwrap().header.audio_id, 0x99);
        assert_eq!(std::fs::read(trash.join("E41").join("0A0B0C01.043")).unwrap(), on_card);
    }

    #[test]
    fn restores_as_a_new_tonie() {
        let (_root, content, trash) = card();
        let item = trash.join("E41").join("5E6F7A8B.043");
        let restored = restore(&item, "E004035011223344", &content, Some(0x1234), false).unwrap();
        let tonie = TonieFile::open(&restored, true).unwrap();
        assert_eq!(tonie.header.audio_id, 0x1234);
        assert!(tonie.hash_correct);
        assert!(!item.exists());
        /* the folder still has another file */
        assert!(trash.join("E41").join("notes.txt").exists());
    }

    #[test]
    fn deletes_permanently() {
        let (_root, _content, trash) = card();
        delete(&trash.join("E40").join("1A2B3C4D.043")).unwrap();
        assert!(!trash.join("E40").exists());
        assert_eq!(scan(&trash).len(), 1);
    }
}
