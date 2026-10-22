//! The Tonie files in a CONTENT directory and what is known about them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tonie_core::tonies::{self, TonieData};
use tonie_core::TonieFile;

use crate::uid::uid_of_file;

/// A Tonie file found in the directory, before it was analyzed.
#[derive(Clone, Debug, PartialEq)]
pub struct FoundFile {
    pub path: PathBuf,
    pub uid: String,
    /// Name to show until the file is analyzed
    pub label: String,
    pub created: Option<SystemTime>,
    pub modified: Option<SystemTime>,
    pub live: bool,
}

/// Header data read from a Tonie file.
#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    pub audio_id: u32,
    pub hash: String,
    pub chapters: Vec<u32>,
    /// Length in 48 kHz samples
    pub total_granules: u64,
}

impl Analysis {
    pub fn read(path: &Path) -> Result<Analysis, tonie_core::Error> {
        let file = TonieFile::open(path, false)?;
        Ok(Analysis {
            audio_id: file.header.audio_id,
            hash: file.hash_string(),
            chapters: file.header.audio_chapters.clone(),
            total_granules: file.statistics.highest_granule,
        })
    }

    pub fn duration_secs(&self) -> f64 {
        self.total_granules as f64 / 48000.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Not in tonies.json and not made by TeddyBench
    Unknown,
    /// Made by TeddyBench (audio ID below 0x50000000) or named by the user
    Custom,
    /// A retail Tonie found in tonies.json
    Retail,
}

/// Lists the Tonie files of a CONTENT directory: `<dir>/<8 hex>/*0304E0`, and files renamed by
/// `teddy -m rename` (`<model> - <audio id> - <title>`) directly in the directory.
pub fn scan_directory(dir: &Path) -> std::io::Result<Vec<FoundFile>> {
    let mut found = Vec::new();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();

    for entry in &entries {
        if entry.is_dir() {
            let Ok(files) = std::fs::read_dir(entry) else { continue };
            let mut files: Vec<PathBuf> = files.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_file()).collect();
            files.sort();
            if let Some(file) = files
                .into_iter()
                .find(|f| f.file_name().map(|n| n.to_string_lossy().ends_with("0304E0")).unwrap_or(false))
            {
                let uid = uid_of_file(&file);
                found.push(found_file(file, uid.clone(), uid));
            }
        } else if let Some(label) = renamed_label(&entry.file_name().unwrap_or_default().to_string_lossy()) {
            found.push(found_file(entry.clone(), uid_of_file(entry), label));
        }
    }
    Ok(found)
}

fn found_file(path: PathBuf, uid: String, label: String) -> FoundFile {
    let meta = std::fs::metadata(&path).ok();
    FoundFile {
        live: crate::fsattr::is_live(&path).unwrap_or(false),
        created: meta.as_ref().and_then(|m| m.created().ok()),
        modified: meta.as_ref().and_then(|m| m.modified().ok()),
        uid,
        label,
        path,
    }
}

/// `<8 digits or NN-NNNN> - <8 hex> - <name>` -> `<product> - <name>`
fn renamed_label(name: &str) -> Option<String> {
    let (product, rest) = name.split_once(" - ")?;
    let product_ok = (product.len() == 8 && product.bytes().all(|b| b.is_ascii_digit()))
        || (product.len() == 7 && product.as_bytes()[2] == b'-' && product.bytes().enumerate().all(|(i, b)| i == 2 || b.is_ascii_digit()));
    let (id, title) = rest.split_once(" - ")?;
    let id_ok = id.len() == 8 && id.bytes().all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b));
    (product_ok && id_ok).then(|| format!("{product} - {title}"))
}

/// tonies.json plus the names the user gave to Tonies (customTonies.json, keyed by audio hash).
#[derive(Clone, Debug, Default)]
pub struct TonieDb {
    pub tonies: Vec<TonieData>,
    pub custom_names: BTreeMap<String, String>,
}

/// How a Tonie is shown in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub name: String,
    pub kind: Kind,
    /// tonies.json entry, if known
    pub info: Option<TonieData>,
}

impl TonieDb {
    pub fn load_custom_names(path: &Path) -> BTreeMap<String, String> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_custom_names(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(&self.custom_names).map_err(std::io::Error::other)?)
    }

    /// Name and kind like the original TeddyBench: the tonies.json title with model number,
    /// overridden by a custom name; "Unnamed Teddy" for files TeddyBench encoded.
    pub fn resolve(&self, uid: &str, analysis: &Analysis) -> Resolved {
        let custom = self.custom_names.get(&analysis.hash);
        let unknown = format!("[{uid}]\n(unknown: {:08X})", analysis.audio_id);

        if let Some(info) = tonies::find_by_hash(&self.tonies, &analysis.hash) {
            let mut name = info.title().to_string();
            if let Some(model) = info.model.as_deref().filter(|m| !m.is_empty()) {
                name = format!("{model} - {name}");
            }
            return Resolved {
                name: custom.cloned().unwrap_or(name),
                kind: Kind::Retail,
                info: Some(info.clone()),
            };
        }

        if let Some(name) = custom {
            Resolved {
                name: name.clone(),
                kind: Kind::Custom,
                info: None,
            }
        } else if analysis.audio_id < 0x5000_0000 {
            Resolved {
                name: format!("Unnamed Teddy - {unknown}"),
                kind: Kind::Custom,
                info: None,
            }
        } else {
            Resolved {
                name: unknown,
                kind: Kind::Unknown,
                info: None,
            }
        }
    }
}

/// Whether every word of `query` is found in one of `fields`, ignoring case
pub fn matches_query(fields: &[&str], query: &str) -> bool {
    let fields: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
    query.to_lowercase().split_whitespace().all(|word| fields.iter().any(|f| f.contains(word)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_words_must_all_match() {
        let fields = ["Bibi Blocksberg - Hexen", "E0040350010C0B0A", "0A0B0C01", "0x5E034216"];
        assert!(matches_query(&fields, ""));
        assert!(matches_query(&fields, "bibi hexen"));
        assert!(matches_query(&fields, "010c0b"));
        assert!(matches_query(&fields, "5e0342"));
        assert!(!matches_query(&fields, "bibi benjamin"));
    }

    #[test]
    fn finds_tonie_files_and_renamed_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("0A0B0C01")).unwrap();
        std::fs::write(d.join("0A0B0C01").join("500304E0"), b"").unwrap();
        std::fs::create_dir(d.join("0A0B0C02")).unwrap();
        std::fs::write(d.join("0A0B0C02").join("other"), b"").unwrap();
        std::fs::write(d.join("10000123 - 5E034216 - Some Title"), b"").unwrap();
        std::fs::write(d.join("notes.txt"), b"").unwrap();

        let found = scan_directory(d).unwrap();
        let labels: Vec<&str> = found.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, ["E0040350010C0B0A", "10000123 - Some Title"]);
        assert_eq!(found[0].uid, "E0040350010C0B0A");
    }

    #[test]
    fn names_like_the_original() {
        let tonie: TonieData = serde_json::from_str(r#"{"model":"10000123","title":"Some Title","hash":["AB"],"audio_id":["1"]}"#).unwrap();
        let mut db = TonieDb {
            tonies: vec![tonie],
            custom_names: BTreeMap::new(),
        };
        let a = |id, hash: &str| Analysis {
            audio_id: id,
            hash: hash.into(),
            chapters: vec![0],
            total_granules: 0,
        };

        assert_eq!(db.resolve("E004", &a(1, "AB")).name, "10000123 - Some Title");
        assert_eq!(db.resolve("E004", &a(1, "AB")).kind, Kind::Retail);
        assert_eq!(db.resolve("E004", &a(0x5E00_0000, "CD")).kind, Kind::Unknown);
        assert_eq!(db.resolve("E004", &a(0x5E00_0000, "CD")).name, "[E004]\n(unknown: 5E000000)");
        assert_eq!(db.resolve("E004", &a(0x1000, "CD")).name, "Unnamed Teddy - [E004]\n(unknown: 00001000)");

        db.custom_names.insert("CD".into(), "My Tonie".into());
        assert_eq!(
            db.resolve("E004", &a(0x5E00_0000, "CD")),
            Resolved {
                name: "My Tonie".into(),
                kind: Kind::Custom,
                info: None
            }
        );
    }
}
