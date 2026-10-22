//! Tag UIDs and where their files live on the SD card.
//!
//! A tag with UID `E00403 50 01 0C 0B 0A` (as read by NFC readers) is stored as
//! `CONTENT/0A0B0C01/500304E0`: the byte order is reversed and split after 4 bytes.

use std::path::{Path, PathBuf};

pub const UID_PREFIX: &str = "E00403";
/// What the UID of Tonie tags starts with: only the last 8 digits have to be entered
pub const UID_PREFILL: &str = "E0040350";

/// Reverses the byte order of a hex string, ignoring anything that is not a hex pair.
pub fn reverse_uid(uid: &str) -> String {
    let chars: Vec<char> = uid.chars().collect();
    let mut groups = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i].is_ascii_hexdigit() && chars[i + 1].is_ascii_hexdigit() {
            groups.push(format!("{}{}", chars[i], chars[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }
    groups.reverse();
    groups.concat()
}

/// A valid Tonie tag UID: 16 hex digits starting with E00403.
pub fn is_valid_uid(uid: &str) -> bool {
    uid.len() == 16 && uid.to_ascii_uppercase().starts_with(UID_PREFIX) && uid.chars().all(|c| c.is_ascii_hexdigit())
}

/// The UID of the Tonie file at `path`, from its directory and file name.
pub fn uid_of_file(path: &Path) -> String {
    let dir = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    reverse_uid(&format!("{dir}{name}"))
}

/// Path of the Tonie file for `uid` inside the CONTENT directory.
pub fn file_for_uid(content_dir: &Path, uid: &str) -> PathBuf {
    let rev = reverse_uid(&uid.to_ascii_uppercase());
    content_dir.join(&rev[0..8]).join(&rev[8..16])
}

/// A random UID with the Tonie prefix, for files that should not belong to a real tag.
pub fn random_uid() -> String {
    /* no need for a cryptographic generator, just avoid collisions */
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0) as u64;
    seed ^= (std::process::id() as u64) << 32;
    let mut x = seed | 1;
    let mut digits = String::new();
    for _ in 0..8 {
        /* xorshift */
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        digits.push(char::from_digit((x % 16) as u32, 16).unwrap().to_ascii_uppercase());
    }
    format!("{UID_PREFILL}{digits}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uid_and_path_mapping() {
        assert_eq!(reverse_uid("E0040350010C0B0A"), "0A0B0C01500304E0");
        let p = file_for_uid(Path::new("CONTENT"), "E0040350010C0B0A");
        assert_eq!(p, Path::new("CONTENT").join("0A0B0C01").join("500304E0"));
        assert_eq!(uid_of_file(&p), "E0040350010C0B0A");
        assert!(is_valid_uid("E0040350010C0B0A"));
        assert!(!is_valid_uid("E0040350010C0B0"));
        assert!(!is_valid_uid("A0040350010C0B0A"));
    }

    #[test]
    fn random_uids_are_valid() {
        let a = random_uid();
        assert!(is_valid_uid(&a), "{a}");
        assert!(a.starts_with(UID_PREFILL));
    }
}
