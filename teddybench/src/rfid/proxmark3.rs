//! Proxmark3 over its USB serial port.
//!
//! Commands are sent in the old fixed-size frame format (the emulation in the new one), answers
//! come in either format. Unlocking tags in privacy mode and the emulation need the firmware from
//! <https://github.com/g3gg0/proxmark3/tree/iso15693_slix_l_features>.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::iso15693::{self, uid_to_string};
use super::{Device, DeviceInfo, Link, ReaderKind, Tag};
use crate::applog;

const MAGIC_COMMAND_NG: u32 = 0x6133_4d50; // "PM3a"
const MAGIC_RESPONSE_NG: u32 = 0x6233_4d50; // "PM3b"
/// Instead of a CRC: the firmware does not check it
const MAGIC_CRC: u16 = 0x3361;

const DATA_SIZE: usize = 512;
/// cmd, 3 args, data
const OLD_FRAME_SIZE: usize = 8 + 3 * 8 + DATA_SIZE;
const NG_HEADER_SIZE: usize = 10;

pub mod cmd {
    pub const DEVICE_INFO: u16 = 0x0000;
    pub const HARDWARE_RESET: u16 = 0x0004;
    pub const DEBUG_STRING: u16 = 0x0100;
    pub const VERSION_INFO: u16 = 0x0107;
    pub const PING: u16 = 0x0109;
    pub const WTX: u16 = 0x0116;
    pub const ISO15693_SIMULATE: u16 = 0x0311;
    pub const ISO15693_COMMAND: u16 = 0x0313;
    pub const ISO15693_SLIX_DISABLE_PRIVACY: u16 = 0x0317;
    pub const MEASURE_ANTENNA_TUNING: u16 = 0x0400;
    pub const ACK: u16 = 0x00FF;
}

/// Bits of the device info
const DEVICE_INFO_MODE_BOOTROM: u64 = 4;

/// The passwords Tonies use for the privacy mode
const PRIVACY_PASSWORDS: [u32; 2] = [0x0F0F_0F0F, 0x7FFD_6E5B];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub cmd: u16,
    pub status: i16,
    /// In the old format, with arguments
    pub legacy: bool,
    pub args: [u64; 3],
    pub data: Vec<u8>,
}

impl Response {
    fn u16_at(&self, index: usize) -> u16 {
        self.data.get(index * 2..index * 2 + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).unwrap_or(0)
    }

    fn u32_at(&self, index: usize) -> u32 {
        self.data
            .get(index * 4..index * 4 + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .unwrap_or(0)
    }
}

/// A command in the old format: cmd and three arguments as 64 bit values, 512 bytes of data
pub fn old_frame(cmd: u16, args: [u64; 3], data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(OLD_FRAME_SIZE);
    buf.extend_from_slice(&(cmd as u64).to_le_bytes());
    for a in args {
        buf.extend_from_slice(&a.to_le_bytes());
    }
    let len = data.len().min(DATA_SIZE);
    buf.extend_from_slice(&data[..len]);
    buf.resize(OLD_FRAME_SIZE, 0);
    buf
}

/// A command in the new format: header, data of any length up to 512 bytes, CRC
pub fn ng_frame(cmd: u16, data: &[u8]) -> Vec<u8> {
    let data = &data[..data.len().min(DATA_SIZE)];
    let mut buf = Vec::with_capacity(NG_HEADER_SIZE + data.len());
    buf.extend_from_slice(&MAGIC_COMMAND_NG.to_le_bytes());
    buf.extend_from_slice(&(data.len() as u16 | 0x8000).to_le_bytes());
    buf.extend_from_slice(&cmd.to_le_bytes());
    buf.extend_from_slice(data);
    buf.extend_from_slice(&MAGIC_CRC.to_le_bytes());
    buf
}

/// Reads until `buf` is full or the device stays silent for the port's timeout.
fn read_full(link: &mut dyn Link, buf: &mut [u8]) -> io::Result<usize> {
    let mut pos = 0;
    while pos < buf.len() {
        match link.read(&mut buf[pos..]) {
            Ok(0) => break,
            Ok(n) => pos += n,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(pos)
}

/// The next answer in either format. `None` if the device did not answer (completely).
pub fn read_response(link: &mut dyn Link) -> io::Result<Option<Response>> {
    let mut header = [0u8; NG_HEADER_SIZE];
    if read_full(link, &mut header)? != header.len() {
        return Ok(None);
    }

    if u32::from_le_bytes([header[0], header[1], header[2], header[3]]) == MAGIC_RESPONSE_NG {
        let len = (u16::from_le_bytes([header[4], header[5]]) & 0x7FFF) as usize;
        if len > DATA_SIZE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("answer with {len} bytes of data")));
        }
        /* data and CRC */
        let mut rest = vec![0u8; len + 2];
        if read_full(link, &mut rest)? != rest.len() {
            applog::debug(format!("[PM3] Answer incomplete, {len} bytes of data expected"));
            return Ok(None);
        }
        rest.truncate(len);
        return Ok(Some(Response {
            cmd: u16::from_le_bytes([header[8], header[9]]),
            status: i16::from_le_bytes([header[6], header[7]]),
            legacy: false,
            args: [0; 3],
            data: rest,
        }));
    }

    let mut frame = vec![0u8; OLD_FRAME_SIZE];
    frame[..NG_HEADER_SIZE].copy_from_slice(&header);
    if NG_HEADER_SIZE + read_full(link, &mut frame[NG_HEADER_SIZE..])? != OLD_FRAME_SIZE {
        applog::debug("[PM3] Answer in the old format incomplete");
        return Ok(None);
    }
    let u64_at = |pos: usize| u64::from_le_bytes(frame[pos..pos + 8].try_into().unwrap());
    Ok(Some(Response {
        cmd: u64_at(0) as u16,
        status: 0,
        legacy: true,
        args: [u64_at(8), u64_at(16), u64_at(24)],
        data: frame[32..].to_vec(),
    }))
}

/// Removes color codes (`ESC [ ... m`) from the firmware's texts
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            while let Some(&n) = chars.peek() {
                chars.next();
                if !(n.is_ascii_digit() || n == ';') {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Result of the antenna measurement
#[derive(Clone, Debug)]
pub struct Measurement {
    pub v_lf125: f32,
    pub v_lf134: f32,
    pub v_hf: f32,
    /// Divisor of the 12 MHz clock with the highest LF amplitude
    pub peak_f: u32,
    pub peak_v: f32,
    pub amplitudes: Vec<u8>,
}

impl Measurement {
    fn from_response(r: &Response) -> Measurement {
        Measurement {
            v_lf134: r.u32_at(0) as f32 * 0.002,
            v_lf125: r.u32_at(1) as f32 * 0.002,
            v_hf: r.u32_at(3) as f32 / 1000.0,
            peak_v: r.u32_at(4) as f32 * 0.002,
            peak_f: r.u32_at(5),
            amplitudes: r.data.get(28..28 + 256).map(<[u8]>::to_vec).unwrap_or_default(),
        }
    }

    /// The LF sweep: (frequency in kHz, amplitude in V)
    pub fn sweep(&self) -> Vec<(f64, f64)> {
        self.amplitudes
            .iter()
            .enumerate()
            .map(|(i, &a)| (12_000.0 / (i + 1) as f64, a as f64 * 512.0 / 1000.0))
            .collect()
    }

    pub fn peak_frequency_khz(&self) -> f64 {
        12_000.0 / (self.peak_f + 1) as f64
    }

    /// How good the HF field is, for the Tonie tags
    pub fn hf_rating(&self) -> &'static str {
        match self.v_hf {
            v if v > 45.0 => "critical",
            v if v > 40.0 => "danger",
            v if v > 33.0 => "wow",
            v if v > 25.0 => "good",
            v if v > 20.0 => "weak",
            v if v > 10.0 => "meh",
            _ => "bad",
        }
    }
}

pub struct Proxmark3 {
    link: Box<dyn Link>,
    info: DeviceInfo,
}

impl Proxmark3 {
    /// Checks whether a Proxmark3 is at the port and asks for its details.
    /// A device in bootloader mode is reset; flashing it is not supported here.
    pub fn open(mut link: Box<dyn Link>) -> io::Result<Option<Proxmark3>> {
        let port = link.name();
        link.clear_input()?;
        /* whatever the device still had to say */
        read_response(link.as_mut())?;

        link.write_all(&old_frame(cmd::DEVICE_INFO, [0; 3], &[]))?;
        let flags = match read_response(link.as_mut())? {
            Some(r) if r.cmd == cmd::DEVICE_INFO && r.legacy => r.args[0],
            Some(r) if r.cmd == cmd::DEVICE_INFO => r.u32_at(0) as u64,
            _ => {
                applog::debug(format!("[PM3] {port} did not reply to a device info"));
                return Ok(None);
            }
        };
        applog::debug(format!("[PM3] Device info: 0x{flags:02X}"));

        if flags & DEVICE_INFO_MODE_BOOTROM != 0 {
            applog::error(
                "[PM3] The device started in bootloader mode. Flashing is not supported by this version, \
                 please use the Proxmark3 tools. Resetting the device.",
            );
            link.write_all(&old_frame(cmd::HARDWARE_RESET, [0; 3], &[]))?;
            return Ok(None);
        }

        link.write_all(&old_frame(cmd::PING, [0; 3], &[]))?;
        if read_response(link.as_mut())?.map(|r| r.cmd) != Some(cmd::PING) {
            applog::debug(format!("[PM3] {port} did not reply to a ping"));
            return Ok(None);
        }

        link.write_all(&old_frame(cmd::VERSION_INFO, [0; 3], &[]))?;
        let version = match read_response(link.as_mut())? {
            Some(r) if r.cmd == cmd::VERSION_INFO => r,
            _ => {
                applog::debug(format!("[PM3] {port} did not reply to a version info"));
                return Ok(None);
            }
        };
        std::thread::sleep(Duration::from_millis(500));
        link.clear_input()?;

        let chip_id = version.u32_at(0);
        let len = (version.u32_at(2) as usize).saturating_sub(1);
        let text = version.data.get(12..).unwrap_or_default();
        let text = strip_ansi(String::from_utf8_lossy(&text[..len.min(text.len())]).trim_end_matches('\0'));
        applog::debug(format!("[PM3] Chip ID: 0x{chip_id:08X}"));
        for line in text.lines() {
            applog::debug(format!("[PM3]     {line}"));
        }
        let hardware = match chip_id {
            0x0006_3660 => text.lines().find(|l| !l.trim().is_empty()).unwrap_or("Proxmark3").trim().to_string(),
            0xBB3D_270B | 0x270B_0A4F => "Proxmark3 (SAM7S)".into(),
            _ => "Proxmark3 (unknown controller)".into(),
        };

        let mut pm3 = Proxmark3 {
            link,
            info: DeviceInfo {
                kind: ReaderKind::Proxmark3,
                hardware,
                port: port.clone(),
                unlock_supported: false,
                antenna_voltage: None,
            },
        };

        /* the stock firmware does not answer the unlock command */
        pm3.info.unlock_supported = pm3.unlock(0xDEAD_BEEF)?.is_some();
        applog::debug(format!(
            "[PM3] Device does {}support the SLIX-L unlock command",
            if pm3.info.unlock_supported { "" } else { "*NOT* " }
        ));
        pm3.info.antenna_voltage = pm3.measure_antenna(AntennaType::Hf, &AtomicBool::new(false))?.map(|m| m.v_hf);

        applog::info(format!("[PM3] {} found at {port}", pm3.info.hardware));
        Ok(Some(pm3))
    }

    /// Logs debug messages and waits as asked. `false` if the device did not answer.
    fn handle_default(&self, response: Option<&Response>, comment: &str) -> bool {
        match response {
            None => {
                applog::debug(format!("[PM3] timeout {comment}"));
                false
            }
            Some(r) if r.cmd == cmd::DEBUG_STRING => {
                applog::debug(format!("[PM3] DebugMessage {} '{}'", r.u16_at(0), debug_text(r)));
                true
            }
            Some(r) if r.cmd == cmd::WTX => {
                let wait = r.u16_at(0);
                applog::debug(format!("[PM3] WTX received: {wait}"));
                std::thread::sleep(Duration::from_millis(wait as u64));
                true
            }
            Some(r) => {
                applog::debug(format!("[PM3] Unhandled answer 0x{:04X}", r.cmd));
                true
            }
        }
    }

    /// Unlocks a tag in privacy mode. `None` if the firmware does not know the command.
    fn unlock(&mut self, password: u32) -> io::Result<Option<bool>> {
        applog::debug(format!("[PM3] UnlockTag: Send request for pass 0x{password:08X}"));
        self.link
            .write_all(&old_frame(cmd::ISO15693_SLIX_DISABLE_PRIVACY, [0; 3], &password.to_le_bytes()))?;
        let mut timeouts = 0;
        loop {
            let response = read_response(self.link.as_mut())?;
            if let Some(r) = response.as_ref().filter(|r| r.cmd == cmd::ISO15693_SLIX_DISABLE_PRIVACY) {
                applog::debug(format!(
                    "[PM3] UnlockTag: {}",
                    if r.status == 0 {
                        "ACK".into()
                    } else {
                        format!("NACK (reason: {})", r.status)
                    }
                ));
                return Ok(Some(r.status == 0));
            }
            if !self.handle_default(response.as_ref(), "") {
                timeouts += 1;
                if timeouts > 4 {
                    return Ok(None);
                }
            }
        }
    }

    /// Sends an ISO 15693 request to the tag. The answer, if there is one with a valid CRC.
    fn transceive(&mut self, request: &[u8]) -> io::Result<Option<Vec<u8>>> {
        applog::debug(format!("[PM3] Send {}", hex(request)));
        /* length, receive an answer, high speed */
        self.link.write_all(&old_frame(cmd::ISO15693_COMMAND, [request.len() as u64, 1, 1], request))?;
        let mut timeouts = 0;
        loop {
            let response = read_response(self.link.as_mut())?;
            if let Some(r) = response.as_ref().filter(|r| r.cmd == cmd::ISO15693_COMMAND) {
                if r.data.is_empty() {
                    applog::debug("[PM3] no tag answered");
                    return Ok(None);
                }
                if !iso15693::check_crc(&r.data) {
                    applog::debug("[PM3] Answer with a wrong checksum");
                    return Ok(None);
                }
                return Ok(Some(r.data.clone()));
            }
            if !self.handle_default(response.as_ref(), "") {
                timeouts += 1;
                if timeouts > 4 {
                    return Ok(None);
                }
            }
        }
    }

    /// Asks for a random number, which SLIX-L tags answer even in privacy mode.
    /// With a UID only that tag answers.
    fn get_random(&mut self, uid: Option<&[u8]>) -> io::Result<bool> {
        let answer = self.transceive(&iso15693::build_command(iso15693::NXP_GET_RANDOM_NUMBER, uid, &[]))?;
        Ok(answer.is_some_and(|a| a.len() == 5))
    }

    /// The UID of the tag in the field, least significant byte first
    fn get_uid(&mut self, tries: usize, cancel: &AtomicBool) -> io::Result<Option<Vec<u8>>> {
        for _ in 0..tries {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            /* flags, DSFID, UID, CRC */
            match self.transceive(&iso15693::build_command(iso15693::INVENTORY, None, &[0]))? {
                Some(a) if a.len() == 12 => return Ok(Some(a[2..10].to_vec())),
                Some(a) => applog::debug(format!("[PM3] GetUID: Failed ({} bytes)", a.len())),
                None => applog::debug("[PM3] GetUID: Failed (no answer)"),
            }
        }
        Ok(None)
    }

    fn read_block(&mut self, block: u8, tries: usize, cancel: &AtomicBool) -> io::Result<Option<[u8; 4]>> {
        for _ in 0..tries {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            /* flags, data, CRC */
            match self.transceive(&iso15693::build_command(iso15693::READ_BLOCK, None, &[block]))? {
                Some(a) if a.len() == 7 => return Ok(Some([a[1], a[2], a[3], a[4]])),
                _ => applog::debug(format!("[PM3] ReadMemory: Failed to read block {block}")),
            }
        }
        Ok(None)
    }

    pub fn measure_antenna(&mut self, kind: AntennaType, cancel: &AtomicBool) -> io::Result<Option<Measurement>> {
        applog::debug("[PM3] MeasureAntenna: Start");
        self.link.write_all(&old_frame(cmd::MEASURE_ANTENNA_TUNING, [kind as u64, 0, 0], &[]))?;
        let mut timeouts = 0;
        while !cancel.load(Ordering::Relaxed) {
            let response = read_response(self.link.as_mut())?;
            if let Some(r) = response.as_ref().filter(|r| r.cmd == cmd::MEASURE_ANTENNA_TUNING) {
                return Ok(Some(Measurement::from_response(r)));
            }
            if !self.handle_default(response.as_ref(), "(measurement takes a while)") {
                timeouts += 1;
                if timeouts > 10 {
                    break;
                }
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AntennaType {
    Hf = 2,
    Both = 3,
}

/// The text of a debug message: flags, then the text
fn debug_text(r: &Response) -> String {
    let text = String::from_utf8_lossy(r.data.get(2..).unwrap_or_default());
    strip_ansi(text.trim_end_matches('\0'))
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect()
}

impl Device for Proxmark3 {
    fn info(&self) -> DeviceInfo {
        self.info.clone()
    }

    fn identify(&mut self) -> io::Result<Option<Tag>> {
        self.link.clear_input()?;
        if !self.get_random(None)? {
            return Ok(None);
        }
        applog::debug("[PM3] Tag detected, identifying");
        let never = AtomicBool::new(false);
        let mut uid = self.get_uid(1, &never)?;

        if uid.is_none() && self.info.unlock_supported {
            applog::debug("[PM3] Tag in privacy mode, try to unlock");
            for password in PRIVACY_PASSWORDS {
                match self.unlock(password)? {
                    Some(true) => {
                        applog::debug("[PM3] Unlocked tag");
                        uid = self.get_uid(1, &never)?;
                        if uid.is_none() {
                            applog::debug("[PM3] But it did still not respond to an INVENTORY command?!");
                            return Ok(None);
                        }
                        break;
                    }
                    Some(false) => {}
                    None => return Err(io::Error::other("no answer to the unlock command")),
                }
            }
        } else if uid.is_none() {
            applog::warning("[PM3] Tag in privacy mode, unlock not supported by your firmware");
            applog::warning("[PM3] Either use our custom firmware from");
            applog::warning("[PM3] -> https://github.com/g3gg0/proxmark3/tree/iso15693_slix_l_features");
            applog::warning("[PM3] or use the \"knock method\" to get it out of privacy mode");
            applog::warning("[PM3] -> https://youtu.be/IiZYM5k90pY");
            applog::warning("[PM3] Please remove the tag");
        }

        Ok(Some(Tag {
            uid: uid.as_deref().map(uid_to_string),
            raw: uid.unwrap_or_default(),
        }))
    }

    fn still_there(&mut self, tag: &Tag) -> io::Result<bool> {
        self.link.clear_input()?;
        let uid = (!tag.raw.is_empty()).then_some(&tag.raw[..]);
        self.get_random(uid)
    }

    fn read_memory(&mut self, cancel: &AtomicBool) -> io::Result<Option<Vec<u8>>> {
        self.link.clear_input()?;
        let Some(uid) = self.get_uid(100, cancel)? else { return Ok(None) };
        /* the UID as printed, then 8 blocks of 4 bytes */
        let mut data: Vec<u8> = uid.iter().rev().copied().collect();
        for block in 0..8 {
            match self.read_block(block, 100, cancel)? {
                Some(b) => {
                    applog::debug(format!("[PM3] ReadMemory: #{block}  {}", hex(&b)));
                    data.extend_from_slice(&b);
                }
                None => {
                    applog::debug(format!("[PM3] ReadMemory: Failed to read block {block}, aborting"));
                    return Ok(None);
                }
            }
        }
        Ok(Some(data))
    }

    fn emulate(&mut self, data: &[u8], cancel: &AtomicBool) -> io::Result<()> {
        applog::debug("[PM3] Emulate: Start");
        self.link.write_all(&ng_frame(cmd::ISO15693_SIMULATE, data))?;
        while !cancel.load(Ordering::Relaxed) {
            let response = read_response(self.link.as_mut())?;
            if response.as_ref().is_some_and(|r| r.cmd == cmd::ISO15693_SIMULATE) {
                applog::debug("[PM3] Emulate: Done");
                continue;
            }
            if !self.handle_default(response.as_ref(), "") {
                break;
            }
        }
        Ok(())
    }

    fn measure_antenna(&mut self, cancel: &AtomicBool) -> io::Result<Option<Measurement>> {
        let m = Proxmark3::measure_antenna(self, AntennaType::Both, cancel)?;
        if let Some(m) = &m {
            self.info.antenna_voltage = Some(m.v_hf);
        }
        Ok(m)
    }

    fn console(&mut self) -> io::Result<()> {
        match read_response(self.link.as_mut())? {
            Some(r) if r.cmd == cmd::DEBUG_STRING => applog::info(format!("[PM3] {}", debug_text(&r))),
            Some(r) => {
                self.handle_default(Some(&r), "");
            }
            None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfid::tests::FakeLink;

    fn ng_response(cmd: u16, status: i16, data: &[u8]) -> Vec<u8> {
        let mut buf = MAGIC_RESPONSE_NG.to_le_bytes().to_vec();
        buf.extend_from_slice(&(data.len() as u16 | 0x8000).to_le_bytes());
        buf.extend_from_slice(&status.to_le_bytes());
        buf.extend_from_slice(&cmd.to_le_bytes());
        buf.extend_from_slice(data);
        buf.extend_from_slice(&[0, 0]);
        buf
    }

    #[test]
    fn frames() {
        let f = old_frame(cmd::ISO15693_COMMAND, [3, 1, 1], &[0xAA, 0xBB, 0xCC]);
        assert_eq!(f.len(), 544);
        assert_eq!(&f[..8], &[0x13, 0x03, 0, 0, 0, 0, 0, 0]);
        assert_eq!(f[8], 3);
        assert_eq!(f[16], 1);
        assert_eq!(&f[32..36], &[0xAA, 0xBB, 0xCC, 0]);

        let f = ng_frame(cmd::ISO15693_SIMULATE, &[1, 2, 3]);
        assert_eq!(f, [0x50, 0x4D, 0x33, 0x61, 0x03, 0x80, 0x11, 0x03, 1, 2, 3, 0x61, 0x33]);
    }

    #[test]
    fn reads_both_answer_formats() {
        let mut old = vec![0u8; 544];
        old[0] = cmd::DEVICE_INFO as u8;
        old[8] = 0x2B;
        old[32] = 0x42;
        let mut input = ng_response(cmd::PING, -4, &[9, 8, 7]);
        input.extend_from_slice(&old);
        let mut link = FakeLink::new(input);

        let r = read_response(&mut link).unwrap().unwrap();
        assert_eq!((r.cmd, r.status, r.data), (cmd::PING, -4, vec![9, 8, 7]));
        let r = read_response(&mut link).unwrap().unwrap();
        assert_eq!((r.cmd, r.args[0], r.data.len(), r.data[0]), (cmd::DEVICE_INFO, 0x2B, 512, 0x42));
        /* nothing more */
        assert_eq!(read_response(&mut link).unwrap(), None);

        /* cut off in the middle */
        let mut link = FakeLink::new(ng_response(cmd::PING, 0, &[1, 2, 3, 4])[..12].to_vec());
        assert_eq!(read_response(&mut link).unwrap(), None);
    }

    #[test]
    fn identifies_a_tag() {
        let uid = [0x0A, 0x0B, 0x0C, 0x01, 0x50, 0x03, 0x04, 0xE0];
        let with_crc = |mut a: Vec<u8>| {
            let sum = iso15693::crc(&a);
            a.extend_from_slice(&sum.to_le_bytes());
            a
        };
        let mut input = Vec::new();
        /* random number, then inventory: flags, DSFID, UID */
        input.extend(ng_response(cmd::ISO15693_COMMAND, 0, &with_crc(vec![0, 0x12, 0x34])));
        let mut inventory = vec![0, 0];
        inventory.extend_from_slice(&uid);
        input.extend(ng_response(cmd::DEBUG_STRING, 0, b"\0\0\x1b[32mhello\x1b[0m\0"));
        input.extend(ng_response(cmd::ISO15693_COMMAND, 0, &with_crc(inventory)));

        let link = FakeLink::new(input);
        let sent = link.sent.clone();
        let mut pm3 = Proxmark3 {
            link: Box::new(link),
            info: DeviceInfo {
                kind: ReaderKind::Proxmark3,
                hardware: String::new(),
                port: String::new(),
                unlock_supported: true,
                antenna_voltage: None,
            },
        };
        let tag = pm3.identify().unwrap().unwrap();
        assert_eq!(tag.uid.as_deref(), Some("E0040350010C0B0A"));
        assert_eq!(tag.raw, uid);

        {
            let sent = sent.lock().unwrap();
            assert_eq!(sent.len(), 2 * 544);
            /* the request is in the data of the old frame */
            assert_eq!(&sent[32..35], &[0x02, 0xB2, 0x04]);
            assert_eq!(&sent[544 + 32..544 + 35], &[0x26, 0x01, 0x00]);
        }

        /* the tag is gone: no answer */
        assert!(!pm3.still_there(&tag).unwrap());
    }

    #[test]
    fn strips_color_codes() {
        assert_eq!(strip_ansi("\x1b[1;32mOK\x1b[0m done"), "OK done");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn measurement_values() {
        let mut data = vec![0u8; 28 + 256];
        data[0..4].copy_from_slice(&1000u32.to_le_bytes());
        data[12..16].copy_from_slice(&30500u32.to_le_bytes());
        data[20..24].copy_from_slice(&88u32.to_le_bytes());
        data[28] = 100;
        let m = Measurement::from_response(&Response {
            cmd: cmd::MEASURE_ANTENNA_TUNING,
            status: 0,
            legacy: false,
            args: [0; 3],
            data,
        });
        assert!((m.v_lf134 - 2.0).abs() < 1e-6);
        assert!((m.v_hf - 30.5).abs() < 1e-6);
        assert_eq!(m.hf_rating(), "good");
        assert!((m.peak_frequency_khz() - 134.83).abs() < 0.01);
        let sweep = m.sweep();
        assert_eq!(sweep.len(), 256);
        assert_eq!(sweep[0], (12_000.0, 51.2));
    }
}
