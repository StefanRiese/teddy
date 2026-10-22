//! Reading Tonie files and splitting them into Ogg files.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

use crate::header::{FileHeader, BLOCK_SIZE};
use crate::ogg::{Page, FLAG_EOS, PAGE_HEADER_LEN};
use crate::Error;

/// Page statistics of the audio stream, as shown by `teddy -m info`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Statistics {
    pub total_segments: u64,
    pub segment_length: u64,
    pub min_segments: u32,
    pub max_segments: u32,
    pub min_granule: u64,
    pub max_granule: u64,
    pub highest_granule: u64,
    /// Problems found while walking the pages
    pub errors: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TonieFile {
    pub path: PathBuf,
    pub header: FileHeader,
    /// Audio data (everything after the header block). Empty if read without audio.
    pub audio: Vec<u8>,
    /// Length of the header when encoded again
    pub header_length: usize,
    pub hash_correct: bool,
    pub statistics: Statistics,
    /// Granule position of each page, in the order the pages appear
    page_granules: Vec<(u32, u64)>,
}

impl TonieFile {
    /// Reads a Tonie file. With `read_audio == false` only the header block is kept in memory,
    /// the page statistics are still calculated from the file.
    pub fn open(path: impl AsRef<Path>, read_audio: bool) -> Result<TonieFile, Error> {
        let path = path.as_ref();
        let mut file = File::open(path)?;
        let len = file.metadata()?.len();
        if len < 2 * BLOCK_SIZE as u64 {
            return Err(Error::InvalidFile("file too short".into()));
        }

        let mut content = vec![0u8; if read_audio { len as usize } else { BLOCK_SIZE }];
        file.read_exact(&mut content)?;

        let mut tonie = TonieFile::parse(path, &content)?;
        let (stats, granules) = calculate_statistics(&mut file, len)?;
        tonie.statistics = stats;
        tonie.page_granules = granules;
        Ok(tonie)
    }

    fn parse(path: &Path, content: &[u8]) -> Result<TonieFile, Error> {
        let proto_len = u32::from_be_bytes(content[0..4].try_into().unwrap()) as usize;
        if proto_len > 0x10000 || 4 + proto_len > content.len() {
            return Err(Error::InvalidFile(format!("invalid header length 0x{proto_len:X}")));
        }

        let header = FileHeader::decode(&content[4..4 + proto_len])?;
        let audio = content[4 + proto_len..].to_vec();
        let hash_correct = Sha1::digest(&audio).as_slice() == header.hash.as_slice();
        let header_length = header.encode().len();

        Ok(TonieFile {
            path: path.to_path_buf(),
            header,
            audio,
            header_length,
            hash_correct,
            statistics: Statistics::default(),
            page_granules: Vec::new(),
        })
    }

    pub fn hash_string(&self) -> String {
        hex_upper(&self.header.hash)
    }

    /// Granule position of the page with the given sequence number.
    pub fn granule_by_page(&self, page: u32) -> Option<u64> {
        self.page_granules.iter().find(|(p, _)| *p == page).map(|(_, g)| *g)
    }

    /// Sequence number of the last page.
    pub fn highest_page(&self) -> u32 {
        self.page_granules.last().map(|(p, _)| *p).unwrap_or(0)
    }

    /// The complete file with another audio ID, which is also the serial number of the Ogg stream.
    /// The audio is not re-encoded: the serial of every page is replaced and its CRC recalculated,
    /// so the pages keep their size and 4 KiB alignment. Needs the audio (`open(.., true)`).
    pub fn with_audio_id(&self, audio_id: u32) -> Result<Vec<u8>, Error> {
        let mut audio = self.audio.clone();
        if audio.is_empty() {
            return Err(Error::InvalidFile("the audio was not read".into()));
        }
        let mut offset = 0;
        while offset < audio.len() {
            let Some((_, size)) = Page::parse(&audio, offset) else {
                return Err(Error::InvalidFile(format!("no Ogg page at offset 0x{:X}", BLOCK_SIZE + offset)));
            };
            let page = &mut audio[offset..offset + size];
            page[14..18].copy_from_slice(&audio_id.to_le_bytes());
            page[22..26].fill(0);
            let checksum = crate::ogg::crc(page);
            page[22..26].copy_from_slice(&checksum.to_le_bytes());
            offset += size;
        }

        let mut header = self.header.clone();
        header.audio_id = audio_id;
        header.hash = Sha1::digest(&audio).to_vec();
        let mut out = header.to_block();
        out.extend_from_slice(&audio);
        Ok(out)
    }

    /// The OpusHead/OpusTags pages at the start of the audio.
    fn opus_header_pages(&self) -> Result<(Vec<Page>, usize), Error> {
        let mut pages = Vec::new();
        let mut offset = 0;
        while let Some((page, size)) = Page::parse(&self.audio, offset) {
            if page.segments.is_empty() || !page.is_opus_header() {
                break;
            }
            pages.push(page);
            offset += size;
        }
        Ok((pages, offset))
    }

    /// Writes the audio as `<name>.ogg` (`single`), or one Ogg file per track named
    /// `Track_01.ogg` or `<name> - Track_01.ogg`. `tags` are added to every file,
    /// `titles[i]` becomes the TITLE of track i.
    pub fn dump_audio_files(&self, out_dir: &Path, name: &str, single: bool, tags: &[String], titles: Option<&[String]>) -> Result<Vec<PathBuf>, Error> {
        if self.audio.is_empty() {
            return Err(Error::InvalidFile("file was read without audio".into()));
        }

        let (mut meta_pages, _) = self.opus_header_pages()?;
        add_tags(&mut meta_pages, tags);

        if single {
            let out = out_dir.join(format!("{name}.ogg"));
            std::fs::write(&out, &self.audio)?;
            return Ok(vec![out]);
        }

        let chapters = &self.header.audio_chapters;
        let mut written = Vec::new();
        for chapter in 0..chapters.len() {
            let file_name = if name.is_empty() {
                format!("Track_{:02}.ogg", chapter + 1)
            } else {
                format!("{name} - Track_{:02}.ogg", chapter + 1)
            };
            let out_path = out_dir.join(file_name);
            let mut out = Vec::new();

            let mut track_meta = meta_pages.clone();
            if let Some(title) = titles.and_then(|t| t.get(chapter)) {
                add_tags(&mut track_meta, &[format!("TITLE={title}")]);
            }
            for page in &track_meta {
                out.extend_from_slice(&page.to_bytes());
            }

            /* page n lives in audio block n - 2: the first block holds the two header pages and page 2 */
            let block_offset = |page: u32| (BLOCK_SIZE as i64 * (page as i64 - 2)).max(0) as usize;
            let mut offset = block_offset(chapters[chapter]);
            let end_offset = chapters.get(chapter + 1).map(|&c| block_offset(c)).unwrap_or(usize::MAX);

            let mut granule_start = None;
            while let Some((mut page, size)) = Page::parse(&self.audio, offset) {
                offset += size;
                let done = offset >= end_offset || offset >= self.audio.len();
                if done {
                    page.header_type = FLAG_EOS;
                }

                /* the header pages were already written, they only appear in the first track */
                if !page.is_opus_header() {
                    /* granule positions and page numbers relative to the track start */
                    let (g0, p0) = *granule_start.get_or_insert((page.granule_position, page.sequence));
                    page.granule_position = page.granule_position.wrapping_sub(g0);
                    page.sequence = page.sequence.wrapping_sub(p0).wrapping_add(2);
                    out.extend_from_slice(&page.to_bytes());
                }

                if done {
                    break;
                }
            }

            std::fs::write(&out_path, out)?;
            written.push(out_path);
        }
        Ok(written)
    }
}

/// Walks all pages of the audio and collects statistics, checking that no page crosses a 4 KiB block.
fn calculate_statistics<R: Read + Seek>(file: &mut R, len: u64) -> Result<(Statistics, Vec<(u32, u64)>), Error> {
    let mut stats = Statistics {
        min_segments: 0xFF,
        min_granule: i64::MAX as u64,
        ..Default::default()
    };
    let mut granules: Vec<(u32, u64)> = Vec::new();
    let mut cur_pos = BLOCK_SIZE as u64;
    let mut last_granule = 0u64;
    let mut hdr = [0u8; PAGE_HEADER_LEN];

    while cur_pos < len {
        let last_pos = cur_pos;
        file.seek(SeekFrom::Start(cur_pos))?;
        if file.read_exact(&mut hdr).is_err() || &hdr[0..3] != b"Ogg" {
            stats.errors.push(format!("Not an Ogg page header at 0x{cur_pos:08X}"));
            break;
        }

        let granule = u64::from_le_bytes(hdr[6..14].try_into().unwrap());
        let page_num = u32::from_le_bytes(hdr[18..22].try_into().unwrap());
        let segment_count = hdr[26];
        if !granules.iter().any(|(p, _)| *p == page_num) {
            granules.push((page_num, granule));
        }

        let mut lengths = vec![0u8; segment_count as usize];
        file.read_exact(&mut lengths)?;
        cur_pos += (PAGE_HEADER_LEN + lengths.len()) as u64;
        for &l in &lengths {
            stats.total_segments += 1;
            stats.segment_length += u64::from(l);
            cur_pos += u64::from(l);
        }

        let last_offset = last_pos % BLOCK_SIZE as u64;
        let cur_offset = cur_pos % BLOCK_SIZE as u64;
        if last_offset >= cur_offset && cur_offset != 0 {
            stats.errors.push(format!("Ogg page ends in next block at 0x{cur_pos:08X}"));
            break;
        }

        if last_pos >= 2 * BLOCK_SIZE as u64 && cur_pos < len {
            if last_granule > granule {
                stats.errors.push(format!("granule at 0x{cur_pos:08X} is lower than last."));
                break;
            }
            let delta = granule - last_granule;
            stats.min_segments = stats.min_segments.min(u32::from(segment_count));
            stats.max_segments = stats.max_segments.max(u32::from(segment_count));
            stats.min_granule = stats.min_granule.min(delta);
            stats.max_granule = stats.max_granule.max(delta);
        }

        stats.highest_granule = stats.highest_granule.max(granule);
        last_granule = granule;
    }

    Ok((stats, granules))
}

/// Appends Vorbis comments to the OpusTags packet.
fn add_tags(pages: &mut [Page], tags: &[String]) {
    for page in pages.iter_mut().filter(|p| p.packet_magic() == Some(b"OpusTags")) {
        let packet = &mut page.segments[0];
        let vendor_len = u32::from_le_bytes(packet[8..12].try_into().unwrap()) as usize;
        let count_pos = 8 + 4 + vendor_len;
        for tag in tags {
            packet.extend_from_slice(&(tag.len() as u32).to_le_bytes());
            packet.extend_from_slice(tag.as_bytes());
            let count = u32::from_le_bytes(packet[count_pos..count_pos + 4].try_into().unwrap());
            packet[count_pos..count_pos + 4].copy_from_slice(&(count + 1).to_le_bytes());
        }
    }
}

pub fn hex_upper(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_the_audio_id_without_reencoding() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/reference/album_cbr96/0A0B0C01/500304E0");
        let original = TonieFile::open(&path, true).unwrap();
        let bytes = original.with_audio_id(0x1234_5678).unwrap();
        assert_eq!(bytes.len(), std::fs::metadata(&path).unwrap().len() as usize);

        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("500304E0");
        std::fs::write(&out, &bytes).unwrap();
        let changed = TonieFile::open(&out, true).unwrap();
        assert!(changed.hash_correct);
        assert_eq!(changed.header.audio_id, 0x1234_5678);
        assert_eq!(changed.header.audio_chapters, original.header.audio_chapters);
        assert_eq!(changed.header.audio_length, original.header.audio_length);
        assert_ne!(changed.header.hash, original.header.hash);
        assert!(changed.statistics.errors.is_empty(), "{:?}", changed.statistics.errors);
        assert_eq!(changed.statistics.highest_granule, original.statistics.highest_granule);

        /* every page: new serial, valid CRC, same content */
        let mut offset = 0;
        let mut pages = 0;
        while let Some((page, size)) = Page::parse(&changed.audio, offset) {
            let (old, _) = Page::parse(&original.audio, offset).unwrap();
            assert_eq!(page.serial, 0x1234_5678);
            assert_eq!(page.to_bytes(), changed.audio[offset..offset + size], "CRC of page {}", page.sequence);
            assert_eq!(
                (page.segments, page.granule_position, page.sequence),
                (old.segments, old.granule_position, old.sequence)
            );
            offset += size;
            pages += 1;
        }
        assert_eq!(offset, changed.audio.len());
        assert!(pages > 3);
    }
}
