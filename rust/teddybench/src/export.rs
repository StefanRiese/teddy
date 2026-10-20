//! Exporting Tonies to Ogg/MP3 and copying Tonie files.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use id3::TagLike;
use tonie_core::tonies::TonieData;
use tonie_core::writer::CHANNELS;
use tonie_core::TonieFile;

use crate::covers::{find_cover_file, save_cover_image};
use crate::settings::DataDir;

/// Progress of a background job, shared with the UI.
#[derive(Default)]
pub struct JobStatus {
    pub cancel: AtomicBool,
    pub finished_items: AtomicUsize,
    pub state: Mutex<JobText>,
}

#[derive(Default, Clone)]
pub struct JobText {
    pub current: String,
    pub detail: String,
    /// Lines for the log shown in the job window
    pub log: Vec<String>,
    pub done: bool,
}

impl JobStatus {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn set_current(&self, s: impl Into<String>) {
        self.state.lock().unwrap().current = s.into();
    }

    pub fn set_detail(&self, s: impl Into<String>) {
        self.state.lock().unwrap().detail = s.into();
    }

    pub fn log(&self, s: impl Into<String>) {
        let s = s.into();
        crate::applog::info(&s);
        self.state.lock().unwrap().log.push(s);
    }

    pub fn finish(&self) {
        self.state.lock().unwrap().done = true;
    }

    pub fn text(&self) -> JobText {
        self.state.lock().unwrap().clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Ogg,
    Mp3,
}

/// One Tonie to export
#[derive(Clone, Debug)]
pub struct ExportItem {
    pub path: PathBuf,
    pub uid: String,
    pub info: Option<TonieData>,
}

pub struct ExportOptions {
    pub format: ExportFormat,
    pub out_dir: PathBuf,
    /// One directory per Tonie
    pub create_dirs: bool,
    pub version: String,
}

/// Removes characters that are not allowed in file names on any platform.
pub fn remove_invalid_chars(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control())
        .collect()
}

/// Directory for one Tonie: `<title>` or `<audio id> - <uid>`.
fn item_dir(options_dir: &Path, create_dirs: bool, item: &ExportItem, audio_id: u32) -> PathBuf {
    if !create_dirs {
        return options_dir.to_path_buf();
    }
    match &item.info {
        Some(info) => options_dir.join(remove_invalid_chars(info.title()).trim()),
        None => options_dir.join(format!("{audio_id:08X} - {}", item.uid)),
    }
}

/// A Tonie counts as exported once its directory has non-empty MP3s and no Ogg is left
/// unconverted, so a partly failed export is picked up again instead of being skipped.
pub fn is_already_exported_as_mp3(dir: &Path) -> bool {
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    let ext = |p: &Path, e: &str| p.extension().map(|x| x.eq_ignore_ascii_case(e)).unwrap_or(false);
    files
        .iter()
        .any(|f| ext(f, "mp3") && std::fs::metadata(f).map(|m| m.len() > 0).unwrap_or(false))
        && !files.iter().any(|f| ext(f, "ogg"))
}

/// `Track_01.ogg` -> 1
fn track_number(path: &Path) -> Option<usize> {
    path.file_stem()?.to_string_lossy().rsplit_once("Track_")?.1.parse().ok()
}

/// Exports the Tonies one after another. Errors of single Tonies are logged and skipped.
pub fn export(items: &[ExportItem], options: &ExportOptions, data_dir: &DataDir, status: &JobStatus) {
    for item in items {
        if status.cancelled() {
            status.log("Export cancelled.");
            break;
        }
        if let Err(e) = export_one(item, options, data_dir, status) {
            status.log(format!("Failed to save file '{}': {e}", item.path.display()));
        }
        status.finished_items.fetch_add(1, Ordering::Relaxed);
    }
    status.finish();
}

fn export_one(item: &ExportItem, options: &ExportOptions, data_dir: &DataDir, status: &JobStatus) -> Result<(), String> {
    let display = item
        .info
        .as_ref()
        .map(|i| i.title().to_string())
        .unwrap_or_else(|| item.path.file_name().unwrap_or_default().to_string_lossy().to_string());
    status.set_current(&display);
    status.set_detail("Reading tonie file...");

    let tonie = TonieFile::open(&item.path, true).map_err(|e| e.to_string())?;
    let hash = tonie.hash_string();
    let dir = item_dir(&options.out_dir, options.create_dirs, item, tonie.header.audio_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create '{}': {e}", dir.display()))?;

    let cover_result = match item.info.as_ref().and_then(|i| i.pic.as_deref()).filter(|p| !p.is_empty()) {
        Some(pic) => save_cover_image(data_dir, pic, &hash, &dir, false),
        None if item.info.is_none() => Some("skipped (tonie not recognized, no cover URL known)".into()),
        None => Some("skipped (no cover URL in tonies.json for this tonie)".into()),
    };
    if let Some(r) = cover_result {
        status.log(format!("Cover art for '{display}': {r}"));
        status.set_detail(format!("Cover art: {r}"));
    }

    if options.format == ExportFormat::Mp3 && is_already_exported_as_mp3(&dir) {
        status.log(format!("'{display}': skipping, .mp3 files already exist"));
        return Ok(());
    }

    let mut tags = vec![format!("TeddyVersion={}", options.version), format!("TeddyFile={}", item.path.display())];
    if let Some(info) = &item.info {
        tags.push(format!("ALBUM={}", info.title()));
        tags.push(format!("ARTIST={}", info.series.as_deref().unwrap_or("")));
        tags.push(format!("LANGUAGE={}", info.language.as_deref().unwrap_or("")));
    }
    tags.push(format!("HASH={hash}"));
    let titles = item.info.as_ref().map(|i| i.tracks.as_slice());

    let oggs = tonie.dump_audio_files(&dir, "", false, &tags, titles).map_err(|e| e.to_string())?;
    if options.format == ExportFormat::Mp3 {
        convert_to_mp3(&oggs, tonie.header.audio_chapters.len(), item.info.as_ref(), &dir, status);
    }
    Ok(())
}

/// Converts the exported Ogg tracks to MP3 in parallel. An Ogg is only deleted once its MP3 was written.
fn convert_to_mp3(oggs: &[PathBuf], track_count: usize, info: Option<&TonieData>, dir: &Path, status: &JobStatus) {
    let cover = find_cover_file(dir).and_then(|c| std::fs::read(&c).ok().map(|data| (c, data)));
    let next = AtomicUsize::new(0);
    let converted = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(oggs.len().max(1));
    status.set_detail(format!("Converting {} track(s) to .mp3...", oggs.len()));

    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= oggs.len() || status.cancelled() {
                    break;
                }
                let ogg = &oggs[i];
                let mp3 = ogg.with_extension("mp3");
                let done_before = std::fs::metadata(&mp3).map(|m| m.len() > 0).unwrap_or(false);
                let result = if done_before {
                    Ok(())
                } else {
                    ogg_to_mp3(ogg, &mp3, track_count, info, cover.as_ref())
                };
                match result {
                    Ok(()) => {
                        std::fs::remove_file(ogg).ok();
                    }
                    Err(e) => {
                        std::fs::remove_file(&mp3).ok();
                        status.log(format!("Failed to convert '{}' to mp3, keeping the .ogg: {e}", ogg.display()));
                    }
                }
                let n = converted.fetch_add(1, Ordering::Relaxed) + 1;
                status.set_detail(format!("Converted {n} of {} track(s) to .mp3", oggs.len()));
            });
        }
    });
}

/// Decodes one exported Ogg track and encodes it as MP3 (LAME VBR, about 190 kbps) with ID3v2.3 tags.
pub fn ogg_to_mp3(ogg: &Path, mp3: &Path, track_count: usize, info: Option<&TonieData>, cover: Option<&(PathBuf, Vec<u8>)>) -> Result<(), String> {
    use mp3lame_encoder::{Builder, FlushNoGap, InterleavedPcm};

    let lame_err = |e: &dyn std::fmt::Display| e.to_string();
    let mut builder = Builder::new().ok_or("cannot create LAME encoder")?;
    builder.set_num_channels(CHANNELS as u8).map_err(|e| lame_err(&e))?;
    builder.set_sample_rate(48000).map_err(|e| lame_err(&e))?;
    builder.set_vbr_mode(mp3lame_encoder::VbrMode::Mtrh).map_err(|e| lame_err(&e))?;
    builder.set_vbr_quality(mp3lame_encoder::Quality::NearBest).map_err(|e| lame_err(&e))?;
    let mut encoder = builder.build().map_err(|e| lame_err(&e))?;

    let mut out = Vec::new();
    let mut pcm = Vec::new();
    tonie_core::audio_in::decode_file(
        ogg,
        &mut |samples| {
            pcm.clear();
            pcm.extend(samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
            out.reserve(mp3lame_encoder::max_required_buffer_size(pcm.len() / CHANNELS));
            encoder
                .encode_to_vec(InterleavedPcm(&pcm), &mut out)
                .map_err(|e| tonie_core::Error::Encoder(e.to_string()))?;
            Ok(())
        },
        &mut |_| {},
    )
    .map_err(|e| e.to_string())?;
    out.reserve(7200);
    encoder.flush_to_vec::<FlushNoGap>(&mut out).map_err(|e| lame_err(&e))?;
    std::fs::write(mp3, &out).map_err(|e| e.to_string())?;

    let mut tag = id3::Tag::new();
    let number = track_number(ogg);
    let title = number
        .and_then(|n| info.and_then(|i| i.tracks.get(n - 1)))
        .cloned()
        .unwrap_or_else(|| ogg.file_stem().unwrap_or_default().to_string_lossy().to_string());
    tag.set_title(title);
    if let Some(n) = number {
        tag.set_track(n as u32);
        tag.set_total_tracks(track_count as u32);
    }
    if let Some(info) = info {
        if let Some(series) = info.series.as_deref() {
            tag.set_artist(series);
        }
        tag.set_album(info.episodes.as_deref().filter(|e| !e.is_empty()).unwrap_or(info.title()));
    }
    if let Some((path, data)) = cover {
        let mime = if path.extension().map(|e| e.eq_ignore_ascii_case("png")).unwrap_or(false) {
            "image/png"
        } else {
            "image/jpeg"
        };
        tag.add_frame(id3::frame::Picture {
            mime_type: mime.into(),
            picture_type: id3::frame::PictureType::CoverFront,
            description: "Cover (front)".into(),
            data: data.clone(),
        });
    }
    tag.write_to_path(mp3, id3::Version::Id3v23).map_err(|e| e.to_string())
}

/// Re-downloads the covers of Tonies that were already exported to `out_dir`.
pub fn update_covers(items: &[ExportItem], out_dir: &Path, create_dirs: bool, data_dir: &DataDir, status: &JobStatus) {
    for item in items {
        if status.cancelled() {
            break;
        }
        let name = item.info.as_ref().map(|i| i.title().to_string()).unwrap_or_else(|| item.uid.clone());
        status.set_current(&name);
        let result = (|| -> Result<String, String> {
            let tonie = TonieFile::open(&item.path, false).map_err(|e| e.to_string())?;
            let dir = item_dir(out_dir, create_dirs, item, tonie.header.audio_id);
            if !dir.is_dir() {
                return Ok("skipped, not exported yet".into());
            }
            let pic = item
                .info
                .as_ref()
                .and_then(|i| i.pic.as_deref())
                .filter(|p| !p.is_empty())
                .ok_or("no cover URL known")?;
            Ok(save_cover_image(data_dir, pic, &tonie.hash_string(), &dir, true).unwrap_or_default())
        })();
        status.log(match result {
            Ok(r) => format!("'{name}': {r}"),
            Err(e) => format!("'{name}': {e}"),
        });
        status.finished_items.fetch_add(1, Ordering::Relaxed);
    }
    status.finish();
}

/// Copies the directory of each Tonie (e.g. `0A0B0C01/`) into `target`, keeping existing files.
pub fn copy_to_sd(files: &[PathBuf], target: &Path, status: &JobStatus) {
    for file in files {
        if status.cancelled() {
            break;
        }
        let Some(src) = file.parent() else { continue };
        let dest = target.join(src.file_name().unwrap_or_default());
        status.set_current(dest.display().to_string());
        if let Err(e) = copy_dir(src, &dest) {
            status.log(format!("Failed to copy directory '{}': {e}", src.display()));
        }
        status.finished_items.fetch_add(1, Ordering::Relaxed);
    }
    status.finish();
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if !target.exists() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies a Tonie file as `<model> - <audio id> - <title>`, the naming of `teddy -m rename`.
pub fn export_tonie_file(item: &ExportItem, audio_id: u32, out_dir: &Path) -> std::io::Result<PathBuf> {
    let model = item.info.as_ref().and_then(|i| i.model.clone()).unwrap_or_default();
    let title = item.info.as_ref().map(|i| i.title().to_string()).unwrap_or_default();
    let dest = out_dir.join(format!("{model} - {audio_id:08X} - {}", remove_invalid_chars(&title).trim()));
    if dest.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("'{}' already exists", dest.display()),
        ));
    }
    std::fs::copy(&item.path, &dest)?;
    Ok(dest)
}

/// Shared handle for a job running in a background thread.
pub type Job = Arc<JobStatus>;

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_tonie() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/reference/album_cbr96/0A0B0C01/500304E0")
    }

    #[test]
    fn exports_ogg_and_mp3() {
        let data = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let info: TonieData = serde_json::from_str(r#"{"title":"Test Album","series":"Series","tracks":["One","Two","Three"]}"#).unwrap();
        let item = ExportItem {
            path: reference_tonie(),
            uid: "E0040350010C0B0A".into(),
            info: Some(info),
        };

        for format in [ExportFormat::Ogg, ExportFormat::Mp3] {
            let status = JobStatus::default();
            let options = ExportOptions {
                format,
                out_dir: out.path().join(format!("{format:?}")),
                create_dirs: true,
                version: "test".into(),
            };
            std::fs::create_dir_all(&options.out_dir).unwrap();
            export(std::slice::from_ref(&item), &options, &DataDir(data.path().to_path_buf()), &status);
            assert!(status.text().done);
            assert_eq!(status.finished_items.load(Ordering::Relaxed), 1);

            let dir = options.out_dir.join("Test Album");
            let ext = if format == ExportFormat::Ogg { "ogg" } else { "mp3" };
            for n in 1..=3 {
                assert!(dir.join(format!("Track_0{n}.{ext}")).is_file(), "{format:?} track {n}");
            }
            if format == ExportFormat::Mp3 {
                assert!(is_already_exported_as_mp3(&dir));
                let tag = id3::Tag::read_from_path(dir.join("Track_02.mp3")).unwrap();
                assert_eq!(tag.title(), Some("Two"));
                assert_eq!(tag.track(), Some(2));
                assert_eq!(tag.total_tracks(), Some(3));
                assert_eq!(tag.artist(), Some("Series"));
                assert_eq!(tag.album(), Some("Test Album"));

                /* the MP3 decodes and has about the length of the track */
                let mut frames = 0usize;
                tonie_core::audio_in::decode_file(
                    &dir.join("Track_02.mp3"),
                    &mut |s| {
                        frames += s.len() / 2;
                        Ok(())
                    },
                    &mut |_| {},
                )
                .unwrap();
                assert!(frames > 48000 * 4, "{frames}");
            }
        }
    }

    #[test]
    fn copies_tonie_directories_without_overwriting() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let file = src.path().join("0A0B0C01").join("500304E0");
        std::fs::create_dir(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"new").unwrap();
        std::fs::create_dir(dest.path().join("0A0B0C01")).unwrap();
        std::fs::write(dest.path().join("0A0B0C01").join("500304E0"), b"old").unwrap();

        let status = JobStatus::default();
        copy_to_sd(&[file], dest.path(), &status);
        assert_eq!(std::fs::read(dest.path().join("0A0B0C01").join("500304E0")).unwrap(), b"old");
    }
}
