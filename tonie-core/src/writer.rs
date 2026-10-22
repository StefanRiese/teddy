//! Ogg Opus writer that lays out pages the way the Toniebox needs them.
//!
//! Port of the modified Concentus `OpusOggWriteStream` used by the original Teddy:
//! - every page ends exactly at a 4 KiB block boundary (the first block is 0x200 bytes
//!   shorter, the header pages live there)
//! - packets are sized to fit the remaining space of the page and padded if only a few bytes
//!   would be left over
//! - the stream must not end at a block boundary

use std::io::Write;

use crate::header::BLOCK_SIZE;
use crate::ogg::{Crc, FLAG_BOS, FLAG_EOS};
use crate::Error;

pub const SAMPLE_RATE: u32 = 48000;
pub const CHANNELS: usize = 2;
const FRAME_SIZE_MS: usize = 20;
/// Samples per channel in one Opus frame
pub const FRAME_SAMPLES: usize = SAMPLE_RATE as usize * FRAME_SIZE_MS / 1000;

const PAGE_FLAGS_POS: usize = 5;
const GRANULE_POS: usize = 6;
const CHECKSUM_POS: usize = 22;
const SEGMENT_COUNT_POS: usize = 26;

/// Encodes one 20 ms stereo frame into at most `out.len()` bytes.
pub trait FrameEncoder {
    fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> Result<usize, Error>;
}

impl FrameEncoder for opus::Encoder {
    fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> Result<usize, Error> {
        opus::Encoder::encode(self, pcm, out).map_err(|e| Error::Encoder(e.to_string()))
    }
}

/// Creates the Opus encoder used for Tonie files. `bitrate` is in bits per second.
pub fn opus_encoder(bitrate: i32, vbr: bool) -> Result<opus::Encoder, Error> {
    let map = |e: opus::Error| Error::Encoder(e.to_string());
    let mut enc = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Stereo, opus::Application::Audio).map_err(map)?;
    enc.set_bitrate(opus::Bitrate::Bits(bitrate)).map_err(map)?;
    enc.set_vbr(vbr).map_err(map)?;
    Ok(enc)
}

/// Vorbis comments written to the OpusTags packet.
pub struct OpusTags {
    pub vendor: String,
    pub fields: Vec<(String, String)>,
}

impl OpusTags {
    /// The tags of files made by the original Teddy, which also pad the header pages to 0x200 bytes.
    pub fn teddy() -> OpusTags {
        OpusTags {
            vendor: "Lavf56.40.101".into(),
            fields: vec![
                ("encoder".into(), "opusenc from opus-tools 0.1.9".into()),
                ("encoder_options".into(), "--quiet --bitrate 96 --vbr".into()),
                ("pad".into(), "0".repeat(0x138)),
            ],
        }
    }
}

pub struct TonieOggWriter<E: FrameEncoder, W: Write> {
    encoder: E,
    out: W,
    /// Bytes written to `out`
    position: u64,
    frame: Vec<i16>,
    frame_index: usize,
    header: [u8; 27 + 255],
    header_index: usize,
    payload: Vec<u8>,
    payload_index: usize,
    lacing_count: u8,
    page_counter: u32,
    serial: u32,
    granule: u64,
    /// Start and length of each packet in the payload of the current page
    packets: Vec<(usize, usize)>,
    finished: bool,
    /// Messages for the user, collected until [`TonieOggWriter::take_warnings`] is called
    warnings: Vec<String>,
}

impl<E: FrameEncoder, W: Write> TonieOggWriter<E, W> {
    pub fn new(encoder: E, out: W, tags: &OpusTags, serial: u32) -> Result<Self, Error> {
        let mut w = TonieOggWriter {
            encoder,
            out,
            position: 0,
            frame: vec![0; FRAME_SAMPLES * CHANNELS],
            frame_index: 0,
            header: [0; 27 + 255],
            header_index: 0,
            payload: vec![0; BLOCK_SIZE],
            payload_index: 0,
            lacing_count: 0,
            page_counter: 0,
            serial,
            granule: 0,
            packets: Vec::new(),
            finished: false,
            warnings: Vec::new(),
        };
        w.begin_new_page();
        w.write_opus_head()?;
        w.write_opus_tags(tags)?;
        Ok(w)
    }

    /// Number of the page that will be started next, used for the chapter table.
    pub fn page_counter(&self) -> u32 {
        self.page_counter
    }

    pub fn serial(&self) -> u32 {
        self.serial
    }

    /// Bytes written so far
    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    /// Writes interleaved stereo samples at 48 kHz.
    pub fn write_samples(&mut self, mut data: &[i16]) -> Result<(), Error> {
        if self.finished {
            return Err(Error::Encoder("stream already finished".into()));
        }

        while !data.is_empty() {
            let n = (self.frame.len() - self.frame_index).min(data.len());
            self.frame[self.frame_index..self.frame_index + n].copy_from_slice(&data[..n]);
            self.frame_index += n;
            data = &data[n..];

            if self.frame_index == self.frame.len() {
                self.encode_frame()?;
                self.frame_index = 0;
            }
        }
        Ok(())
    }

    /// The size the current page has to reach, and the largest packet that still fits into it.
    fn space(&self) -> (i32, i32) {
        let max_block_size: i32 = if self.position < BLOCK_SIZE as u64 {
            (BLOCK_SIZE - 0x200) as i32
        } else {
            BLOCK_SIZE as i32
        };

        let page_free = max_block_size - self.payload_index as i32 - self.header_index as i32;
        let mut max_payload = (page_free / 256) * 255 + (page_free % 256) - 1;
        let reconstructed = (max_payload / 255) + 1 + max_payload;

        /* when due to segment sizes we would end up with a 1 byte gap, make sure that the next run will have at least 64 byte.
         * reason why this could happen is that "adding one byte" would require one segment more and thus occupies two byte more.
         * if this would happen, just reduce the calculated free space such that there is room for another segment.
         */
        if page_free != reconstructed && max_payload > 64 {
            max_payload -= 64;
        }
        let lacing_space = (255 - i32::from(self.lacing_count)) * 255;
        (max_block_size, lacing_space.min(max_payload))
    }

    fn encode_frame(&mut self) -> Result<(), Error> {
        let (max_block_size, mut max_opus_data) = self.space();
        if max_opus_data <= 0 {
            return Err(Error::Encoder(format!("no space left in Ogg page at offset 0x{:08X}", self.position)));
        }

        let start = self.payload_index;
        let mut packet_size = self.encoder.encode(&self.frame, &mut self.payload[start..start + max_opus_data as usize])?;
        max_opus_data -= packet_size as i32;

        /* pad to full remaining size if we would leave just a few bytes behind. picked a random number that seemed enough for opus */
        if self.header[PAGE_FLAGS_POS] != FLAG_EOS {
            if max_opus_data > 0 && max_opus_data < 43 {
                let new_size = packet_size + max_opus_data as usize;
                match opus::packet::pad(&mut self.payload[start..start + new_size], packet_size) {
                    Ok(_) => packet_size = new_size,
                    Err(_) => {
                        return Err(Error::Padding(format!(
                            "Failed to pad opus data properly at offset 0x{:08X}. Please try CBR with bitrates a multiple of 24 kbps",
                            self.position
                        )))
                    }
                }
            }
        } else if max_opus_data == 0 {
            self.warnings.push("Hit block boundary with last frame. Writing another one.".into());
            self.header[PAGE_FLAGS_POS] = 0;
        }

        self.payload_index += packet_size;
        self.packets.push((start, packet_size));

        /* Opus granules are measured in 48 kHz samples, the frame size is fixed */
        self.granule += FRAME_SAMPLES as u64;

        let mut segment_length = packet_size;
        while segment_length >= 255 {
            segment_length -= 255;
            self.push_lacing(0xFF);
        }
        self.push_lacing(segment_length as u8);

        /* check if there would be enough space for lacing and payload each */
        let remain = max_block_size - self.payload_index as i32 - self.header_index as i32;
        if self.lacing_count >= 248 || remain < 16 {
            if remain > 0 {
                return Err(Error::Encoder(format!(
                    "Ogg page with just a few bytes remaining at offset 0x{:08X}. Weird bit rate chosen?",
                    self.position
                )));
            }
            self.finalize_page()?;
        }
        Ok(())
    }

    /// Appends an already encoded Opus packet, e.g. copied from another Tonie file. A frame that
    /// was started by [`TonieOggWriter::write_samples`] is completed with silence first.
    pub fn write_packet(&mut self, packet: &[u8]) -> Result<(), Error> {
        if self.finished {
            return Err(Error::Encoder("stream already finished".into()));
        }
        self.complete_frame()?;
        let samples = opus::packet::get_nb_samples(packet, SAMPLE_RATE).map_err(|e| Error::Encoder(format!("invalid Opus packet: {e}")))?;

        if self.space().1 < packet.len() as i32 {
            self.close_page()?;
        }
        if self.space().1 < packet.len() as i32 {
            return Err(Error::Encoder(format!("Opus packet of {} bytes does not fit into a page", packet.len())));
        }
        let start = self.payload_index;
        self.payload[start..start + packet.len()].copy_from_slice(packet);
        self.payload_index += packet.len();
        self.packets.push((start, packet.len()));
        let mut segment_length = packet.len();
        while segment_length >= 255 {
            segment_length -= 255;
            self.push_lacing(0xFF);
        }
        self.push_lacing(segment_length as u8);
        self.granule += samples as u64;

        let (max_block_size, _) = self.space();
        let remain = max_block_size - self.payload_index as i32 - self.header_index as i32;
        if self.lacing_count >= 248 || remain < 64 {
            self.close_page()?;
        }
        Ok(())
    }

    /// Encodes a frame that was started by [`TonieOggWriter::write_samples`], completed with silence.
    pub fn complete_frame(&mut self) -> Result<(), Error> {
        if self.frame_index > 0 {
            let missing = self.frame.len() - self.frame_index;
            self.write_samples(&vec![0; missing])?;
        }
        Ok(())
    }

    /// Ends the current page exactly at the block boundary by padding its last packets, so the
    /// next packet starts a new page.
    pub fn close_page(&mut self) -> Result<(), Error> {
        if self.lacing_count == 0 {
            return Ok(());
        }
        let (target, _) = self.space();
        let missing = target - (self.header_index + self.payload_index) as i32;
        if missing > 0 {
            self.pad_page(missing as usize)?;
        }
        self.finalize_page()
    }

    /// Grows the page by `missing` bytes, padding the last one or two packets. A packet growing past
    /// a multiple of 255 bytes needs another lacing value, so not every size can be reached with one.
    fn pad_page(&mut self, missing: usize) -> Result<(), Error> {
        let mut packets: Vec<Vec<u8>> = self.packets.iter().map(|&(s, l)| self.payload[s..s + l].to_vec()).collect();
        let n = packets.len();
        let lacing = |len: usize| len / 255 + 1;
        let growth = |len: usize, x: usize| x + lacing(len + x) - lacing(len);
        let padded = |p: &[u8], x: usize| -> Option<Vec<u8>> {
            if x == 0 {
                return Some(p.to_vec());
            }
            let mut buf = p.to_vec();
            buf.resize(p.len() + x, 0);
            opus::packet::pad(&mut buf, p.len()).ok().map(|_| buf)
        };

        let mut found = None;
        'search: for x2 in 0..=missing {
            let g2 = match x2 {
                0 => 0,
                _ if n < 2 => break,
                _ => growth(packets[n - 2].len(), x2),
            };
            if g2 > missing {
                break;
            }
            let rest = missing - g2;
            let last = packets[n - 1].len();
            for x1 in rest.saturating_sub(32)..=rest {
                if growth(last, x1) != rest {
                    continue;
                }
                let second = if x2 > 0 { padded(&packets[n - 2], x2) } else { None };
                if x2 > 0 && second.is_none() {
                    continue;
                }
                if let Some(first) = padded(&packets[n - 1], x1) {
                    found = Some((first, second));
                    break 'search;
                }
            }
        }
        let Some((last, second)) = found else {
            return Err(Error::Padding(format!("Failed to pad the Ogg page at offset 0x{:08X}", self.position)));
        };
        packets[n - 1] = last;
        if let Some(second) = second {
            packets[n - 2] = second;
        }

        /* lay out the page again */
        self.header_index = 27;
        self.payload_index = 0;
        self.lacing_count = 0;
        self.packets.clear();
        for p in &packets {
            let start = self.payload_index;
            self.payload[start..start + p.len()].copy_from_slice(p);
            self.payload_index += p.len();
            self.packets.push((start, p.len()));
            let mut segment_length = p.len();
            while segment_length >= 255 {
                segment_length -= 255;
                self.push_lacing(0xFF);
            }
            self.push_lacing(segment_length as u8);
        }
        Ok(())
    }

    /// Number of the page being filled
    pub fn current_page(&self) -> u32 {
        self.page_counter - 1
    }

    fn push_lacing(&mut self, value: u8) {
        self.header[self.header_index] = value;
        self.header_index += 1;
        self.lacing_count += 1;
    }

    /// Writes silence to complete the last frame and closes the stream.
    /// Returns the output and the warnings not yet taken.
    pub fn finish(mut self) -> Result<(W, Vec<String>), Error> {
        self.header[PAGE_FLAGS_POS] = FLAG_EOS;
        let missing = self.frame.len() - self.frame_index;
        self.write_samples(&vec![0; missing])?;
        self.finalize_page()?;

        /* but the *file* must not end at 0x1000 boundary. this needs some quirk here. write another page. */
        if self.position.is_multiple_of(BLOCK_SIZE as u64) {
            self.header[PAGE_FLAGS_POS] = FLAG_EOS;
            self.write_samples(&vec![0; self.frame.len()])?;
            self.finalize_page()?;
        }

        self.finished = true;
        self.out.flush()?;
        Ok((self.out, self.warnings))
    }

    fn write_opus_head(&mut self) -> Result<(), Error> {
        let mut p = Vec::with_capacity(19);
        p.extend_from_slice(b"OpusHead");
        p.push(1); // version
        p.push(CHANNELS as u8);
        p.extend_from_slice(&356u16.to_le_bytes()); // pre-skip
        p.extend_from_slice(&44100u32.to_le_bytes()); // input sample rate, as in the original files
        p.extend_from_slice(&0i16.to_le_bytes()); // output gain
        p.push(0); // channel mapping family

        self.payload[..p.len()].copy_from_slice(&p);
        self.payload_index = p.len();
        self.push_lacing(p.len() as u8);
        self.header[PAGE_FLAGS_POS] = FLAG_BOS;
        self.finalize_page()
    }

    fn write_opus_tags(&mut self, tags: &OpusTags) -> Result<(), Error> {
        let mut p = Vec::new();
        p.extend_from_slice(b"OpusTags");
        p.extend_from_slice(&(tags.vendor.len() as u32).to_le_bytes());
        p.extend_from_slice(tags.vendor.as_bytes());
        let fields: Vec<String> = tags
            .fields
            .iter()
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        p.extend_from_slice(&(fields.len() as u32).to_le_bytes());
        for f in &fields {
            p.extend_from_slice(&(f.len() as u32).to_le_bytes());
            p.extend_from_slice(f.as_bytes());
        }
        if p.len() > self.payload.len() {
            return Err(Error::Encoder("Opus tags too long".into()));
        }

        self.payload[..p.len()].copy_from_slice(&p);
        self.payload_index = p.len();
        let mut size = p.len();
        while size >= 255 {
            self.push_lacing(255);
            size -= 255;
        }
        self.push_lacing(size as u8);
        self.finalize_page()
    }

    fn begin_new_page(&mut self) {
        self.header_index = 0;
        self.payload_index = 0;
        self.lacing_count = 0;
        self.packets.clear();

        let h = &mut self.header;
        h[0..4].copy_from_slice(b"OggS");
        h[4] = 0; // version
        h[PAGE_FLAGS_POS] = 0;
        h[GRANULE_POS..GRANULE_POS + 8].copy_from_slice(&self.granule.to_le_bytes());
        h[14..18].copy_from_slice(&self.serial.to_le_bytes());
        h[18..22].copy_from_slice(&self.page_counter.to_le_bytes());
        h[CHECKSUM_POS..CHECKSUM_POS + 4].fill(0);
        h[SEGMENT_COUNT_POS] = 0;
        self.header_index = 27;

        self.page_counter += 1;
    }

    fn finalize_page(&mut self) -> Result<(), Error> {
        if self.lacing_count == 0 {
            return Ok(());
        }

        self.header[SEGMENT_COUNT_POS] = self.lacing_count;
        self.header[GRANULE_POS..GRANULE_POS + 8].copy_from_slice(&self.granule.to_le_bytes());
        let mut crc = Crc::default();
        crc.update(&self.header[..self.header_index]);
        crc.update(&self.payload[..self.payload_index]);
        self.header[CHECKSUM_POS..CHECKSUM_POS + 4].copy_from_slice(&crc.value().to_le_bytes());

        self.out.write_all(&self.header[..self.header_index])?;
        self.out.write_all(&self.payload[..self.payload_index])?;
        self.position += (self.header_index + self.payload_index) as u64;

        /* pages must end at 4k boundary */
        if self.header[PAGE_FLAGS_POS] != FLAG_EOS && self.position >= BLOCK_SIZE as u64 && !self.position.is_multiple_of(BLOCK_SIZE as u64) {
            self.warnings
                .push(format!("Ogg page does not end at a 4k boundary, offset 0x{:08X}", self.position));
        }

        self.begin_new_page();
        Ok(())
    }
}
