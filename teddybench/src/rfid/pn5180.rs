//! An ESP with a PN5180 reader, speaking a line protocol over USB serial:
//! `v` answers the version (containing `Pn5180Esp`), `i` the UID of the tag in the field
//! (16 hex digits, empty without a tag), `u` unlocks a tag in privacy mode (`ok`).
//! Since 2.4.0, `w0` turns WiFi and MQTT off (`WiFi/MQTT disabled`) and `w1` on again.

use std::io;

use super::{Device, DeviceInfo, Link, ReaderKind, Tag};
use crate::applog;

pub struct Pn5180Esp {
    link: Box<dyn Link>,
    info: DeviceInfo,
}

/// Read timeouts of the port (500 ms each) to wait for an answer: reading a tag takes the reader
/// up to about a second.
const ANSWER_TIMEOUTS: u32 = 4;

/// Sends a command and reads the answer line. A missing answer is an error, like a lost device.
fn request(link: &mut dyn Link, command: &str) -> io::Result<String> {
    link.write_all(format!("{command}\n").as_bytes())?;
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    let mut timeouts = 0;
    loop {
        match link.read(&mut byte) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(_) if byte[0] == b'\n' => break,
            Ok(_) => line.push(byte[0]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::TimedOut && timeouts + 1 < ANSWER_TIMEOUTS => timeouts += 1,
            Err(e) => return Err(e),
        }
        if line.len() > 256 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
        }
    }
    Ok(String::from_utf8_lossy(&line).replace('\r', ""))
}

/// The version in an answer like `Pn5180Esp - 2.4.0`
fn firmware_version(answer: &str) -> Option<(u32, u32, u32)> {
    let mut parts = answer.rsplit(' ').next()?.trim().split('.').map(|p| p.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)))
}

impl Pn5180Esp {
    pub fn open(mut link: Box<dyn Link>) -> io::Result<Option<Pn5180Esp>> {
        link.clear_input()?;
        let version = request(link.as_mut(), "v")?;
        if !version.contains("Pn5180Esp") {
            return Ok(None);
        }
        let port = link.name();
        applog::info(format!("[Pn5180Esp] {} found at {port}", version.trim()));
        /* WiFi slows down the answers and can restart the ESP, TeddyBench only needs USB */
        if firmware_version(&version).is_some_and(|v| v >= (2, 4, 0)) {
            match request(link.as_mut(), "w0") {
                Ok(answer) if answer.contains("disabled") => applog::info(format!("[Pn5180Esp] {answer}")),
                Ok(answer) => applog::warning(format!("[Pn5180Esp] Turning off WiFi: unexpected answer \"{answer}\"")),
                Err(e) => applog::warning(format!("[Pn5180Esp] Turning off WiFi: {e}")),
            }
        }
        Ok(Some(Pn5180Esp {
            link,
            info: DeviceInfo {
                kind: ReaderKind::Pn5180Esp,
                hardware: "Pn5180Esp".into(),
                port,
                /* the unlock happens in its firmware */
                unlock_supported: false,
                antenna_voltage: None,
            },
        }))
    }

    fn get_uid(&mut self) -> io::Result<Option<String>> {
        let uid = request(self.link.as_mut(), "i")?;
        if uid.len() != 16 {
            applog::debug(format!(
                "[Pn5180Esp] GetUID: Failed ({})",
                if uid.is_empty() { "no resp".into() } else { format!("{} bytes", uid.len()) }
            ));
            return Ok(None);
        }
        Ok(Some(uid.to_ascii_uppercase()))
    }
}

impl Device for Pn5180Esp {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn identify(&mut self) -> io::Result<Option<Tag>> {
        self.link.clear_input()?;
        let mut uid = self.get_uid()?;
        if uid.is_none() {
            /* no tag, or one in privacy mode: the firmware knows the passwords */
            if request(self.link.as_mut(), "u")? == "ok" {
                applog::debug("[Pn5180Esp] Unlocked tag");
                uid = self.get_uid()?;
            }
        }
        Ok(uid.map(|uid| Tag {
            uid: Some(uid),
            raw: Vec::new(),
        }))
    }

    fn still_there(&mut self, _tag: &Tag) -> io::Result<bool> {
        self.link.clear_input()?;
        Ok(self.get_uid()?.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfid::tests::FakeLink;

    #[test]
    fn reads_uids() {
        let link = FakeLink::new(b"Pn5180Esp v1.0\r\n\r\nok\r\ne0040350010c0b0a\r\ne0040350010c0b0a\n\n".to_vec());
        let sent = link.sent.clone();
        let mut reader = Pn5180Esp::open(Box::new(link)).unwrap().unwrap();

        let tag = reader.identify().unwrap().unwrap();
        assert_eq!(tag.uid.as_deref(), Some("E0040350010C0B0A"));
        assert!(reader.still_there(&tag).unwrap());
        assert!(!reader.still_there(&tag).unwrap());
        assert_eq!(&sent.lock().unwrap()[..], b"v\ni\nu\ni\ni\ni\n");

        /* no answer at all: the device is gone */
        assert!(reader.still_there(&tag).is_err());
    }

    #[test]
    fn turns_off_wifi_since_2_4() {
        let link = FakeLink::new(b"Pn5180Esp - 2.4.0\r\nWiFi/MQTT disabled\r\n".to_vec());
        let sent = link.sent.clone();
        assert!(Pn5180Esp::open(Box::new(link)).unwrap().is_some());
        assert_eq!(&sent.lock().unwrap()[..], b"v\nw0\n");

        let link = FakeLink::new(b"Pn5180Esp - 2.3.0\r\n".to_vec());
        let sent = link.sent.clone();
        assert!(Pn5180Esp::open(Box::new(link)).unwrap().is_some());
        assert_eq!(&sent.lock().unwrap()[..], b"v\n");

        assert_eq!(firmware_version("Pn5180Esp - 2.10.1"), Some((2, 10, 1)));
        assert_eq!(firmware_version("Pn5180Esp v1.0"), None);
    }

    #[test]
    fn ignores_other_devices() {
        assert!(Pn5180Esp::open(Box::new(FakeLink::new(b"hello\n".to_vec()))).unwrap().is_none());
    }
}
