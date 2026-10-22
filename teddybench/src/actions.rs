//! Changes to the Tonie files on the SD card.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use id3::TagLike;
use tonie_core::encode::{EncodeOptions, EncodeProgress};
use tonie_core::TonieFile;

use crate::export::JobStatus;
use crate::uid::file_for_uid;

/// Initial order of files for a new Tonie: by ID3 track number, files without one at the end.
pub fn initial_track_order(mut files: Vec<PathBuf>) -> Vec<PathBuf> {
    files.sort_by_key(|f| id3::Tag::read_from_path(f).ok().and_then(|t| t.track()).unwrap_or(u32::MAX));
    files
}

/// "Artist - Title" from the ID3 tag, for the track list.
pub fn id3_description(path: &Path) -> String {
    match id3::Tag::read_from_path(path) {
        Ok(t) => format!("{} - {}", t.artist().unwrap_or(""), t.title().unwrap_or("")),
        Err(_) => String::new(),
    }
}

/// A name for a new Tonie from the tags of its files: "Album Artist - Album", "Artist - Album" or
/// "Album" of the first file that has an album, else the name of the folder of the files.
pub fn title_from_tags(files: &[PathBuf]) -> Option<String> {
    let tagged = files.iter().filter_map(|f| id3::Tag::read_from_path(f).ok()).find_map(|t| {
        let album = t.album().map(str::trim).filter(|a| !a.is_empty())?.to_string();
        let artist = t.album_artist().or(t.artist()).map(str::trim).filter(|a| !a.is_empty());
        Some(match artist {
            Some(artist) => format!("{artist} - {album}"),
            None => album,
        })
    });
    tagged.or_else(|| {
        let dir = files.first()?.parent()?;
        files
            .iter()
            .all(|f| f.parent() == Some(dir))
            .then(|| dir.file_name().map(|n| n.to_string_lossy().to_string()))
            .flatten()
    })
}

/// Forwards encoder progress to the job window, in the style of the original log.
struct JobProgress<'a> {
    status: &'a JobStatus,
    line: String,
}

impl EncodeProgress for JobProgress<'_> {
    fn file_start(&mut self, track: usize, path: &Path) {
        let name: String = tonie_core::encode::display_name(path).chars().take(30).collect();
        self.line = format!("Track {track:>3} - {name}  [");
        self.status.set_current(format!("Track {track}: {name}"));
    }

    fn progress(&mut self, fraction: f64) {
        let step = (fraction * 20.0) as i32;
        if step % 5 == 0 {
            if step != 20 {
                self.line += &format!("{}%", step * 5);
            }
        } else {
            self.line.push('.');
        }
        self.status.set_detail(format!("{:.0} %", fraction * 100.0));
    }

    fn file_done(&mut self) {
        self.status.log(format!("{}]", self.line));
        self.status.finished_items.fetch_add(1, Ordering::Relaxed);
    }

    fn failed(&mut self, message: &str) {
        self.status.log(format!("{}] FAILED", self.line));
        self.status.log(message);
    }

    fn warning(&mut self, message: &str) {
        self.status.log(format!("Warning: {message}"));
    }

    fn info(&mut self, message: &str) {
        self.status.log(message);
    }
}

/// Encodes `files` into a new Tonie for `uid` in `content_dir`. A Tonie with that UID is replaced.
/// Returns the path of the new file.
pub fn encode_tonie(files: &[PathBuf], uid: &str, audio_id: u32, content_dir: &Path, status: &JobStatus) -> Result<PathBuf, String> {
    let target = file_for_uid(content_dir, uid);
    let dir = target.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("Failed to create directory '{}': {e}", dir.display()))?;

    let mut progress = JobProgress { status, line: String::new() };
    let options = EncodeOptions {
        audio_id,
        ..Default::default()
    };
    /* encode next to the target, so a failed run does not destroy an existing Tonie */
    let mut temp = tempfile_in(dir)?;
    tonie_core::encode(files, &options, temp.as_file_mut(), &mut progress)
        .map_err(|e| format!("Failed to encode file. Most likely an audio file is corrupted. ({e})"))?;
    temp.persist(&target)
        .map_err(|e| format!("Failed to write file '{}': {}", target.display(), e.error))?;
    Ok(target)
}

fn tempfile_in(dir: &Path) -> Result<TempFile, String> {
    TempFile::new_in(dir).map_err(|e| format!("Failed to create a file in '{}': {e}", dir.display()))
}

/// A file deleted on drop unless persisted.
struct TempFile {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl TempFile {
    fn new_in(dir: &Path) -> std::io::Result<TempFile> {
        for i in 0..100u32 {
            let path = dir.join(format!(".teddybench-{}-{i}.tmp", std::process::id()));
            match std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path) {
                Ok(file) => return Ok(TempFile { path, file: Some(file) }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(std::io::ErrorKind::AlreadyExists.into())
    }

    fn as_file_mut(&mut self) -> &mut std::fs::File {
        self.file.as_mut().unwrap()
    }

    fn persist(mut self, target: &Path) -> Result<(), PersistError> {
        drop(self.file.take());
        /* rename does not replace an existing file on every platform */
        if target.exists() {
            std::fs::remove_file(target).map_err(|error| PersistError { error })?;
        }
        std::fs::rename(&self.path, target).map_err(|error| PersistError { error })
    }
}

struct PersistError {
    error: std::io::Error,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        drop(self.file.take());
        std::fs::remove_file(&self.path).ok();
    }
}

/// Copies an existing Tonie file to the place for `uid`.
pub fn copy_tonie(file: &Path, uid: &str, content_dir: &Path) -> Result<PathBuf, String> {
    TonieFile::open(file, false).map_err(|_| "The file you have chosen is not supported.".to_string())?;
    let target = file_for_uid(content_dir, uid);
    std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| format!("Failed to create directory: {e}"))?;
    std::fs::copy(file, &target).map_err(|e| format!("Failed to write file '{}': {e}", target.display()))?;
    Ok(target)
}

/// Moves a Tonie file to the place for `new_uid`. Returns a note if the old directory was kept.
pub fn reassign_uid(file: &Path, new_uid: &str, content_dir: &Path) -> Result<Option<String>, String> {
    let target = file_for_uid(content_dir, new_uid);
    if target == file {
        return Ok(None);
    }
    if target.exists() {
        return Err(format!("Failed to assign UID '{new_uid}' as this file already exists"));
    }
    std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| format!("Failed to create directory: {e}"))?;
    std::fs::rename(file, &target).map_err(|e| format!("Failed to write file '{}': {e}", target.display()))?;
    Ok(remove_dir_if_empty(file, content_dir))
}

/// Deletes a Tonie file and its directory, if that is empty then.
pub fn delete_tonie(file: &Path, content_dir: &Path) -> Result<(), String> {
    std::fs::remove_file(file).map_err(|e| format!("Failed to delete file '{}': {e}", file.display()))?;
    remove_dir_if_empty(file, content_dir);
    Ok(())
}

fn remove_dir_if_empty(file: &Path, content_dir: &Path) -> Option<String> {
    let dir = file.parent()?;
    if dir == content_dir {
        return None;
    }
    match std::fs::read_dir(dir).map(|mut d| d.next().is_none()) {
        Ok(true) => {
            std::fs::remove_dir(dir).ok();
            None
        }
        _ => Some(format!("Will not delete directory '{}' as it is not empty.", dir.display())),
    }
}

/// Shows the file in the system's file manager.
pub fn show_in_file_manager(file: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(file.as_os_str());
        std::process::Command::new("explorer").arg(arg).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg("-R").arg(file).spawn().map(|_| ())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(file.parent().unwrap_or(file)).spawn().map(|_| ())
    }
}

/// Opens a file with the default program, e.g. a PDF viewer.
pub fn open_with_default_program(file: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd").arg("/C").arg("start").arg("").arg(file).spawn().map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(file).spawn().map(|_| ())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(file).spawn().map(|_| ())
    }
}

/// Audio ID for a Tonie made by TeddyBench: the time in seconds, moved below the range of retail
/// Tonies like the original does. Never `last` or less and none of `taken`, so Tonies made within
/// the same second still get different IDs.
pub fn custom_audio_id(now_secs: u64, last: Option<u32>, taken: &HashSet<u32>) -> u32 {
    let mut id = (now_secs as u32).wrapping_sub(0x5000_0000);
    if let Some(last) = last.filter(|&l| id <= l) {
        id = last.wrapping_add(1);
    }
    while taken.contains(&id) {
        id = id.wrapping_add(1);
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_from_tags() {
        let album = fixtures().join("inputs/album");
        /* the album has no artist */
        assert_eq!(title_from_tags(&[album.join("a_intro.mp3")]).as_deref(), Some("Test Album"));
        /* no tags: the folder */
        let prefix = fixtures().join("inputs/prefix");
        assert_eq!(title_from_tags(&[prefix.join("0001.mp3"), prefix.join("0002.mp3")]).as_deref(), Some("prefix"));
        /* no tags, different folders */
        assert_eq!(title_from_tags(&[prefix.join("0001.mp3"), fixtures().join("inputs/silence.mp3")]), None);
    }

    #[test]
    fn audio_ids_are_unique() {
        let now = 0x6900_0000;
        let none = HashSet::new();
        assert_eq!(custom_audio_id(now, None, &none), 0x1900_0000);
        /* the same second again */
        assert_eq!(custom_audio_id(now, Some(0x1900_0000), &none), 0x1900_0001);
        /* already on the card */
        let taken = HashSet::from([0x1900_0000, 0x1900_0001]);
        assert_eq!(custom_audio_id(now, None, &taken), 0x1900_0002);
        /* the clock went back */
        assert_eq!(custom_audio_id(now - 10, Some(0x1900_0005), &none), 0x1900_0006);
    }

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures")
    }

    #[test]
    fn track_order_uses_id3_numbers() {
        let album = fixtures().join("inputs/album");
        let files = vec![
            album.join("c_end.mp3"),
            album.join("a_intro.mp3"),
            fixtures().join("inputs/opus_input.ogg"),
            album.join("b_song.mp3"),
        ];
        let names: Vec<String> = initial_track_order(files)
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["b_song.mp3", "a_intro.mp3", "c_end.mp3", "opus_input.ogg"]);
    }

    #[test]
    fn encode_reassign_and_delete() {
        let content = tempfile::tempdir().unwrap();
        let status = JobStatus::default();
        let files = vec![fixtures().join("inputs/mono22k.mp3")];

        let file = encode_tonie(&files, "E0040350010C0B0A", 0x1234, content.path(), &status).unwrap();
        assert_eq!(file, content.path().join("0A0B0C01").join("500304E0"));
        assert_eq!(TonieFile::open(&file, true).unwrap().header.audio_id, 0x1234);
        assert!(status.text().log.iter().any(|l| l.starts_with("Track   1")));
        /* no temporary files left */
        assert_eq!(std::fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);

        assert!(reassign_uid(&file, "E0040350010C0B0A", content.path()).unwrap().is_none());
        assert!(reassign_uid(&file, "E004035002AABBCC", content.path()).unwrap().is_none());
        let moved = content.path().join("CCBBAA02").join("500304E0");
        assert!(moved.is_file());
        assert!(!content.path().join("0A0B0C01").exists());

        delete_tonie(&moved, content.path()).unwrap();
        assert_eq!(std::fs::read_dir(content.path()).unwrap().count(), 0);
    }

    #[test]
    fn failed_encode_keeps_existing_tonie() {
        let content = tempfile::tempdir().unwrap();
        let target = content.path().join("0A0B0C01").join("500304E0");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"existing").unwrap();

        let status = JobStatus::default();
        let broken = content.path().join("broken.mp3");
        std::fs::write(&broken, b"not audio").unwrap();
        assert!(encode_tonie(&[broken], "E0040350010C0B0A", 1, content.path(), &status).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"existing");
        assert_eq!(std::fs::read_dir(target.parent().unwrap()).unwrap().count(), 1);
    }
}
