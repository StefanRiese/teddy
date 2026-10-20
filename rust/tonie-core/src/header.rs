//! The protobuf header in the first 4 KiB block of a Tonie file.
//!
//! Layout: a 4 byte big endian length (always 0xFFC), followed by the protobuf message
//! padded so the header fills exactly one 4 KiB block.

use crate::Error;

pub const BLOCK_SIZE: usize = 0x1000;
pub const HEADER_PROTO_SIZE: usize = BLOCK_SIZE - 4;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileHeader {
    /// SHA-1 of the audio data
    pub hash: Vec<u8>,
    pub audio_length: i32,
    pub audio_id: u32,
    /// Page sequence numbers where each track starts
    pub audio_chapters: Vec<u32>,
    pub padding: Vec<u8>,
    /// Only present in sfx.bin (set to zero there). Never written.
    pub usable: Option<bool>,
}

const WIRE_VARINT: u64 = 0;
const WIRE_FIXED64: u64 = 1;
const WIRE_LEN: u64 = 2;
const WIRE_FIXED32: u64 = 5;

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_key(out: &mut Vec<u8>, field: u64, wire: u64) {
    put_varint(out, (field << 3) | wire);
}

fn put_bytes(out: &mut Vec<u8>, field: u64, data: &[u8]) {
    put_key(out, field, WIRE_LEN);
    put_varint(out, data.len() as u64);
    out.extend_from_slice(data);
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn varint(&mut self) -> Result<u64, Error> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = *self.data.get(self.pos).ok_or_else(|| Error::InvalidHeader("truncated varint".into()))?;
            self.pos += 1;
            value |= u64::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Error::InvalidHeader("varint too long".into()))
    }

    fn bytes(&mut self, len: usize) -> Result<&[u8], Error> {
        let data = self
            .data
            .get(self.pos..self.pos + len)
            .ok_or_else(|| Error::InvalidHeader("truncated field".into()))?;
        self.pos += len;
        Ok(data)
    }
}

impl FileHeader {
    pub fn decode(data: &[u8]) -> Result<FileHeader, Error> {
        let mut header = FileHeader::default();
        let mut r = Reader { data, pos: 0 };

        while r.pos < data.len() {
            let key = r.varint()?;
            let (field, wire) = (key >> 3, key & 7);

            match (field, wire) {
                (1, WIRE_LEN) => {
                    let len = r.varint()? as usize;
                    header.hash = r.bytes(len)?.to_vec();
                }
                (2, WIRE_VARINT) => header.audio_length = r.varint()? as i32,
                (3, WIRE_VARINT) => header.audio_id = r.varint()? as u32,
                (4, WIRE_LEN) => {
                    let len = r.varint()? as usize;
                    let mut packed = Reader { data: r.bytes(len)?, pos: 0 };
                    while packed.pos < len {
                        header.audio_chapters.push(packed.varint()? as u32);
                    }
                }
                /* not packed */
                (4, WIRE_VARINT) => header.audio_chapters.push(r.varint()? as u32),
                (5, WIRE_LEN) => {
                    let len = r.varint()? as usize;
                    header.padding = r.bytes(len)?.to_vec();
                }
                (6, WIRE_VARINT) => header.usable = Some(r.varint()? != 0),
                (_, WIRE_VARINT) => {
                    r.varint()?;
                }
                (_, WIRE_LEN) => {
                    let len = r.varint()? as usize;
                    r.bytes(len)?;
                }
                (_, WIRE_FIXED64) => {
                    r.bytes(8)?;
                }
                (_, WIRE_FIXED32) => {
                    r.bytes(4)?;
                }
                _ => return Err(Error::InvalidHeader(format!("unsupported wire type {wire} for field {field}"))),
            }
        }

        Ok(header)
    }

    /// Serializes the header fields in the order the Toniebox expects. `usable` is never written.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_bytes(&mut out, 1, &self.hash);
        put_key(&mut out, 2, WIRE_VARINT);
        /* negative values are sign extended like in the original encoder */
        put_varint(&mut out, self.audio_length as i64 as u64);
        put_key(&mut out, 3, WIRE_VARINT);
        put_varint(&mut out, u64::from(self.audio_id));
        let mut chapters = Vec::new();
        for &c in &self.audio_chapters {
            put_varint(&mut chapters, u64::from(c));
        }
        put_bytes(&mut out, 4, &chapters);
        put_bytes(&mut out, 5, &self.padding);
        out
    }

    /// Sets the padding so the encoded header is exactly [`HEADER_PROTO_SIZE`] bytes and returns
    /// the complete 4 KiB header block including the length prefix.
    pub fn to_block(&mut self) -> Vec<u8> {
        /* first use one byte padding, then determine how many extra bytes to fill */
        self.padding = vec![0; 1];
        let pre = self.encode();
        let padding = HEADER_PROTO_SIZE.saturating_sub(pre.len());
        self.padding = vec![0; padding];
        let data = self.encode();

        let mut block = vec![0u8; BLOCK_SIZE];
        block[0..4].copy_from_slice(&(HEADER_PROTO_SIZE as u32).to_be_bytes());
        let n = data.len().min(HEADER_PROTO_SIZE);
        block[4..4 + n].copy_from_slice(&data[..n]);
        block
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_is_exactly_filled() {
        let mut h = FileHeader {
            hash: vec![0xAB; 20],
            audio_length: 160012,
            audio_id: 0x5E03_4216,
            audio_chapters: vec![0, 15, 32],
            ..Default::default()
        };
        let block = h.to_block();
        assert_eq!(block.len(), BLOCK_SIZE);
        assert_eq!(h.encode().len(), HEADER_PROTO_SIZE);
        assert_eq!(h.padding.len(), 4052);

        let decoded = FileHeader::decode(&block[4..]).unwrap();
        assert_eq!(decoded, h);
    }
}
