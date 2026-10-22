//! Minimal Ogg page handling as used by Tonie files: pages never span packets and
//! are aligned to 4 KiB blocks, so a simple page parser/writer is enough.

pub const PAGE_HEADER_LEN: usize = 27;

pub const FLAG_CONTINUED: u8 = 1;
pub const FLAG_BOS: u8 = 2;
pub const FLAG_EOS: u8 = 4;

const CRC_TABLE: [u32; 256] = make_crc_table();

const fn make_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut s = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            s = (s << 1) ^ if s >= (1 << 31) { 0x04c1_1db7 } else { 0 };
            j += 1;
        }
        table[i] = s;
        i += 1;
    }
    table
}

/// Ogg page checksum (CRC-32, polynomial 0x04c11db7, no reflection, zero init).
#[derive(Default)]
pub struct Crc(u32);

impl Crc {
    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.0 = (self.0 << 8) ^ CRC_TABLE[(b as u32 ^ (self.0 >> 24)) as usize];
        }
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

pub fn crc(data: &[u8]) -> u32 {
    let mut c = Crc::default();
    c.update(data);
    c.0
}

/// One Ogg page; each entry in `segments` is a complete packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page {
    pub version: u8,
    pub header_type: u8,
    pub granule_position: u64,
    pub serial: u32,
    pub sequence: u32,
    pub checksum: u32,
    pub segments: Vec<Vec<u8>>,
}

impl Page {
    /// Parses the page at `offset`. Returns the page and its size in bytes, or `None`
    /// if there is no complete page there.
    pub fn parse(data: &[u8], offset: usize) -> Option<(Page, usize)> {
        let hdr = data.get(offset..offset + PAGE_HEADER_LEN)?;
        if &hdr[0..4] != b"OggS" {
            return None;
        }
        let segment_count = hdr[26] as usize;
        let table = data.get(offset + PAGE_HEADER_LEN..offset + PAGE_HEADER_LEN + segment_count)?;

        let mut segments = Vec::new();
        let mut pos = offset + PAGE_HEADER_LEN + segment_count;
        let mut i = 0;
        while i < segment_count {
            /* lacing values of 255 continue the packet */
            let mut len = table[i] as usize;
            let mut last = table[i];
            i += 1;
            while last == 0xFF && i < segment_count {
                last = table[i];
                len += last as usize;
                i += 1;
            }
            segments.push(data.get(pos..pos + len)?.to_vec());
            pos += len;
        }

        let page = Page {
            version: hdr[4],
            header_type: hdr[5],
            granule_position: u64::from_le_bytes(hdr[6..14].try_into().unwrap()),
            serial: u32::from_le_bytes(hdr[14..18].try_into().unwrap()),
            sequence: u32::from_le_bytes(hdr[18..22].try_into().unwrap()),
            checksum: u32::from_le_bytes(hdr[22..26].try_into().unwrap()),
            segments,
        };
        Some((page, pos - offset))
    }

    pub fn segment_table(&self) -> Vec<u8> {
        let mut table = Vec::new();
        for seg in &self.segments {
            let mut len = seg.len();
            while len >= 0xFF {
                table.push(0xFF);
                len -= 0xFF;
            }
            table.push(len as u8);
        }
        table
    }

    /// Serializes the page with a freshly calculated checksum.
    pub fn to_bytes(&self) -> Vec<u8> {
        let table = self.segment_table();
        let mut out = Vec::with_capacity(PAGE_HEADER_LEN + table.len() + self.segments.iter().map(Vec::len).sum::<usize>());
        out.extend_from_slice(b"OggS");
        out.push(self.version);
        out.push(self.header_type);
        out.extend_from_slice(&self.granule_position.to_le_bytes());
        out.extend_from_slice(&self.serial.to_le_bytes());
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(table.len() as u8);
        out.extend_from_slice(&table);
        for seg in &self.segments {
            out.extend_from_slice(seg);
        }
        let checksum = crc(&out);
        out[22..26].copy_from_slice(&checksum.to_le_bytes());
        out
    }

    /// The first 8 bytes of the first packet, e.g. `OpusHead` or `OpusTags`.
    pub fn packet_magic(&self) -> Option<&[u8]> {
        self.segments.first().and_then(|s| s.get(0..8))
    }

    pub fn is_opus_header(&self) -> bool {
        matches!(self.packet_magic(), Some(b"OpusHead") | Some(b"OpusTags"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_ogg_reference() {
        /* CRC of "OggS" header bytes computed with libogg */
        assert_eq!(crc(b""), 0);
        assert_eq!(crc(b"123456789"), 0x89a1_897f);
    }

    #[test]
    fn page_roundtrip() {
        let page = Page {
            version: 0,
            header_type: FLAG_BOS,
            granule_position: 12345,
            serial: 0x5E03_4216,
            sequence: 3,
            checksum: 0,
            segments: vec![vec![1; 300], vec![2; 255], vec![3; 7]],
        };
        let bytes = page.to_bytes();
        let (parsed, size) = Page::parse(&bytes, 0).unwrap();
        assert_eq!(size, bytes.len());
        assert_eq!(parsed.segments, page.segments);
        assert_eq!(parsed.to_bytes(), bytes);
        assert_eq!(
            parsed.checksum,
            crc(&{
                let mut b = bytes.clone();
                b[22..26].fill(0);
                b
            })
        );
    }
}
