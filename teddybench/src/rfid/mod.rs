//! NFC readers on a serial port: a Proxmark3, or an ESP with a PN5180.
//!
//! A [`Reader`] runs in the background, looks for the device on the USB serial ports and reports
//! the UID of the tag in its field, so the matching Tonie can be selected or the UID filled in.

pub mod iso15693;
pub mod pn5180;
pub mod proxmark3;

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::applog;
pub use proxmark3::Measurement;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaderKind {
    Proxmark3,
    Pn5180Esp,
}

impl ReaderKind {
    pub const ALL: [ReaderKind; 2] = [ReaderKind::Proxmark3, ReaderKind::Pn5180Esp];

    /// From the `NfcType` setting
    pub fn from_setting(s: &str) -> ReaderKind {
        match s {
            "Pn5180Esp" => ReaderKind::Pn5180Esp,
            _ => ReaderKind::Proxmark3,
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            ReaderKind::Proxmark3 => "Proxmark3",
            ReaderKind::Pn5180Esp => "Pn5180Esp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ReaderKind::Proxmark3 => "Proxmark3",
            ReaderKind::Pn5180Esp => "ESP with PN5180",
        }
    }

    fn log_prefix(self) -> &'static str {
        match self {
            ReaderKind::Proxmark3 => "[PM3]",
            ReaderKind::Pn5180Esp => "[Pn5180Esp]",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub kind: ReaderKind,
    pub hardware: String,
    pub port: String,
    /// Can unlock tags in privacy mode, read and emulate tags (Proxmark3 with the SLIX-L firmware)
    pub unlock_supported: bool,
    /// HF antenna voltage, measured when the device was found
    pub antenna_voltage: Option<f32>,
}

/// A tag in the field of the reader
#[derive(Clone, Debug)]
pub struct Tag {
    /// As printed, `E00403...`. None if it could not be read (privacy mode).
    pub uid: Option<String>,
    /// The UID as the reader needs it to address the tag
    raw: Vec<u8>,
}

/// The connection to the device
pub trait Link: Read + Write + Send {
    fn name(&self) -> String;
    /// Discards what was received and not read
    fn clear_input(&mut self) -> io::Result<()>;
}

impl Link for Box<dyn serialport::SerialPort> {
    fn name(&self) -> String {
        self.as_ref().name().unwrap_or_default()
    }

    fn clear_input(&mut self) -> io::Result<()> {
        self.clear(serialport::ClearBuffer::Input).map_err(io::Error::from)
    }
}

/// A reader that was found. Errors mean the device is lost.
trait Device: Send {
    fn info(&self) -> DeviceInfo;
    /// Looks for a tag in the field
    fn identify(&mut self) -> io::Result<Option<Tag>>;
    /// Whether the tag found by `identify` is still in the field
    fn still_there(&mut self, tag: &Tag) -> io::Result<bool>;

    /// The UID (as printed) and the 32 bytes of memory of the tag in the field
    fn read_memory(&mut self, _cancel: &AtomicBool) -> io::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    /// Acts as a tag with the UID and memory as returned by `read_memory`
    fn emulate(&mut self, _data: &[u8], _cancel: &AtomicBool) -> io::Result<()> {
        Ok(())
    }
    fn measure_antenna(&mut self, _cancel: &AtomicBool) -> io::Result<Option<Measurement>> {
        Ok(None)
    }
    /// Shows what the device sends, instead of looking for tags
    fn console(&mut self) -> io::Result<()> {
        std::thread::sleep(Duration::from_millis(100));
        Ok(())
    }
}

#[derive(Debug)]
pub enum Event {
    /// A device was found, or lost (None)
    Device(Option<DeviceInfo>),
    /// A tag was put on the reader, or removed (None)
    Uid(Option<String>),
    /// Result of [`Reader::read_memory`]: None if no tag was found
    Memory(Option<Vec<u8>>),
    /// Result of [`Reader::measure_antenna`]
    Measurement(Option<Measurement>),
    /// [`Reader::emulate`] ended
    EmulationEnded,
}

enum Request {
    ReadMemory,
    Emulate(Vec<u8>),
    MeasureAntenna,
    Console(bool),
}

/// The background thread looking for the device and tags. Stops when dropped.
pub struct Reader {
    requests: Sender<Request>,
    pub events: Receiver<Event>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Reader {
    /// `notify` is called after each event, e.g. to repaint the window.
    /// `port` is the one chosen in the settings; without one the USB serial ports are searched.
    pub fn start(kind: ReaderKind, port: Option<String>, notify: impl Fn() + Send + 'static) -> Reader {
        let (requests, request_rx) = channel();
        let (event_tx, events) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut worker = Worker {
            kind,
            requests: request_rx,
            events: event_tx,
            notify: Box::new(notify),
            stop: stop.clone(),
            cancel: cancel.clone(),
            ports: Ports {
                chosen: port,
                ..Default::default()
            },
            device: None,
            tag: None,
            reported: None,
            console: false,
        };
        let thread = std::thread::Builder::new().name("nfc reader".into()).spawn(move || worker.run()).ok();
        Reader {
            requests,
            events,
            stop,
            cancel,
            thread,
        }
    }

    /// Reads the UID and memory of the tag, answered with [`Event::Memory`]
    pub fn read_memory(&self) {
        self.request(Request::ReadMemory);
    }

    /// Emulates a tag (UID as printed and 32 bytes of memory) until the button on the device is
    /// pressed or [`Reader::cancel`]; answered with [`Event::EmulationEnded`]
    pub fn emulate(&self, data: Vec<u8>) {
        self.request(Request::Emulate(data));
    }

    /// Answered with [`Event::Measurement`]
    pub fn measure_antenna(&self) {
        self.request(Request::MeasureAntenna);
    }

    /// Stops looking for tags and logs what the device sends instead
    pub fn set_console(&self, on: bool) {
        self.request(Request::Console(on));
    }

    /// Gives up the running read, emulation or measurement
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn request(&self, request: Request) {
        self.cancel.store(false, Ordering::Relaxed);
        self.requests.send(request).ok();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
        /* the port has to be closed before a new reader can open it */
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

/// Serial ports and when they appeared or failed
#[derive(Default)]
struct Ports {
    appeared: HashMap<String, Instant>,
    /// When a port that failed may be tried again
    retry_at: HashMap<String, Instant>,
    /// Ports a reader was found at: while it restarts (an ESP connects to WiFi first), it does not
    /// answer, so these are tried again soon
    had_reader: HashSet<String>,
    /// The port chosen in the settings, tried instead of searching
    chosen: Option<String>,
}

/// All serial ports, to choose the reader's port from
pub fn available_ports() -> Vec<String> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.port_name)
        /* on macOS every device is there twice, the tty. one waits for a carrier on open */
        .filter(|p| !p.starts_with("/dev/tty."))
        .collect()
}

/// A port that could not be opened (busy, e.g. still closed by a reader stopped a moment ago, or
/// no permission) is tried again soon; one that opened but has no reader not before a minute,
/// unless a reader was found there before.
const RETRY_OPEN: Duration = Duration::from_secs(2);
const RETRY_PROBE: Duration = Duration::from_secs(60);

impl Ports {
    /// USB serial ports that are worth a try: present for a second (so the device has booted),
    /// and not failed recently (see [`RETRY_OPEN`], [`RETRY_PROBE`]). `TEDDYBENCH_NFC_PORT` or the
    /// port chosen in the settings is tried instead, e.g. for a reader that is not connected through USB.
    fn candidates(&mut self, prefix: &str) -> Vec<String> {
        let fixed = std::env::var("TEDDYBENCH_NFC_PORT").ok().filter(|p| !p.is_empty()).or(self.chosen.clone());
        let present: Vec<String> = match fixed {
            Some(port) => vec![port],
            None => serialport::available_ports()
                .unwrap_or_default()
                .into_iter()
                .filter(|p| matches!(p.port_type, serialport::SerialPortType::UsbPort(_)))
                /* on macOS every device is there twice, the tty. one waits for a carrier on open */
                .filter(|p| !p.port_name.starts_with("/dev/tty."))
                .map(|p| p.port_name)
                .collect(),
        };

        let now = Instant::now();
        for p in &present {
            if !self.appeared.contains_key(p) {
                applog::debug(format!("{prefix} New port: {p}"));
                self.appeared.insert(p.clone(), now);
            }
        }
        self.appeared.retain(|p, _| {
            let keep = present.contains(p);
            if !keep {
                applog::debug(format!("{prefix} Port disappeared: {p}"));
            }
            keep
        });
        self.retry_at.retain(|p, _| present.contains(p));

        present
            .into_iter()
            .filter(|p| now.duration_since(self.appeared[p]) > Duration::from_secs(1))
            .filter(|p| self.retry_at.get(p).is_none_or(|t| now >= *t))
            .collect()
    }

    /// Notes a failed port. Returns whether it failed for the first time.
    fn failed(&mut self, port: &str, retry: Duration) -> bool {
        self.retry_at.insert(port.to_string(), Instant::now() + retry).is_none()
    }
}

fn open_port(name: &str) -> io::Result<Box<dyn serialport::SerialPort>> {
    let builder = serialport::new(name, 115_200).timeout(Duration::from_millis(500));
    /* An ESP resets when RTS is set and DTR is not. On Windows both start cleared, like in .NET.
     * On Linux and macOS the open sets both: clearing them one after the other would reset it. */
    #[cfg(windows)]
    let builder = builder.dtr_on_open(false);
    #[allow(unused_mut)]
    let mut port = builder.open()?;
    #[cfg(windows)]
    port.write_request_to_send(false).ok();
    Ok(port)
}

struct Worker {
    kind: ReaderKind,
    requests: Receiver<Request>,
    events: Sender<Event>,
    notify: Box<dyn Fn() + Send>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    ports: Ports,
    device: Option<Box<dyn Device>>,
    tag: Option<Tag>,
    /// The UID last sent to the UI
    reported: Option<String>,
    console: bool,
}

impl Worker {
    fn send(&self, event: Event) {
        self.events.send(event).ok();
        (self.notify)();
    }

    fn sleep(&self, ms: u64) {
        if !self.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(ms));
        }
    }

    fn run(&mut self) {
        let prefix = self.kind.log_prefix();
        while !self.stop.load(Ordering::Relaxed) {
            while let Ok(request) = self.requests.try_recv() {
                if let Err(e) = self.handle(request) {
                    self.lost(&e);
                }
            }

            if self.device.is_none() {
                match self.detect() {
                    Some(device) => {
                        self.send(Event::Device(Some(device.info())));
                        self.device = Some(device);
                    }
                    None => self.sleep(250),
                }
            } else if let Err(e) = self.step() {
                self.lost(&e);
            }
            self.sleep(50);
        }
        applog::debug(format!("{prefix} Stopped"));
    }

    fn detect(&mut self) -> Option<Box<dyn Device>> {
        let prefix = self.kind.log_prefix();
        for port in self.ports.candidates(prefix) {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            applog::debug(format!("{prefix} Try port {port}"));
            let link: Box<dyn Link> = match open_port(&port) {
                Ok(p) => Box::new(p),
                Err(e) => {
                    let hint = if cfg!(target_os = "linux") && e.kind() == io::ErrorKind::PermissionDenied {
                        " (add your user to the dialout group to use the reader)"
                    } else {
                        ""
                    };
                    let message = format!("{prefix} Cannot open {port}: {e}{hint}");
                    if self.ports.failed(&port, RETRY_OPEN) {
                        applog::warning(message);
                    } else {
                        applog::debug(message);
                    }
                    continue;
                }
            };
            let device: io::Result<Option<Box<dyn Device>>> = match self.kind {
                ReaderKind::Proxmark3 => proxmark3::Proxmark3::open(link).map(|d| d.map(|d| Box::new(d) as Box<dyn Device>)),
                ReaderKind::Pn5180Esp => pn5180::Pn5180Esp::open(link).map(|d| d.map(|d| Box::new(d) as Box<dyn Device>)),
            };
            match device {
                Ok(Some(d)) => {
                    self.ports.had_reader.insert(port);
                    return Some(d);
                }
                Ok(None) => applog::debug(format!("{prefix} No {} at {port}", self.kind.label())),
                Err(e) => applog::debug(format!("{prefix} No {} at {port}: {e}", self.kind.label())),
            }
            /* a reader found there before, or chosen by the user, is probably just restarting */
            let expected = self.ports.had_reader.contains(&port) || self.ports.chosen.as_ref() == Some(&port);
            let retry = if expected { RETRY_OPEN } else { RETRY_PROBE };
            self.ports.failed(&port, retry);
        }
        None
    }

    /// Looks for a new tag, or whether the current one is still there
    fn step(&mut self) -> io::Result<()> {
        let Some(device) = self.device.as_mut() else { return Ok(()) };
        if self.console {
            return device.console();
        }
        match &self.tag {
            None => match device.identify()? {
                Some(tag) => {
                    let uid = tag.uid.clone();
                    self.tag = Some(tag);
                    self.report(uid);
                }
                None => {
                    self.report(None);
                    self.sleep(100);
                }
            },
            Some(tag) => {
                if device.still_there(tag)? {
                    self.sleep(100);
                } else {
                    applog::debug(format!("{} Tag disappeared", self.kind.log_prefix()));
                    self.tag = None;
                    self.report(None);
                }
            }
        }
        Ok(())
    }

    fn report(&mut self, uid: Option<String>) {
        if self.reported == uid {
            return;
        }
        let prefix = self.kind.log_prefix();
        match &uid {
            Some(u) => applog::debug(format!("{prefix} Report tag UID: {u}")),
            None => applog::debug(format!("{prefix} Report removed tag")),
        }
        self.reported = uid.clone();
        self.send(Event::Uid(uid));
    }

    fn handle(&mut self, request: Request) -> io::Result<()> {
        let cancel = self.cancel.clone();
        /* look for the tag again afterwards */
        self.tag = None;
        let device = self.device.as_mut();
        match request {
            Request::ReadMemory => {
                let data = match device {
                    Some(d) => d.read_memory(&cancel)?,
                    None => None,
                };
                self.send(Event::Memory(data));
            }
            Request::Emulate(data) => {
                if let Some(d) = device {
                    d.emulate(&data, &cancel)?;
                }
                self.send(Event::EmulationEnded);
            }
            Request::MeasureAntenna => {
                let m = match device {
                    Some(d) => d.measure_antenna(&cancel)?,
                    None => None,
                };
                if let Some(d) = &self.device {
                    /* with the new antenna voltage */
                    self.send(Event::Device(Some(d.info())));
                }
                self.send(Event::Measurement(m));
            }
            Request::Console(on) => {
                if on {
                    applog::info(format!(
                        "{} Console mode. If you know what you are doing, then long-press the PM3 button.",
                        self.kind.log_prefix()
                    ));
                }
                self.console = on;
            }
        }
        Ok(())
    }

    fn lost(&mut self, error: &io::Error) {
        applog::debug(format!("{} Device failed. Closing. ({error})", self.kind.log_prefix()));
        if self.device.take().is_some() {
            self.send(Event::Device(None));
        }
        self.tag = None;
        self.console = false;
        self.report(None);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Answers with prepared bytes and records what was sent. Times out when there is nothing left.
    pub struct FakeLink {
        input: VecDeque<u8>,
        pub sent: Arc<Mutex<Vec<u8>>>,
    }

    impl FakeLink {
        pub fn new(input: Vec<u8>) -> FakeLink {
            FakeLink {
                input: input.into(),
                sent: Default::default(),
            }
        }
    }

    impl Read for FakeLink {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.input.is_empty() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            /* a few bytes at a time, like a serial port */
            let n = buf.len().min(self.input.len()).min(7);
            for b in &mut buf[..n] {
                *b = self.input.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for FakeLink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.sent.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Link for FakeLink {
        fn name(&self) -> String {
            "fake".into()
        }

        fn clear_input(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_ports_are_retried_later() {
        let mut ports = Ports::default();
        assert!(ports.failed("COM3", RETRY_OPEN));
        /* the warning is only logged the first time */
        assert!(!ports.failed("COM3", RETRY_OPEN));
        let busy_until = ports.retry_at["COM3"];
        assert!(busy_until <= Instant::now() + RETRY_OPEN);
        ports.failed("COM4", RETRY_PROBE);
        assert!(ports.retry_at["COM4"] > busy_until + Duration::from_secs(50));
    }

    #[test]
    fn reader_kinds_from_settings() {
        for k in ReaderKind::ALL {
            assert_eq!(ReaderKind::from_setting(k.setting()), k);
        }
        assert_eq!(ReaderKind::from_setting("whatever"), ReaderKind::Proxmark3);
    }
}
