//! Background threads that watch the CONTENT directory, analyze Tonie files and load thumbnails.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use image::RgbaImage;

use crate::covers::load_thumbnail;
use crate::drives::find_content_dirs;
use crate::library::{scan_directory, Analysis, FoundFile};
use crate::settings::DataDir;

pub enum Event {
    /// A directory was opened (by request or because an SD card was found)
    Opened(PathBuf),
    /// The directory disappeared, e.g. the SD card was removed
    Lost,
    /// The Tonie files currently in the directory
    Files(Vec<FoundFile>),
    Analyzed(PathBuf, Result<Analysis, String>),
    Thumbnail(String, Result<RgbaImage, String>),
}

pub enum Command {
    Open(PathBuf),
    /// Look at the directory again now
    Rescan,
    /// Load the thumbnail for (hash, url)
    Thumbnail(String, String),
}

/// Handle to the scanner threads. `notify` is called after every event, e.g. to request a repaint.
pub struct Scanner {
    commands: Sender<Command>,
    pub events: Receiver<Event>,
}

impl Scanner {
    pub fn start(data_dir: DataDir, thumbnail_size: u32, auto_open: bool, notify: impl Fn() + Send + Clone + 'static) -> Scanner {
        let (cmd_tx, cmd_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        let (thumb_tx, thumb_rx) = channel::<(String, String)>();

        let events = ev_tx.clone();
        let n = notify.clone();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || {
                for (hash, url) in thumb_rx {
                    let result = load_thumbnail(&data_dir, &url, &hash, thumbnail_size);
                    if events.send(Event::Thumbnail(hash, result)).is_err() {
                        break;
                    }
                    n();
                }
            })
            .expect("start thumbnail thread");

        std::thread::Builder::new()
            .name("scanner".into())
            .spawn(move || {
                ScanLoop {
                    events: ev_tx,
                    thumbs: thumb_tx,
                    notify,
                    current: None,
                    auto_open,
                    files: Vec::new(),
                    analyzed: HashMap::new(),
                }
                .run(cmd_rx)
            })
            .expect("start scanner thread");

        Scanner {
            commands: cmd_tx,
            events: ev_rx,
        }
    }

    pub fn send(&self, cmd: Command) {
        self.commands.send(cmd).ok();
    }
}

struct ScanLoop<N> {
    events: Sender<Event>,
    thumbs: Sender<(String, String)>,
    notify: N,
    current: Option<PathBuf>,
    /// Open the first SD card that shows up
    auto_open: bool,
    files: Vec<FoundFile>,
    /// Modification time at the last analysis, so unchanged files are not read again
    analyzed: HashMap<PathBuf, Option<SystemTime>>,
}

impl<N: Fn()> ScanLoop<N> {
    fn emit(&self, e: Event) -> bool {
        let ok = self.events.send(e).is_ok();
        (self.notify)();
        ok
    }

    fn run(mut self, commands: Receiver<Command>) {
        let mut last_scan = Instant::now() - Duration::from_secs(10);
        loop {
            match commands.recv_timeout(Duration::from_millis(500)) {
                Ok(Command::Open(dir)) => {
                    self.open(dir);
                    last_scan = Instant::now() - Duration::from_secs(10);
                }
                Ok(Command::Rescan) => last_scan = Instant::now() - Duration::from_secs(10),
                Ok(Command::Thumbnail(hash, url)) => {
                    self.thumbs.send((hash, url)).ok();
                    continue;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }

            if self.current.is_none() && self.auto_open {
                if let Some(dir) = find_content_dirs().into_iter().next() {
                    self.open(dir);
                    last_scan = Instant::now() - Duration::from_secs(10);
                }
            }

            let Some(dir) = self.current.clone() else { continue };
            if !dir.is_dir() {
                self.current = None;
                self.auto_open = true;
                self.files.clear();
                if !self.emit(Event::Lost) {
                    return;
                }
                continue;
            }

            if last_scan.elapsed() >= Duration::from_secs(2) {
                last_scan = Instant::now();
                if !self.rescan(&dir) {
                    return;
                }
            }
        }
    }

    fn open(&mut self, dir: PathBuf) {
        self.auto_open = false;
        self.current = Some(dir.clone());
        self.files.clear();
        self.analyzed.clear();
        self.emit(Event::Opened(dir));
    }

    /// Lists the directory and analyzes new or changed files. Returns false if the UI is gone.
    fn rescan(&mut self, dir: &std::path::Path) -> bool {
        let files = scan_directory(dir).unwrap_or_default();
        if files != self.files {
            self.files = files.clone();
            if !self.emit(Event::Files(files)) {
                return false;
            }
        }

        let todo: Vec<FoundFile> = self.files.iter().filter(|f| self.analyzed.get(&f.path) != Some(&f.modified)).cloned().collect();
        /* reading the page statistics walks the whole file, use a few threads */
        for chunk in todo.chunks(4) {
            let results: Vec<(PathBuf, Result<Analysis, String>)> = std::thread::scope(|s| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|f| s.spawn(|| (f.path.clone(), Analysis::read(&f.path).map_err(|e| e.to_string()))))
                    .collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            for (f, (path, result)) in chunk.iter().zip(results) {
                self.analyzed.insert(f.path.clone(), f.modified);
                if !self.emit(Event::Analyzed(path, result)) {
                    return false;
                }
            }
        }
        true
    }
}
