//! ISO 15693 requests for the NXP SLIX-L tags of the Tonies.

pub const INVENTORY: u8 = 0x01;
pub const READ_BLOCK: u8 = 0x20;
pub const NXP_GET_RANDOM_NUMBER: u8 = 0xB2;

const FLAG_DATARATE_HIGH: u8 = 1 << 1;
const FLAG_INVENTORY: u8 = 1 << 2;
/// Without INVENTORY: the request carries a UID
const FLAG_ADDRESS: u8 = 1 << 5;
/// With INVENTORY: one slot only
const FLAG_INV_SLOT1: u8 = 1 << 5;

const MANUFACTURER_NXP: u8 = 0x04;

/// CRC-16 of ISO 15693 (0x8408 reflected, init 0xFFFF, inverted)
pub fn crc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0x8408 } else { crc >> 1 };
        }
    }
    !crc
}

/// Whether the last two bytes are the CRC of the others
pub fn check_crc(frame: &[u8]) -> bool {
    if frame.len() < 2 {
        return false;
    }
    let (data, sum) = frame.split_at(frame.len() - 2);
    crc(data).to_le_bytes() == sum
}

/// A request with the CRC appended. `uid` (least significant byte first, as the tag sends it)
/// addresses a single tag; custom commands (0xA0 and up) get the NXP manufacturer code.
pub fn build_command(command: u8, uid: Option<&[u8]>, payload: &[u8]) -> Vec<u8> {
    let mut flags = FLAG_DATARATE_HIGH;
    if command == INVENTORY {
        flags |= FLAG_INVENTORY | FLAG_INV_SLOT1;
    } else if uid.is_some() {
        flags |= FLAG_ADDRESS;
    }

    let mut buf = vec![flags, command];
    if command >= 0xA0 {
        buf.push(MANUFACTURER_NXP);
    }
    if let (Some(uid), true) = (uid, flags & FLAG_INVENTORY == 0) {
        buf.extend_from_slice(uid);
    }
    buf.extend_from_slice(payload);
    let sum = crc(&buf);
    buf.extend_from_slice(&sum.to_le_bytes());
    buf
}

/// The UID as printed on the tag and used for file names: most significant byte first
pub fn uid_to_string(uid: &[u8]) -> String {
    uid.iter().rev().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard() {
        /* check value of CRC-16/X-25, the same CRC */
        assert_eq!(crc(b"123456789"), 0x906E);
        let frame = build_command(INVENTORY, None, &[0]);
        assert!(check_crc(&frame));
        let mut broken = frame.clone();
        broken[1] ^= 1;
        assert!(!check_crc(&broken));
        assert!(!check_crc(&[0x12]));
    }

    #[test]
    fn builds_requests() {
        /* inventory, one slot, mask length 0 */
        assert_eq!(&build_command(INVENTORY, None, &[0])[..3], &[0x26, 0x01, 0x00]);
        assert_eq!(&build_command(READ_BLOCK, None, &[3])[..3], &[0x02, 0x20, 0x03]);

        let uid = [0x0A, 0x0B, 0x0C, 0x01, 0x50, 0x03, 0x04, 0xE0];
        let req = build_command(NXP_GET_RANDOM_NUMBER, Some(&uid), &[]);
        assert_eq!(&req[..3], &[0x22, 0xB2, 0x04]);
        assert_eq!(&req[3..11], &uid);
        assert_eq!(req.len(), 13);
        assert_eq!(build_command(NXP_GET_RANDOM_NUMBER, None, &[]).len(), 5);

        assert_eq!(uid_to_string(&uid), "E0040350010C0B0A");
    }
}
