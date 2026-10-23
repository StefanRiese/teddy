//! Messages for the debug window: what happens in the background, e.g. failed reads or downloads.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warning,
    Error,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warning => "WARN",
            Level::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Line {
    pub time: SystemTime,
    pub level: Level,
    pub text: String,
}

/// Oldest lines are dropped beyond this
const CAPACITY: usize = 5000;

static LOG: Mutex<VecDeque<Line>> = Mutex::new(VecDeque::new());

pub fn log(level: Level, text: impl Into<String>) {
    let text = text.into();
    if cfg!(debug_assertions) {
        eprintln!("{} {}", level.label(), text);
    }
    let mut log = LOG.lock().unwrap_or_else(|e| e.into_inner());
    if log.len() >= CAPACITY {
        log.pop_front();
    }
    log.push_back(Line {
        time: SystemTime::now(),
        level,
        text,
    });
}

pub fn debug(text: impl Into<String>) {
    log(Level::Debug, text);
}

pub fn info(text: impl Into<String>) {
    log(Level::Info, text);
}

pub fn warning(text: impl Into<String>) {
    log(Level::Warning, text);
}

pub fn error(text: impl Into<String>) {
    log(Level::Error, text);
}

/// The lines at or above `level`
pub fn lines(level: Level) -> Vec<Line> {
    LOG.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|l| l.level >= level)
        .cloned()
        .collect()
}

pub fn clear() {
    LOG.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// `hh:mm:ss` of the line, UTC
pub fn format_time(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format!("{:02}:{:02}:{:02}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_level() {
        warning("tagpdf test warning");
        debug("tagpdf test debug");
        let warnings = lines(Level::Warning);
        assert!(warnings.iter().any(|l| l.text == "tagpdf test warning"));
        assert!(!warnings.iter().any(|l| l.text == "tagpdf test debug"));
    }
}
