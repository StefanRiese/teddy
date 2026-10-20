//! `teddyBench.cfg` and the location of TeddyBench's data files.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SETTINGS_FILE: &str = "teddyBench.cfg";

/// Settings, stored in the same JSON format as the C# TeddyBench.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct Settings {
    pub username: String,
    pub nfc_enabled: bool,
    pub nfc_type: String,
    pub debug_window: bool,
    pub download_json: bool,
    pub cache_image_size: u32,
    pub icon_size: u32,
    /// UID, Product, Date or Type
    pub sort_by: String,

    pub custom_tag_non_printable_margin: i32,
    pub custom_tag_gap_x: i32,
    pub custom_tag_gap_y: i32,
    pub custom_tag_initial_column: i32,
    pub custom_tag_initial_row: i32,
    pub custom_tag_size: i32,
    pub custom_tag_outline_width: f64,
    pub custom_tag_cover_filename: String,
    pub custom_tag_backside_filename: String,

    /// Settings of other versions, kept when saving
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            username: String::new(),
            nfc_enabled: false,
            nfc_type: "Proxmark3".into(),
            debug_window: false,
            download_json: true,
            cache_image_size: 256,
            icon_size: 128,
            sort_by: "Date".into(),
            custom_tag_non_printable_margin: 15,
            custom_tag_gap_x: 5,
            custom_tag_gap_y: 5,
            custom_tag_initial_column: 0,
            custom_tag_initial_row: 0,
            custom_tag_size: 40,
            custom_tag_outline_width: 0.5,
            custom_tag_cover_filename: "CustomTagCover.pdf".into(),
            custom_tag_backside_filename: "CustomTagBackside.pdf".into(),
            extra: Default::default(),
        }
    }
}

impl Settings {
    /// Reads the settings, using the defaults if the file is missing or broken.
    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)
    }
}

/// Where tonies.json, customTonies.json, the settings and the image cache live.
#[derive(Clone, Debug)]
pub struct DataDir(pub PathBuf);

impl DataDir {
    /// `TEDDYBENCH_DATA_DIR` if set; the program's directory if it already holds a teddyBench.cfg
    /// (portable installation, like the C# version); otherwise the platform's data directory.
    pub fn locate() -> DataDir {
        if let Some(dir) = std::env::var_os("TEDDYBENCH_DATA_DIR") {
            return DataDir(PathBuf::from(dir));
        }
        if let Some(exe_dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            if exe_dir.join(SETTINGS_FILE).is_file() {
                return DataDir(exe_dir);
            }
        }
        match directories::ProjectDirs::from("", "", "TeddyBench") {
            Some(dirs) => DataDir(dirs.data_dir().to_path_buf()),
            None => DataDir(PathBuf::from(".")),
        }
    }

    pub fn settings(&self) -> PathBuf {
        self.0.join(SETTINGS_FILE)
    }

    pub fn tonies_json(&self) -> PathBuf {
        self.0.join("tonies.json")
    }

    pub fn custom_tonies_json(&self) -> PathBuf {
        self.0.join("customTonies.json")
    }

    /// Thumbnails for the list, `<hash>.png`
    pub fn thumbnail_cache(&self) -> PathBuf {
        self.0.join("cache")
    }

    /// Full resolution covers, `<hash>.<ext>`
    pub fn cover_cache(&self) -> PathBuf {
        self.0.join("cache").join("covers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_csharp_format_and_keeps_unknown_fields() {
        let json = r#"{ "Username": "me", "NfcEnabled": true, "DownloadJson": false, "CacheImageSize": 300, "updateToniesJson": true }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.username, "me");
        assert!(s.nfc_enabled);
        assert!(!s.download_json);
        assert_eq!(s.cache_image_size, 300);
        assert_eq!(s.icon_size, 128);

        let out = serde_json::to_string(&s).unwrap();
        assert!(out.contains("\"updateToniesJson\":true"));
        assert!(out.contains("\"IconSize\":128"));
        assert!(out.contains("\"CustomTagOutlineWidth\":0.5"));
    }
}
