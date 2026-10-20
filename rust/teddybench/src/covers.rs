//! Cover images from tonies.json: thumbnails for the list and covers for exports.

use std::io::Read;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::RgbaImage;

use crate::settings::DataDir;

const COVER_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png"];

pub fn download(url: &str) -> Result<Vec<u8>, String> {
    let mut response = ureq::get(url).call().map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    response.body_mut().as_reader().read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.is_empty() {
        return Err("server returned an empty image".into());
    }
    Ok(data)
}

/// Crops the image to the centered square and scales it to `size`.
pub fn square_thumbnail(data: &[u8], size: u32) -> Result<RgbaImage, String> {
    let img = image::load_from_memory(data).map_err(|e| e.to_string())?;
    let side = img.width().min(img.height());
    let cropped = img.crop_imm((img.width() - side) / 2, (img.height() - side) / 2, side, side);
    Ok(cropped.resize_exact(size, size, FilterType::Lanczos3).to_rgba8())
}

/// The list thumbnail of a Tonie: from `cache/<hash>.png`, otherwise downloaded and cached.
pub fn load_thumbnail(data_dir: &DataDir, pic_url: &str, hash: &str, size: u32) -> Result<RgbaImage, String> {
    let cache = data_dir.thumbnail_cache().join(format!("{hash}.png"));
    if let Ok(img) = image::open(&cache) {
        return Ok(img.to_rgba8());
    }
    let thumb = square_thumbnail(&download(pic_url)?, size)?;
    std::fs::create_dir_all(data_dir.thumbnail_cache()).ok();
    thumb.save(&cache).ok();
    Ok(thumb)
}

fn is_non_empty_file(path: &Path) -> bool {
    std::fs::metadata(path).map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
}

/// The `cover.<ext>` in an export directory, if there is one.
pub fn find_cover_file(dir: &Path) -> Option<PathBuf> {
    COVER_EXTENSIONS.iter().map(|e| dir.join(format!("cover.{e}"))).find(|p| is_non_empty_file(p))
}

/// Removes covers with another extension, e.g. a thumbnail fallback cover.png once cover.jpg exists.
fn delete_other_covers(dir: &Path, keep: &Path) {
    for ext in COVER_EXTENSIONS {
        let other = dir.join(format!("cover.{ext}"));
        if other != keep && other.exists() {
            std::fs::remove_file(other).ok();
        }
    }
}

fn url_extension(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or("");
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() && ext.len() <= 5 => ext.to_ascii_lowercase(),
        _ => "jpg".into(),
    }
}

/// Stores the cover as `cover.<ext>` in `out_dir`. Uses the full resolution cache
/// (`cache/covers/<hash>.<ext>`) before downloading; if the download fails, falls back to the list
/// thumbnail. An existing cover is kept unless `overwrite` is set. Returns `None` if nothing had to
/// be done, otherwise a short description of what happened.
pub fn save_cover_image(data_dir: &DataDir, pic_url: &str, hash: &str, out_dir: &Path, overwrite: bool) -> Option<String> {
    save_cover_with(data_dir, pic_url, hash, out_dir, overwrite, download)
}

fn save_cover_with(
    data_dir: &DataDir,
    pic_url: &str,
    hash: &str,
    out_dir: &Path,
    overwrite: bool,
    fetch: impl Fn(&str) -> Result<Vec<u8>, String>,
) -> Option<String> {
    let ext = url_extension(pic_url);
    let cover = out_dir.join(format!("cover.{ext}"));

    /* an empty file (e.g. left behind by an interrupted write) does not count as present */
    if is_non_empty_file(&cover) && !overwrite {
        return None;
    }

    let cached = data_dir.cover_cache().join(format!("{hash}.{ext}"));
    if !overwrite && is_non_empty_file(&cached) && std::fs::copy(&cached, &cover).is_ok() {
        delete_other_covers(out_dir, &cover);
        return Some(format!("reused cached cover ({})", cached.display()));
    }

    match fetch(pic_url) {
        Ok(data) => {
            /* write to a temporary file first, so a broken-off write never looks like a complete cover */
            let part = out_dir.join(format!("cover.{ext}.part"));
            if let Err(e) = std::fs::write(&part, &data).and_then(|_| std::fs::rename(&part, &cover)) {
                std::fs::remove_file(&part).ok();
                return Some(format!("FAILED to save cover image '{pic_url}' to '{}': {e}", cover.display()));
            }
            delete_other_covers(out_dir, &cover);
            if std::fs::create_dir_all(data_dir.cover_cache()).is_ok() {
                std::fs::copy(&cover, &cached).ok();
            }
            Some(format!("downloaded ({})", cover.display()))
        }
        Err(e) => {
            if is_non_empty_file(&cover) {
                return Some(format!("download failed ({e}), kept existing cover ({})", cover.display()));
            }
            let thumbnail = data_dir.thumbnail_cache().join(format!("{hash}.png"));
            if thumbnail.is_file() {
                let fallback = out_dir.join("cover.png");
                if std::fs::copy(&thumbnail, &fallback).is_ok() {
                    return Some(format!("download failed ({e}), used local thumbnail cache instead ({})", fallback.display()));
                }
            }
            Some(format!("FAILED to save cover image '{pic_url}' to '{}': {e}", cover.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        RgbaImage::from_pixel(w, h, image::Rgba([200, 10, 10, 255]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn thumbnails_are_square() {
        let t = square_thumbnail(&png(300, 200), 64).unwrap();
        assert_eq!(t.dimensions(), (64, 64));
    }

    #[test]
    fn extension_from_url() {
        assert_eq!(url_extension("https://cdn.example/files/a_b.JPG?v=171"), "jpg");
        assert_eq!(url_extension("https://cdn.example/files/a.png"), "png");
        assert_eq!(url_extension("https://cdn.example/files/noext"), "jpg");
    }

    #[test]
    fn cover_download_cache_and_fallback() {
        let data = tempfile::tempdir().unwrap();
        let data_dir = DataDir(data.path().to_path_buf());
        let out = tempfile::tempdir().unwrap();
        let url = "https://cdn.example/x.png";

        let msg = save_cover_with(&data_dir, url, "AB", out.path(), false, |_| Ok(png(10, 10))).unwrap();
        assert!(msg.starts_with("downloaded"), "{msg}");
        assert!(out.path().join("cover.png").is_file());
        assert!(data_dir.cover_cache().join("AB.png").is_file());

        /* already there */
        assert_eq!(
            save_cover_with(&data_dir, url, "AB", out.path(), false, |_| panic!("no download expected")),
            None
        );

        /* other directory: from the full resolution cache */
        let out2 = tempfile::tempdir().unwrap();
        let msg = save_cover_with(&data_dir, url, "AB", out2.path(), false, |_| panic!("no download expected")).unwrap();
        assert!(msg.starts_with("reused cached cover"), "{msg}");

        /* download fails and nothing cached: thumbnail fallback */
        std::fs::create_dir_all(data_dir.thumbnail_cache()).unwrap();
        std::fs::write(data_dir.thumbnail_cache().join("CD.png"), png(4, 4)).unwrap();
        let out3 = tempfile::tempdir().unwrap();
        let msg = save_cover_with(&data_dir, "https://cdn.example/y.jpg", "CD", out3.path(), false, |_| Err("offline".into())).unwrap();
        assert!(msg.contains("used local thumbnail cache"), "{msg}");
        assert!(out3.path().join("cover.png").is_file());
    }
}
