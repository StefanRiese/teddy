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
    /// The CONTENT directory opened last, opened again at the next start
    pub last_content_directory: String,

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
            last_content_directory: String::new(),
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

/// Files that hold user data and are carried over to the portable directory
const USER_FILES: &[&str] = &[SETTINGS_FILE, "customTonies.json"];

/// `TeddyBench.app` if the executable runs from a macOS app bundle
fn app_bundle(exe: &Path) -> Option<PathBuf> {
    /* <dir>/TeddyBench.app/Contents/MacOS/teddybench */
    let dir = exe.parent()?;
    let bundle = dir.parent()?.parent()?;
    (dir.ends_with("Contents/MacOS") && bundle.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))).then(|| bundle.to_path_buf())
}

/// Where the program is installed: the directory of the executable, or the one containing
/// `TeddyBench.app`. A `Content` folder there is opened on startup.
pub fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    match app_bundle(&exe) {
        Some(bundle) => bundle.parent().map(Path::to_path_buf),
        None => exe.parent().map(Path::to_path_buf),
    }
}

/// The portable data directory: next to the executable, or inside the app bundle
/// (`TeddyBench.app/Contents/Data`) on macOS.
pub fn program_dir() -> Option<PathBuf> {
    program_dir_for(&std::env::current_exe().ok()?)
}

fn program_dir_for(exe: &Path) -> Option<PathBuf> {
    match app_bundle(exe) {
        Some(bundle) => Some(bundle.join("Contents").join("Data")),
        None => exe.parent().map(Path::to_path_buf),
    }
}

/// The platform's data directory, used when the program directory is read-only.
pub fn user_data_dir() -> PathBuf {
    match directories::ProjectDirs::from("", "", "TeddyBench") {
        Some(dirs) => dirs.data_dir().to_path_buf(),
        None => PathBuf::from("."),
    }
}

fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".teddybench-write-test-{}", std::process::id()));
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            std::fs::remove_file(&probe).ok();
            true
        }
        Err(_) => false,
    }
}

/// Copies settings and custom names that are only in `from`, e.g. when switching to the portable layout.
fn carry_over(from: &Path, to: &Path) {
    for name in USER_FILES {
        let (src, dest) = (from.join(name), to.join(name));
        if src.is_file() && !dest.exists() {
            std::fs::copy(src, dest).ok();
        }
    }
}

/// Default folder for exports: `Export` next to the program on Windows and Linux, like the C#
/// version; `TeddyBench Export` in the Music folder on macOS (next to the app is usually
/// /Applications) and when the program directory is read-only.
pub fn default_export_dir() -> PathBuf {
    let music = || {
        directories::UserDirs::new()
            .and_then(|u| u.audio_dir().or(Some(u.home_dir())).map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("TeddyBench Export")
    };
    if cfg!(target_os = "macos") {
        return music();
    }
    export_dir_next_to(install_dir()).unwrap_or_else(music)
}

fn export_dir_next_to(install_dir: Option<PathBuf>) -> Option<PathBuf> {
    install_dir.filter(|d| is_writable(d)).map(|d| d.join("Export"))
}

impl DataDir {
    /// Portable by default: the program directory, or `Contents/Data` in the macOS app bundle (see
    /// [`program_dir`]), so everything stays together like with the C# version. If that is not
    /// writable (e.g. Program Files, or macOS running a downloaded app from a read-only location),
    /// the platform's data directory.
    /// `TEDDYBENCH_DATA_DIR` overrides both.
    pub fn locate() -> DataDir {
        if let Some(dir) = std::env::var_os("TEDDYBENCH_DATA_DIR") {
            return DataDir(PathBuf::from(dir));
        }
        Self::choose(program_dir(), user_data_dir())
    }

    fn choose(program_dir: Option<PathBuf>, user_dir: PathBuf) -> DataDir {
        /* the data directory inside an app bundle does not exist on the first start */
        if let Some(dir) = program_dir.as_ref().filter(|d| !d.exists() && d.ends_with("Contents/Data")) {
            std::fs::create_dir_all(dir).ok();
        }
        match program_dir {
            Some(dir) if is_writable(&dir) => {
                carry_over(&user_dir, &dir);
                DataDir(dir)
            }
            _ => DataDir(user_dir),
        }
    }

    /// Whether the data lives next to the program
    pub fn is_portable(&self) -> bool {
        program_dir().is_some_and(|p| p == self.0)
    }

    pub fn settings(&self) -> PathBuf {
        self.0.join(SETTINGS_FILE)
    }

    /// tonies.json as downloaded from api.revvox.de, or edited by the user
    pub fn tonies_json(&self) -> PathBuf {
        self.0.join("tonies.json")
    }

    /// toniesV2.json as downloaded from the tonies-json repository
    pub fn tonies_v2_json(&self) -> PathBuf {
        self.0.join("toniesV2.json")
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
    fn program_dir_of_executables_and_app_bundles() {
        assert_eq!(program_dir_for(Path::new("/opt/teddy/teddybench")), Some(PathBuf::from("/opt/teddy")));
        assert_eq!(
            program_dir_for(Path::new("/Applications/TeddyBench.app/Contents/MacOS/teddybench")),
            Some(PathBuf::from("/Applications/TeddyBench.app/Contents/Data"))
        );
        /* not a bundle, just a directory called MacOS */
        assert_eq!(
            program_dir_for(Path::new("/x/Contents/MacOS/teddybench")),
            Some(PathBuf::from("/x/Contents/MacOS"))
        );
    }

    #[test]
    fn portable_directory_with_carried_over_settings() {
        let program = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        std::fs::write(user.path().join(SETTINGS_FILE), "{\"IconSize\": 64}").unwrap();
        std::fs::write(user.path().join("customTonies.json"), "{}").unwrap();

        let dir = DataDir::choose(Some(program.path().to_path_buf()), user.path().to_path_buf());
        assert_eq!(dir.0, program.path());
        assert_eq!(Settings::load(&dir.settings()).icon_size, 64);
        assert!(dir.custom_tonies_json().is_file());
        /* nothing left behind from the write test */
        assert_eq!(std::fs::read_dir(program.path()).unwrap().count(), 2);
    }

    #[test]
    fn creates_the_data_directory_in_an_app_bundle() {
        let apps = tempfile::tempdir().unwrap();
        let data = apps.path().join("TeddyBench.app").join("Contents").join("Data");
        std::fs::create_dir_all(data.parent().unwrap().join("MacOS")).unwrap();
        let user = tempfile::tempdir().unwrap();
        assert_eq!(DataDir::choose(Some(data.clone()), user.path().to_path_buf()).0, data);
        assert!(data.is_dir());
    }

    #[test]
    fn export_directory_next_to_the_program() {
        let program = tempfile::tempdir().unwrap();
        assert_eq!(export_dir_next_to(Some(program.path().to_path_buf())), Some(program.path().join("Export")));
        assert_eq!(export_dir_next_to(Some(program.path().join("missing"))), None);
        assert_eq!(export_dir_next_to(None), None);
    }

    #[test]
    fn falls_back_to_the_user_directory() {
        let user = tempfile::tempdir().unwrap();
        let missing = user.path().join("does-not-exist");
        assert_eq!(DataDir::choose(Some(missing), user.path().to_path_buf()).0, user.path());
        assert_eq!(DataDir::choose(None, user.path().to_path_buf()).0, user.path());
    }

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
