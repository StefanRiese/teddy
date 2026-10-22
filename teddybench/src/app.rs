//! The TeddyBench window.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use eframe::egui::{self, Color32, ColorImage, Id, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use teddybench::export::{ExportFormat, ExportItem, ExportOptions, JobStatus};
use teddybench::library::{Analysis, FoundFile, Kind, Resolved, TonieDb};
use teddybench::player::{Player, TonieSource};
use teddybench::rfid::{self, DeviceInfo, Measurement, Reader, ReaderKind};
use teddybench::scanner::{Command, Event, Scanner};
use teddybench::settings::{DataDir, Settings};
use teddybench::tagpdf::{TagLayout, TagSide};
use teddybench::trashcan::{self, TrashItem};
use teddybench::uid::{is_valid_uid, random_uid, UID_PREFIX};
use teddybench::{actions, applog, export, fsattr, tagpdf, uid, VERSION};

const RELEASES_URL: &str = "https://github.com/StefanRiese/teddy/releases";
use tonie_core::tonies::TonieData;

const MIN_ICON: u32 = 32;
const MAX_ICON: u32 = 256;
const HEADER_HEIGHT: f32 = 28.0;
const SEARCH_ID: &str = "search";
const LABEL_LINES: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Uid,
    Product,
    Date,
    Type,
}

impl SortBy {
    const ALL: [SortBy; 4] = [SortBy::Uid, SortBy::Product, SortBy::Date, SortBy::Type];

    fn from_setting(s: &str) -> SortBy {
        SortBy::ALL.into_iter().find(|v| v.key() == s).unwrap_or(SortBy::Date)
    }

    fn key(self) -> &'static str {
        match self {
            SortBy::Uid => "UID",
            SortBy::Product => "Product",
            SortBy::Date => "Date",
            SortBy::Type => "Type",
        }
    }

    fn label(self) -> &'static str {
        match self {
            SortBy::Uid => "UID",
            SortBy::Product => "Product Number",
            SortBy::Date => "Date",
            SortBy::Type => "Type",
        }
    }
}

struct Entry {
    file: FoundFile,
    analysis: Option<Result<Analysis, String>>,
    resolved: Option<Resolved>,
}

impl Entry {
    fn name(&self) -> String {
        let name = self.resolved.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| self.file.label.clone());
        if self.file.live {
            format!("[live] {name}")
        } else {
            name
        }
    }

    fn kind(&self) -> Kind {
        self.resolved.as_ref().map(|r| r.kind).unwrap_or(Kind::Unknown)
    }

    fn info(&self) -> Option<&TonieData> {
        self.resolved.as_ref().and_then(|r| r.info.as_ref())
    }

    fn analysis(&self) -> Option<&Analysis> {
        self.analysis.as_ref().and_then(|a| a.as_ref().ok())
    }

    fn date(&self) -> Option<SystemTime> {
        self.file.created.or(self.file.modified)
    }

    /// Key of the texture to show
    fn image_key(&self) -> String {
        match (self.kind(), self.analysis()) {
            (Kind::Retail, Some(a)) => a.hash.clone(),
            (Kind::Custom, _) => "custom".into(),
            _ => "unknown".into(),
        }
    }

    fn export_item(&self) -> ExportItem {
        ExportItem {
            path: self.file.path.clone(),
            uid: self.file.uid.clone(),
            info: self.info().cloned(),
        }
    }
}

/// Creating a new Tonie from audio files, or copying a Tonie file
struct NewTonie {
    files: Vec<PathBuf>,
    descriptions: Vec<String>,
    selected: Option<usize>,
    uid: String,
    custom_audio_id: bool,
    audio_id: String,
    /// A single existing Tonie file that is copied instead of encoded
    copy_tonie_file: bool,
    /// Custom name for the new Tonie, from the tags of the files
    name: String,
}

struct ExportDialog {
    items: Vec<ExportItem>,
    format: ExportFormat,
    create_dirs: bool,
    out_dir: String,
}

/// The CustomTag* settings while they are edited
struct TagLayoutDraft {
    margin: i32,
    gap_x: i32,
    gap_y: i32,
    initial_column: i32,
    initial_row: i32,
    size: i32,
    outline_width: f64,
}

enum Dialog {
    NewTonie(NewTonie),
    AssignUid {
        path: PathBuf,
        uid: String,
    },
    Rename {
        hash: String,
        name: String,
    },
    ConfirmDelete(Vec<PathBuf>),
    /// Remove the live flag from these Tonies
    ConfirmRemoveLive(Vec<PathBuf>),
    Export(ExportDialog),
    Message {
        title: String,
        text: String,
    },
    About,
    TagLayout(TagLayoutDraft),
    /// UID and memory of a tag as hex: shown after reading, or entered for the emulation
    TagDump {
        text: String,
        input: bool,
        error: Option<String>,
    },
    Antenna(Measurement),
    Trashcan(TrashView),
}

/// The deleted Tonies in the TRASHCAN folder of the card
struct TrashView {
    /// None while the files are read
    items: Option<Vec<TrashItem>>,
    rx: Option<Receiver<Vec<TrashItem>>>,
    selected: Option<usize>,
    /// UID to restore the selected Tonie for
    uid: String,
    /// Restore with a new audio ID instead of byte for byte
    as_new: bool,
    confirm_delete: bool,
    /// Result of the last restore or delete, and whether it failed
    status: Option<(String, bool)>,
}

/// What the NFC reader is busy with while a "Tag operation" window is shown
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagOperation {
    Read,
    Emulate,
    Measure,
}

/// The NFC reader and what it reported
#[derive(Default)]
struct Nfc {
    /// The kind the running reader looks for
    kind: Option<ReaderKind>,
    reader: Option<Reader>,
    device: Option<DeviceInfo>,
    /// The tag on the reader
    uid: Option<String>,
    /// The selection was made because a tag was put on the reader, and is undone when it is removed
    auto_selected: bool,
    operation: Option<TagOperation>,
    console: bool,
    /// Tag content last read, offered for the emulation
    last_dump: String,
}

/// A selection frame being dragged in the grid
struct FrameSelect {
    /// Where the drag started, relative to the top left of the grid content (moves with scrolling)
    start: egui::Pos2,
    /// Selection when the drag started, kept with Ctrl/Cmd
    base: BTreeSet<PathBuf>,
}

struct RunningJob {
    title: String,
    status: Arc<JobStatus>,
    total: usize,
}

/// Things the user asked for while the UI was drawn; applied afterwards.
enum Action {
    OpenDirectory,
    Add(Vec<PathBuf>),
    Play,
    Stop,
    Seek(f64),
    ToggleLive,
    RemoveAllLive,
    Trashcan,
    Rename,
    Delete,
    AssignUid,
    RandomUid,
    ShowInFolder,
    Export(ExportFormat),
    ExportTonieFile,
    CopyToSd,
    DownloadJson,
    SelectAll,
    TagPdf(TagSide),
    ReadTag,
    EmulateTag,
    MeasureAntenna,
}

pub struct TeddyBenchApp {
    data_dir: DataDir,
    settings: Settings,
    db: TonieDb,
    json_status: String,
    json_rx: Option<Receiver<(Vec<TonieData>, String)>>,
    scanner: Scanner,
    current_dir: Option<PathBuf>,
    entries: Vec<Entry>,
    textures: HashMap<String, TextureHandle>,
    thumbs_requested: HashSet<String>,
    selection: BTreeSet<PathBuf>,
    /// Anchor for shift-click range selection
    anchor: Option<PathBuf>,
    frame_select: Option<FrameSelect>,
    sort: SortBy,
    dialog: Option<Dialog>,
    job: Option<RunningJob>,
    nfc: Nfc,
    /// Scroll the grid to this Tonie
    scroll_to: Option<PathBuf>,
    /// Audio ID last given to a new Tonie
    last_audio_id: Option<u32>,
    /// Custom names of Tonies being created, stored once their hash is known
    pending_names: HashMap<PathBuf, String>,
    /// Only Tonies matching this are shown
    search: String,
    player: Option<Player>,
    playing: Option<PathBuf>,
    loading_rx: Option<Receiver<Result<TonieSource, String>>>,
    /// Position while the user drags the slider
    seek_drag: Option<f64>,
    title: String,
    /// Development aid (debug builds): save a screenshot to this file once the window settled, then quit
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    screenshot: Option<(PathBuf, std::time::Instant)>,
}

fn texture_from(ctx: &egui::Context, name: &str, img: &image::RgbaImage) -> TextureHandle {
    let color = ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    ctx.load_texture(name, color, TextureOptions::LINEAR)
}

fn format_time(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn format_date(t: Option<SystemTime>) -> String {
    t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| tonie_core::info::format_unix_time(d.as_secs() as i64).replace('T', " "))
        .unwrap_or_default()
}

impl TeddyBenchApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> TeddyBenchApp {
        let data_dir = DataDir::locate();
        std::fs::create_dir_all(&data_dir.0).ok();
        let settings = Settings::load(&data_dir.settings());
        let custom_names = TonieDb::load_custom_names(&data_dir.custom_tonies_json());

        /* a directory given on the command line, or a Content folder next to the program, like the original */
        /* older macOS passes a process serial number (-psn_...) when started from the Finder */
        let start_dir = std::env::args_os()
            .skip(1)
            .find(|a| !a.to_string_lossy().starts_with("-psn"))
            .map(PathBuf::from)
            .or_else(|| Some(teddybench::settings::install_dir()?.join("Content")).filter(|d| d.is_dir()));

        let ctx = cc.egui_ctx.clone();
        let scanner = Scanner::start(
            data_dir.clone(),
            settings.cache_image_size.clamp(MIN_ICON, 1024),
            start_dir.is_none(),
            move || ctx.request_repaint(),
        );
        if let Some(dir) = start_dir {
            scanner.send(Command::Open(dir));
        }

        let mut textures = HashMap::new();
        for (key, bytes) in [
            ("unknown", &include_bytes!("../assets/unknown.png")[..]),
            ("custom", &include_bytes!("../assets/custom.png")[..]),
        ] {
            if let Ok(img) = image::load_from_memory(bytes) {
                let img = img.resize_exact(256, 256, image::imageops::FilterType::Lanczos3).to_rgba8();
                textures.insert(key.to_string(), texture_from(&cc.egui_ctx, key, &img));
            }
        }

        let mut app = TeddyBenchApp {
            data_dir,
            settings,
            db: TonieDb {
                tonies: Vec::new(),
                custom_names,
            },
            json_status: "Loading tonies.json...".into(),
            json_rx: None,
            scanner,
            current_dir: None,
            entries: Vec::new(),
            textures,
            thumbs_requested: HashSet::new(),
            selection: BTreeSet::new(),
            anchor: None,
            frame_select: None,
            sort: SortBy::Date,
            dialog: None,
            job: None,
            nfc: Nfc::default(),
            scroll_to: None,
            last_audio_id: None,
            pending_names: HashMap::new(),
            search: String::new(),
            player: None,
            playing: None,
            loading_rx: None,
            seek_drag: None,
            title: String::new(),
            screenshot: std::env::var_os("TEDDYBENCH_SCREENSHOT").map(|p| (PathBuf::from(p), std::time::Instant::now())),
        };
        app.sort = SortBy::from_setting(&app.settings.sort_by);
        app.load_json(app.settings.download_json, &cc.egui_ctx);
        app.update_reader(&cc.egui_ctx);
        app
    }

    /// Starts or stops the NFC reader as the settings say
    fn update_reader(&mut self, ctx: &egui::Context) {
        let kind = self.settings.nfc_enabled.then(|| ReaderKind::from_setting(&self.settings.nfc_type));
        if kind == self.nfc.kind {
            return;
        }
        if let Some(old) = self.nfc.reader.take() {
            /* stopping waits for the device to answer, don't block the window meanwhile */
            std::thread::spawn(move || drop(old));
        }
        self.uid_found(None);
        let last_dump = std::mem::take(&mut self.nfc.last_dump);
        self.nfc = Nfc {
            kind,
            last_dump,
            ..Default::default()
        };
        if let Some(kind) = kind {
            let ctx = ctx.clone();
            self.nfc.reader = Some(Reader::start(kind, move || ctx.request_repaint()));
        }
    }

    /// A tag was put on the reader (or removed): select its Tonie and fill in the UID where one is asked for
    fn uid_found(&mut self, uid: Option<String>) {
        if uid == self.nfc.uid {
            return;
        }
        self.nfc.uid = uid.clone();

        let found = uid.as_ref().and_then(|uid| {
            match &mut self.dialog {
                Some(Dialog::AssignUid { uid: u, .. }) => *u = uid.clone(),
                Some(Dialog::NewTonie(nt)) => nt.uid = uid.clone(),
                Some(Dialog::Trashcan(t)) => t.uid = uid.clone(),
                _ => {}
            }
            self.entries.iter().find(|e| e.file.uid.eq_ignore_ascii_case(uid)).map(|e| e.file.path.clone())
        });
        match found {
            Some(path) => {
                self.selection = BTreeSet::from([path.clone()]);
                self.anchor = Some(path.clone());
                self.scroll_to = Some(path);
                self.nfc.auto_selected = true;
            }
            None if self.nfc.auto_selected => {
                self.selection.clear();
                self.nfc.auto_selected = false;
            }
            None => {}
        }
    }

    fn process_reader_events(&mut self) {
        let events: Vec<rfid::Event> = match &self.nfc.reader {
            Some(r) => r.events.try_iter().collect(),
            None => return,
        };
        for event in events {
            match event {
                rfid::Event::Device(device) => {
                    if device.is_none() {
                        self.nfc.console = false;
                        if self.nfc.operation.take().is_some() {
                            self.message("NFC reader", "The reader was lost.");
                        }
                    }
                    self.nfc.device = device;
                }
                rfid::Event::Uid(uid) => self.uid_found(uid),
                rfid::Event::Memory(data) => {
                    /* None if cancelled meanwhile */
                    if self.nfc.operation.take() != Some(TagOperation::Read) {
                        continue;
                    }
                    match data {
                        Some(data) => {
                            let text: String = data.iter().map(|b| format!("{b:02X}")).collect();
                            self.nfc.last_dump = text.clone();
                            self.dialog = Some(Dialog::TagDump {
                                text,
                                input: false,
                                error: None,
                            });
                        }
                        None => self.message("Failed", "No tag found. Please make sure you position it correctly."),
                    }
                }
                rfid::Event::Measurement(m) => {
                    if self.nfc.operation.take() != Some(TagOperation::Measure) {
                        continue;
                    }
                    match m {
                        Some(m) => self.dialog = Some(Dialog::Antenna(m)),
                        None => self.message("Failed", "Measurement failed."),
                    }
                }
                rfid::Event::EmulationEnded => {
                    if self.nfc.operation == Some(TagOperation::Emulate) {
                        self.nfc.operation = None;
                    }
                }
            }
        }
    }

    fn save_settings(&self) {
        self.settings.save(&self.data_dir.settings()).ok();
    }

    fn message(&mut self, title: &str, text: impl Into<String>) {
        let text = text.into();
        applog::info(format!("{title}: {text}"));
        self.dialog = Some(Dialog::Message { title: title.into(), text });
    }

    fn load_json(&mut self, download: bool, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let data_dir = self.data_dir.clone();
        let ctx = ctx.clone();
        self.json_status = if download { "Downloading...".into() } else { "Loading...".into() };
        std::thread::spawn(move || {
            tx.send(teddybench::load_tonies_json(&data_dir, download)).ok();
            ctx.request_repaint();
        });
        self.json_rx = Some(rx);
    }

    // ---- background events ----

    fn process_events(&mut self, ctx: &egui::Context) {
        self.process_reader_events();

        if let Some(rx) = &self.json_rx {
            if let Ok((tonies, status)) = rx.try_recv() {
                self.db.tonies = tonies;
                self.json_status = status;
                self.json_rx = None;
                self.resolve_all();
            }
        }

        while let Ok(event) = self.scanner.events.try_recv() {
            match event {
                Event::Opened(dir) => {
                    self.current_dir = Some(dir);
                    self.entries.clear();
                    self.selection.clear();
                }
                Event::Lost => {
                    self.current_dir = None;
                    self.entries.clear();
                    self.selection.clear();
                    self.stop_playback();
                }
                Event::Files(files) => {
                    let mut old: HashMap<PathBuf, Entry> = self.entries.drain(..).map(|e| (e.file.path.clone(), e)).collect();
                    self.entries = files
                        .into_iter()
                        .map(|file| match old.remove(&file.path) {
                            Some(mut e) => {
                                e.file = file;
                                e
                            }
                            None => Entry {
                                file,
                                analysis: None,
                                resolved: None,
                            },
                        })
                        .collect();
                    let paths: HashSet<&PathBuf> = self.entries.iter().map(|e| &e.file.path).collect();
                    self.selection.retain(|p| paths.contains(p));
                }
                Event::Analyzed(path, result) => {
                    if let (Ok(a), Some(name)) = (&result, self.pending_names.remove(&path)) {
                        self.db.custom_names.insert(a.hash.clone(), name);
                        self.db.save_custom_names(&self.data_dir.custom_tonies_json()).ok();
                    }
                    if let Some(i) = self.entries.iter().position(|e| e.file.path == path) {
                        self.entries[i].analysis = Some(result);
                        self.resolve(i);
                    }
                }
                Event::Thumbnail(hash, result) => {
                    if let Ok(img) = result {
                        let tex = texture_from(ctx, &hash, &img);
                        self.textures.insert(hash, tex);
                    }
                }
            }
        }

        if let Some(rx) = &self.loading_rx {
            if let Ok(result) = rx.try_recv() {
                self.loading_rx = None;
                match result {
                    Ok(source) => match self.player.as_mut() {
                        Some(p) => p.play(source),
                        None => self.playing = None,
                    },
                    Err(e) => {
                        self.playing = None;
                        self.message("Play file", format!("Failed to play file: {e}"));
                    }
                }
            }
        }
        if self.loading_rx.is_none() && self.playing.is_some() && !self.player.as_ref().map(|p| p.is_playing()).unwrap_or(false) {
            self.playing = None;
        }

        if let Some(job) = &self.job {
            if job.status.text().done && job.status.cancelled() {
                self.job = None;
                self.scanner.send(Command::Rescan);
            }
        }
    }

    fn resolve(&mut self, i: usize) {
        let entry = &mut self.entries[i];
        let Some(analysis) = entry.analysis.as_ref().and_then(|a| a.as_ref().ok()) else {
            return;
        };
        let resolved = self.db.resolve(&entry.file.uid, analysis);
        if let (Kind::Retail, Some(pic)) = (resolved.kind, resolved.info.as_ref().and_then(|i| i.pic.clone()).filter(|p| !p.is_empty())) {
            if self.thumbs_requested.insert(analysis.hash.clone()) {
                self.scanner.send(Command::Thumbnail(analysis.hash.clone(), pic));
            }
        }
        entry.resolved = Some(resolved);
    }

    fn resolve_all(&mut self) {
        for i in 0..self.entries.len() {
            self.resolve(i);
        }
    }

    // ---- selection ----

    /// Whether the Tonie matches the search: name, UID, folder, audio ID or file name
    fn matches_search(&self, e: &Entry) -> bool {
        if self.search.trim().is_empty() {
            return true;
        }
        let name = e.name();
        let folder = e
            .file
            .path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = e.file.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let audio_id = e.analysis().map(|a| format!("0x{:08X}", a.audio_id)).unwrap_or_default();
        teddybench::library::matches_query(&[&name, &e.file.uid, &folder, &file, &audio_id], &self.search)
    }

    /// Deselects the Tonies the search hides, so actions only affect visible ones
    fn deselect_hidden(&mut self) {
        let visible: HashSet<PathBuf> = self.entries.iter().filter(|e| self.matches_search(e)).map(|e| e.file.path.clone()).collect();
        self.selection.retain(|p| visible.contains(p));
    }

    fn sorted_indices(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.entries.len()).filter(|&i| self.matches_search(&self.entries[i])).collect();
        let e = &self.entries;
        match self.sort {
            SortBy::Uid => idx.sort_by(|&a, &b| e[a].file.uid.cmp(&e[b].file.uid)),
            SortBy::Product => idx.sort_by(|&a, &b| {
                let m = |i: usize| e[i].info().and_then(|t| t.model.clone());
                m(a).cmp(&m(b)).then(e[a].file.uid.cmp(&e[b].file.uid))
            }),
            SortBy::Date => idx.sort_by(|&a, &b| e[a].date().cmp(&e[b].date()).then(e[a].file.uid.cmp(&e[b].file.uid))),
            SortBy::Type => idx.sort_by(|&a, &b| {
                let k = |i: usize| (e[i].kind(), e[i].info().map(|t| t.sort_string()), e[i].name());
                k(a).cmp(&k(b))
            }),
        }
        idx
    }

    fn selected_entries(&self) -> Vec<&Entry> {
        self.entries.iter().filter(|e| self.selection.contains(&e.file.path)).collect()
    }

    fn single_selected(&self) -> Option<&Entry> {
        let sel = self.selected_entries();
        (sel.len() == 1).then(|| sel[0])
    }

    fn click(&mut self, path: &Path, modifiers: egui::Modifiers, order: &[usize]) {
        self.nfc.auto_selected = false;
        if modifiers.shift {
            if let Some(anchor) = &self.anchor {
                let pos = |p: &Path| order.iter().position(|&i| self.entries[i].file.path == p);
                if let (Some(a), Some(b)) = (pos(anchor), pos(path)) {
                    let (a, b) = (a.min(b), a.max(b));
                    if !modifiers.command {
                        self.selection.clear();
                    }
                    for &i in &order[a..=b] {
                        self.selection.insert(self.entries[i].file.path.clone());
                    }
                    return;
                }
            }
        }
        if modifiers.command {
            if !self.selection.remove(path) {
                self.selection.insert(path.to_path_buf());
            }
        } else {
            self.selection.clear();
            self.selection.insert(path.to_path_buf());
        }
        self.anchor = Some(path.to_path_buf());
    }

    // ---- actions ----

    fn apply(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::OpenDirectory => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Open the CONTENT directory of the SD card").pick_folder() {
                    self.scanner.send(Command::Open(dir));
                }
            }
            Action::Add(files) => self.start_new_tonie(files),
            Action::Play => self.play_selected(ctx),
            Action::Stop => self.stop_playback(),
            Action::Seek(secs) => {
                if let Some(p) = &self.player {
                    p.seek(Duration::from_secs_f64(secs));
                }
            }
            Action::ToggleLive => {
                let files: Vec<PathBuf> = self.selected_entries().iter().map(|e| e.file.path.clone()).collect();
                /* with several Tonies selected, the live flag is removed from all of them */
                let unset_all = files.len() > 1;
                let mut errors = Vec::new();
                for f in &files {
                    let live = !unset_all && !fsattr::is_live(f).unwrap_or(false);
                    if let Err(e) = fsattr::set_live(f, live) {
                        errors.push(format!("{}: {e}", f.display()));
                    }
                }
                if !errors.is_empty() {
                    self.message("Toggle live flag", errors.join("\n"));
                }
                self.scanner.send(Command::Rescan);
            }
            Action::Trashcan => {
                let Some(dir) = self.current_dir.as_deref().and_then(trashcan::find) else {
                    self.message(
                        "Deleted Tonies",
                        "There is no TRASHCAN folder next to the CONTENT folder. A Toniebox connected to the cloud \
                         moves deleted Tonies there.",
                    );
                    return;
                };
                let (tx, rx) = channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    tx.send(trashcan::scan(&dir)).ok();
                    ctx.request_repaint();
                });
                self.dialog = Some(Dialog::Trashcan(TrashView {
                    items: None,
                    rx: Some(rx),
                    selected: None,
                    uid: self.nfc.uid.clone().unwrap_or_else(|| UID_PREFIX.into()),
                    as_new: false,
                    confirm_delete: false,
                    status: None,
                }));
            }
            Action::RemoveAllLive => {
                let files: Vec<PathBuf> = self.entries.iter().filter(|e| e.file.live).map(|e| e.file.path.clone()).collect();
                if !files.is_empty() {
                    self.dialog = Some(Dialog::ConfirmRemoveLive(files));
                }
            }
            Action::Rename => {
                if let Some(e) = self.single_selected() {
                    match e.analysis() {
                        Some(a) => {
                            let name = e.resolved.as_ref().map(|r| r.name.clone()).unwrap_or_default();
                            self.dialog = Some(Dialog::Rename { hash: a.hash.clone(), name });
                        }
                        None => self.message("Rename", "The file has not been read yet."),
                    }
                }
            }
            Action::Delete => {
                let files: Vec<PathBuf> = self.selection.iter().cloned().collect();
                if !files.is_empty() {
                    self.dialog = Some(Dialog::ConfirmDelete(files));
                }
            }
            Action::AssignUid => {
                if let Some(e) = self.single_selected() {
                    self.dialog = Some(Dialog::AssignUid {
                        path: e.file.path.clone(),
                        uid: e.file.uid.clone(),
                    });
                }
            }
            Action::RandomUid => {
                if let (Some(e), Some(dir)) = (self.single_selected(), self.current_dir.clone()) {
                    let path = e.file.path.clone();
                    self.assign_uid(&path, &random_uid(), &dir);
                }
            }
            Action::ShowInFolder => {
                if let Some(e) = self.single_selected() {
                    if let Err(err) = actions::show_in_file_manager(&e.file.path) {
                        self.message("Show in file manager", err.to_string());
                    }
                }
            }
            Action::Export(format) => {
                let items: Vec<ExportItem> = self.selected_entries().iter().map(|e| e.export_item()).collect();
                if !items.is_empty() {
                    let out_dir = self
                        .settings
                        .extra
                        .get("ExportDirectory")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| teddybench::settings::default_export_dir().display().to_string());
                    self.dialog = Some(Dialog::Export(ExportDialog {
                        items,
                        format,
                        create_dirs: true,
                        out_dir,
                    }));
                }
            }
            Action::ExportTonieFile => {
                let items: Vec<(ExportItem, u32)> = self
                    .selected_entries()
                    .iter()
                    .filter_map(|e| e.analysis().map(|a| (e.export_item(), a.audio_id)))
                    .collect();
                if items.is_empty() {
                    return;
                }
                if let Some(dir) = rfd::FileDialog::new().set_title("Export Tonie files to").pick_folder() {
                    let errors: Vec<String> = items
                        .iter()
                        .filter_map(|(item, id)| export::export_tonie_file(item, *id, &dir).err().map(|e| e.to_string()))
                        .collect();
                    self.message("Export Tonie file", if errors.is_empty() { "Finished".to_string() } else { errors.join("\n") });
                }
            }
            Action::CopyToSd => {
                let files: Vec<PathBuf> = self.selection.iter().cloned().collect();
                if files.is_empty() {
                    return;
                }
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Copy to MicroSD: choose the CONTENT directory of the target card")
                    .pick_folder()
                {
                    let job = self.start_job("Copy to MicroSD", files.len());
                    std::thread::spawn(move || export::copy_to_sd(&files, &dir, &job));
                }
            }
            Action::DownloadJson => self.load_json(true, ctx),
            Action::SelectAll => self.selection = self.sorted_indices().into_iter().map(|i| self.entries[i].file.path.clone()).collect(),
            Action::TagPdf(side) => self.create_tag_pdf(side),
            Action::ReadTag => {
                if let Some(r) = &self.nfc.reader {
                    r.read_memory();
                    self.nfc.operation = Some(TagOperation::Read);
                }
            }
            Action::EmulateTag => {
                self.dialog = Some(Dialog::TagDump {
                    text: self.nfc.last_dump.clone(),
                    input: true,
                    error: None,
                });
            }
            Action::MeasureAntenna => {
                if let Some(r) = &self.nfc.reader {
                    r.measure_antenna();
                    self.nfc.operation = Some(TagOperation::Measure);
                }
            }
        }
    }

    fn start_job(&mut self, title: &str, total: usize) -> Arc<JobStatus> {
        let status = Arc::new(JobStatus::default());
        self.job = Some(RunningJob {
            title: title.into(),
            status: status.clone(),
            total,
        });
        status
    }

    fn start_new_tonie(&mut self, files: Vec<PathBuf>) {
        if self.current_dir.is_none() {
            self.message("Add", "Please open the CONTENT directory of the SD card first.");
            return;
        }
        if files.is_empty() {
            return;
        }
        let audio: Vec<PathBuf> = files.iter().filter(|f| tonie_core::audio_in::is_supported(f)).cloned().collect();
        let copy_tonie_file = files.len() == 1 && audio.is_empty();
        if !copy_tonie_file && audio.len() != files.len() {
            self.message(
                "Add",
                format!("Please select audio files only ({}).", tonie_core::audio_in::SUPPORTED_EXTENSIONS.join(", ")),
            );
            return;
        }
        let files = if copy_tonie_file { files } else { actions::initial_track_order(audio) };
        let descriptions = files
            .iter()
            .map(|f| if copy_tonie_file { String::new() } else { actions::id3_description(f) })
            .collect();
        let name = if copy_tonie_file {
            String::new()
        } else {
            actions::title_from_tags(&files).unwrap_or_default()
        };
        self.dialog = Some(Dialog::NewTonie(NewTonie {
            files,
            descriptions,
            selected: None,
            /* the tag on the reader, if there is one */
            uid: self.nfc.uid.clone().unwrap_or_else(|| UID_PREFIX.into()),
            custom_audio_id: false,
            audio_id: String::new(),
            copy_tonie_file,
            name,
        }));
    }

    fn play_selected(&mut self, ctx: &egui::Context) {
        let Some(path) = self.single_selected().map(|e| e.file.path.clone()) else {
            return;
        };
        if self.player.is_none() {
            match Player::new() {
                Ok(p) => self.player = Some(p),
                Err(e) => {
                    self.message("Play file", format!("No audio output available: {e}"));
                    return;
                }
            }
        }
        self.stop_playback();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let p = path.clone();
        std::thread::spawn(move || {
            tx.send(TonieSource::open(&p)).ok();
            ctx.request_repaint();
        });
        self.loading_rx = Some(rx);
        self.playing = Some(path);
    }

    fn stop_playback(&mut self) {
        if let Some(p) = self.player.as_mut() {
            p.stop();
        }
        self.playing = None;
        self.loading_rx = None;
    }

    fn assign_uid(&mut self, path: &Path, uid: &str, content_dir: &Path) {
        match actions::reassign_uid(path, uid, content_dir) {
            Ok(Some(note)) => self.message("Successfully re-assigned UID", format!("Success. {note}")),
            Ok(None) => {}
            Err(e) => self.message("Re-assigning UID failed", e),
        }
        self.selection.clear();
        self.scanner.send(Command::Rescan);
    }

    // ---- drawing ----

    fn menu_bar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open directory...").clicked() {
                    actions.push(Action::OpenDirectory);
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            ui.menu_button("Custom tag", |ui| {
                let any = !self.selection.is_empty();
                if ui.add_enabled(any, egui::Button::new("Create tag cover...")).clicked() {
                    actions.push(Action::TagPdf(TagSide::Cover));
                }
                if ui.add_enabled(any, egui::Button::new("Create tag backside...")).clicked() {
                    actions.push(Action::TagPdf(TagSide::Backside));
                }
                ui.separator();
                if ui.button("Print layout...").clicked() {
                    let s = &self.settings;
                    self.dialog = Some(Dialog::TagLayout(TagLayoutDraft {
                        margin: s.custom_tag_non_printable_margin,
                        gap_x: s.custom_tag_gap_x,
                        gap_y: s.custom_tag_gap_y,
                        initial_column: s.custom_tag_initial_column,
                        initial_row: s.custom_tag_initial_row,
                        size: s.custom_tag_size,
                        outline_width: s.custom_tag_outline_width,
                    }));
                }
            });
            ui.menu_button("NFC reader", |ui| self.nfc_menu(ui, actions));
            ui.menu_button("Options", |ui| {
                if ui.checkbox(&mut self.settings.download_json, "Download tonies.json on startup").changed() {
                    self.save_settings();
                }
                if ui.button("Download tonies.json now").clicked() {
                    actions.push(Action::DownloadJson);
                }
                if ui.checkbox(&mut self.settings.debug_window, "Debug window").changed() {
                    self.save_settings();
                }
                ui.separator();
                let kind = if self.data_dir.is_portable() {
                    "portable"
                } else {
                    "program directory is read-only"
                };
                ui.label(format!("Data directory ({kind}): {}", self.data_dir.0.display()));
                if ui.button("Open data directory").clicked() {
                    if let Err(e) = actions::open_with_default_program(&self.data_dir.0) {
                        applog::warning(format!("Cannot open {}: {e}", self.data_dir.0.display()));
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Releases page").on_hover_text(RELEASES_URL).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(RELEASES_URL));
                }
                if ui.button("About").clicked() {
                    self.dialog = Some(Dialog::About);
                }
            });
        });
    }

    fn nfc_menu(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let mut changed = ui
            .checkbox(&mut self.settings.nfc_enabled, "Autodetect reader")
            .on_hover_text("Look for the reader on the USB serial ports. The UID of a tag on it selects its Tonie and is filled in where one is asked for.")
            .changed();
        let mut kind = ReaderKind::from_setting(&self.settings.nfc_type);
        for k in ReaderKind::ALL {
            changed |= ui.radio_value(&mut kind, k, k.label()).changed();
        }
        if changed {
            self.settings.nfc_type = kind.setting().into();
            self.save_settings();
            self.update_reader(ui.ctx());
        }

        ui.separator();
        match (&self.nfc.device, self.settings.nfc_enabled) {
            (Some(d), _) => ui.label(format!("{} at {}", d.hardware, d.port)),
            (None, true) => ui.label(egui::RichText::new(format!("Looking for a {}...", kind.label())).weak()),
            (None, false) => ui.label(egui::RichText::new("Not looking for a reader").weak()),
        };

        let pm3 = self.nfc.device.as_ref().filter(|d| d.kind == ReaderKind::Proxmark3);
        let slix = pm3.is_some_and(|d| d.unlock_supported);
        let busy = self.nfc.operation.is_some();
        ui.add_enabled_ui(pm3.is_some(), |ui| {
            ui.menu_button("Advanced actions", |ui| {
                const NEEDS_FIRMWARE: &str = "Needs the Proxmark3 firmware with the SLIX-L features";
                let mut item = |ui: &mut egui::Ui, enabled: bool, text: &str, action: fn() -> Action| {
                    let r = ui.add_enabled(enabled && !busy, egui::Button::new(text));
                    let r = if enabled { r } else { r.on_disabled_hover_text(NEEDS_FIRMWARE) };
                    if r.clicked() {
                        actions.push(action());
                        ui.close();
                    }
                };
                item(ui, slix, "Read tag content", || Action::ReadTag);
                item(ui, slix, "Emulate tag...", || Action::EmulateTag);
                item(ui, true, "Measure antenna", || Action::MeasureAntenna);
                ui.separator();
                let r = ui.add_enabled(slix, egui::Checkbox::new(&mut self.nfc.console, "Console mode (experts only)"));
                let r = if slix {
                    r.on_hover_text("Stop looking for tags and show the messages of the device in the debug window")
                } else {
                    r.on_disabled_hover_text(NEEDS_FIRMWARE)
                };
                if r.changed() {
                    if let Some(reader) = &self.nfc.reader {
                        reader.set_console(self.nfc.console);
                    }
                    if self.nfc.console && !self.settings.debug_window {
                        self.settings.debug_window = true;
                        self.save_settings();
                    }
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            ui.label("Sort by");
            egui::ComboBox::from_id_salt("sort").selected_text(self.sort.label()).show_ui(ui, |ui| {
                for s in SortBy::ALL {
                    if ui.selectable_value(&mut self.sort, s, s.label()).changed() {
                        self.settings.sort_by = s.key().into();
                        self.save_settings();
                    }
                }
            });
            ui.separator();

            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id(Id::unique(SEARCH_ID))
                    .desired_width(180.0)
                    .hint_text("Search"),
            );
            let search = search.on_hover_text("Name, UID, folder or audio ID. Start typing anywhere to search, Esc clears.");
            if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search.clear();
            }
            if !self.search.is_empty() && ui.small_button("✕").on_hover_text("Clear the search").clicked() {
                self.search.clear();
            }
            if search.changed() || self.search.is_empty() {
                self.deselect_hidden();
            }
            ui.separator();

            let busy = self.loading_rx.is_some();
            if self.playing.is_some() {
                if ui.button(if busy { "Loading..." } else { "Stop" }).clicked() {
                    actions.push(Action::Stop);
                }
            } else if ui.add_enabled(self.single_selected().is_some(), egui::Button::new("Play file")).clicked() {
                actions.push(Action::Play);
            }

            let (pos, len) = self
                .player
                .as_ref()
                .filter(|_| self.playing.is_some() && !busy)
                .map(|p| (p.position(), p.duration()))
                .unwrap_or_default();
            let mut value = self.seek_drag.unwrap_or(pos.as_secs_f64());
            let slider = ui.add_enabled(
                len > Duration::ZERO,
                egui::Slider::new(&mut value, 0.0..=len.as_secs_f64().max(1.0)).show_value(false),
            );
            if slider.dragged() {
                self.seek_drag = Some(value);
            }
            if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
                actions.push(Action::Seek(value));
                self.seek_drag = None;
            }
            ui.label(if len > Duration::ZERO {
                format!("{}/{}", format_time(Duration::from_secs_f64(value)), format_time(len))
            } else {
                "--:--:--/--:--:--".into()
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{} px", self.settings.icon_size));
                let r = ui.add(
                    egui::Slider::new(&mut self.settings.icon_size, MIN_ICON..=MAX_ICON)
                        .show_value(false)
                        .step_by(8.0),
                );
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    self.save_settings();
                }
                ui.label("Icons");
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let has_dir = self.current_dir.is_some();
        let any = !self.selection.is_empty();
        ui.add_space(4.0);
        let full = |ui: &mut egui::Ui, enabled: bool, text: &str| {
            ui.add_enabled(enabled, egui::Button::new(text).min_size(Vec2::new(ui.available_width(), 0.0)))
                .clicked()
        };
        if full(ui, has_dir, "Add...") {
            if let Some(files) = rfd::FileDialog::new()
                .set_title("Audio files for a new Tonie, or one Tonie file")
                .add_filter("Audio files", tonie_core::audio_in::SUPPORTED_EXTENSIONS)
                .add_filter("All files", &["*"])
                .pick_files()
            {
                actions.push(Action::Add(files));
            }
        }
        if full(ui, any, "Delete...") {
            actions.push(Action::Delete);
        }
        let any_live = self.entries.iter().any(|e| e.file.live);
        let remove_live = ui
            .add_enabled(
                any_live,
                egui::Button::new("Remove all live flags...").min_size(Vec2::new(ui.available_width(), 0.0)),
            )
            .on_hover_text(format!("Clear the live flag of every Tonie that has it ({})", shortcut_text("L")));
        if remove_live.clicked() {
            actions.push(Action::RemoveAllLive);
        }
        if full(ui, has_dir, "Deleted Tonies (TRASHCAN)...") {
            actions.push(Action::Trashcan);
        }
        ui.separator();
        if full(ui, any, "Save as .ogg...") {
            actions.push(Action::Export(ExportFormat::Ogg));
        }
        if full(ui, any, "Save as .mp3...") {
            actions.push(Action::Export(ExportFormat::Mp3));
        }
        if full(ui, any, "Copy to MicroSD...") {
            actions.push(Action::CopyToSd);
        }
        ui.separator();

        if let Some(e) = self.single_selected() {
            details(ui, e);
        } else if any {
            ui.label(format!("{} Tonies selected", self.selection.len()));
        }
    }

    fn grid(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let order = self.sorted_indices();
        let icon = self.settings.icon_size as f32;
        let tile = Vec2::new((icon + 24.0).max(110.0), icon + 16.0 + LABEL_LINES as f32 * 15.0);
        let spacing = 6.0;
        let width = ui.available_width();
        let cols = (((width + spacing) / (tile.x + spacing)).floor() as usize).max(1);

        /* rows of tiles, with a header per type when sorting by type */
        enum Row {
            Header(&'static str),
            Tiles(Vec<usize>),
        }
        let mut rows = Vec::new();
        let groups: Vec<(Option<&'static str>, Vec<usize>)> = if self.sort == SortBy::Type {
            [(Kind::Unknown, "Unknown"), (Kind::Custom, "Custom"), (Kind::Retail, "Retail")]
                .into_iter()
                .map(|(k, name)| (Some(name), order.iter().copied().filter(|&i| self.entries[i].kind() == k).collect::<Vec<_>>()))
                .filter(|(_, v)| !v.is_empty())
                .collect()
        } else {
            vec![(None, order.clone())]
        };
        for (header, items) in groups {
            if let Some(h) = header {
                rows.push(Row::Header(h));
            }
            rows.extend(items.chunks(cols).map(|c| Row::Tiles(c.to_vec())));
        }
        let row_height = |r: &Row| if matches!(r, Row::Header(_)) { HEADER_HEIGHT } else { tile.y + spacing };
        let total: f32 = rows.iter().map(row_height).sum();

        /* position of every tile relative to the content, for the selection frame */
        let mut tile_rects: Vec<(usize, Rect)> = Vec::new();
        let mut y = 0.0;
        for row in &rows {
            if let Row::Tiles(items) = row {
                for (col, &i) in items.iter().enumerate() {
                    tile_rects.push((i, Rect::from_min_size(egui::pos2(col as f32 * (tile.x + spacing), y), tile)));
                }
            }
            y += row_height(row);
        }

        let mut clicked: Option<(PathBuf, egui::Modifiers)> = None;
        let scroll_to = self
            .scroll_to
            .take()
            .and_then(|p| tile_rects.iter().find(|(i, _)| self.entries[*i].file.path == p).map(|(_, r)| *r));
        /* dragging the content only scrolls on touch screens, so mouse drags draw the selection frame */
        egui::ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, viewport| {
            ui.set_height(total.max(viewport.height()));
            ui.set_width(width);
            let origin = ui.min_rect().min;
            if let Some(rect) = scroll_to {
                ui.scroll_to_rect(rect.translate(origin.to_vec2()), None);
            }

            /* the background below the tiles: dragging draws a selection frame, a click clears the selection */
            let background = ui.interact(
                Rect::from_min_size(origin, Vec2::new(width, total.max(viewport.height()))),
                Id::unique("grid_background"),
                Sense::click_and_drag(),
            );
            let frame = self.selection_frame(ui, &background, origin, &tile_rects);

            let mut y = 0.0;
            for row in &rows {
                let h = row_height(row);
                if y + h >= viewport.min.y && y <= viewport.max.y {
                    match row {
                        Row::Header(text) => {
                            let rect = Rect::from_min_size(origin + Vec2::new(0.0, y), Vec2::new(width, HEADER_HEIGHT));
                            ui.painter().text(
                                rect.left_center() + Vec2::new(4.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                *text,
                                egui::FontId::proportional(16.0),
                                ui.visuals().strong_text_color(),
                            );
                        }
                        Row::Tiles(items) => {
                            for (col, &i) in items.iter().enumerate() {
                                let rect = Rect::from_min_size(origin + Vec2::new(col as f32 * (tile.x + spacing), y), tile);
                                if let Some(m) = self.tile(ui, i, rect, icon, actions) {
                                    clicked = Some((self.entries[i].file.path.clone(), m));
                                }
                            }
                        }
                    }
                }
                y += h;
            }

            /* drawn last, so the frame lies on top of the tiles */
            if let Some(area) = frame {
                let visuals = ui.visuals();
                ui.painter().rect_filled(area, 2.0, visuals.selection.bg_fill.gamma_multiply(0.25));
                ui.painter().rect_stroke(area, 2.0, visuals.selection.stroke, egui::StrokeKind::Inside);
            }
        });

        if let Some((path, modifiers)) = clicked {
            self.click(&path, modifiers, &order);
        }
    }

    /// Rubber band selection: everything the frame touches is selected, with Ctrl/Cmd in addition
    /// to the previous selection. Scrolls when dragging at the top or bottom edge.
    /// Returns the frame to draw, in screen coordinates.
    fn selection_frame(&mut self, ui: &mut egui::Ui, background: &egui::Response, origin: egui::Pos2, tiles: &[(usize, Rect)]) -> Option<Rect> {
        let modifiers = ui.input(|i| i.modifiers);
        let pointer = ui.input(|i| i.pointer.interact_pos());

        if background.clicked() && !modifiers.command && !modifiers.shift {
            self.selection.clear();
            self.anchor = None;
        }
        if background.drag_started() {
            /* the drag is recognized after a few pixels; start the frame where the button went down */
            if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(pointer) {
                let base = if modifiers.command { self.selection.clone() } else { BTreeSet::new() };
                self.frame_select = Some(FrameSelect {
                    start: p - origin.to_vec2(),
                    base,
                });
            }
        }
        if background.drag_stopped() {
            self.frame_select = None;
        }

        let (Some(frame), Some(p)) = (&self.frame_select, pointer) else { return None };
        if !background.dragged() {
            return None;
        }
        let current = p - origin.to_vec2();
        let area = Rect::from_two_pos(frame.start, current);
        let mut selection = frame.base.clone();
        for (i, rect) in tiles {
            if rect.intersects(area) {
                selection.insert(self.entries[*i].file.path.clone());
            }
        }
        self.selection = selection;

        /* keep scrolling while the pointer is at the edge of the visible area */
        let clip = ui.clip_rect();
        let edge = 24.0;
        let speed = if p.y < clip.top() + edge {
            (clip.top() + edge - p.y).min(edge)
        } else if p.y > clip.bottom() - edge {
            -(p.y - (clip.bottom() - edge)).min(edge)
        } else {
            0.0
        };
        if speed != 0.0 {
            ui.scroll_with_delta(Vec2::new(0.0, speed));
            ui.ctx().request_repaint();
        }
        Some(area.translate(origin.to_vec2()))
    }

    /// Draws one tile. Returns the modifiers if it was clicked.
    fn tile(&mut self, ui: &mut egui::Ui, i: usize, rect: Rect, icon: f32, actions: &mut Vec<Action>) -> Option<egui::Modifiers> {
        let entry = &self.entries[i];
        let path = entry.file.path.clone();
        let selected = self.selection.contains(&path);
        let response = ui.interact(rect, Id::unique(("tile", &path)), Sense::click());
        let visuals = ui.visuals();

        if selected {
            ui.painter().rect_filled(rect, 4.0, visuals.selection.bg_fill);
        } else if response.hovered() {
            ui.painter().rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }

        let img_rect = Rect::from_min_size(rect.min + Vec2::new((rect.width() - icon) / 2.0, 6.0), Vec2::splat(icon));
        let key = entry.image_key();
        if let Some(tex) = self.textures.get(&key).or_else(|| self.textures.get("unknown")) {
            ui.painter().image(
                tex.id(),
                img_rect,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        let text_color = if selected { visuals.selection.stroke.color } else { visuals.text_color() };
        let mut job = egui::text::LayoutJob::simple(entry.name(), egui::FontId::proportional(12.0), text_color, rect.width() - 8.0);
        job.wrap.max_rows = LABEL_LINES;
        job.halign = egui::Align::Center;
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(egui::pos2(rect.center().x, img_rect.max.y + 4.0), galley, text_color);

        let response = response.on_hover_ui(|ui| tooltip(ui, entry));
        let mut result = None;
        if response.clicked() {
            result = Some(ui.input(|i| i.modifiers));
        }
        if response.double_clicked() {
            self.selection = BTreeSet::from([path.clone()]);
            actions.push(Action::AssignUid);
        }
        if response.secondary_clicked() && !selected {
            self.selection = BTreeSet::from([path.clone()]);
            self.anchor = Some(path);
        }
        response.context_menu(|ui| {
            let single = self.selection.len() == 1;
            let mut item = |ui: &mut egui::Ui, enabled: bool, text: &str, action: fn() -> Action| {
                if ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
                    actions.push(action());
                    ui.close();
                }
            };
            item(ui, single, "Play", || Action::Play);
            item(ui, true, "Toggle live flag", || Action::ToggleLive);
            item(ui, single, "Rename", || Action::Rename);
            item(ui, true, "Delete", || Action::Delete);
            ui.separator();
            item(ui, single, "Assign new UID", || Action::AssignUid);
            item(ui, single, "Assign random UID", || Action::RandomUid);
            item(ui, single, "Show in file manager", || Action::ShowInFolder);
            ui.separator();
            item(ui, true, "Export to .ogg", || Action::Export(ExportFormat::Ogg));
            item(ui, true, "Export to .mp3", || Action::Export(ExportFormat::Mp3));
            item(ui, true, "Export Tonie file...", || Action::ExportTonieFile);
            item(ui, true, "Copy to MicroSD", || Action::CopyToSd);
        });
        result
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(d) = &self.nfc.device {
                let mut fw = String::new();
                if d.kind == ReaderKind::Proxmark3 {
                    fw = format!(" (FW: {}", if d.unlock_supported { "SLIX-L enabled" } else { "stock" });
                    if let (true, Some(v)) = (self.settings.debug_window, d.antenna_voltage) {
                        fw += &format!(", {v:.2} V");
                    }
                    fw += ")";
                }
                ui.label(format!("{}{fw} found at {}.", d.hardware, d.port))
                    .on_hover_text("The UID of the tag will be automatically used where applicable.");
                ui.separator();
            }
            let mut text = if self.db.tonies.is_empty() {
                "No tonies.json loaded, consider downloading it".to_string()
            } else {
                format!("Have {} entries in tonies.json", self.db.tonies.len())
            };
            if !self.json_status.is_empty() {
                text += &format!(" | {}", self.json_status);
            }
            ui.label(text);
            if let Some(dir) = &self.current_dir {
                ui.separator();
                let shown = if self.search.trim().is_empty() {
                    self.entries.len().to_string()
                } else {
                    format!("{} of {}", self.entries.iter().filter(|e| self.matches_search(e)).count(), self.entries.len())
                };
                ui.label(format!("{shown} Tonies in {}", dir.display()));
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let keep;

        match &mut dialog {
            Dialog::Message { title, text } => {
                let r = egui::Modal::new(Id::unique("message")).show(ctx, |ui| {
                    ui.set_max_width(500.0);
                    ui.heading(title.as_str());
                    ui.label(text.as_str());
                    ui.add_space(8.0);
                    ui.button("OK").clicked()
                });
                keep = !(r.inner || r.should_close());
            }
            Dialog::About => {
                let r = egui::Modal::new(Id::unique("about")).show(ctx, |ui| {
                    ui.set_max_width(480.0);
                    ui.heading(format!("TeddyBench {VERSION}"));
                    ui.label("Manage the audio files on a Toniebox SD card.");
                    ui.hyperlink_to("github.com/StefanRiese/teddy", "https://github.com/StefanRiese/teddy");
                    ui.horizontal(|ui| {
                        ui.label("New versions:");
                        ui.hyperlink_to("Releases page", RELEASES_URL);
                    });
                    ui.add_space(8.0);
                    ui.label(
                        "This tool is in no way endorsed by the creator of the Tonie Boxes. Its purpose is solely meant for understanding the file \
                         exchange format and only to be used for interoperability for personal use.",
                    );
                    ui.add_space(8.0);
                    ui.button("Close").clicked()
                });
                keep = !(r.inner || r.should_close());
            }
            Dialog::TagLayout(d) => {
                let r = egui::Modal::new(Id::unique("tag_layout")).show(ctx, |ui| {
                    ui.heading("Print layout for custom tags");
                    ui.label(egui::RichText::new("A4 page, all values in millimeters").weak());
                    egui::Grid::new("tag_layout_grid").num_columns(2).show(ui, |ui| {
                        let field = |ui: &mut egui::Ui, label: &str, v: &mut i32, range: std::ops::RangeInclusive<i32>| {
                            ui.label(label);
                            ui.add(egui::DragValue::new(v).range(range));
                            ui.end_row();
                        };
                        field(ui, "Tag diameter", &mut d.size, 10..=100);
                        field(ui, "Non-printable margin", &mut d.margin, 0..=50);
                        field(ui, "Gap between columns", &mut d.gap_x, 0..=50);
                        field(ui, "Gap between rows", &mut d.gap_y, 0..=50);
                        field(ui, "Start in column (0 = first)", &mut d.initial_column, 0..=20);
                        field(ui, "Start in row (0 = first)", &mut d.initial_row, 0..=30);
                        ui.label("Cutting outline width (0 = none)");
                        ui.add(egui::DragValue::new(&mut d.outline_width).range(0.0..=3.0).speed(0.05).fixed_decimals(2));
                        ui.end_row();
                    });
                    ui.label(egui::RichText::new("Start column and row let you continue on a partly used sheet.").weak());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| (ui.button("OK").clicked(), ui.button("Cancel").clicked())).inner
                });
                let (ok, cancel) = r.inner;
                if ok {
                    let s = &mut self.settings;
                    s.custom_tag_non_printable_margin = d.margin;
                    s.custom_tag_gap_x = d.gap_x;
                    s.custom_tag_gap_y = d.gap_y;
                    s.custom_tag_initial_column = d.initial_column;
                    s.custom_tag_initial_row = d.initial_row;
                    s.custom_tag_size = d.size;
                    s.custom_tag_outline_width = d.outline_width;
                    self.save_settings();
                }
                keep = !(ok || cancel || r.should_close());
            }
            Dialog::TagDump { text, input, error } => {
                let r = egui::Modal::new(Id::unique("tag_dump")).show(ctx, |ui| {
                    ui.set_width(480.0);
                    ui.heading(if *input { "Emulate tag" } else { "Tag content" });
                    ui.label(if *input {
                        "Enter the UID and memory content of your tag: 40 bytes in hex, 8 bytes UID and 32 bytes data."
                    } else {
                        "This is the UID and memory content of your tag"
                    });
                    let edit = egui::TextEdit::multiline(text)
                        .font(egui::TextStyle::Monospace)
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .interactive(*input);
                    ui.add(edit);
                    if let Some(e) = error {
                        ui.colored_label(Color32::from_rgb(190, 60, 80), e.as_str());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if *input {
                            (ui.button("Emulate").clicked(), ui.button("Cancel").clicked())
                        } else {
                            if ui.button("Copy").clicked() {
                                ui.ctx().copy_text(text.clone());
                            }
                            (false, ui.button("Close").clicked())
                        }
                    })
                    .inner
                });
                let (ok, cancel) = r.inner;
                keep = !(cancel || r.should_close());
                if ok {
                    match parse_tag_dump(text) {
                        Ok(data) => {
                            self.nfc.last_dump = text.trim().to_string();
                            if let Some(reader) = &self.nfc.reader {
                                reader.emulate(data);
                                self.nfc.operation = Some(TagOperation::Emulate);
                            }
                            return;
                        }
                        Err(e) => *error = Some(e),
                    }
                }
            }
            Dialog::Antenna(m) => {
                let r = egui::Modal::new(Id::unique("antenna")).show(ctx, |ui| {
                    ui.heading("Proxmark3 antenna");
                    egui::Grid::new("antenna_values").num_columns(2).show(ui, |ui| {
                        let mut row = |k: &str, v: egui::RichText| {
                            ui.label(k);
                            ui.label(v);
                            ui.end_row();
                        };
                        let rating = m.hf_rating();
                        let color = match rating {
                            "good" | "wow" => Color32::from_rgb(40, 140, 60),
                            "weak" | "meh" | "danger" => Color32::from_rgb(200, 140, 30),
                            _ => Color32::from_rgb(190, 60, 80),
                        };
                        row("HF", egui::RichText::new(format!("{:.2} V ({rating})", m.v_hf)).color(color).strong());
                        row("LF 125 kHz", format!("{:.2} V", m.v_lf125).into());
                        row("LF 134 kHz", format!("{:.2} V", m.v_lf134).into());
                        row("LF optimal frequency", format!("{:.2} kHz", m.peak_frequency_khz()).into());
                        row("LF optimal voltage", format!("{:.2} V", m.peak_v).into());
                    });
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("LF antenna plot (no relevance for Tonies)").weak());
                    antenna_plot(ui, m);
                    ui.add_space(8.0);
                    ui.button("Close").clicked()
                });
                keep = !(r.inner || r.should_close());
            }
            Dialog::Rename { hash, name } => {
                let r = egui::Modal::new(Id::unique("rename")).show(ctx, |ui| {
                    ui.heading("Rename");
                    let edit = ui.text_edit_singleline(name);
                    edit.request_focus();
                    ui.horizontal(|ui| {
                        (
                            ui.button("OK").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)),
                            ui.button("Cancel").clicked(),
                        )
                    })
                    .inner
                });
                let (ok, cancel) = r.inner;
                if ok {
                    self.db.custom_names.insert(hash.clone(), name.trim().to_string());
                    self.db.save_custom_names(&self.data_dir.custom_tonies_json()).ok();
                    self.resolve_all();
                }
                keep = !(ok || cancel || r.should_close());
            }
            Dialog::Trashcan(t) => {
                keep = self.trashcan_ui(ctx, t);
            }
            Dialog::ConfirmRemoveLive(files) => {
                let names: Vec<String> = files
                    .iter()
                    .map(|f| {
                        self.entries
                            .iter()
                            .find(|e| &e.file.path == f)
                            .map(|e| e.name().trim_start_matches("[live] ").to_string())
                            .unwrap_or_else(|| f.display().to_string())
                    })
                    .collect();
                let r = egui::Modal::new(Id::unique("remove_live")).show(ctx, |ui| {
                    ui.set_max_width(500.0);
                    ui.heading(format!("Remove the live flag from {} Tonie(s)?", files.len()));
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for n in &names {
                            ui.label(n.replace('\n', " "));
                        }
                    });
                    ui.horizontal(|ui| (ui.button("Remove").clicked(), ui.button("Cancel").clicked())).inner
                });
                let (ok, cancel) = r.inner;
                if ok {
                    let errors: Vec<String> = files
                        .iter()
                        .filter_map(|f| fsattr::set_live(f, false).err().map(|e| format!("{}: {e}", f.display())))
                        .collect();
                    self.scanner.send(Command::Rescan);
                    if !errors.is_empty() {
                        self.message("Remove all live flags", errors.join("\n"));
                        return;
                    }
                }
                keep = !(ok || cancel || r.should_close());
            }
            Dialog::ConfirmDelete(files) => {
                let names: Vec<String> = files
                    .iter()
                    .map(|f| {
                        self.entries
                            .iter()
                            .find(|e| &e.file.path == f)
                            .map(|e| e.name())
                            .unwrap_or_else(|| f.display().to_string())
                    })
                    .collect();
                let r = egui::Modal::new(Id::unique("delete")).show(ctx, |ui| {
                    ui.heading(if files.len() == 1 { "Delete file?" } else { "Delete files?" });
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for n in &names {
                            ui.label(n.replace('\n', " "));
                        }
                    });
                    ui.horizontal(|ui| (ui.button("Delete").clicked(), ui.button("Cancel").clicked())).inner
                });
                let (ok, cancel) = r.inner;
                if ok {
                    self.stop_playback();
                    let dir = self.current_dir.clone().unwrap_or_default();
                    let errors: Vec<String> = files.iter().filter_map(|f| actions::delete_tonie(f, &dir).err()).collect();
                    self.selection.clear();
                    self.scanner.send(Command::Rescan);
                    if !errors.is_empty() {
                        self.dialog = Some(Dialog::Message {
                            title: "Delete".into(),
                            text: errors.join("\n"),
                        });
                        return;
                    }
                }
                keep = !(ok || cancel || r.should_close());
            }
            Dialog::AssignUid { path, uid } => {
                let r = egui::Modal::new(Id::unique("assign")).show(ctx, |ui| {
                    ui.heading("Assign new UID");
                    let ok = uid_field(ui, uid);
                    ui.horizontal(|ui| (ui.add_enabled(ok, egui::Button::new("OK")).clicked(), ui.button("Cancel").clicked()))
                        .inner
                });
                let (ok, cancel) = r.inner;
                if ok {
                    let (path, uid) = (path.clone(), uid.to_ascii_uppercase());
                    if let Some(dir) = self.current_dir.clone() {
                        self.assign_uid(&path, &uid, &dir);
                    }
                    if self.dialog.is_some() {
                        return;
                    }
                }
                keep = !(ok || cancel || r.should_close());
            }
            Dialog::NewTonie(nt) => {
                let r = egui::Modal::new(Id::unique("new_tonie")).show(ctx, |ui| new_tonie_ui(ui, nt));
                match r.inner {
                    Some(true) => {
                        keep = false;
                        self.run_new_tonie(std::mem::replace(
                            nt,
                            NewTonie {
                                files: Vec::new(),
                                descriptions: Vec::new(),
                                selected: None,
                                uid: String::new(),
                                custom_audio_id: false,
                                audio_id: String::new(),
                                copy_tonie_file: false,
                                name: String::new(),
                            },
                        ));
                        if self.dialog.is_some() {
                            return;
                        }
                    }
                    Some(false) => keep = false,
                    None => keep = !r.should_close(),
                }
            }
            Dialog::Export(ed) => {
                let r = egui::Modal::new(Id::unique("export")).show(ctx, |ui| export_ui(ui, ed));
                match r.inner {
                    Some(ExportChoice::Cancel) => keep = false,
                    Some(choice) => {
                        keep = false;
                        let covers_only = matches!(choice, ExportChoice::CoversOnly);
                        let out_dir = PathBuf::from(&ed.out_dir);
                        self.settings
                            .extra
                            .insert("ExportDirectory".into(), serde_json::Value::String(ed.out_dir.clone()));
                        self.save_settings();
                        if let Err(e) = std::fs::create_dir_all(&out_dir) {
                            self.message("Export", format!("Cannot create '{}': {e}", out_dir.display()));
                            return;
                        }
                        let items = std::mem::take(&mut ed.items);
                        let data_dir = self.data_dir.clone();
                        let create_dirs = ed.create_dirs;
                        if covers_only {
                            let job = self.start_job("Updating covers", items.len());
                            std::thread::spawn(move || export::update_covers(&items, &out_dir, create_dirs, &data_dir, &job));
                        } else {
                            let format = ed.format;
                            let title = if format == ExportFormat::Ogg {
                                "Exporting to .ogg"
                            } else {
                                "Exporting to .mp3"
                            };
                            let job = self.start_job(title, items.len());
                            let options = ExportOptions {
                                format,
                                out_dir,
                                create_dirs,
                                version: VERSION.into(),
                            };
                            std::thread::spawn(move || export::export(&items, &options, &data_dir, &job));
                        }
                    }
                    None => keep = !r.should_close(),
                }
            }
        }

        if keep {
            self.dialog = Some(dialog);
        }
    }

    fn run_new_tonie(&mut self, nt: NewTonie) {
        let Some(dir) = self.current_dir.clone() else { return };
        let uid = nt.uid.to_ascii_uppercase();
        if !nt.name.trim().is_empty() {
            self.pending_names.insert(uid::file_for_uid(&dir, &uid), nt.name.trim().to_string());
        }
        if nt.copy_tonie_file {
            match actions::copy_tonie(&nt.files[0], &uid, &dir) {
                Ok(_) => self.scanner.send(Command::Rescan),
                Err(e) => self.message("Add file...", e),
            }
            return;
        }
        let audio_id = if nt.custom_audio_id {
            u32::from_str_radix(nt.audio_id.trim().trim_start_matches("0x"), 16).unwrap_or(0)
        } else {
            self.new_custom_audio_id()
        };
        let job = self.start_job("Encoding", nt.files.len());
        std::thread::spawn(move || {
            match actions::encode_tonie(&nt.files, &uid, audio_id, &dir, &job) {
                Ok(path) => job.log(format!("Written content to {}", path.display())),
                Err(e) => job.log(e),
            }
            job.finish();
        });
    }

    fn create_tag_pdf(&mut self, side: TagSide) {
        /* only Tonies from tonies.json have a cover, series and model number */
        let tonies: Vec<(TonieData, String)> = self
            .selected_entries()
            .iter()
            .filter_map(|e| Some((e.info()?.clone(), e.analysis()?.hash.clone())))
            .collect();
        let skipped = self.selection.len() - tonies.len();
        if tonies.is_empty() {
            self.message(
                "Custom tag",
                "None of the selected Tonies is in tonies.json, so there is no cover, series or model number to print.",
            );
            return;
        }
        let Some(dir) = rfd::FileDialog::new().set_title("Save the PDF in").pick_folder() else {
            return;
        };
        let name = match side {
            TagSide::Cover => &self.settings.custom_tag_cover_filename,
            TagSide::Backside => &self.settings.custom_tag_backside_filename,
        };
        let out_file = dir.join(name);
        let layout = TagLayout::from_settings(&self.settings);
        let data_dir = self.data_dir.clone();
        let title = if side == TagSide::Cover {
            "Creating tag covers"
        } else {
            "Creating tag backsides"
        };
        let job = self.start_job(title, tonies.len());
        if skipped > 0 {
            job.log(format!("Skipped {skipped} Tonie(s) that are not in tonies.json"));
        }
        std::thread::spawn(move || {
            match tagpdf::create(side, &tonies, &layout, &data_dir, &out_file, &job) {
                Ok(()) => {
                    if let Err(e) = actions::open_with_default_program(&out_file) {
                        job.log(format!("Cannot open the PDF viewer: {e}"));
                    }
                }
                Err(e) => job.log(e),
            }
            job.finish();
        });
    }

    fn log_window(&mut self, ctx: &egui::Context) {
        if !self.settings.debug_window {
            return;
        }
        let mut open = true;
        let verbose_id = Id::unique("log_verbose");
        let mut verbose = ctx.data(|d| d.get_temp::<bool>(verbose_id).unwrap_or(false));
        egui::Window::new("Debug window")
            .open(&mut open)
            .default_pos([60.0, 120.0])
            .default_size([700.0, 320.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut verbose, "Show debug messages");
                    if ui.button("Copy all").clicked() {
                        let text: String = applog::lines(applog::Level::Debug)
                            .iter()
                            .map(|l| format!("{} {:5} {}\n", applog::format_time(l.time), l.level.label(), l.text))
                            .collect();
                        ui.ctx().copy_text(text);
                    }
                    if ui.button("Clear").clicked() {
                        applog::clear();
                    }
                });
                ui.separator();
                let lines = applog::lines(if verbose { applog::Level::Debug } else { applog::Level::Info });
                let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .auto_shrink(false)
                    .show_rows(ui, row_height, lines.len(), |ui, range| {
                        for l in &lines[range] {
                            let color = match l.level {
                                applog::Level::Error => Color32::from_rgb(220, 80, 80),
                                applog::Level::Warning => Color32::from_rgb(220, 170, 60),
                                applog::Level::Debug => ui.visuals().weak_text_color(),
                                applog::Level::Info => ui.visuals().text_color(),
                            };
                            ui.label(
                                egui::RichText::new(format!("{} {:5} {}", applog::format_time(l.time), l.level.label(), l.text))
                                    .monospace()
                                    .color(color),
                            );
                        }
                    });
            });
        ctx.data_mut(|d| d.insert_temp(verbose_id, verbose));
        if !open {
            self.settings.debug_window = false;
            self.save_settings();
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }

    fn job_window(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let text = job.status.text();
        let finished = job.status.finished_items.load(Ordering::Relaxed);
        let total = job.total.max(1);
        let mut close = false;

        egui::Modal::new(Id::unique("job")).show(ctx, |ui| {
            ui.set_width(520.0);
            ui.heading(&job.title);
            ui.label(&text.current);
            ui.label(egui::RichText::new(&text.detail).weak());
            ui.add(egui::ProgressBar::new(finished as f32 / total as f32).text(format!("{finished} / {}", job.total)));
            egui::ScrollArea::vertical().max_height(200.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &text.log {
                    ui.monospace(line);
                }
            });
            ui.add_space(6.0);
            if text.done {
                ui.label(if job.status.cancelled() { "Cancelled." } else { "Finished." });
                close = ui.button("Close").clicked();
            } else if job.status.cancelled() {
                ui.label("Cancelling...");
            } else if ui.button("Cancel").clicked() {
                job.status.cancel.store(true, Ordering::Relaxed);
            }
        });

        if close {
            self.job = None;
            self.scanner.send(Command::Rescan);
        }
    }

    /// The TRASHCAN window. Returns whether it stays open.
    fn trashcan_ui(&mut self, ctx: &egui::Context, t: &mut TrashView) -> bool {
        if let Some(items) = t.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            t.items = Some(items);
            t.rx = None;
        }
        let mut restore = false;
        let mut delete = false;
        let r = egui::Modal::new(Id::unique("trashcan")).show(ctx, |ui| {
            ui.set_width(760.0);
            ui.heading("Deleted Tonies (TRASHCAN)");
            ui.label(
                egui::RichText::new(
                    "A Toniebox connected to the cloud moves deleted Tonies into the TRASHCAN folder of the card. \
                     Their file names don't tell the UID of the tag: enter it, or put the tag on the NFC reader.",
                )
                .weak(),
            );
            ui.add_space(6.0);
            let Some(items) = &t.items else {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Reading the TRASHCAN...");
                });
                return ui.button("Close").clicked();
            };
            if items.is_empty() {
                ui.label("The TRASHCAN is empty.");
            }
            egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                egui::Grid::new("trash_items").striped(true).num_columns(5).show(ui, |ui| {
                    for h in ["Name", "Audio ID", "Length", "Deleted", "File"] {
                        ui.label(egui::RichText::new(h).strong());
                    }
                    ui.end_row();
                    for (i, item) in items.iter().enumerate() {
                        let file = item
                            .path
                            .strip_prefix(item.path.parent().and_then(Path::parent).unwrap_or(Path::new("")))
                            .unwrap_or(&item.path)
                            .display()
                            .to_string();
                        let (name, id, length) = match &item.analysis {
                            Ok(a) => {
                                let resolved = self.db.resolve("", a);
                                let mut name = match resolved.kind {
                                    Kind::Unknown => "Unknown Tonie".to_string(),
                                    _ if resolved.name.starts_with("Unnamed Teddy") => "Unnamed Teddy".to_string(),
                                    _ => resolved.name.replace('\n', " "),
                                };
                                if let Some(e) = self.entries.iter().find(|e| e.analysis().is_some_and(|b| b.hash == a.hash)) {
                                    name += &format!(" (on the card as {})", e.file.uid);
                                }
                                (name, format!("0x{:08X}", a.audio_id), format_time(Duration::from_secs_f64(a.duration_secs())))
                            }
                            Err(e) => (format!("Not readable: {e}"), String::new(), String::new()),
                        };
                        if ui.selectable_label(t.selected == Some(i), name).clicked() {
                            t.selected = Some(i);
                            t.confirm_delete = false;
                        }
                        ui.label(id);
                        ui.label(length);
                        ui.label(format_date(item.deleted));
                        ui.label(egui::RichText::new(file).weak());
                        ui.end_row();
                    }
                });
            });

            ui.separator();
            let selected = t.selected.and_then(|i| items.get(i));
            let readable = selected.is_some_and(|i| i.analysis.is_ok());
            ui.add_enabled_ui(readable, |ui| {
                let uid_ok = uid_field(ui, &mut t.uid);
                ui.checkbox(&mut t.as_new, "Restore as a new Tonie (new audio ID)").on_hover_text(
                    "Gives the file a new audio ID, as for a newly created Tonie. The audio is not re-encoded. \
                     Without this, the file is restored byte for byte.",
                );
                ui.horizontal(|ui| {
                    restore = ui.add_enabled(uid_ok, egui::Button::new("Restore")).clicked();
                    if t.confirm_delete {
                        ui.label("Delete it for good?");
                        delete = ui.button("Yes, delete").clicked();
                        if ui.button("No").clicked() {
                            t.confirm_delete = false;
                        }
                    } else if ui.button("Delete permanently...").clicked() {
                        t.confirm_delete = true;
                    }
                });
            });
            if let Some((text, failed)) = &t.status {
                let color = if *failed {
                    Color32::from_rgb(190, 60, 80)
                } else {
                    Color32::from_rgb(40, 140, 60)
                };
                ui.colored_label(color, text.as_str());
            }
            ui.add_space(6.0);
            ui.button("Close").clicked()
        });

        let selected = t.selected.and_then(|i| t.items.as_ref()?.get(i).cloned());
        if let (true, Some(item), Some(dir)) = (restore, &selected, self.current_dir.clone()) {
            let uid = t.uid.to_ascii_uppercase();
            let new_id = if t.as_new { Some(self.new_custom_audio_id()) } else { None };
            t.status = Some(match trashcan::restore(&item.path, &uid, &dir, new_id) {
                Ok(_) => {
                    /* a new audio ID changes the hash: keep the name */
                    if let (Some(id), Ok(a)) = (new_id, &item.analysis) {
                        let name = self.db.resolve(&uid, a);
                        if name.kind != Kind::Unknown && !name.name.starts_with("Unnamed Teddy") {
                            if let Some(hash) = Analysis::read(&uid::file_for_uid(&dir, &uid)).ok().map(|n| n.hash) {
                                self.db.custom_names.insert(hash, name.name);
                                self.db.save_custom_names(&self.data_dir.custom_tonies_json()).ok();
                            }
                        }
                        applog::info(format!("Restored {} as {uid} with audio ID 0x{id:08X}", item.path.display()));
                    }
                    self.scanner.send(Command::Rescan);
                    self.remove_trash_item(t);
                    (format!("Restored as the Tonie for {uid}."), false)
                }
                Err(e) => (e, true),
            });
        }
        if let (true, Some(item)) = (delete, &selected) {
            t.status = Some(match trashcan::delete(&item.path) {
                Ok(()) => {
                    self.remove_trash_item(t);
                    (format!("Deleted {}.", item.path.display()), false)
                }
                Err(e) => (e, true),
            });
        }
        !(r.inner || r.should_close())
    }

    /// Audio ID for a new Tonie, different from all on the card and those given before
    fn new_custom_audio_id(&mut self) -> u32 {
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let taken: HashSet<u32> = self.entries.iter().filter_map(|e| e.analysis().map(|a| a.audio_id)).collect();
        let id = actions::custom_audio_id(now, self.last_audio_id, &taken);
        self.last_audio_id = Some(id);
        id
    }

    fn remove_trash_item(&self, t: &mut TrashView) {
        if let (Some(items), Some(i)) = (t.items.as_mut(), t.selected.take()) {
            items.remove(i);
        }
        t.confirm_delete = false;
    }

    fn tag_operation_window(&mut self, ctx: &egui::Context) {
        let Some(op) = self.nfc.operation else { return };
        let mut cancel = false;
        egui::Modal::new(Id::unique("tag_operation")).show(ctx, |ui| {
            ui.set_width(360.0);
            ui.heading("Tag operation...");
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(match op {
                    TagOperation::Read => "Reading the tag. Please place it on the reader.",
                    TagOperation::Emulate => "Emulating the tag. Press the button on the Proxmark3 to stop.",
                    TagOperation::Measure => "Measuring the antenna, this takes a few seconds.",
                });
            });
            ui.add_space(8.0);
            cancel = ui.button("Cancel").clicked();
        });
        if cancel {
            if let Some(r) = &self.nfc.reader {
                r.cancel();
            }
            self.nfc.operation = None;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.dialog.is_some() || self.job.is_some() || self.nfc.operation.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(egui::Key::A) {
                actions.push(Action::SelectAll);
            }
            if i.key_pressed(egui::Key::F2) {
                actions.push(Action::Rename);
            }
            if i.key_pressed(egui::Key::Delete) {
                actions.push(Action::Delete);
            }
            if i.modifiers.command && i.key_pressed(egui::Key::L) {
                actions.push(Action::RemoveAllLive);
            }
        });

        /* typing anywhere starts the search, Esc clears it */
        if self.current_dir.is_some() && !ctx.memory(|m| m.focused().is_some()) {
            let typed: String = ctx.input(|i| {
                if i.modifiers.command || i.modifiers.alt {
                    return String::new();
                }
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect()
            });
            if !typed.trim().is_empty() {
                self.search.push_str(&typed);
                let id = Id::unique(SEARCH_ID);
                ctx.memory_mut(|m| m.request_focus(id));
                /* continue typing at the end instead of replacing the text */
                let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
                let end = egui::text::CCursor::new(self.search.chars().count());
                state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
                state.store(ctx, id);
                self.deselect_hidden();
            } else if !self.search.is_empty() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.search.clear();
            }
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() {
            actions.push(Action::Add(dropped));
        }
    }
}

/// How the shortcut Ctrl/Cmd + `key` is written on this platform
fn shortcut_text(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("Cmd+{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// Text field for a tag UID with validation. Returns whether the UID is valid.
fn uid_field(ui: &mut egui::Ui, uid: &mut String) -> bool {
    if !uid.to_ascii_uppercase().starts_with(UID_PREFIX) {
        *uid = UID_PREFIX.into();
    }
    let valid = is_valid_uid(uid);
    ui.horizontal(|ui| {
        ui.label("Tag UID:");
        let color = if valid {
            Color32::from_rgb(40, 140, 60)
        } else {
            Color32::from_rgb(190, 60, 80)
        };
        ui.add(
            egui::TextEdit::singleline(uid)
                .desired_width(170.0)
                .font(egui::TextStyle::Monospace)
                .text_color(color)
                .char_limit(16),
        );
        if ui.button("Random").on_hover_text("A UID that does not belong to a real tag").clicked() {
            *uid = random_uid();
        }
    });
    ui.label(egui::RichText::new("16 hex digits starting with E00403, as printed by NFC readers").weak());
    valid
}

/// Returns Some(true) to start, Some(false) to cancel.
fn new_tonie_ui(ui: &mut egui::Ui, nt: &mut NewTonie) -> Option<bool> {
    ui.set_width(640.0);
    ui.heading(if nt.copy_tonie_file { "Add Tonie file" } else { "New Tonie" });
    let uid_ok = uid_field(ui, &mut nt.uid);

    if nt.copy_tonie_file {
        ui.label(format!("Copy {}", nt.files[0].display()));
    } else {
        ui.horizontal(|ui| {
            ui.label("Name:");
            ui.add(egui::TextEdit::singleline(&mut nt.name).desired_width(400.0).hint_text("Unnamed Teddy"))
                .on_hover_text("From the tags of the files (album artist and album) or their folder; can be changed later with Rename");
        });
        ui.add_space(6.0);
        ui.label("Track order:");
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            egui::Grid::new("tracks").striped(true).num_columns(3).show(ui, |ui| {
                for (i, f) in nt.files.iter().enumerate() {
                    let name = f.file_name().unwrap_or_default().to_string_lossy();
                    if ui.selectable_label(nt.selected == Some(i), format!("{:>2}", i + 1)).clicked() {
                        nt.selected = Some(i);
                    }
                    if ui.selectable_label(nt.selected == Some(i), name.as_ref()).clicked() {
                        nt.selected = Some(i);
                    }
                    ui.label(&nt.descriptions[i]);
                    ui.end_row();
                }
            });
        });
        ui.horizontal(|ui| {
            let sel = nt.selected;
            if ui.add_enabled(sel.map(|s| s > 0).unwrap_or(false), egui::Button::new("Up")).clicked() {
                let s = sel.unwrap();
                nt.files.swap(s, s - 1);
                nt.descriptions.swap(s, s - 1);
                nt.selected = Some(s - 1);
            }
            if ui
                .add_enabled(sel.map(|s| s + 1 < nt.files.len()).unwrap_or(false), egui::Button::new("Down"))
                .clicked()
            {
                let s = sel.unwrap();
                nt.files.swap(s, s + 1);
                nt.descriptions.swap(s, s + 1);
                nt.selected = Some(s + 1);
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.checkbox(&mut nt.custom_audio_id, "Set a specific Audio-ID (hex)");
            ui.add_enabled(
                nt.custom_audio_id,
                egui::TextEdit::singleline(&mut nt.audio_id)
                    .desired_width(90.0)
                    .char_limit(8)
                    .font(egui::TextStyle::Monospace),
            );
        });
    }
    let id_ok = !nt.custom_audio_id || (!nt.audio_id.is_empty() && u32::from_str_radix(nt.audio_id.trim(), 16).is_ok());

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(uid_ok && id_ok, egui::Button::new(if nt.copy_tonie_file { "Copy" } else { "Encode" }))
            .clicked()
        {
            return Some(true);
        }
        if ui.button("Cancel").clicked() {
            return Some(false);
        }
        None
    })
    .inner
}

enum ExportChoice {
    Export,
    CoversOnly,
    Cancel,
}

fn export_ui(ui: &mut egui::Ui, ed: &mut ExportDialog) -> Option<ExportChoice> {
    ui.set_width(560.0);
    ui.heading(format!("Export {} Tonie(s)", ed.items.len()));
    ui.horizontal(|ui| {
        ui.radio_value(&mut ed.format, ExportFormat::Ogg, ".ogg");
        ui.radio_value(&mut ed.format, ExportFormat::Mp3, ".mp3 (with tags and cover)");
    });
    ui.checkbox(&mut ed.create_dirs, "Create a directory per Tonie");
    ui.horizontal(|ui| {
        ui.label("Folder:");
        ui.add(egui::TextEdit::singleline(&mut ed.out_dir).desired_width(380.0));
        if ui.button("Choose...").clicked() {
            if let Some(d) = rfd::FileDialog::new().set_directory(&ed.out_dir).pick_folder() {
                ed.out_dir = d.display().to_string();
            }
        }
    });
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Please do not share exported files: they could contain information which can be used to identify your tonie. \
             Also be aware that sharing these files is most likely illegal in your country.",
        )
        .weak(),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let ok = !ed.out_dir.trim().is_empty();
        if ui.add_enabled(ok, egui::Button::new("Export")).clicked() {
            return Some(ExportChoice::Export);
        }
        if ui
            .add_enabled(ok, egui::Button::new("Update covers only"))
            .on_hover_text("Download the covers again into folders that were already exported")
            .clicked()
        {
            return Some(ExportChoice::CoversOnly);
        }
        if ui.button("Cancel").clicked() {
            return Some(ExportChoice::Cancel);
        }
        None
    })
    .inner
}

/// The hex text of the tag dump dialog: UID and memory, 40 bytes
fn parse_tag_dump(text: &str) -> Result<Vec<u8>, String> {
    let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let error = || "The hex code must represent 40 byte in hexadecimal numbers: 8 byte UUID and 32 byte data.".to_string();
    if digits.len() != 80 || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(error());
    }
    (0..80)
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|_| error()))
        .collect()
}

/// The LF sweep of the antenna measurement, amplitude over frequency
fn antenna_plot(ui: &mut egui::Ui, m: &Measurement) {
    const F: (f64, f64) = (46.0, 600.0);
    const V: (f64, f64) = (0.0, 65.0);
    let (response, painter) = ui.allocate_painter(Vec2::new(480.0, 220.0), Sense::hover());
    let visuals = ui.visuals();
    let font = egui::FontId::proportional(10.0);
    /* room for the axis labels */
    let area = Rect::from_min_max(response.rect.min + Vec2::new(32.0, 4.0), response.rect.max - Vec2::new(8.0, 18.0));
    let pos = |f: f64, v: f64| {
        egui::pos2(
            area.left() + ((f - F.0) / (F.1 - F.0)) as f32 * area.width(),
            area.bottom() - ((v - V.0) / (V.1 - V.0)) as f32 * area.height(),
        )
    };
    let grid = egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    painter.rect_filled(area, 0.0, visuals.extreme_bg_color);
    for f in (100..=600).step_by(100) {
        let x = pos(f as f64, 0.0).x;
        painter.line_segment([egui::pos2(x, area.top()), egui::pos2(x, area.bottom())], grid);
        painter.text(
            egui::pos2(x, area.bottom() + 2.0),
            egui::Align2::CENTER_TOP,
            format!("{f}"),
            font.clone(),
            visuals.weak_text_color(),
        );
    }
    for v in (0..=60).step_by(20) {
        let y = pos(F.0, v as f64).y;
        painter.line_segment([egui::pos2(area.left(), y), egui::pos2(area.right(), y)], grid);
        painter.text(
            egui::pos2(area.left() - 4.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{v} V"),
            font.clone(),
            visuals.weak_text_color(),
        );
    }
    painter.text(
        area.right_bottom() + Vec2::new(0.0, 2.0),
        egui::Align2::RIGHT_TOP,
        "kHz",
        font,
        visuals.weak_text_color(),
    );
    let points: Vec<egui::Pos2> = m
        .sweep()
        .into_iter()
        .filter(|(f, _)| (F.0..=F.1).contains(f))
        .map(|(f, v)| pos(f, v.min(V.1)))
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, visuals.selection.stroke.color)));
}

fn tooltip(ui: &mut egui::Ui, e: &Entry) {
    ui.monospace(format!("File:     {}", e.file.path.display()));
    ui.monospace(format!(
        "Name:     {}",
        e.resolved.as_ref().map(|r| r.name.replace('\n', " ")).unwrap_or_default()
    ));
    ui.monospace(format!("UID:      {}", e.file.uid));
    ui.monospace(format!("Date:     {}", format_date(e.date())));
    if let Some(a) = e.analysis() {
        ui.monospace(format!("AudioID:  0x{:08X}", a.audio_id));
        ui.monospace(format!("Chapters: {}", a.chapters.len()));
    }
}

fn details(ui: &mut egui::Ui, e: &Entry) {
    ui.label(egui::RichText::new(e.name().replace('\n', " ")).strong());
    egui::Grid::new("details").num_columns(2).show(ui, |ui| {
        let mut row = |k: &str, v: String| {
            ui.label(egui::RichText::new(k).weak());
            ui.label(v);
            ui.end_row();
        };
        row("UID", e.file.uid.clone());
        row("Date", format_date(e.date()));
        row("Live", if e.file.live { "yes".into() } else { "no".into() });
        match &e.analysis {
            Some(Ok(a)) => {
                row("Audio ID", format!("0x{:08X}", a.audio_id));
                row("Length", format_time(Duration::from_secs_f64(a.duration_secs())));
                row("Tracks", a.chapters.len().to_string());
                row("Hash", format!("{}…", &a.hash[..a.hash.len().min(12)]));
            }
            Some(Err(err)) => row("Error", err.clone()),
            None => row("", "reading...".into()),
        }
    });
    if let Some(info) = e.info() {
        if !info.tracks.is_empty() {
            ui.add_space(6.0);
            egui::ScrollArea::vertical().id_salt("tracks").show(ui, |ui| {
                for (i, t) in info.tracks.iter().enumerate() {
                    ui.label(format!("{:>2}. {t}", i + 1));
                }
            });
        }
    }
    ui.add_space(6.0);
    ui.label(egui::RichText::new(e.file.path.display().to_string()).weak().small());
}

impl eframe::App for TeddyBenchApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.process_events(&ctx);
        let mut actions = Vec::new();

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui, &mut actions));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));

        if self.current_dir.is_some() {
            egui::Panel::right("actions")
                .default_size(240.0)
                .size_range(180.0..=420.0)
                .show(ui, |ui| self.side_panel(ui, &mut actions));
        }

        egui::CentralPanel::default_margins().show(ui, |ui| {
            if self.current_dir.is_some() {
                self.grid(ui, &mut actions);
            } else {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() / 3.0);
                        ui.heading("Please insert your box' SD card");
                        ui.label("or open the CONTENT directory of a card or a copy of it.");
                        ui.add_space(8.0);
                        if ui.button("Open directory...").clicked() {
                            actions.push(Action::OpenDirectory);
                        }
                    });
                });
            }
        });

        self.shortcuts(&ctx, &mut actions);
        self.dialogs(&ctx);
        self.job_window(&ctx);
        self.tag_operation_window(&ctx);
        self.log_window(&ctx);

        for a in actions {
            self.apply(a, &ctx);
        }

        let title = match &self.current_dir {
            Some(d) => format!("TeddyBench - {VERSION} - {}", d.display()),
            None => format!("TeddyBench - {VERSION}"),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }

        if self.playing.is_some() || self.job.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        #[cfg(debug_assertions)]
        self.screenshot_hook(&ctx);
    }
}

#[cfg(debug_assertions)]
impl TeddyBenchApp {
    /// `TEDDYBENCH_SCREENSHOT=<file.png>` saves the window after 3 seconds and quits.
    /// `TEDDYBENCH_SCREENSHOT_SELECT=<n>` selects the n-th Tonie first, `TEDDYBENCH_SCREENSHOT_DIALOG=export|layout|new|live|trash` opens a dialog,
    /// `TEDDYBENCH_SCREENSHOT_SEARCH=<text>` searches.
    fn screenshot_hook(&mut self, ctx: &egui::Context) {
        let Some((path, started)) = self.screenshot.clone() else { return };
        ctx.request_repaint_after(Duration::from_millis(100));
        let elapsed = started.elapsed();

        if elapsed > Duration::from_millis(2500) && self.search.is_empty() {
            if let Ok(q) = std::env::var("TEDDYBENCH_SCREENSHOT_SEARCH") {
                self.search = q;
            }
        }
        if elapsed > Duration::from_millis(2500) && self.selection.is_empty() {
            if let Some(n) = std::env::var("TEDDYBENCH_SCREENSHOT_SELECT").ok().and_then(|n| n.parse::<usize>().ok()) {
                if let Some(&i) = self.sorted_indices().get(n) {
                    self.selection.insert(self.entries[i].file.path.clone());
                    match std::env::var("TEDDYBENCH_SCREENSHOT_DIALOG").as_deref() {
                        Ok("export") => self.apply(Action::Export(ExportFormat::Mp3), ctx),
                        Ok("live") => self.apply(Action::RemoveAllLive, ctx),
                        Ok("trash") => self.apply(Action::Trashcan, ctx),
                        Ok("layout") => {
                            let s = &self.settings;
                            self.dialog = Some(Dialog::TagLayout(TagLayoutDraft {
                                margin: s.custom_tag_non_printable_margin,
                                gap_x: s.custom_tag_gap_x,
                                gap_y: s.custom_tag_gap_y,
                                initial_column: s.custom_tag_initial_column,
                                initial_row: s.custom_tag_initial_row,
                                size: s.custom_tag_size,
                                outline_width: s.custom_tag_outline_width,
                            }));
                        }
                        Ok("new") => {
                            let inputs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/inputs/album");
                            let files = std::fs::read_dir(inputs)
                                .map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect())
                                .unwrap_or_default();
                            self.start_new_tonie(files);
                        }
                        _ => {}
                    }
                }
            }
        }

        if elapsed > Duration::from_secs(3) && !ctx.data(|d| d.get_temp::<bool>(Id::unique("screenshot_requested")).unwrap_or(false)) {
            ctx.data_mut(|d| d.insert_temp(Id::unique("screenshot_requested"), true));
            let c = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |image| {
                let [w, h] = image.size;
                let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Some(img) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
                    img.save(&path).ok();
                }
                c.send_viewport_cmd(egui::ViewportCommand::Close);
                c.request_repaint();
            })));
        }
    }
}
