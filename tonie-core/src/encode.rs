//! Creating Tonie files from audio files.

use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use id3::TagLike;
use sha1::{Digest, Sha1};

use crate::audio_in::{self, decode_file};
use crate::header::{FileHeader, BLOCK_SIZE};
use crate::writer::{opus_encoder, FrameEncoder, OpusTags, TonieOggWriter, CHANNELS};
use crate::Error;

/// Files close to this size are not accepted by the Toniebox
const MAX_SIZE: u64 = 0x7735_9400;
/// Samples are passed to the encoder in chunks of this many frames; chunks of pure digital silence are dropped.
const CHUNK_FRAMES: usize = 2880;

#[derive(Clone, Debug)]
pub struct EncodeOptions {
    /// Audio ID and Ogg stream serial. 0 uses the current time.
    pub audio_id: u32,
    /// Opus bit rate in bits per second
    pub bitrate: i32,
    pub vbr: bool,
    /// Directory with `0001.mp3`, `0002.mp3`, ... that get prepended to each track
    pub prefix_dir: Option<PathBuf>,
    /// Copy Ogg Opus files that fit a Tonie (stereo, 20 ms packets) without re-encoding them,
    /// e.g. tracks exported from a Tonie. Not with prefix files.
    pub copy_opus: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            audio_id: 0,
            bitrate: 96000,
            vbr: false,
            prefix_dir: None,
            copy_opus: true,
        }
    }
}

/// Progress reports while encoding. All methods do nothing by default.
pub trait EncodeProgress {
    fn file_start(&mut self, _track: usize, _path: &Path) {}
    /// Fraction of the current file, reported whenever it advances by 5 %
    fn progress(&mut self, _fraction: f64) {}
    fn file_done(&mut self) {}
    fn failed(&mut self, _message: &str) {}
    fn warning(&mut self, _message: &str) {}
    /// Informational messages, e.g. about the track order
    fn info(&mut self, _message: &str) {}
}

impl EncodeProgress for () {}

/// Expands directories into their audio files. Files in a directory are sorted by their
/// ID3 track number if every file has an ID3 tag, otherwise by file name.
pub fn build_file_list(sources: &[PathBuf], progress: &mut dyn EncodeProgress) -> Result<Vec<PathBuf>, Error> {
    let mut list = Vec::new();

    for source in sources {
        let item = PathBuf::from(source.to_string_lossy().trim_matches('"'));

        if item.is_dir() {
            let mut files: Vec<PathBuf> = std::fs::read_dir(&item)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && audio_in::is_supported(p))
                .collect();
            files.sort_by(|a, b| compare_file_names(a, b));

            let tracks: Option<Vec<u32>> = files.iter().map(|f| id3::Tag::read_from_path(f).ok().map(|t| t.track().unwrap_or(0))).collect();
            match tracks {
                Some(tracks) => {
                    let mut sorted: Vec<(u32, PathBuf)> = tracks.into_iter().zip(files).collect();
                    /* stable sort keeps the file name order for equal track numbers */
                    sorted.sort_by_key(|(t, _)| *t);
                    list.extend(sorted.into_iter().map(|(_, f)| f));
                }
                None => {
                    progress.info("Tried to sort using MP3 tag for track number, but not all files in");
                    progress.info("this folder have valid ID3 tags. ");
                    progress.info("Please make sure all files have correct ID3 fields to have them sorted correctly.");
                    progress.info("");
                    progress.info("Sorting files by their filename.");
                    list.extend(files);
                }
            }
        } else if item.is_file() {
            if !audio_in::is_supported(&item) {
                return Err(Error::InvalidFile(format!(
                    "Specified item '{}' is no supported audio file ({})",
                    item.display(),
                    audio_in::SUPPORTED_EXTENSIONS.join(", ")
                )));
            }
            list.push(item);
        } else {
            return Err(Error::NotFound(format!("Specified item '{}' not found or supported", item.display())));
        }
    }
    Ok(list)
}

/// Case-insensitive file name order, close to what Windows Explorer shows.
fn compare_file_names(a: &Path, b: &Path) -> std::cmp::Ordering {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let (a, b) = (name(a), name(b));
    a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(&b))
}

/// The title from the ID3 tag, or the file name.
pub fn display_name(path: &Path) -> String {
    id3::Tag::read_from_path(path)
        .ok()
        .and_then(|t| t.title().map(str::to_string))
        .unwrap_or_else(|| path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())
}

/// Counts and hashes everything written to the audio part of the file.
struct HashingWriter<'a, W: Write> {
    inner: &'a mut W,
    sha1: Sha1,
    len: u64,
}

impl<W: Write> Write for HashingWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.sha1.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Encodes the given audio files (see [`build_file_list`]) into a Tonie file written to `out`,
/// one chapter per file. Returns the header that was written.
pub fn encode<W: Write + Seek>(files: &[PathBuf], options: &EncodeOptions, out: &mut W, progress: &mut dyn EncodeProgress) -> Result<FileHeader, Error> {
    let encoder = opus_encoder(options.bitrate, options.vbr)?;
    encode_with(encoder, files, options, out, progress)
}

pub fn encode_with<E: FrameEncoder, W: Write + Seek>(
    encoder: E,
    files: &[PathBuf],
    options: &EncodeOptions,
    out: &mut W,
    progress: &mut dyn EncodeProgress,
) -> Result<FileHeader, Error> {
    let tracks: Vec<TrackSource> = files.iter().cloned().map(TrackSource::File).collect();
    build_with(encoder, &tracks, options, out, progress)
}

/// A track of a Tonie file being built
#[derive(Clone, Debug)]
pub enum TrackSource {
    /// An audio file to encode
    File(PathBuf),
    /// Opus packets copied from a Tonie file (see [`crate::TonieFile::track_packets`]), not re-encoded.
    /// `label` is shown in the progress.
    Packets { label: String, packets: Vec<Vec<u8>> },
}

/// Builds a Tonie file from encoded audio files and tracks copied from other Tonie files, e.g. to
/// add, remove or reorder the tracks of a Tonie without re-encoding the tracks it keeps. A copied
/// track starts on a new page, so its chapter starts exactly where the track does.
pub fn build<W: Write + Seek>(tracks: &[TrackSource], options: &EncodeOptions, out: &mut W, progress: &mut dyn EncodeProgress) -> Result<FileHeader, Error> {
    let encoder = opus_encoder(options.bitrate, options.vbr)?;
    build_with(encoder, tracks, options, out, progress)
}

fn build_with<E: FrameEncoder, W: Write + Seek>(
    encoder: E,
    tracks: &[TrackSource],
    options: &EncodeOptions,
    out: &mut W,
    progress: &mut dyn EncodeProgress,
) -> Result<FileHeader, Error> {
    let audio_id = if options.audio_id == 0 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(1)
    } else {
        options.audio_id
    };

    /* the header block is written at the end, once the hash is known */
    out.seek(SeekFrom::Start(0))?;
    out.write_all(&[0u8; BLOCK_SIZE])?;

    let mut hashing = HashingWriter {
        inner: out,
        sha1: Sha1::new(),
        len: 0,
    };
    let mut writer = TonieOggWriter::new(encoder, &mut hashing, &OpusTags::teddy(), audio_id)?;

    let mut chapters = Vec::new();
    let mut last_index = 0u32;
    let mut previous_copied = false;
    let mut warned = false;

    for (index, source) in tracks.iter().enumerate() {
        let track = index + 1;
        if writer.position() + BLOCK_SIZE as u64 >= MAX_SIZE {
            progress.warning("Close to 2 GiB, stopping");
            break;
        }

        let after_copy = previous_copied;
        let opus = match source {
            TrackSource::File(file) if options.copy_opus && options.prefix_dir.is_none() => audio_in::tonie_ready_opus_packets(file)?,
            _ => None,
        };
        previous_copied = opus.is_some() || matches!(source, TrackSource::Packets { .. });
        let result = match source {
            TrackSource::File(file) if opus.is_some() => {
                progress.file_start(track, file);
                progress.info(&format!("{}: Ogg Opus, copied without re-encoding", display_name(file)));
                copy_track(&mut writer, opus.as_deref().unwrap_or_default(), &mut chapters, &mut last_index)
            }
            TrackSource::File(file) => {
                if after_copy {
                    /* don't put the start of the track into the last page of the copied one */
                    writer.close_page()?;
                    chapters.push(writer.current_page());
                } else {
                    chapters.push(last_index);
                }
                progress.file_start(track, file);
                encode_track(&mut writer, track, file, options, &mut last_index, progress)
            }
            TrackSource::Packets { label, packets } => {
                progress.file_start(track, Path::new(label));
                copy_track(&mut writer, packets, &mut chapters, &mut last_index)
            }
        };
        let file = match source {
            TrackSource::File(file) => file.as_path(),
            TrackSource::Packets { label, .. } => Path::new(label),
        };
        for w in writer.take_warnings() {
            progress.warning(&w);
        }
        if let Err(e) = result {
            let message = match &e {
                Error::Padding(m) | Error::NotFound(m) => m.clone(),
                Error::PrefixFailed(m) => m.clone(),
                _ => format!("Failed processing {}: {e}", file.display()),
            };
            progress.failed(&message);
            return Err(match e {
                Error::Padding(_) | Error::NotFound(_) | Error::PrefixFailed(_) => e,
                _ => Error::Encoder(message),
            });
        }
        progress.file_done();

        if !warned && writer.position() >= MAX_SIZE / 2 {
            progress.warning("Approaching 2 GiB, please reduce the bitrate");
            warned = true;
        }
    }

    let serial = writer.serial();
    let (_, warnings) = writer.finish()?;
    for w in warnings {
        progress.warning(&w);
    }

    let hash = hashing.sha1.finalize().to_vec();
    let audio_length = hashing.len;
    let mut header = FileHeader {
        hash,
        audio_length: audio_length as i32,
        audio_id: serial,
        audio_chapters: chapters,
        ..Default::default()
    };
    let block = header.to_block();
    out.seek(SeekFrom::Start(0))?;
    out.write_all(&block)?;
    out.seek(SeekFrom::End(0))?;
    out.flush()?;
    Ok(header)
}

/// Appends copied packets as a track that starts on a new page.
fn copy_track<E: FrameEncoder, W: Write>(
    writer: &mut TonieOggWriter<E, W>,
    packets: &[Vec<u8>],
    chapters: &mut Vec<u32>,
    last_index: &mut u32,
) -> Result<(), Error> {
    /* the rest of the previous track belongs to its pages */
    writer.complete_frame()?;
    writer.close_page()?;
    /* the first chapter is always 0, the header pages belong to it */
    chapters.push(if chapters.is_empty() { 0 } else { writer.current_page() });
    for p in packets {
        writer.write_packet(p)?;
    }
    *last_index = writer.page_counter();
    Ok(())
}

fn encode_track<E: FrameEncoder, W: Write>(
    writer: &mut TonieOggWriter<E, W>,
    track: usize,
    file: &Path,
    options: &EncodeOptions,
    last_index: &mut u32,
    progress: &mut dyn EncodeProgress,
) -> Result<(), Error> {
    /* prepend an audio file for e.g. the chapter number */
    if let Some(prefix_dir) = &options.prefix_dir {
        let prefix = prefix_dir.join(format!("{track:04}.mp3"));
        if !prefix.is_file() {
            return Err(Error::NotFound(format!("Missing prefix file '{}'", prefix.display())));
        }
        let mut chunker = Chunker::new();
        decode_file(&prefix, &mut |samples| chunker.push(samples, writer, last_index, true), &mut |_| {})
            .and_then(|_| chunker.flush(writer, last_index, true))
            .map_err(|_| Error::PrefixFailed(format!("Failed processing prefix file '{}'", prefix.display())))?;
    }

    let mut chunker = Chunker::new();
    let mut last_step = 0;
    decode_file(file, &mut |samples| chunker.push(samples, writer, last_index, false), &mut |fraction| {
        let step = (fraction * 20.0) as i32;
        if step != last_step {
            last_step = step;
            progress.progress(fraction);
        }
    })?;
    chunker.flush(writer, last_index, false)
}

/// Collects samples into fixed size chunks for the encoder and drops chunks that are entirely silent.
struct Chunker {
    buf: Vec<i16>,
}

impl Chunker {
    fn new() -> Chunker {
        Chunker {
            buf: Vec::with_capacity(CHUNK_FRAMES * CHANNELS),
        }
    }

    fn push<E: FrameEncoder, W: Write>(
        &mut self,
        samples: &[f32],
        writer: &mut TonieOggWriter<E, W>,
        last_index: &mut u32,
        size_limited: bool,
    ) -> Result<(), Error> {
        for &s in samples {
            /* same scaling as the original encoder */
            self.buf.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
            if self.buf.len() == CHUNK_FRAMES * CHANNELS {
                self.flush(writer, last_index, size_limited)?;
            }
        }
        Ok(())
    }

    fn flush<E: FrameEncoder, W: Write>(&mut self, writer: &mut TonieOggWriter<E, W>, last_index: &mut u32, size_limited: bool) -> Result<(), Error> {
        if self.buf.iter().any(|&s| s != 0) && !(size_limited && writer.position() + BLOCK_SIZE as u64 + self.buf.len() as u64 >= MAX_SIZE) {
            writer.write_samples(&self.buf)?;
        }
        self.buf.clear();
        *last_index = writer.page_counter();
        Ok(())
    }
}

#[cfg(test)]
mod build_tests {
    use super::*;
    use crate::ogg::Page;
    use crate::TonieFile;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures")
    }

    #[test]
    fn exported_tracks_come_back_without_reencoding() {
        let original = TonieFile::open(fixtures().join("reference/album_cbr96/0A0B0C01/500304E0"), true).unwrap();
        let tracks = original.track_packets().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let oggs = original.dump_audio_files(dir.path(), "", false, &[], None).unwrap();
        assert_eq!(oggs.len(), 3);

        /* the exported tracks and an MP3, which is encoded */
        let mut files = oggs.clone();
        files.push(fixtures().join("inputs/silence.mp3"));
        let path = dir.path().join("500304E0");
        let mut out = std::fs::File::create(&path).unwrap();
        encode(&files, &EncodeOptions::default(), &mut out, &mut ()).unwrap();
        drop(out);

        let rebuilt = TonieFile::open(&path, true).unwrap();
        assert!(rebuilt.hash_correct);
        assert!(rebuilt.statistics.errors.is_empty(), "{:?}", rebuilt.statistics.errors);
        let got = rebuilt.track_packets().unwrap();
        assert_eq!(got.len(), 4);
        assert_eq!(got[..3], tracks[..]);

        /* with re-encoding the packets are new */
        let mut out = std::fs::File::create(&path).unwrap();
        let options = EncodeOptions {
            copy_opus: false,
            ..Default::default()
        };
        encode(&oggs, &options, &mut out, &mut ()).unwrap();
        drop(out);
        assert_ne!(TonieFile::open(&path, true).unwrap().track_packets().unwrap()[0], tracks[0]);

        /* an MP3 is not an Ogg Opus file, the Opus test input is copied */
        assert!(audio_in::tonie_ready_opus_packets(&fixtures().join("inputs/silence.mp3")).unwrap().is_none());
        assert!(audio_in::tonie_ready_opus_packets(&fixtures().join("inputs/opus_input.ogg")).unwrap().is_some());
        assert!(audio_in::tonie_ready_opus_packets(&fixtures().join("inputs/vorbis_input.ogg"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn rebuilds_a_tonie_without_reencoding_its_tracks() {
        let original = TonieFile::open(fixtures().join("reference/album_cbr96/0A0B0C01/500304E0"), true).unwrap();
        let tracks = original.track_packets().unwrap();
        assert_eq!(tracks.len(), 3);
        assert!(tracks.iter().all(|t| !t.is_empty()));

        /* a new track first, then the old ones in another order */
        let copy = |i: usize| TrackSource::Packets {
            label: format!("Track {}", i + 1),
            packets: tracks[i].clone(),
        };
        let sources = vec![TrackSource::File(fixtures().join("inputs/silence.mp3")), copy(2), copy(0), copy(1)];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("500304E0");
        let mut out = std::fs::File::create(&path).unwrap();
        let options = EncodeOptions {
            audio_id: 0x1234,
            ..Default::default()
        };
        build(&sources, &options, &mut out, &mut ()).unwrap();
        drop(out);

        let rebuilt = TonieFile::open(&path, true).unwrap();
        assert!(rebuilt.hash_correct);
        assert_eq!(rebuilt.header.audio_id, 0x1234);
        assert_eq!(rebuilt.header.audio_chapters.len(), 4);
        assert!(rebuilt.statistics.errors.is_empty(), "{:?}", rebuilt.statistics.errors);

        /* the copied tracks are unchanged, the last one gets the closing silent frame */
        let got = rebuilt.track_packets().unwrap();
        assert_eq!(got[1], tracks[2]);
        assert_eq!(got[2], tracks[0]);
        assert!(got[3].starts_with(&tracks[1]) && got[3].len() <= tracks[1].len() + 2);

        /* every audio page but the last ends at a 4 KiB boundary, granules grow */
        let mut offset = 0;
        let mut granule = 0;
        while let Some((page, size)) = Page::parse(&rebuilt.audio, offset) {
            offset += size;
            assert!(page.granule_position >= granule);
            granule = page.granule_position;
            /* the small header pages share the first block with the first audio page */
            if offset < rebuilt.audio.len() && !page.is_opus_header() {
                assert_eq!(
                    (BLOCK_SIZE + offset) % BLOCK_SIZE,
                    0,
                    "page {} ends at 0x{:X}",
                    page.sequence,
                    BLOCK_SIZE + offset
                );
            }
        }
        assert_eq!(offset, rebuilt.audio.len());
        assert_ne!((BLOCK_SIZE + offset) % BLOCK_SIZE, 0, "the file must not end at a block boundary");

        /* the length: the new track plus the copied ones */
        let samples = |t: &Vec<Vec<u8>>| t.iter().map(|p| opus::packet::get_nb_samples(p, 48000).unwrap() as u64).sum::<u64>();
        let copied: u64 = tracks.iter().map(samples).sum();
        assert!(rebuilt.statistics.highest_granule > copied);
    }
}
